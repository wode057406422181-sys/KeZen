use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::{broadcast, mpsc};

use crate::config::AppConfig;
use crate::engine::events::{EngineEvent, UserAction};
use crate::permissions::RiskLevel;

use super::ui;

// ─── Spinner ────────────────────────────────────────────────────────────────

/// Braille spinner characters (modern, terminal-inspired)
const SPINNER_CHARS: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

// ─── Data types ─────────────────────────────────────────────────────────────

/// A single chat message displayed in the TUI message list.
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
}

/// Role/type of a chat message.
pub enum MessageRole {
    User,
    Assistant,
    Tool {
        #[allow(dead_code)]
        name: String,
        is_error: bool,
    },
    System,
}

/// A permission request awaiting user decision.
pub struct PendingPermission {
    pub id: String,
    pub tool: String,
    pub description: String,
    pub risk_level: RiskLevel,
    #[allow(dead_code)]
    pub suggestion: Option<String>,
}

/// A currently-executing tool displayed as a spinner line.
pub struct ActiveTool {
    pub name: String,
    pub input_preview: String,
}

// ─── App state ──────────────────────────────────────────────────────────────

/// Central application state driving all TUI rendering.
pub struct App {
    // ── Messages ──────────────────────────────────────────────
    /// All finalised messages (user + assistant + tool).
    pub messages: Vec<ChatMessage>,
    /// Text being streamed in from the current assistant turn.
    pub streaming_text: String,
    /// Thinking text being streamed (extended-thinking).
    pub streaming_thinking: String,
    pub in_thinking: bool,
    pub is_streaming: bool,
    /// True between sending a message and receiving the first engine event.
    /// Shows a "waiting for response" spinner so the UI doesn't look frozen.
    pub waiting_for_response: bool,

    // ── Input ─────────────────────────────────────────────────
    pub input: String,
    /// Cursor position as a **character index** (not byte offset).
    /// Ranges from 0 to `self.input.chars().count()` inclusive.
    pub cursor_pos: usize,
    /// Previous inputs for up/down history browsing.
    pub input_history: Vec<String>,
    pub history_index: Option<usize>,
    /// Messages queued while the engine is busy (sent automatically when Done).
    pub queued_user_messages: Vec<String>,
    /// Transient error/info for the queue panel (message, created_at).
    pub queue_toast: Option<(String, Instant)>,
    /// Timestamp of the last queued message auto-send (cooldown gate).
    pub last_queued_send: Option<Instant>,

    // ── Scroll ────────────────────────────────────────────────
    pub scroll_offset: u16,
    /// Stick to bottom while streaming.
    pub auto_scroll: bool,

    // ── Spinner / tools ───────────────────────────────────────
    pub spinner_frame: usize,
    pub active_tools: Vec<ActiveTool>,

    // ── Permission ────────────────────────────────────────────
    pub pending_permission: Option<PendingPermission>,

    // ── Status bar ────────────────────────────────────────────
    pub model_name: String,
    pub session_in_tokens: u64,
    pub session_out_tokens: u64,
    pub session_cache_creation_tokens: u64,
    pub session_cache_read_tokens: u64,
    pub pricing: crate::cost::CostPricing,
    /// Context window size (determined from model or config override).
    pub context_window: u64,

    // ── Lifecycle ─────────────────────────────────────────────
    pub should_quit: bool,
}

impl App {
    /// Create a new App with defaults derived from the config.
    pub fn new(config: &AppConfig) -> Self {
        let model = config.model.clone().unwrap_or_default();
        let pricing = crate::cost::get_model_pricing(&model);
        Self {
            messages: Vec::new(),
            streaming_text: String::new(),
            streaming_thinking: String::new(),
            in_thinking: false,
            is_streaming: false,
            waiting_for_response: false,

            input: String::new(),
            cursor_pos: 0,
            input_history: Vec::new(),
            history_index: None,
            queued_user_messages: Vec::new(),
            queue_toast: None,
            last_queued_send: None,

            scroll_offset: 0,
            auto_scroll: true,

            spinner_frame: 0,
            active_tools: Vec::new(),

            pending_permission: None,

            model_name: model.clone(),
            session_in_tokens: 0,
            session_out_tokens: 0,
            session_cache_creation_tokens: 0,
            session_cache_read_tokens: 0,
            pricing,
            context_window: config
                .context_window
                .unwrap_or_else(|| crate::engine::compact::context_window_for_model(&model)),

            should_quit: false,
        }
    }

    /// Whether the spinner should be animating (any activity).
    pub fn is_busy(&self) -> bool {
        self.is_streaming || self.waiting_for_response || !self.active_tools.is_empty()
    }

    /// Advance the spinner animation by one frame.
    pub fn tick(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER_CHARS.len();
        // Expire queue toast after 3 seconds
        if let Some((_, created)) = &self.queue_toast {
            if created.elapsed().as_millis() > 3000 {
                self.queue_toast = None;
            }
        }
    }

    /// Current spinner character.
    pub fn spinner_char(&self) -> char {
        SPINNER_CHARS[self.spinner_frame]
    }

    // ── Engine event handling ────────────────────────────────────────────

    pub fn handle_engine_event(&mut self, event: EngineEvent) {
        // Any engine event clears the "waiting for response" state.
        self.waiting_for_response = false;

        match event {
            EngineEvent::TextDelta { text } => {
                if self.in_thinking {
                    if !self.streaming_thinking.is_empty() {
                        self.messages.push(ChatMessage {
                            role: MessageRole::System,
                            content: format!("💭 {}", self.streaming_thinking),
                        });
                        self.streaming_thinking.clear();
                    }
                    self.in_thinking = false;
                }
                self.is_streaming = true;
                self.streaming_text.push_str(&text);
                if self.auto_scroll {
                    self.scroll_to_bottom();
                }
            }
            EngineEvent::ThinkingDelta { text } => {
                self.in_thinking = true;
                self.is_streaming = true;
                self.streaming_thinking.push_str(&text);
            }
            EngineEvent::ToolUseStart { id: _, name, input } => {
                if self.in_thinking {
                    if !self.streaming_thinking.is_empty() {
                        self.messages.push(ChatMessage {
                            role: MessageRole::System,
                            content: format!("💭 {}", self.streaming_thinking),
                        });
                        self.streaming_thinking.clear();
                    }
                    self.in_thinking = false;
                }
                self.flush_streaming();

                let preview = serde_json::to_string(&input).unwrap_or_else(|_| input.to_string());
                let preview_short = if preview.chars().count() > 80 {
                    let truncated: String = preview.chars().take(77).collect();
                    format!("{}…", truncated)
                } else {
                    preview.clone()
                };
                self.messages.push(ChatMessage {
                    role: MessageRole::Tool {
                        name: name.clone(),
                        is_error: false,
                    },
                    content: format!("🔧 {} {}", name, preview_short),
                });
                self.active_tools.push(ActiveTool {
                    name,
                    input_preview: preview_short,
                });
                if self.auto_scroll {
                    self.scroll_to_bottom();
                }
            }
            EngineEvent::ToolResult {
                id: _,
                output,
                is_error,
            } => {
                // Remove the first matching active tool (not blindly FIFO,
                // because tools may complete out of order in parallel execution).
                // We remove the first one since ToolResult doesn't carry the name;
                // this is still FIFO but guarded against empty vec.
                if !self.active_tools.is_empty() {
                    self.active_tools.remove(0);
                }
                let display = if output.chars().count() > 200 {
                    let truncated: String = output.chars().take(197).collect();
                    format!("{}…", truncated)
                } else {
                    output
                };
                self.messages.push(ChatMessage {
                    role: MessageRole::Tool {
                        name: String::new(),
                        is_error,
                    },
                    content: if is_error {
                        format!("✖ {}", display)
                    } else {
                        format!("✓ {}", display)
                    },
                });
                if self.auto_scroll {
                    self.scroll_to_bottom();
                }
            }
            EngineEvent::PermissionRequest {
                id,
                tool,
                description,
                risk_level,
                suggestion,
            } => {
                self.pending_permission = Some(PendingPermission {
                    id,
                    tool,
                    description,
                    risk_level,
                    suggestion,
                });
            }
            EngineEvent::CostUpdate(usage) => {
                self.session_in_tokens = usage.input_tokens;
                self.session_out_tokens = usage.output_tokens;
                self.session_cache_creation_tokens = usage.cache_creation_input_tokens;
                self.session_cache_read_tokens = usage.cache_read_input_tokens;
            }
            EngineEvent::Done => {
                self.flush_streaming();
                self.is_streaming = false;
                self.in_thinking = false;
            }
            EngineEvent::Error { message } => {
                self.messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: format!("❌ Error: {}", message),
                });
                self.is_streaming = false;
                if self.auto_scroll {
                    self.scroll_to_bottom();
                }
            }
            EngineEvent::Warning(message) => {
                self.messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: format!("⚠ {}", message),
                });
                if self.auto_scroll {
                    self.scroll_to_bottom();
                }
            }

            EngineEvent::SlashCommandResult { command, output } => {
                self.messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: format!("{} →\n{}", command, output),
                });
                if self.auto_scroll {
                    self.scroll_to_bottom();
                }
            }
            EngineEvent::CompactProgress { message } => {
                self.messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: format!("🗜 {}", message),
                });
                if self.auto_scroll {
                    self.scroll_to_bottom();
                }
            }
            EngineEvent::SkillLoaded { name } => {
                self.messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: format!("⚡ Skill invoked: [{}]", name),
                });
                if self.auto_scroll {
                    self.scroll_to_bottom();
                }
            }
            EngineEvent::SessionRestored { messages } => {
                // Push a separator header
                self.messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: "📜 ── Restored session history ──".to_string(),
                });
                // Convert each restored message to a ChatMessage
                for msg in &messages {
                    let role = match msg.role {
                        crate::api::types::Role::User => MessageRole::User,
                        crate::api::types::Role::Assistant => MessageRole::Assistant,
                        crate::api::types::Role::System => MessageRole::System,
                    };
                    let mut text_parts = Vec::new();
                    for block in &msg.content {
                        match block {
                            crate::api::types::ContentBlock::Text { text } => {
                                let display = if text.len() > 500 {
                                    format!("{}...", &text[..500])
                                } else {
                                    text.clone()
                                };
                                text_parts.push(display);
                            }
                            crate::api::types::ContentBlock::Thinking { thinking } => {
                                let preview = if thinking.len() > 100 {
                                    format!("💭 {}...", &thinking[..100])
                                } else {
                                    format!("💭 {}", thinking)
                                };
                                text_parts.push(preview);
                            }
                            crate::api::types::ContentBlock::ToolUse { name, input, .. } => {
                                let input_str = serde_json::to_string(input).unwrap_or_default();
                                let preview = if input_str.len() > 80 {
                                    format!("🔧 {} {}...", name, &input_str[..80])
                                } else {
                                    format!("🔧 {} {}", name, input_str)
                                };
                                text_parts.push(preview);
                            }
                            crate::api::types::ContentBlock::ToolResult {
                                content,
                                is_error,
                                ..
                            } => {
                                let symbol = if *is_error { "✖" } else { "✓" };
                                let preview = if content.len() > 100 {
                                    format!("{} {}...", symbol, &content[..100])
                                } else {
                                    format!("{} {}", symbol, content)
                                };
                                text_parts.push(preview);
                            }
                        }
                    }
                    if !text_parts.is_empty() {
                        self.messages.push(ChatMessage {
                            role,
                            content: text_parts.join("\n"),
                        });
                    }
                }
                self.messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: "── End of restored history ──".to_string(),
                });
                if self.auto_scroll {
                    self.scroll_to_bottom();
                }
            }
        }
    }

    /// Convert accumulated streaming text into a permanent assistant message.
    fn flush_streaming(&mut self) {
        if !self.streaming_text.is_empty() {
            self.messages.push(ChatMessage {
                role: MessageRole::Assistant,
                content: std::mem::take(&mut self.streaming_text),
            });
        }
    }

    /// Set scroll offset so the newest content is visible.
    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = u16::MAX;
    }

    // ── UTF-8-safe cursor helpers ────────────────────────────────────────

    /// Convert the char-based `cursor_pos` to a byte offset in `self.input`.
    fn cursor_byte_offset(&self) -> usize {
        self.input
            .char_indices()
            .nth(self.cursor_pos)
            .map(|(i, _)| i)
            .unwrap_or(self.input.len())
    }

    /// Number of characters in the input string.
    fn input_char_count(&self) -> usize {
        self.input.chars().count()
    }

    // ── Terminal event handling ──────────────────────────────────────────

    pub async fn handle_key_event(&mut self, key: KeyEvent, action_tx: &mpsc::Sender<UserAction>) {
        // ── Permission dialog intercepts all keys ───────────────────
        if self.pending_permission.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(perm) = self.pending_permission.take() {
                        let _ = action_tx
                            .send(UserAction::PermissionResponse {
                                id: perm.id,
                                allowed: true,
                                always_allow: false,
                            })
                            .await;
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    if let Some(perm) = self.pending_permission.take() {
                        let _ = action_tx
                            .send(UserAction::PermissionResponse {
                                id: perm.id,
                                allowed: false,
                                always_allow: false,
                            })
                            .await;
                    }
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    if let Some(perm) = self.pending_permission.take() {
                        let _ = action_tx
                            .send(UserAction::PermissionResponse {
                                id: perm.id,
                                allowed: true,
                                always_allow: true,
                            })
                            .await;
                    }
                }
                KeyCode::Esc => {
                    if let Some(perm) = self.pending_permission.take() {
                        let _ = action_tx
                            .send(UserAction::PermissionResponse {
                                id: perm.id,
                                allowed: false,
                                always_allow: false,
                            })
                            .await;
                    }
                }
                _ => {}
            }
            return;
        }

        // ── Normal input mode ───────────────────────────────────────
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('d')) => {
                self.should_quit = true;
            }
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if self.is_streaming || self.waiting_for_response {
                    let _ = action_tx.send(UserAction::Cancel).await;
                }
                if !self.queued_user_messages.is_empty() {
                    self.queued_user_messages.clear();
                }
            }
            // Enter → send message (or queue if streaming)
            (_, KeyCode::Enter) => {
                if !self.input.is_empty() {
                    let content = self.input.clone();

                    if content.trim() == "/quit" || content.trim() == "/exit" {
                        self.should_quit = true;
                        return;
                    }

                    self.input_history.push(content.clone());
                    self.history_index = None;
                    self.input.clear();
                    self.cursor_pos = 0;
                    self.auto_scroll = true;

                    if content.trim().starts_with('/') {
                        if self.is_streaming || self.waiting_for_response {
                            self.queue_toast = Some((
                                "✖ Cannot execute slash commands while AI is busy. Press Ctrl+C to cancel.".into(),
                                Instant::now(),
                            ));
                        } else {
                            self.messages.push(ChatMessage {
                                role: MessageRole::User,
                                content: content.clone(),
                            });
                            self.waiting_for_response = true;
                            let _ = action_tx.send(UserAction::SendMessage { content }).await;
                        }
                    } else if self.is_streaming || self.waiting_for_response {
                        const MAX_QUEUED_MESSAGES: usize = 5;
                        if self.queued_user_messages.len() >= MAX_QUEUED_MESSAGES {
                            self.queue_toast = Some((
                                format!(
                                    "Queue full ({}/{}). Press Esc to edit queued messages.",
                                    MAX_QUEUED_MESSAGES, MAX_QUEUED_MESSAGES
                                ),
                                Instant::now(),
                            ));
                        } else {
                            self.queued_user_messages.push(content);
                        }
                    } else {
                        self.messages.push(ChatMessage {
                            role: MessageRole::User,
                            content: content.clone(),
                        });
                        self.waiting_for_response = true;
                        let _ = action_tx.send(UserAction::SendMessage { content }).await;
                    }
                }
            }
            (_, KeyCode::Backspace) => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    let byte_off = self.cursor_byte_offset();
                    // drain() the full char (may be multi-byte UTF-8); remove() only
                    // strips a single byte and would corrupt the string + panic.
                    if let Some(ch) = self.input[byte_off..].chars().next() {
                        self.input.drain(byte_off..byte_off + ch.len_utf8());
                    }
                }
            }
            (_, KeyCode::Delete) => {
                if self.cursor_pos < self.input_char_count() {
                    let byte_off = self.cursor_byte_offset();
                    if let Some(ch) = self.input[byte_off..].chars().next() {
                        self.input.drain(byte_off..byte_off + ch.len_utf8());
                    }
                }
            }
            (_, KeyCode::Left) => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                }
            }
            (_, KeyCode::Right) => {
                if self.cursor_pos < self.input_char_count() {
                    self.cursor_pos += 1;
                }
            }
            (_, KeyCode::Home) => {
                self.cursor_pos = 0;
            }
            (_, KeyCode::End) => {
                self.cursor_pos = self.input_char_count();
            }
            (_, KeyCode::Up) => {
                if !self.input_history.is_empty() {
                    let idx = match self.history_index {
                        Some(i) if i > 0 => i - 1,
                        Some(i) => i,
                        None => self.input_history.len() - 1,
                    };
                    self.history_index = Some(idx);
                    self.input = self.input_history[idx].clone();
                    self.cursor_pos = self.input_char_count();
                }
            }
            (_, KeyCode::Down) => {
                if let Some(idx) = self.history_index {
                    if idx + 1 < self.input_history.len() {
                        let new_idx = idx + 1;
                        self.history_index = Some(new_idx);
                        self.input = self.input_history[new_idx].clone();
                        self.cursor_pos = self.input_char_count();
                    } else {
                        self.history_index = None;
                        self.input.clear();
                        self.cursor_pos = 0;
                    }
                }
            }
            (_, KeyCode::PageUp) => {
                self.auto_scroll = false;
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
            }
            (_, KeyCode::PageDown) => {
                self.scroll_offset = self.scroll_offset.saturating_add(10);
            }
            (_, KeyCode::Esc) => {
                if !self.queued_user_messages.is_empty() {
                    let popped = self.queued_user_messages.pop().unwrap();
                    self.input = popped;
                    self.cursor_pos = self.input_char_count();
                } else {
                    self.input.clear();
                    self.cursor_pos = 0;
                    self.history_index = None;
                }
            }
            (_, KeyCode::Char(c)) => {
                let byte_off = self.cursor_byte_offset();
                self.input.insert(byte_off, c);
                self.cursor_pos += 1;
            }
            (_, KeyCode::Tab) => {
                let byte_off = self.cursor_byte_offset();
                self.input.insert_str(byte_off, "    ");
                self.cursor_pos += 4;
            }
            _ => {}
        }
    }

    // ── Queue drain helper ───────────────────────────────────────────────

    /// Try to send the next queued user message if the engine is idle and
    /// the cooldown has elapsed.  Called from both the engine-event and
    /// tick branches of the main event loop to avoid duplicating the logic.
    async fn try_drain_queue(&mut self, action_tx: &mpsc::Sender<UserAction>) {
        const QUEUED_SEND_COOLDOWN_MS: u128 = 500;
        let cooldown_ok = self
            .last_queued_send
            .map_or(true, |t| t.elapsed().as_millis() >= QUEUED_SEND_COOLDOWN_MS);

        if !self.is_streaming
            && !self.waiting_for_response
            && !self.queued_user_messages.is_empty()
            && cooldown_ok
        {
            let next_msg = self.queued_user_messages.remove(0);

            self.messages.push(ChatMessage {
                role: MessageRole::User,
                content: next_msg.clone(),
            });
            self.waiting_for_response = true;
            self.last_queued_send = Some(Instant::now());
            if let Err(e) = action_tx
                .send(UserAction::SendMessage { content: next_msg })
                .await
            {
                self.queue_toast = Some((
                    format!("Failed to send queued message: {}", e),
                    Instant::now(),
                ));
            }
        }
    }
}

// ─── Main application loop ──────────────────────────────────────────────────

/// Spawn a dedicated thread to read crossterm terminal events and forward
/// them through a tokio channel. This avoids the known issue where
/// `crossterm::event::EventStream` can block the tokio runtime's waker
/// registration in `tokio::select!`, preventing other branches (engine
/// events, tick) from firing until terminal input arrives.
fn spawn_terminal_event_reader() -> mpsc::Receiver<CrosstermEvent> {
    let (tx, rx) = mpsc::channel::<CrosstermEvent>(64);
    std::thread::spawn(move || {
        loop {
            // Poll with a short timeout so the thread can detect channel closure.
            match event::poll(Duration::from_millis(50)) {
                Ok(true) => {
                    if let Ok(evt) = event::read()
                        && tx.blocking_send(evt).is_err()
                    {
                        break; // Receiver dropped → app is shutting down.
                    }
                }
                Ok(false) => {}  // No event within 50 ms, loop back.
                Err(_) => break, // Terminal error, exit.
            }
        }
    });
    rx
}

/// Run the TUI event loop until the user quits.
pub async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: AppConfig,
    action_tx: mpsc::Sender<UserAction>,
    mut event_rx: broadcast::Receiver<EngineEvent>,
    initial_prompt: Option<String>,
) -> anyhow::Result<()> {
    let mut app = App::new(&config);

    if let Some(prompt) = initial_prompt {
        app.messages.push(ChatMessage {
            role: MessageRole::User,
            content: prompt.clone(),
        });
        app.waiting_for_response = true;
        let _ = action_tx
            .send(UserAction::SendMessage { content: prompt })
            .await;
    }

    // Dedicated thread for terminal input — see spawn_terminal_event_reader doc.
    let mut term_rx = spawn_terminal_event_reader();

    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    terminal.draw(|f| ui::draw(f, &app))?;

    loop {
        tokio::select! {
            // ── Engine events ───────────────────────────────────────
            result = event_rx.recv() => {
                match result {
                    Ok(engine_event) => {
                        app.handle_engine_event(engine_event);

                        // Batch-drain: consume ALL buffered events before redrawing.
                        loop {
                            match event_rx.try_recv() {
                                Ok(next_event) => {
                                    app.handle_engine_event(next_event);
                                }
                                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                                    tracing::warn!("TUI batch-drain lagged, skipped {} events", n);
                                    continue;
                                }
                                Err(_) => break,
                            }
                        }

                        // After a Done event, auto-send the next queued user message.
                        app.try_drain_queue(&action_tx).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("TUI event receiver lagged, skipped {} events", n);
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }

            // ── Terminal events (keyboard, resize) ──────────────────
            Some(ct_event) = term_rx.recv() => {
                match ct_event {
                    CrosstermEvent::Key(key) if key.kind == event::KeyEventKind::Press => {
                        app.handle_key_event(key, &action_tx).await;
                    }
                    CrosstermEvent::Resize(_, _) => {}
                    _ => {}
                }
            }

            // ── Spinner tick ────────────────────────────────────────
            _ = tick.tick() => {
                // Always tick — spinner + toast expiry + queued send cooldown
                app.tick();

                // Retry queued send on tick (in case cooldown elapsed since the last event)
                app.try_drain_queue(&action_tx).await;
            }
        }

        terminal.draw(|f| ui::draw(f, &app))?;

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
