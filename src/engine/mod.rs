pub mod compact;
pub mod events;
pub mod session;
pub mod slash_commands;

use futures::StreamExt;
use tokio::sync::mpsc;

use crate::api::debug_logger;
use crate::api::types::{ContentBlock, Message, Role, StreamEvent, Usage};
use crate::api::{self, LlmClient, StreamOptions};
use crate::audit::{AuditEvent, SessionAuditLogger};
use crate::constants::defaults::SKILL_TOOL_NAME;
use crate::config::AppConfig;
use crate::prompts::build_system_prompt;
use crate::tools::registry::ToolRegistry;
use crate::skills::registry::SkillRegistry;
use crate::skills::loader::{discover_all_skills, prepare_skill_content};
use crate::tools::skill_tool::SkillTool;
use std::sync::Arc;

use self::events::{EngineEvent, UserAction};
use self::session::Session;

use crate::permissions::{PermissionDecision, PermissionMode, PermissionState};

/// The core engine that orchestrates LLM interactions.
///
/// Engine communicates with frontends exclusively through channels:
/// - Receives `UserAction` from frontends via `action_rx`
/// - Sends `EngineEvent` to frontends via `event_tx`
///
/// **Invariant**: This module MUST NOT import std::io, crossterm, or rustyline.
pub struct KezenEngine {
    #[allow(dead_code)] // TODO: Use config for runtime settings (e.g. dynamic model switch, permission mode)
    config: AppConfig,
    client: Box<dyn LlmClient>,
    session: Session,
    /// Cached system prompt, built once at construction to avoid repeated
    /// blocking I/O (e.g. `git rev-parse`) on the async runtime.
    system_prompt: String,
    action_rx: mpsc::Receiver<UserAction>,
    event_tx: mpsc::Sender<EngineEvent>,
    registry: ToolRegistry,
    permission_state: PermissionState,
    /// Pre-computed stream options (native search settings etc.)
    stream_options: StreamOptions,
    /// Session audit logger (JSONL)
    audit: SessionAuditLogger,
    skill_registry: Arc<SkillRegistry>,
}

impl KezenEngine {
    pub async fn new(
        config: AppConfig,
        action_rx: mpsc::Receiver<UserAction>,
        event_tx: mpsc::Sender<EngineEvent>,
        mut registry: ToolRegistry,
        permission_mode: PermissionMode,
    ) -> Result<Self, crate::error::KezenError> {
        let client = api::create_client(&config)?;
        let mut skill_registry = SkillRegistry::new();
        let discovered_skills = discover_all_skills().await;
        for skill in discovered_skills {
            skill_registry.register(skill);
        }
        tracing::info!(skills = skill_registry.len(), "Skill registry initialized");
        let skill_registry = Arc::new(skill_registry);

        registry.register(Arc::new(SkillTool::new(Arc::clone(&skill_registry))));

        // Build the system prompt asynchronously (git commands + memory file I/O).
        let system_prompt = build_system_prompt(config.model.as_deref(), Some(&skill_registry)).await;
        let model_name = config.model.clone().unwrap_or_default();
        let pricing = crate::cost::get_model_pricing(&model_name);
        
        // No [search] section: search is OFF, fetch is client-side.
        // Only enable server-side features when explicitly configured as "native".
        let search_cfg = config.search.as_ref();
        let stream_options = StreamOptions {
            enable_server_search: search_cfg
                .map_or(false, |s| s.search_mode == "native"),
            enable_server_fetch: search_cfg
                .map_or(false, |s| s.fetch_mode == "native"),
            search_strategy: search_cfg
                .and_then(|s| s.search_strategy.clone()),
        };

        if !config.no_mcp {
            match crate::mcp::client::connect_all_servers().await {
                Ok(result) => {
                    tracing::info!(tools = result.tools.len(), warnings = result.warnings.len(), "MCP servers connected");
                    // Surface connection diagnostics through the event channel
                    // (not eprintln!) to preserve Engine/Frontend separation.
                    for warning in &result.warnings {
                        tracing::warn!(warning = %warning, "MCP connection warning");
                        let _ = event_tx.send(EngineEvent::Error {
                            message: warning.clone(),
                        }).await;
                    }
                    for tool in result.tools {
                        registry.register(tool);
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "MCP init failed");
                    let _ = event_tx.send(EngineEvent::Error {
                        message: format!("MCP init error: {}", e),
                    }).await;
                }
            }
        }

        let session = Session::new(model_name.clone(), pricing);
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        // Initialize audit logger
        let mut audit = SessionAuditLogger::new(&session.id).await
            .map_err(|e| crate::error::KezenError::Config(format!("Failed to create audit logger: {}", e)))?;
        audit.log(&AuditEvent::SessionStart {
            session_id: session.id.clone(),
            timestamp: SessionAuditLogger::now(),
            model: model_name.clone(),
            cwd,
        }).await;

        tracing::info!(model = %model_name, session_id = %session.id, "Engine initialized");

        Ok(Self {
            config,
            client,
            session,
            system_prompt,
            action_rx,
            event_tx,
            registry,
            permission_state: PermissionState::new(permission_mode),
            stream_options,
            audit,
            skill_registry,
        })
    }

    /// Main engine loop. Runs until the action channel is closed.
    pub async fn run(mut self) {
        while let Some(action) = self.action_rx.recv().await {
            match action {
                UserAction::SendMessage { content } => {
                    if let Some((cmd, args)) = slash_commands::parse(&content) {
                        self.handle_slash_command(cmd, args).await;
                    } else {
                        // Audit: user message
                        let msg_uuid = SessionAuditLogger::new_uuid();
                        self.audit.log(&AuditEvent::UserMessage {
                            session_id: self.session.id.clone(),
                            uuid: msg_uuid.clone(),
                            timestamp: SessionAuditLogger::now(),
                            content: content.clone(),
                        }).await;
                        self.handle_send_message(content).await;
                    }
                }
                UserAction::Cancel => {
                    // Ignored if not currently streaming
                }
                UserAction::PermissionResponse { .. } => {
                    // Handled synchronously during the tool execution loop
                }
            }
        }
    }

    /// Handle a user message: send to LLM, stream response, accumulate history.
    async fn handle_send_message(&mut self, content: String) {
        self.session.add_message(Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: content }],
        });

        let mut iterations = 0;
        let max_iterations = 25;

        // Agentic loop: keeps calling the LLM until it stops requesting tools
        // or we hit the safety cap.
        loop {
            // Fix #4: Skip auto-compact for very short conversations (< 4 messages)
            // to avoid edge cases where keep_count logic produces empty results.
            if self.session.message_count() >= 4
                && compact::should_auto_compact(self.session.total_usage().input_tokens, &self.session.model_name, self.config.context_window)
            {
                tracing::info!(tokens = self.session.total_usage().input_tokens, "Auto-compact triggered");
                self.compact_context(None).await;
            }

            if iterations >= max_iterations {
                tracing::warn!("Max tool loop iterations reached");
                let _ = self.event_tx.send(EngineEvent::Error { message: "Max tool loop iterations reached".into() }).await;
                break;
            }
            iterations += 1;
            tracing::debug!(iteration = iterations, "Agentic loop iteration");

            let schemas = self.registry.schemas();
            let tools_arg = if schemas.is_empty() { None } else { Some(schemas.as_slice()) };

            let stream_result = self
                .client
                .stream(self.session.messages(), Some(&self.system_prompt), tools_arg, &self.stream_options, None)
                .await;

            let mut assistant_text = String::new();
            let mut thinking_text = String::new();
            let mut turn_usage = Usage::default();

            let mut pending_tools = Vec::new();
            // Tracks the tool currently being streamed: (id, name, accumulated_json_input)
            let mut current_tool: Option<(String, String, String)> = None;
            let mut stream_interrupted = false;

            match stream_result {
                Ok(mut stream) => {
                    loop {
                        // biased: check cancel before stream to ensure responsiveness
                        tokio::select! {
                            biased;

                            cancel_action = self.action_rx.recv() => {
                                match cancel_action {
                                    Some(UserAction::Cancel) | None => {
                                        stream_interrupted = true;
                                        break;
                                    }
                                    Some(_) => {}
                                }
                            }

                            evt_opt = stream.next() => {
                                match evt_opt {
                                    Some(Ok(evt)) => {
                                        match evt {
                                            StreamEvent::TextDelta { text } => {
                                                assistant_text.push_str(&text);
                                                let _ = self.event_tx.send(EngineEvent::TextDelta { text }).await;
                                            }
                                            StreamEvent::ThinkingDelta { text } => {
                                                thinking_text.push_str(&text);
                                                let _ = self.event_tx.send(EngineEvent::ThinkingDelta { text }).await;
                                            }
                                            StreamEvent::ToolUseStart { id, name } => {
                                                current_tool = Some((id, name, String::new()));
                                            }
                                            StreamEvent::ToolUseInputDelta { text } => {
                                                if let Some((_, _, ref mut input)) = current_tool {
                                                    input.push_str(&text);
                                                }
                                            }
                                            StreamEvent::ToolUseInputDone | StreamEvent::ContentBlockStop { .. } => {
                                                if let Some((id, name, input_str)) = current_tool.take() {
                                                    let input = serde_json::from_str(&if input_str.is_empty() { "{}".to_string() } else { input_str.clone() }).unwrap_or(serde_json::json!({}));
                                                    let _ = self.event_tx.send(EngineEvent::ToolUseStart { id: id.clone(), name: name.clone(), input: input.clone() }).await;
                                                    pending_tools.push((id, name, input));
                                                }
                                            }
                                            StreamEvent::MessageStart { usage: Some(u), .. } => {
                                                turn_usage.input_tokens = u.input_tokens;
                                            }
                                            StreamEvent::MessageDelta { usage: Some(u), .. } => {
                                                if u.output_tokens > 0 { turn_usage.output_tokens = u.output_tokens; }
                                                if u.input_tokens > 0 { turn_usage.input_tokens = u.input_tokens; }
                                            }
                                            StreamEvent::MessageDelta { usage: None, .. } => {}
                                            // Flush pending tool on MessageStop in case
                                            // ContentBlockStop was not emitted (some providers).
                                            StreamEvent::MessageStop => {
                                                if let Some((id, name, input_str)) = current_tool.take() {
                                                    let input = serde_json::from_str(&if input_str.is_empty() { "{}".to_string() } else { input_str.clone() }).unwrap_or(serde_json::json!({}));
                                                    let _ = self.event_tx.send(EngineEvent::ToolUseStart { id: id.clone(), name: name.clone(), input: input.clone() }).await;
                                                    pending_tools.push((id, name, input));
                                                }
                                                break;
                                            }
                                            // ContentBlockStart is intentionally not handled here.
                                            // For text blocks it carries no content (deltas arrive via TextDelta).
                                            // For tool blocks the id/name are already extracted as ToolUseStart
                                            // by the provider-level SSE parser (anthropic.rs / openai.rs).
                                            _ => {}
                                        }
                                    }
                                    Some(Err(e)) => {
                                        tracing::error!(error = %e, "Stream error");
                                        let _ = self.event_tx.send(EngineEvent::Error { message: e.to_string() }).await;
                                        stream_interrupted = true;
                                        break;
                                    }
                                    None => {
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    self.session.update_usage(&turn_usage);
                    debug_logger::log_stream_end(
                        &self.config.provider.to_string(),
                        turn_usage.input_tokens,
                        turn_usage.output_tokens,
                    );
                    let _ = self.event_tx.send(EngineEvent::CostUpdate(turn_usage)).await;

                    if stream_interrupted {
                        let _ = self.event_tx.send(EngineEvent::Done).await;
                        let _ = self.event_tx.send(EngineEvent::SessionSnapshotUpdate { snapshot: self.session.snapshot() }).await;
                        break;
                    }

                    // Record this turn's assistant response in conversation history.
                    let mut blocks = Vec::new();
                    if !thinking_text.is_empty() {
                        blocks.push(ContentBlock::Thinking { thinking: thinking_text });
                    }
                    if !assistant_text.is_empty() {
                        blocks.push(ContentBlock::Text { text: assistant_text.clone() });
                    }
                    for (id, name, input) in &pending_tools {
                        blocks.push(ContentBlock::ToolUse { id: id.clone(), name: name.clone(), input: input.clone() });
                    }

                    if !blocks.is_empty() {
                        self.session.add_message(Message { role: Role::Assistant, content: blocks });
                    }

                    // Audit: assistant text
                    let assistant_uuid = SessionAuditLogger::new_uuid();
                    if !assistant_text.is_empty() {
                        let cost = crate::cost::calculate_cost(
                            turn_usage.input_tokens, turn_usage.output_tokens, &self.session.pricing,
                        );
                        self.audit.log(&AuditEvent::AssistantText {
                            session_id: self.session.id.clone(),
                            uuid: assistant_uuid.clone(),
                            parent_uuid: String::new(), // linked by session order
                            timestamp: SessionAuditLogger::now(),
                            content: assistant_text,
                            input_tokens: turn_usage.input_tokens,
                            output_tokens: turn_usage.output_tokens,
                            cost_usd: cost,
                        }).await;
                    }

                    // No tools requested: the LLM is done, exit the loop.
                    if pending_tools.is_empty() {
                        let _ = self.event_tx.send(EngineEvent::Done).await;
                        let _ = self.event_tx.send(EngineEvent::SessionSnapshotUpdate { snapshot: self.session.snapshot() }).await;
                        break;
                    }

                    // Audit: tool calls
                    for (id, name, input) in &pending_tools {
                        tracing::info!(tool = %name, id = %id, "Tool call started");
                        self.audit.log(&AuditEvent::ToolCall {
                            session_id: self.session.id.clone(),
                            uuid: SessionAuditLogger::new_uuid(),
                            parent_uuid: assistant_uuid.clone(),
                            timestamp: SessionAuditLogger::now(),
                            tool_name: name.clone(),
                            tool_id: id.clone(),
                            input: input.clone(),
                        }).await;
                    }

                    // Phase 1: Filter and prompt for permissions
                    let mut approved_tools = Vec::new();
                    let mut denied_results = Vec::new();

                    for (idx, (id, name, input)) in pending_tools.into_iter().enumerate() {
                        let tool = match self.registry.get(&name) {
                            Some(t) => t,
                            None => {
                                denied_results.push((idx, id.clone(), name.clone(), crate::tools::ToolResult {
                                    content: format!("Tool {} not found", name),
                                    is_error: true,
                                    extraction_usage: None,
                                }));
                                continue;
                            }
                        };

                        // Gather fine-grained permission inputs from the tool
                        let is_read_only = tool.is_read_only(&input);
                        let is_file_tool = tool.is_file_tool();
                        let desc = tool.permission_description(&input);
                        let tool_check = tool.check_permissions(&input).await;
                        let suggestion = tool.permission_suggestion(&input);

                        // Compute the permission decision in a scope so the matcher
                        // (which borrows `tool`) is dropped before we move `tool`.
                        let decision = {
                            let matcher = tool.permission_matcher(&input);
                            let matcher_ref = matcher.as_deref();
                            self.permission_state.check(
                                &name,
                                &input,
                                &tool_check,
                                is_read_only,
                                is_file_tool,
                                desc.clone(),
                                matcher_ref,
                                suggestion,
                            )
                        };

                        // Audit: permission decision
                        let perm_uuid = SessionAuditLogger::new_uuid();
                        let decision_str = match &decision {
                            PermissionDecision::Allow => "allow",
                            PermissionDecision::Deny { .. } => "deny",
                            PermissionDecision::NeedsApproval { .. } => "needs_approval",
                        };
                        let risk_str = match &decision {
                            PermissionDecision::NeedsApproval { risk_level, .. } => format!("{:?}", risk_level),
                            _ => String::new(),
                        };
                        tracing::debug!(tool = %name, decision = decision_str, "Permission decision");
                        self.audit.log(&AuditEvent::PermissionDecision {
                            session_id: self.session.id.clone(),
                            uuid: perm_uuid.clone(),
                            timestamp: SessionAuditLogger::now(),
                            tool_name: name.clone(),
                            decision: decision_str.to_string(),
                            risk_level: risk_str,
                        }).await;

                        match decision {
                            PermissionDecision::Allow => {
                                approved_tools.push((idx, id, name, input, tool));
                            }
                            PermissionDecision::Deny { message } => {
                                denied_results.push((idx, id.clone(), name.clone(), crate::tools::ToolResult {
                                    content: message,
                                    is_error: true,
                                    extraction_usage: None,
                                }));
                            }
                            PermissionDecision::NeedsApproval { tool_name, description, risk_level, suggestion } => {
                                // Block and ask user
                                // Borrow suggestion before moving tool_name into event
                                let suggestion_ref: Option<&str> = suggestion.as_deref();
                                let _ = self.event_tx.send(EngineEvent::PermissionRequest {
                                    id: id.clone(),
                                    tool: tool_name,
                                    description,
                                    risk_level,
                                    suggestion: suggestion.clone(),
                                }).await;

                                // Wait for UserAction::PermissionResponse
                                let mut was_allowed = false;
                                let mut was_always_allow = false;
                                while let Some(action) = self.action_rx.recv().await {
                                    match action {
                                        UserAction::PermissionResponse { id: resp_id, allowed, always_allow } => {
                                            if resp_id == id {
                                                if always_allow && allowed {
                                                    // Fine-grained: store rule content from suggestion
                                                    self.permission_state.add_allow_rule(
                                                        &name,
                                                        suggestion_ref,
                                                    );
                                                }
                                                was_allowed = allowed;
                                                was_always_allow = always_allow;
                                                break;
                                            }
                                        }
                                        UserAction::Cancel => {
                                            // Handle cancel as deny
                                            break;
                                        }

                                        UserAction::SendMessage { .. } => {
                                            // Interleave message not allowed while asking permission
                                            let _ = self.event_tx.send(EngineEvent::Error { 
                                                message: "Cannot send message while waiting for permission approval".into() 
                                            }).await;
                                        }
                                    }
                                }

                                // Audit: permission response
                                self.audit.log(&AuditEvent::PermissionResponse {
                                    session_id: self.session.id.clone(),
                                    uuid: SessionAuditLogger::new_uuid(),
                                    parent_uuid: perm_uuid.clone(),
                                    timestamp: SessionAuditLogger::now(),
                                    allowed: was_allowed,
                                    always_allow: was_always_allow,
                                }).await;

                                if was_allowed {
                                    approved_tools.push((idx, id, name, input, tool));
                                } else {
                                    denied_results.push((idx, id.clone(), name.clone(), crate::tools::ToolResult {
                                        content: "User denied permission to execute this tool".to_string(),
                                        is_error: true,
                                        extraction_usage: None,
                                    }));
                                }
                            }
                        }
                    }

                    // Phase 2: Execute approved tools concurrently.
                    // For Skill tools, extract the skill name from the input
                    // before spawning so we can identify it without parsing XML.
                    let mut join_set = tokio::task::JoinSet::new();
                    for (idx, id, name, input, tool) in approved_tools {
                        let extracted_skill_name = if name == SKILL_TOOL_NAME {
                            input.get("skill").and_then(|v| v.as_str()).map(|s| s.trim().strip_prefix('/').unwrap_or(s.trim()).to_string())
                        } else {
                            None
                        };
                        join_set.spawn(async move {
                            let result = tool.call(input).await;
                            (idx, id, name, extracted_skill_name, result)
                        });
                    }

                    // Collect results and sort by original index to preserve order.
                    // Merge denied results with completed tool results.
                    // Denied results have no extracted_skill_name.
                    let mut indexed_results: Vec<(usize, String, String, Option<String>, crate::tools::ToolResult)> =
                        denied_results.into_iter().map(|(idx, id, name, result)| (idx, id, name, None, result)).collect();
                    while let Some(join_result) = join_set.join_next().await {
                        if let Ok(r) = join_result {
                            indexed_results.push(r);
                        }
                    }
                    indexed_results.sort_by_key(|(idx, _, _, _, _)| *idx);

                    let mut tool_results = Vec::new();
                    let mut skill_injections = Vec::new();
                    // Track extraction usage from sub-LLM calls (e.g. WebFetch content extraction)
                    let mut extraction_usage_total = Usage::default();
                    for (_idx, id, name, extracted_skill_name, result) in indexed_results {
                        // Accumulate any extraction usage into the turn total
                        if let Some(eu) = &result.extraction_usage {
                            extraction_usage_total.input_tokens += eu.input_tokens;
                            extraction_usage_total.output_tokens += eu.output_tokens;
                        }

                        tracing::info!(tool_id = %id, is_error = result.is_error, "Tool call completed");

                        // Audit: tool result
                        let (truncated_output, was_truncated) = SessionAuditLogger::truncate_output(&result.content);
                        self.audit.log(&AuditEvent::ToolResult {
                            session_id: self.session.id.clone(),
                            uuid: SessionAuditLogger::new_uuid(),
                            parent_uuid: id.clone(),
                            timestamp: SessionAuditLogger::now(),
                            tool_id: id.clone(),
                            is_error: result.is_error,
                            output: truncated_output,
                            truncated: was_truncated,
                        }).await;

                        if name == SKILL_TOOL_NAME && !result.is_error {
                            // C-1 fix: Use the skill name extracted from the tool input
                            // (before spawning) instead of parsing XML from the result body.
                            let loaded_name = extracted_skill_name.unwrap_or_else(|| "unknown".to_string());

                            let _ = self.event_tx.send(EngineEvent::SkillLoaded {
                                name: loaded_name.clone(),
                            }).await;

                            tracing::info!(skill = %loaded_name, "Skill loaded, injecting pseudo-instruction");

                            // Clone `id` here because it is also used in the else branch's
                            // ToolResult — we need it in both paths.
                            tool_results.push(ContentBlock::ToolResult {
                                tool_use_id: id.clone(),
                                content: "✅ Skill loaded successfully".to_string(),
                                is_error: false,
                            });
                            skill_injections.push(result.content);
                        } else {
                            let _ = self.event_tx.send(EngineEvent::ToolResult {
                                id: id.clone(),
                                output: result.content.clone(),
                                is_error: result.is_error,
                            }).await;

                            tool_results.push(ContentBlock::ToolResult {
                                tool_use_id: id,
                                content: result.content,
                                is_error: result.is_error,
                            });
                        }
                    }

                    // Report extraction usage to session and frontend
                    if extraction_usage_total.input_tokens > 0 || extraction_usage_total.output_tokens > 0 {
                        self.session.update_usage(&extraction_usage_total);
                        let _ = self.event_tx.send(EngineEvent::CostUpdate(extraction_usage_total)).await;
                    }

                    // Feed tool results back as a "user" message so the LLM
                    // can see the outputs and decide the next step.
                    self.session.add_message(Message { role: Role::User, content: tool_results });

                    // Inject pseudo-instructions for any skills that were loaded
                    for skill_content in &skill_injections {
                        tracing::debug!(content_bytes = skill_content.len(), "Injecting skill pseudo-instruction into session");
                        self.session.add_message(Message {
                            role: Role::Assistant,
                            content: vec![ContentBlock::Text { text: "I'll follow the skill instructions below.".into() }],
                        });
                        self.session.add_message(Message {
                            role: Role::User,
                            content: vec![ContentBlock::Text { text: skill_content.clone() }],
                        });
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "LLM stream creation failed");
                    let _ = self.event_tx.send(EngineEvent::Error { message: e.to_string() }).await;
                    let _ = self.event_tx.send(EngineEvent::Done).await;
                    let _ = self.event_tx.send(EngineEvent::SessionSnapshotUpdate { snapshot: self.session.snapshot() }).await;
                    break;
                }
            }
        }

        // Audit: session end summary
        self.audit.log(&AuditEvent::SessionEnd {
            session_id: self.session.id.clone(),
            timestamp: SessionAuditLogger::now(),
            total_cost_usd: self.session.total_cost_usd,
            total_input_tokens: self.session.total_input_tokens,
            total_output_tokens: self.session.total_output_tokens,
        }).await;
        tracing::info!(session_id = %self.session.id, cost = self.session.total_cost_usd, "Message handling complete");
    }

    async fn handle_slash_command(&mut self, cmd: &str, args: &str) {
        match cmd {
            "help" => {
                let mut output = "Available commands:
  /help       - Provide help on available commands
  /clear      - Clear your chat history
  /compact    - Compact conversation context to save tokens
  /model      - Switch the current model
  /cost       - Show current session cost and token usage
  /resume     - List and resume available sessions"
                    .to_string();

                // M-5: Dynamically list available skill commands.
                let skills = self.skill_registry.all();
                if !skills.is_empty() {
                    output.push_str("\n\nAvailable skills (use /<name> to invoke):");
                    for (name, skill) in skills {
                        let desc = skill.frontmatter.description.as_deref().unwrap_or("No description");
                        output.push_str(&format!("\n  /{:<10} - {}", name, desc));
                    }
                }

                let _ = self.event_tx.send(EngineEvent::SlashCommandResult {
                    command: "/help".into(),
                    output,
                }).await;
            }
            "clear" => {
                self.session.clear();
                let _ = self.event_tx.send(EngineEvent::SlashCommandResult {
                    command: "/clear".into(),
                    output: "Chat history cleared.".into(),
                }).await;
                let _ = self.event_tx.send(EngineEvent::SessionSnapshotUpdate { snapshot: self.session.snapshot() }).await;
            }
            "compact" => {
                // Fix #7: Pass the user-supplied topic to compact_context
                let topic = if args.is_empty() { None } else { Some(args.to_string()) };
                self.compact_context(topic).await;
            }
            "model" => {
                if args.is_empty() {
                    let _ = self.event_tx.send(EngineEvent::SlashCommandResult {
                        command: "/model".into(),
                        output: format!("Current model: {}. Usage: /model <name>", self.config.model.as_deref().unwrap_or("none")),
                    }).await;
                } else {
                    self.config.model = Some(args.to_string());
                    match api::create_client(&self.config) {
                        Ok(client) => {
                            self.client = client;
                            let pricing = crate::cost::get_model_pricing(args);
                            self.session.pricing = pricing;
                            self.session.model_name = args.to_string();
                            let _ = self.event_tx.send(EngineEvent::SlashCommandResult {
                                command: "/model".into(),
                                output: format!("Model switched to {}", args),
                            }).await;
                        }
                        Err(e) => {
                            tracing::warn!(model = %args, error = %e, "Model switch failed");
                            let _ = self.event_tx.send(EngineEvent::SlashCommandResult {
                                command: "/model".into(),
                                output: format!("Failed to switch model: {}", e),
                            }).await;
                        }
                    }
                }
            }
            "cost" => {
                let usage = self.session.total_usage();
                let params = crate::cost::get_model_pricing(&self.session.model_name);
                let cost = crate::cost::calculate_cost(usage.input_tokens, usage.output_tokens, &params);
                let output = format!("Tokens: {} in, {} out.\nCost: ${:.4} (Model: {})", usage.input_tokens, usage.output_tokens, cost, self.session.model_name);
                let _ = self.event_tx.send(EngineEvent::SlashCommandResult {
                    command: "/cost".into(),
                    output,
                }).await;
            }
            "resume" => {
                match crate::session::list_sessions().await {
                    Ok(sessions) => {
                        if args.is_empty() {
                            let mut out = String::from("Available sessions:\n");
                            for s in sessions {
                                out.push_str(&format!("- ID: {} (Model: {}, Msgs: {})\n", s.id, s.model_name, s.messages.len()));
                            }
                            out.push_str("Usage: /resume <id>");
                            let _ = self.event_tx.send(EngineEvent::SlashCommandResult {
                                command: "/resume".into(),
                                output: out,
                            }).await;
                        } else {
                            if let Some(s) = sessions.into_iter().find(|s| s.id == args) {
                                self.session.restore(s);
                                let _ = self.event_tx.send(EngineEvent::SlashCommandResult {
                                    command: "/resume".into(),
                                    output: format!("Resumed session {}", args),
                                }).await;
                                let _ = self.event_tx.send(EngineEvent::SessionSnapshotUpdate { snapshot: self.session.snapshot() }).await;
                            } else {
                                let _ = self.event_tx.send(EngineEvent::SlashCommandResult {
                                    command: "/resume".into(),
                                    output: format!("Session {} not found.", args),
                                }).await;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Session list failed");
                        let _ = self.event_tx.send(EngineEvent::SlashCommandResult {
                            command: "/resume".into(),
                            output: format!("Failed to list sessions: {}", e),
                        }).await;
                    }
                }
            }
            _ => {
                let skill_opt = self.skill_registry.get(cmd).cloned();
                if let Some(skill) = skill_opt {
                    // C-2 fix: Use shared prepare_skill_content() — applies
                    // validation (user_invocable), all substitutions including
                    // ${KEZEN_SESSION_ID}, and XML wrapping.
                    // is_model_invocation = false: slash commands are user-initiated,
                    // so disable_model_invocation does NOT apply.
                    match prepare_skill_content(&skill, args, false).await {
                        Ok(wrapped) => {
                            let _ = self.event_tx.send(EngineEvent::SkillLoaded {
                                name: cmd.to_string(),
                            }).await;

                            tracing::info!(skill = %cmd, "Skill invoked via slash command");

                            let _ = self.event_tx.send(EngineEvent::SlashCommandResult {
                                command: format!("/{}", cmd),
                                output: "Skill invoked directly.".to_string(),
                            }).await;

                            // Audit log for the injected message
                            let msg_uuid = SessionAuditLogger::new_uuid();
                            self.audit.log(&AuditEvent::UserMessage {
                                session_id: self.session.id.clone(),
                                uuid: msg_uuid.clone(),
                                timestamp: SessionAuditLogger::now(),
                                content: wrapped.clone(),
                            }).await;

                            // Recursively trigger the full agentic loop
                            self.handle_send_message(wrapped).await;
                        }
                        Err(e) => {
                            tracing::warn!(skill = %cmd, error = %e, "Failed to load skill via slash command");
                            let _ = self.event_tx.send(EngineEvent::SlashCommandResult {
                                command: format!("/{}", cmd),
                                output: format!("Failed to load skill: {}", e),
                            }).await;
                        }
                    }
                } else {
                    let _ = self.event_tx.send(EngineEvent::SlashCommandResult {
                        command: format!("/{}", cmd),
                        output: format!("Unknown command: /{}", cmd),
                    }).await;
                }
            }
        }
    }

    /// Compact the conversation context into a summary.
    ///
    /// * `topic` — Optional focus area for the summary (e.g. "/compact MCP implementation").
    async fn compact_context(&mut self, topic: Option<String>) {
        const MAX_COMPACT_RETRIES: usize = 2;

        let _ = self.event_tx.send(EngineEvent::CompactProgress {
            message: "Compacting context...".into(),
        }).await;

        let mut messages_to_summarize = self.session.messages().to_vec();
        let mut prompt = compact::compact_prompt();
        // Fix #7: If the user specified a topic, append it to the compact prompt
        if let Some(ref t) = topic {
            prompt.push_str(&format!("\n\nFocus particularly on: {}", t));
        }
        messages_to_summarize.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: prompt }],
        });

        let mut last_error: Option<String> = None;

        for attempt in 1..=MAX_COMPACT_RETRIES {
            match self.client.stream(&messages_to_summarize, None, None, &crate::api::StreamOptions::default(), Some(compact::COMPACT_MAX_OUTPUT_TOKENS)).await {
                Ok(mut stream) => {
                    let mut assistant_text = String::new();
                    let mut stream_errors = Vec::new();

                    while let Some(evt_opt) = stream.next().await {
                        match evt_opt {
                            Ok(StreamEvent::TextDelta { text }) => {
                                assistant_text.push_str(&text);
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Compact: stream error");
                                stream_errors.push(e.to_string());
                            }
                            _ => {}
                        }
                    }

                    match compact::validate_and_extract(&assistant_text, &stream_errors) {
                        Ok((summary, warnings)) => {
                            for w in warnings {
                                let _ = self.event_tx.send(EngineEvent::CompactProgress {
                                    message: format!("Warning: {}", w),
                                }).await;
                            }

                            let summary_msg = Message {
                                role: Role::User,
                                content: vec![ContentBlock::Text { text: format!("[Previous conversation summary]\n\n{}", summary) }],
                            };

                            let original_messages = self.session.messages();
                            let mut keep_count = original_messages.len().min(8);
                            if keep_count % 2 != 0 {
                                keep_count -= 1;
                            }

                            let mut new_messages = vec![summary_msg];

                            // Ensure role alternation: summary is User, so if
                            // the kept tail also starts with User, insert an
                            // empty Assistant placeholder to satisfy the API.
                            if keep_count > 0 {
                                let start_idx = original_messages.len() - keep_count;
                                if original_messages[start_idx].role == Role::User {
                                    new_messages.push(Message {
                                        role: Role::Assistant,
                                        content: vec![ContentBlock::Text {
                                            text: "[Acknowledged — continuing from context above.]".into(),
                                        }],
                                    });
                                }
                                new_messages.extend(original_messages[start_idx..].iter().cloned());
                            }

                            self.session.replace_messages(new_messages);

                            // Fix #3: Reset token counters after successful compaction
                            // to prevent should_auto_compact from immediately re-triggering
                            // on the next agentic loop iteration (infinite compact loop).
                            self.session.reset_usage_counters();

                            let _ = self.event_tx.send(EngineEvent::CompactProgress {
                                message: "Context compacted.".into(),
                            }).await;
                            let _ = self.event_tx.send(EngineEvent::SessionSnapshotUpdate { snapshot: self.session.snapshot() }).await;
                            return;
                        }
                        Err(reason) => {
                            last_error = Some(reason);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Compact: stream creation failed");
                    last_error = Some(e.to_string());
                }
            }

            if attempt < MAX_COMPACT_RETRIES {
                let _ = self.event_tx.send(EngineEvent::CompactProgress {
                    message: format!("Compact attempt {} failed, retrying...", attempt),
                }).await;
            }
        }

        // All retries exhausted — report failure but DON'T touch the message history
        let error_msg = last_error.unwrap_or_default();
        tracing::error!(error = %error_msg, "Compact failed after all retries");
        let _ = self.event_tx.send(EngineEvent::CompactProgress {
            message: format!("Failed to compact after {} attempts: {}", MAX_COMPACT_RETRIES, error_msg),
        }).await;
    }
}
