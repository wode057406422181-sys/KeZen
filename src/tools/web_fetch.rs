use async_trait::async_trait;
use futures::StreamExt;
use serde_json::json;
use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::Arc;

use super::{Tool, ToolResult};
use super::web_cache;
use crate::api;
use crate::api::types::{ContentBlock, Message, Role, StreamEvent, Usage};
use crate::config::AppConfig;

use crate::constants::defaults::{
    FETCH_MAX_MARKDOWN_LENGTH, FETCH_TIMEOUT, FETCH_MAX_CONTENT_LENGTH, FETCH_MAX_URL_LENGTH,
};

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
    async fn fetch_and_process(&self, url: &str, prompt: Option<&str>) -> Result<(String, Option<Usage>), String> {
        // 1. Validate URL
        validate_url(url)?;

        // 1b. Upgrade http → https (done BEFORE cache lookup so that
        //     http:// and https:// variants share the same cache key)
        let url = if url.starts_with("http://") {
            url.replacen("http://", "https://", 1)
        } else {
            url.to_string()
        };

        // 2. Check cache (uses the already-upgraded URL as key)
        if let Some(cached) = web_cache::global_cache().get(&url) {
            return self.maybe_extract(&cached.content, prompt).await;
        }

        let resp = self
            .http
            .get(url.as_str())
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
            && cl > FETCH_MAX_CONTENT_LENGTH
        {
            return Err(format!(
                "Content too large: {} bytes (max {})",
                cl, FETCH_MAX_CONTENT_LENGTH
            ));
        }

        if !resp.status().is_success() {
            return Err(format!(
                "HTTP {} fetching {}",
                status, &url
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
        let markdown = if markdown.len() > FETCH_MAX_MARKDOWN_LENGTH {
            let mut truncated = markdown[..FETCH_MAX_MARKDOWN_LENGTH].to_string();
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
    ///
    /// Returns `(content, Option<Usage>)` — the extracted text and any token
    /// usage consumed by the secondary LLM call.
    async fn maybe_extract(
        &self,
        markdown: &str,
        prompt: Option<&str>,
    ) -> Result<(String, Option<Usage>), String> {
        let prompt = match prompt {
            Some(p) if !p.is_empty() => p,
            _ => return Ok((markdown.to_string(), None)),
        };

        let config = match &self.config {
            Some(c) => c,
            None => return Ok((markdown.to_string(), None)),
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
            .stream(&messages, Some(system), None, &crate::api::StreamOptions::default(), None)
            .await
            .map_err(|e| format!("LLM extraction call failed: {}", e))?;

        // Collect the full response text and usage
        let mut result_text = String::new();
        let mut stream = stream_result;
        let mut extraction_usage = Usage::default();

        while let Some(event_result) = stream.next().await {
            match event_result {
                Ok(StreamEvent::TextDelta { text }) => {
                    result_text.push_str(&text);
                }
                Ok(StreamEvent::MessageStart { usage: Some(u), .. }) => {
                    extraction_usage.input_tokens = u.input_tokens;
                }
                Ok(StreamEvent::MessageDelta { usage: Some(u), .. }) => {
                    if u.output_tokens > 0 { extraction_usage.output_tokens = u.output_tokens; }
                    if u.input_tokens > 0 { extraction_usage.input_tokens = u.input_tokens; }
                }
                Ok(StreamEvent::MessageStop) => break,
                Ok(_) => {} // Skip other events
                Err(e) => {
                    tracing::warn!(error = %e, "WebFetch: extraction stream error");
                    return Err(format!("Stream error during extraction: {}", e));
                }
            }
        }

        let has_usage = extraction_usage.input_tokens > 0 || extraction_usage.output_tokens > 0;

        if result_text.is_empty() {
            Ok((markdown.to_string(), if has_usage { Some(extraction_usage) } else { None }))
        } else {
            Ok((result_text, if has_usage { Some(extraction_usage) } else { None }))
        }
    }
}

/// Validate that a URL is safe to fetch.
///
/// Rejects:
/// - Non-http(s) schemes
/// - Embedded credentials
/// - Single-part hostnames (e.g. `localhost`)
/// - Private / loopback / link-local IP addresses (SSRF prevention)
fn validate_url(url: &str) -> Result<(), String> {
    if url.len() > FETCH_MAX_URL_LENGTH {
        return Err(format!("URL too long: {} chars (max {})", url.len(), FETCH_MAX_URL_LENGTH));
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
        // Block private/internal IP addresses to prevent SSRF
        if let Ok(ip) = host.parse::<IpAddr>() {
            if is_private_or_reserved_ip(&ip) {
                return Err(format!(
                    "Access to private/internal IP address '{}' is not allowed (SSRF protection)",
                    host
                ));
            }
        }

        let parts: Vec<&str> = host.split('.').collect();
        if parts.len() < 2 {
            return Err(format!("Invalid hostname: '{}' (must have at least 2 parts)", host));
        }
    } else {
        return Err("URL has no hostname".to_string());
    }

    Ok(())
}

/// Returns `true` if the IP address is private, loopback, link-local, or
/// otherwise reserved — i.e. should never be fetched by a web tool.
///
/// This guards against SSRF attacks where the LLM is tricked into fetching
/// internal infrastructure (e.g. cloud metadata at 169.254.169.254).
fn is_private_or_reserved_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()           // 127.0.0.0/8
            || v4.is_private()         // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
            || v4.is_link_local()      // 169.254.0.0/16 (AWS metadata!)
            || v4.is_broadcast()       // 255.255.255.255
            || v4.is_unspecified()     // 0.0.0.0
            || v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64 // 100.64.0.0/10 (CGNAT)
            || v4.octets()[0] == 192 && v4.octets()[1] == 0 && v4.octets()[2] == 0 // 192.0.0.0/24 (IETF)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()           // ::1
            || v6.is_unspecified()     // ::
            // fc00::/7 — unique local addresses
            || (v6.segments()[0] & 0xfe00) == 0xfc00
            // fe80::/10 — link-local
            || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
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
        "Fetch content from a URL and convert it to markdown. Optionally extract specific information using a prompt. Use this when you need to read web page content. The URL must be a valid http/https URL. HTTP URLs are automatically upgraded to HTTPS. Includes a 15-minute cache for repeated access. When a prompt is provided, a secondary LLM call extracts relevant information — token Usage from this sub-call is tracked and reported."
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
                return ToolResult::err("Error: missing or empty 'url' parameter".to_string())
            }
        };

        let prompt = input.get("prompt").and_then(|v| v.as_str());

        match self.fetch_and_process(url, prompt).await {
            Ok((content, extraction_usage)) => ToolResult {
                content,
                is_error: false,
                extraction_usage,
            },
            Err(e) => {
                tracing::warn!(error = %e, "WebFetch: fetch failed");
                ToolResult::err(format!("WebFetch failed: {}", e))
            }
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn check_permissions(
        &self,
        input: &serde_json::Value,
    ) -> crate::permissions::PermissionResult {
        let url_str = match input.get("url").and_then(|v| v.as_str()) {
            Some(u) if !u.is_empty() => u,
            _ => {
                return crate::permissions::PermissionResult::Ask {
                    message: "Cannot determine URL — requires approval".to_string(),
                };
            }
        };

        // Attempt to parse — unparseable URLs must NOT silently fall through
        let parsed = match url::Url::parse(url_str) {
            Ok(p) => p,
            Err(_) => {
                return crate::permissions::PermissionResult::Ask {
                    message: format!("Malformed URL requires approval: {}", url_str),
                };
            }
        };

        let host = match parsed.host_str() {
            Some(h) => h,
            None => {
                return crate::permissions::PermissionResult::Ask {
                    message: format!("URL has no hostname — requires approval: {}", url_str),
                };
            }
        };

        // Preapproved documentation domains are auto-allowed
        if is_preapproved_host(host) {
            return crate::permissions::PermissionResult::Allow;
        }

        // Non-preapproved domains: require user approval
        crate::permissions::PermissionResult::Passthrough
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
    async fn test_unknown_domain_needs_approval() {
        let tool = WebFetchTool::new(None);
        let input = json!({"url": "https://unknown-site.com/page"});
        let result = tool.check_permissions(&input).await;
        // Non-preapproved domains should NOT auto-allow
        assert!(matches!(result, crate::permissions::PermissionResult::Passthrough));
    }

    #[tokio::test]
    async fn test_malformed_url_requires_approval() {
        let tool = WebFetchTool::new(None);
        let input = json!({"url": "not-a-url"});
        let result = tool.check_permissions(&input).await;
        assert!(matches!(result, crate::permissions::PermissionResult::Ask { .. }));
    }

    #[tokio::test]
    async fn test_empty_url_requires_approval() {
        let tool = WebFetchTool::new(None);
        let input = json!({"url": ""});
        let result = tool.check_permissions(&input).await;
        assert!(matches!(result, crate::permissions::PermissionResult::Ask { .. }));
    }

    #[tokio::test]
    async fn test_missing_url_requires_approval() {
        let tool = WebFetchTool::new(None);
        let input = json!({});
        let result = tool.check_permissions(&input).await;
        assert!(matches!(result, crate::permissions::PermissionResult::Ask { .. }));
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

    #[test]
    fn test_htmd_link_conversion() {
        let html = r#"<a href="https://example.com">Click</a>"#;
        let md = htmd::convert(html).unwrap();
        assert!(md.contains("Click"));
        assert!(md.contains("https://example.com"));
    }

    // ── URL validation edge cases ────────────────────────────────────

    #[test]
    fn test_validate_url_private_ip_rejected() {
        // Private IPs must be blocked to prevent SSRF
        let result = validate_url("https://192.168.1.1/path");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("SSRF protection"));
    }

    #[test]
    fn test_validate_url_loopback_rejected() {
        let result = validate_url("https://127.0.0.1/admin");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("SSRF protection"));
    }

    #[test]
    fn test_validate_url_link_local_rejected() {
        // AWS metadata endpoint
        let result = validate_url("http://169.254.169.254/latest/meta-data/");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("SSRF protection"));
    }

    #[test]
    fn test_validate_url_public_ip_ok() {
        // Public IPs should be allowed (e.g. 8.8.8.8)
        assert!(validate_url("https://8.8.8.8/dns-query").is_ok());
    }

    #[test]
    fn test_validate_url_ipv6_loopback_rejected() {
        // [::1] is rejected — either by the SSRF IP check or by the
        // hostname parts check (IPv6 literals have no dot-separated parts).
        assert!(validate_url("https://[::1]/path").is_err());
    }

    #[test]
    fn test_validate_url_private_ipv4_10_rejected() {
        let result = validate_url("https://10.0.0.1/internal");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("SSRF protection"));
    }

    #[test]
    fn test_validate_url_with_port() {
        assert!(validate_url("https://example.com:8080/path").is_ok());
    }

    #[test]
    fn test_validate_url_with_query_string() {
        assert!(validate_url("https://example.com/search?q=test&page=1").is_ok());
    }

    #[test]
    fn test_validate_url_with_fragment() {
        assert!(validate_url("https://example.com/page#section").is_ok());
    }

    #[test]
    fn test_validate_url_data_scheme_rejected() {
        let result = validate_url("data:text/html,<h1>hello</h1>");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unsupported URL scheme"));
    }

    #[test]
    fn test_validate_url_javascript_scheme_rejected() {
        let result = validate_url("javascript:alert(1)");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_url_no_hostname() {
        let result = validate_url("https:///path");
        assert!(result.is_err());
    }

    // ── Preapproved hosts comprehensive ──────────────────────────────

    #[test]
    fn test_preapproved_github_docs() {
        assert!(is_preapproved_host("docs.github.com"));
    }

    #[test]
    fn test_preapproved_python_docs() {
        assert!(is_preapproved_host("docs.python.org"));
    }

    #[test]
    fn test_preapproved_kubernetes() {
        assert!(is_preapproved_host("kubernetes.io"));
    }

    #[test]
    fn test_preapproved_claude_platform() {
        assert!(is_preapproved_host("platform.claude.com"));
    }

    #[test]
    fn test_preapproved_mcp() {
        assert!(is_preapproved_host("modelcontextprotocol.io"));
    }

    #[test]
    fn test_not_preapproved_subdomain_of_approved() {
        // sub.doc.rust-lang.org is NOT explicitly in the set
        assert!(!is_preapproved_host("sub.doc.rust-lang.org"));
    }

    // ── Tool trait detail tests ──────────────────────────────────────

    #[test]
    fn test_input_schema_has_url_required() {
        let tool = WebFetchTool::new(None);
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("url")));
    }

    #[test]
    fn test_input_schema_has_prompt_optional() {
        let tool = WebFetchTool::new(None);
        let schema = tool.input_schema();
        assert!(schema["properties"]["prompt"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(!required.iter().any(|v| v.as_str() == Some("prompt")));
    }

    #[test]
    fn test_description_mentions_cache() {
        let tool = WebFetchTool::new(None);
        assert!(tool.description().contains("cache"));
    }

    // ── Permission description ───────────────────────────────────────

    #[test]
    fn test_permission_description_shows_host() {
        let tool = WebFetchTool::new(None);
        let input = json!({"url": "https://docs.rs/tokio/latest"});
        let desc = tool.permission_description(&input);
        assert!(desc.contains("docs.rs"));
    }

    #[test]
    fn test_permission_description_invalid_url_fallback() {
        let tool = WebFetchTool::new(None);
        let input = json!({"url": "not-a-url"});
        let desc = tool.permission_description(&input);
        assert!(desc.contains("not-a-url"));
    }

    #[test]
    fn test_permission_description_missing_url() {
        let tool = WebFetchTool::new(None);
        let input = json!({});
        let desc = tool.permission_description(&input);
        assert!(desc.contains("unknown"));
    }

    // ── Permission matcher edge cases ────────────────────────────────

    #[test]
    fn test_permission_matcher_without_domain_prefix() {
        let tool = WebFetchTool::new(None);
        let input = json!({"url": "https://example.com/path"});
        let matcher = tool.permission_matcher(&input).unwrap();
        // Bare hostname also matches
        assert!(matcher("example.com"));
        assert!(!matcher("other.com"));
    }

    #[test]
    fn test_permission_suggestion_no_url() {
        let tool = WebFetchTool::new(None);
        let input = json!({});
        assert!(tool.permission_suggestion(&input).is_none());
    }

    #[test]
    fn test_permission_suggestion_invalid_url() {
        let tool = WebFetchTool::new(None);
        let input = json!({"url": "not-valid"});
        assert!(tool.permission_suggestion(&input).is_none());
    }

    // ── HTTP→HTTPS upgrade ──────────────────────────────────────────

    #[test]
    fn test_http_url_passes_validation() {
        // http:// is valid and will be upgraded to https:// in fetch_and_process
        assert!(validate_url("http://example.com/page").is_ok());
    }

    #[test]
    fn test_description_mentions_upgrade() {
        let tool = WebFetchTool::new(None);
        assert!(tool.description().contains("upgraded to HTTPS"));
    }

    // ── extraction_usage on error results ────────────────────────────

    #[tokio::test]
    async fn test_error_result_has_no_extraction_usage() {
        let tool = WebFetchTool::new(None);
        let result = tool.call(json!({})).await;
        assert!(result.is_error);
        assert!(result.extraction_usage.is_none());
    }

    #[test]
    fn test_description_mentions_usage_tracking() {
        let tool = WebFetchTool::new(None);
        assert!(tool.description().contains("tracked"));
    }
}
