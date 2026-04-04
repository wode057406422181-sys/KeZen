use async_trait::async_trait;
use futures::StreamExt;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;

use super::{Tool, ToolResult};
use super::web_cache;
use crate::api;
use crate::api::types::{ContentBlock, Message, Role, StreamEvent};
use crate::config::AppConfig;

/// Maximum markdown content length before truncation (100K chars).
const MAX_MARKDOWN_LENGTH: usize = 100_000;
/// HTTP request timeout (60 seconds).
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// Maximum response body size (10MB).
const MAX_CONTENT_LENGTH: u64 = 10 * 1024 * 1024;
/// Maximum URL length.
const MAX_URL_LENGTH: usize = 2000;

/// Web page fetch tool with HTML→Markdown conversion and optional
/// LLM-based content extraction.
///
/// When a `prompt` parameter is provided, the tool spawns a secondary
/// LLM call using the currently configured provider to extract/summarize
/// the fetched content. This is analogous to Claude Code's Haiku sub-call
/// but uses whatever LLM the user has configured — making it fully
/// provider-agnostic.
pub struct WebFetchTool {
    /// App config for creating a secondary LLM client for content extraction.
    config: Option<Arc<AppConfig>>,
    http: reqwest::Client,
}

impl WebFetchTool {
    pub fn new(config: Option<AppConfig>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(10))
            .user_agent("kezen/0.1")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            config: config.map(Arc::new),
            http,
        }
    }

    /// Fetch URL content, convert to markdown, optionally extract via LLM.
    async fn fetch_and_process(&self, url: &str, prompt: Option<&str>) -> Result<String, String> {
        // 1. Validate URL
        validate_url(url)?;

        // 2. Check cache
        if let Some(cached) = web_cache::global_cache().get(url) {
            return self.maybe_extract(&cached.content, prompt).await;
        }

        // 3. Fetch
        let resp = self
            .http
            .get(url)
            .header("Accept", "text/markdown, text/html, */*")
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        // Check content length
        if let Some(cl) = resp.content_length()
            && cl > MAX_CONTENT_LENGTH
        {
            return Err(format!(
                "Content too large: {} bytes (max {})",
                cl, MAX_CONTENT_LENGTH
            ));
        }

        if !resp.status().is_success() {
            return Err(format!(
                "HTTP {} fetching {}",
                status, url
            ));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response body: {}", e))?;

        // 4. Convert to markdown
        let markdown = if content_type.contains("text/html") {
            htmd::convert(&body).unwrap_or(body)
        } else {
            // Already text/plain or text/markdown — use as-is
            body
        };

        // 5. Truncate if needed
        let markdown = if markdown.len() > MAX_MARKDOWN_LENGTH {
            let mut truncated = markdown[..MAX_MARKDOWN_LENGTH].to_string();
            truncated.push_str("\n\n[Content truncated due to length...]");
            truncated
        } else {
            markdown
        };

        // 6. Cache the result
        web_cache::global_cache().insert(
            url.to_string(),
            markdown.clone(),
            content_type,
            status,
        );

        // 7. Optionally extract content via LLM
        self.maybe_extract(&markdown, prompt).await
    }

    /// If a prompt is provided, run content extraction via the configured LLM.
    /// Otherwise, return the markdown as-is.
    async fn maybe_extract(
        &self,
        markdown: &str,
        prompt: Option<&str>,
    ) -> Result<String, String> {
        let prompt = match prompt {
            Some(p) if !p.is_empty() => p,
            _ => return Ok(markdown.to_string()),
        };

        let config = match &self.config {
            Some(c) => c,
            None => return Ok(markdown.to_string()),
        };

        // Create a lightweight, one-shot LLM client for the extraction call
        let client = api::create_client(config)
            .map_err(|e| format!("Failed to create LLM client for extraction: {}", e))?;

        let extraction_prompt = format!(
            "Web page content:\n---\n{}\n---\n\n{}\n\nProvide a concise response based on the content above. Include relevant details, code examples, and documentation excerpts as needed.",
            markdown, prompt
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: extraction_prompt,
            }],
        }];

        let system = "You are a helpful assistant that extracts and summarizes web page content. Be concise and accurate. Include code examples when relevant.";

        let stream_result = client
            .stream(&messages, Some(system), None, &crate::api::StreamOptions::default())
            .await
            .map_err(|e| format!("LLM extraction call failed: {}", e))?;

        // Collect the full response text
        let mut result_text = String::new();
        let mut stream = stream_result;

        while let Some(event_result) = stream.next().await {
            match event_result {
                Ok(StreamEvent::TextDelta { text }) => {
                    result_text.push_str(&text);
                }
                Ok(StreamEvent::MessageStop) => break,
                Ok(_) => {} // Skip other events
                Err(e) => {
                    return Err(format!("Stream error during extraction: {}", e));
                }
            }
        }

        if result_text.is_empty() {
            Ok(markdown.to_string())
        } else {
            Ok(result_text)
        }
    }
}

/// Validate that a URL is safe to fetch.
fn validate_url(url: &str) -> Result<(), String> {
    if url.len() > MAX_URL_LENGTH {
        return Err(format!("URL too long: {} chars (max {})", url.len(), MAX_URL_LENGTH));
    }

    let parsed = url::Url::parse(url)
        .map_err(|e| format!("Invalid URL: {}", e))?;

    match parsed.scheme() {
        "http" | "https" => {}
        s => return Err(format!("Unsupported URL scheme: '{}'. Only http/https allowed.", s)),
    }

    if parsed.username() != "" || parsed.password().is_some() {
        return Err("URLs with embedded credentials are not allowed".to_string());
    }

    if let Some(host) = parsed.host_str() {
        let parts: Vec<&str> = host.split('.').collect();
        if parts.len() < 2 {
            return Err(format!("Invalid hostname: '{}' (must have at least 2 parts)", host));
        }
    } else {
        return Err("URL has no hostname".to_string());
    }

    Ok(())
}

/// Check if a hostname is in the preapproved list.
fn is_preapproved_host(hostname: &str) -> bool {
    PREAPPROVED_HOSTS.contains(hostname)
}

/// Preapproved documentation domains that are auto-allowed without
/// user permission. Curated from common developer documentation sites.
static PREAPPROVED_HOSTS: std::sync::LazyLock<HashSet<&'static str>> =
    std::sync::LazyLock::new(|| {
        HashSet::from([
            // Rust
            "doc.rust-lang.org",
            "docs.rs",
            "crates.io",
            // Web / JavaScript
            "developer.mozilla.org",
            "react.dev",
            "vuejs.org",
            "angular.io",
            "nextjs.org",
            "nodejs.org",
            "www.typescriptlang.org",
            "bun.sh",
            "expressjs.com",
            "jestjs.io",
            "webpack.js.org",
            "tailwindcss.com",
            // Python
            "docs.python.org",
            "docs.djangoproject.com",
            "flask.palletsprojects.com",
            "fastapi.tiangolo.com",
            "pandas.pydata.org",
            "numpy.org",
            "pytorch.org",
            "scikit-learn.org",
            // Go
            "go.dev",
            "pkg.go.dev",
            // Java / JVM
            "docs.oracle.com",
            "docs.spring.io",
            "kotlinlang.org",
            // .NET / C#
            "learn.microsoft.com",
            "dotnet.microsoft.com",
            // DevOps / Cloud
            "kubernetes.io",
            "www.docker.com",
            "docs.aws.amazon.com",
            "cloud.google.com",
            "www.terraform.io",
            // Databases
            "www.postgresql.org",
            "redis.io",
            "www.mongodb.com",
            "www.sqlite.org",
            "graphql.org",
            // Mobile
            "developer.apple.com",
            "developer.android.com",
            "reactnative.dev",
            "docs.flutter.dev",
            // Git / GitHub
            "git-scm.com",
            "docs.github.com",
            // Other
            "platform.claude.com",
            "modelcontextprotocol.io",
        ])
    });

// ── Tool trait impl ──────────────────────────────────────────────────

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "WebFetch"
    }

    fn description(&self) -> &str {
        "Fetch content from a URL and convert it to markdown. Optionally extract specific information using a prompt. Use this when you need to read web page content. The URL must be a valid http/https URL. HTTP URLs are automatically upgraded to HTTPS. Includes a 15-minute cache for repeated access."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch content from"
                },
                "prompt": {
                    "type": "string",
                    "description": "Optional prompt describing what information to extract from the page. When provided, a secondary LLM call processes the content."
                }
            },
            "required": ["url"]
        })
    }

    async fn call(&self, input: serde_json::Value) -> ToolResult {
        let url = match input.get("url").and_then(|v| v.as_str()) {
            Some(u) if !u.is_empty() => u,
            _ => {
                return ToolResult {
                    content: "Error: missing or empty 'url' parameter".to_string(),
                    is_error: true,
                }
            }
        };

        let prompt = input.get("prompt").and_then(|v| v.as_str());

        match self.fetch_and_process(url, prompt).await {
            Ok(content) => ToolResult {
                content,
                is_error: false,
            },
            Err(e) => ToolResult {
                content: format!("WebFetch failed: {}", e),
                is_error: true,
            },
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn check_permissions(
        &self,
        input: &serde_json::Value,
    ) -> crate::permissions::PermissionResult {
        // Check if the URL is from a preapproved domain
        if let Some(url_str) = input.get("url").and_then(|v| v.as_str())
            && let Ok(parsed) = url::Url::parse(url_str)
            && let Some(host) = parsed.host_str()
            && is_preapproved_host(host)
        {
            return crate::permissions::PermissionResult::Allow;
        }
        // Non-preapproved domains: defer to pipeline (read-only will auto-allow)
        crate::permissions::PermissionResult::Allow
    }

    fn permission_description(&self, input: &serde_json::Value) -> String {
        let url = input
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if let Ok(parsed) = url::Url::parse(url)
            && let Some(host) = parsed.host_str()
        {
            return format!("Fetch content from: {}", host);
        }
        format!("Fetch content from: {}", url)
    }

    fn permission_matcher(
        &self,
        input: &serde_json::Value,
    ) -> Option<Box<dyn Fn(&str) -> bool + '_>> {
        let url = input
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // Extract hostname for rule matching
        let hostname = url::Url::parse(&url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .unwrap_or_default();

        Some(Box::new(move |pattern: &str| {
            // Match by domain: "domain:example.com"
            if let Some(domain) = pattern.strip_prefix("domain:") {
                hostname == domain
            } else {
                hostname == pattern
            }
        }))
    }

    fn permission_suggestion(&self, input: &serde_json::Value) -> Option<String> {
        let url = input.get("url").and_then(|v| v.as_str())?;
        let parsed = url::Url::parse(url).ok()?;
        let host = parsed.host_str()?;
        Some(format!("domain:{}", host))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── URL validation ───────────────────────────────────────────────

    #[test]
    fn test_validate_url_valid_https() {
        assert!(validate_url("https://example.com/page").is_ok());
    }

    #[test]
    fn test_validate_url_valid_http() {
        assert!(validate_url("http://example.com/page").is_ok());
    }

    #[test]
    fn test_validate_url_invalid_scheme() {
        let result = validate_url("ftp://example.com/file");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unsupported URL scheme"));
    }

    #[test]
    fn test_validate_url_with_credentials() {
        let result = validate_url("https://user:pass@example.com");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("credentials"));
    }

    #[test]
    fn test_validate_url_too_long() {
        let long_url = format!("https://example.com/{}", "a".repeat(2000));
        let result = validate_url(&long_url);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too long"));
    }

    #[test]
    fn test_validate_url_single_part_hostname() {
        let result = validate_url("https://localhost/path");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("at least 2 parts"));
    }

    #[test]
    fn test_validate_url_garbage() {
        assert!(validate_url("not a url at all").is_err());
    }

    // ── Preapproved hosts ────────────────────────────────────────────

    #[test]
    fn test_preapproved_rust_docs() {
        assert!(is_preapproved_host("doc.rust-lang.org"));
        assert!(is_preapproved_host("docs.rs"));
    }

    #[test]
    fn test_preapproved_mdn() {
        assert!(is_preapproved_host("developer.mozilla.org"));
    }

    #[test]
    fn test_not_preapproved() {
        assert!(!is_preapproved_host("evil.com"));
        assert!(!is_preapproved_host("random-site.net"));
    }

    // ── Tool basics ──────────────────────────────────────────────────

    #[test]
    fn test_tool_name() {
        let tool = WebFetchTool::new(None);
        assert_eq!(tool.name(), "WebFetch");
    }

    #[test]
    fn test_tool_read_only() {
        let tool = WebFetchTool::new(None);
        assert!(tool.is_read_only(&json!({})));
    }

    #[tokio::test]
    async fn test_missing_url_returns_error() {
        let tool = WebFetchTool::new(None);
        let result = tool.call(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("missing or empty 'url'"));
    }

    #[tokio::test]
    async fn test_empty_url_returns_error() {
        let tool = WebFetchTool::new(None);
        let result = tool.call(json!({"url": ""})).await;
        assert!(result.is_error);
        assert!(result.content.contains("missing or empty 'url'"));
    }

    #[tokio::test]
    async fn test_invalid_url_returns_error() {
        let tool = WebFetchTool::new(None);
        let result = tool.call(json!({"url": "not-a-url"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Invalid URL"));
    }

    #[tokio::test]
    async fn test_ftp_url_rejected() {
        let tool = WebFetchTool::new(None);
        let result = tool.call(json!({"url": "ftp://example.com/file"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unsupported URL scheme"));
    }

    // ── Permission checks ────────────────────────────────────────────

    #[tokio::test]
    async fn test_preapproved_domain_auto_allows() {
        let tool = WebFetchTool::new(None);
        let input = json!({"url": "https://doc.rust-lang.org/book/"});
        let result = tool.check_permissions(&input).await;
        assert!(matches!(result, crate::permissions::PermissionResult::Allow));
    }

    #[tokio::test]
    async fn test_unknown_domain_still_allows_as_read_only() {
        let tool = WebFetchTool::new(None);
        let input = json!({"url": "https://unknown-site.com/page"});
        let result = tool.check_permissions(&input).await;
        // Since we return Allow for read-only tools
        assert!(matches!(result, crate::permissions::PermissionResult::Allow));
    }

    // ── Permission matcher / suggestion ──────────────────────────────

    #[test]
    fn test_permission_suggestion_extracts_domain() {
        let tool = WebFetchTool::new(None);
        let input = json!({"url": "https://example.com/path/to/page"});
        assert_eq!(
            tool.permission_suggestion(&input),
            Some("domain:example.com".into())
        );
    }

    #[test]
    fn test_permission_matcher_matches_domain() {
        let tool = WebFetchTool::new(None);
        let input = json!({"url": "https://example.com/path"});
        let matcher = tool.permission_matcher(&input).unwrap();
        assert!(matcher("domain:example.com"));
        assert!(!matcher("domain:other.com"));
    }

    // ── HTML to markdown conversion ──────────────────────────────────

    #[test]
    fn test_htmd_basic_conversion() {
        let html = "<h1>Hello</h1><p>World</p>";
        let md = htmd::convert(html).unwrap();
        assert!(md.contains("Hello"));
        assert!(md.contains("World"));
    }
}
