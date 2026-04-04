use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use super::{Tool, ToolResult};
use crate::config::SearchConfig;

/// A single search result entry returned by any backend.
#[derive(Debug, Clone)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

/// Web search tool supporting multiple search backends.
///
/// Backends are selected via `SearchConfig.provider`:
/// - `"brave"`       — Brave Search API
/// - `"searxng"`     — Self-hosted SearXNG instance
/// - `"google_cse"`  — Google Custom Search Engine
/// - `"bing"`        — Bing Web Search API
pub struct WebSearchTool {
    config: Option<Arc<SearchConfig>>,
    http: reqwest::Client,
}

impl WebSearchTool {
    pub fn new(config: Option<SearchConfig>) -> Self {
        Self {
            config: config.map(Arc::new),
            http: reqwest::Client::new(),
        }
    }

    /// Dispatch to the appropriate search backend.
    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>, String> {
        let config = self.config.as_ref().ok_or_else(|| {
            "WebSearch is not configured. Add a [search] section to ~/.kezen/config/config.toml with provider and api_key.".to_string()
        })?;

        match config.provider.as_str() {
            "brave" => self.search_brave(config, query, max_results).await,
            "searxng" => self.search_searxng(config, query, max_results).await,
            "google_cse" => self.search_google_cse(config, query, max_results).await,
            "bing" => self.search_bing(config, query, max_results).await,
            other => Err(format!(
                "Unknown search provider: '{}'. Supported: brave, searxng, google_cse, bing",
                other
            )),
        }
    }

    // ── Brave Search ─────────────────────────────────────────────────

    async fn search_brave(
        &self,
        config: &SearchConfig,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, String> {
        let api_key = config
            .api_key
            .as_deref()
            .ok_or("Brave Search requires an API key. Set search.api_key in config.toml")?;

        let resp = self
            .http
            .get("https://api.search.brave.com/res/v1/web/search")
            .header("X-Subscription-Token", api_key)
            .header("Accept", "application/json")
            .query(&[
                ("q", query),
                ("count", &max_results.to_string()),
            ])
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("Brave Search request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!(
                "Brave Search returned HTTP {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            ));
        }

        let body: BraveResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse Brave response: {}", e))?;

        Ok(body
            .web
            .and_then(|w| w.results)
            .unwrap_or_default()
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.description.unwrap_or_default(),
            })
            .collect())
    }

    // ── SearXNG ──────────────────────────────────────────────────────

    async fn search_searxng(
        &self,
        config: &SearchConfig,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, String> {
        let base_url = config
            .base_url
            .as_deref()
            .unwrap_or("http://localhost:8080");

        let url = format!("{}/search", base_url.trim_end_matches('/'));

        let resp = self
            .http
            .get(&url)
            .query(&[
                ("q", query),
                ("format", "json"),
                ("categories", "general"),
            ])
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("SearXNG request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("SearXNG returned HTTP {}", resp.status()));
        }

        let body: SearxngResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse SearXNG response: {}", e))?;

        Ok(body
            .results
            .into_iter()
            .take(max_results)
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content.unwrap_or_default(),
            })
            .collect())
    }

    // ── Google Custom Search ─────────────────────────────────────────

    async fn search_google_cse(
        &self,
        config: &SearchConfig,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, String> {
        let api_key = config
            .api_key
            .as_deref()
            .ok_or("Google CSE requires an API key. Set search.api_key in config.toml")?;

        // Google CSE uses the `cx` in base_url or a separate field.
        // Convention: base_url = "{cx_id}"
        let cx = config
            .base_url
            .as_deref()
            .ok_or("Google CSE requires search.base_url = \"<your CX id>\" in config.toml")?;

        let resp = self
            .http
            .get("https://www.googleapis.com/customsearch/v1")
            .query(&[
                ("q", query),
                ("key", api_key),
                ("cx", cx),
                ("num", &max_results.min(10).to_string()),
            ])
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("Google CSE request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!(
                "Google CSE returned HTTP {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            ));
        }

        let body: GoogleCseResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse Google CSE response: {}", e))?;

        Ok(body
            .items
            .unwrap_or_default()
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.link,
                snippet: r.snippet.unwrap_or_default(),
            })
            .collect())
    }

    // ── Bing Web Search ──────────────────────────────────────────────

    async fn search_bing(
        &self,
        config: &SearchConfig,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, String> {
        let api_key = config
            .api_key
            .as_deref()
            .ok_or("Bing Search requires an API key. Set search.api_key in config.toml")?;

        let resp = self
            .http
            .get("https://api.bing.microsoft.com/v7.0/search")
            .header("Ocp-Apim-Subscription-Key", api_key)
            .query(&[
                ("q", query),
                ("count", &max_results.to_string()),
            ])
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("Bing Search request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!(
                "Bing Search returned HTTP {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            ));
        }

        let body: BingResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse Bing response: {}", e))?;

        Ok(body
            .web_pages
            .and_then(|w| w.value)
            .unwrap_or_default()
            .into_iter()
            .map(|r| SearchResult {
                title: r.name,
                url: r.url,
                snippet: r.snippet.unwrap_or_default(),
            })
            .collect())
    }
}

/// Format search results into a readable text block for the LLM.
fn format_results(query: &str, results: &[SearchResult]) -> String {
    if results.is_empty() {
        return format!("No results found for: \"{}\"\n", query);
    }

    let mut out = format!(
        "Search results for: \"{}\"\n{}\n\n",
        query,
        "─".repeat(40)
    );

    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!(
            "{}. {}\n   {}\n   {}\n\n",
            i + 1,
            r.title,
            r.url,
            r.snippet,
        ));
    }

    out.push_str("REMINDER: You MUST include the sources above in your response to the user using markdown hyperlinks, e.g. [Title](URL).\n");
    out
}

// ── JSON response structs ────────────────────────────────────────────

#[derive(Deserialize)]
struct BraveResponse {
    web: Option<BraveWeb>,
}
#[derive(Deserialize)]
struct BraveWeb {
    results: Option<Vec<BraveResult>>,
}
#[derive(Deserialize)]
struct BraveResult {
    title: String,
    url: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct SearxngResponse {
    results: Vec<SearxngResult>,
}
#[derive(Deserialize)]
struct SearxngResult {
    title: String,
    url: String,
    content: Option<String>,
}

#[derive(Deserialize)]
struct GoogleCseResponse {
    items: Option<Vec<GoogleCseItem>>,
}
#[derive(Deserialize)]
struct GoogleCseItem {
    title: String,
    link: String,
    snippet: Option<String>,
}

#[derive(Deserialize)]
struct BingResponse {
    #[serde(rename = "webPages")]
    web_pages: Option<BingWebPages>,
}
#[derive(Deserialize)]
struct BingWebPages {
    value: Option<Vec<BingResult>>,
}
#[derive(Deserialize)]
struct BingResult {
    name: String,
    url: String,
    snippet: Option<String>,
}

// ── Tool trait impl ──────────────────────────────────────────────────

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "WebSearch"
    }

    fn description(&self) -> &str {
        "Search the web for current information. Returns search results with titles, URLs, and snippets. Use this to find up-to-date information beyond the knowledge cutoff."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query to execute"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default 10)"
                }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, input: serde_json::Value) -> ToolResult {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.is_empty() => q,
            _ => {
                return ToolResult {
                    content: "Error: missing or empty 'query' parameter".to_string(),
                    is_error: true,
                }
            }
        };

        let max_results = input
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        match self.search(query, max_results).await {
            Ok(results) => ToolResult {
                content: format_results(query, &results),
                is_error: false,
            },
            Err(e) => ToolResult {
                content: format!("Search failed: {}", e),
                is_error: true,
            },
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn check_permissions(
        &self,
        _input: &serde_json::Value,
    ) -> crate::permissions::PermissionResult {
        crate::permissions::PermissionResult::Allow
    }

    fn permission_description(&self, input: &serde_json::Value) -> String {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        format!("Search the web for: \"{}\"", query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_results_empty() {
        let output = format_results("rust async", &[]);
        assert!(output.contains("No results found"));
    }

    #[test]
    fn test_format_results_with_items() {
        let results = vec![
            SearchResult {
                title: "Async in Rust".into(),
                url: "https://example.com/async".into(),
                snippet: "Learn about async/await in Rust".into(),
            },
            SearchResult {
                title: "Tokio Runtime".into(),
                url: "https://tokio.rs".into(),
                snippet: "Async runtime for Rust".into(),
            },
        ];
        let output = format_results("rust async", &results);
        assert!(output.contains("1. Async in Rust"));
        assert!(output.contains("2. Tokio Runtime"));
        assert!(output.contains("https://example.com/async"));
        assert!(output.contains("https://tokio.rs"));
        assert!(output.contains("REMINDER"));
    }

    #[test]
    fn test_tool_name() {
        let tool = WebSearchTool::new(None);
        assert_eq!(tool.name(), "WebSearch");
    }

    #[test]
    fn test_tool_read_only() {
        let tool = WebSearchTool::new(None);
        assert!(tool.is_read_only(&json!({})));
    }

    #[tokio::test]
    async fn test_missing_config_returns_error() {
        let tool = WebSearchTool::new(None);
        let result = tool.call(json!({"query": "test"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("not configured"));
    }

    #[tokio::test]
    async fn test_missing_query_returns_error() {
        let tool = WebSearchTool::new(None);
        let result = tool.call(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("missing or empty 'query'"));
    }

    #[tokio::test]
    async fn test_empty_query_returns_error() {
        let tool = WebSearchTool::new(None);
        let result = tool.call(json!({"query": ""})).await;
        assert!(result.is_error);
        assert!(result.content.contains("missing or empty 'query'"));
    }

    #[tokio::test]
    async fn test_unknown_provider_returns_error() {
        let config = SearchConfig {
            provider: "unknown_provider".into(),
            api_key: Some("key".into()),
            base_url: None,
        };
        let tool = WebSearchTool::new(Some(config));
        let result = tool.call(json!({"query": "test"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown search provider"));
    }

    #[tokio::test]
    async fn test_brave_missing_api_key() {
        let config = SearchConfig {
            provider: "brave".into(),
            api_key: None,
            base_url: None,
        };
        let tool = WebSearchTool::new(Some(config));
        let result = tool.call(json!({"query": "test"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("requires an API key"));
    }

    #[test]
    fn test_parse_brave_response() {
        let json_str = r#"{
            "web": {
                "results": [
                    {"title": "Test", "url": "https://test.com", "description": "A test result"}
                ]
            }
        }"#;
        let resp: BraveResponse = serde_json::from_str(json_str).unwrap();
        let results = resp.web.unwrap().results.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Test");
    }

    #[test]
    fn test_parse_bing_response() {
        let json_str = r#"{
            "webPages": {
                "value": [
                    {"name": "Bing Test", "url": "https://bing.com", "snippet": "Bing result"}
                ]
            }
        }"#;
        let resp: BingResponse = serde_json::from_str(json_str).unwrap();
        let results = resp.web_pages.unwrap().value.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Bing Test");
    }

    #[test]
    fn test_parse_searxng_response() {
        let json_str = r#"{
            "results": [
                {"title": "SearX Test", "url": "https://searx.com", "content": "SearX result"}
            ]
        }"#;
        let resp: SearxngResponse = serde_json::from_str(json_str).unwrap();
        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].title, "SearX Test");
    }

    #[test]
    fn test_parse_google_cse_response() {
        let json_str = r#"{
            "items": [
                {"title": "Google Test", "link": "https://google.com", "snippet": "Google result"}
            ]
        }"#;
        let resp: GoogleCseResponse = serde_json::from_str(json_str).unwrap();
        let items = resp.items.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Google Test");
    }
}
