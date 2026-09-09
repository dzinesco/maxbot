//! `web_search` tool: thin wrapper over DuckDuckGo's HTML endpoint.
//!
//! We POST a query to `https://html.duckduckgo.com/html/` and parse the
//! result set out of the HTML response. Each hit becomes one line:
//!   `[n] <title>\n    <url>\n    <snippet>`
//! so the model can quote or follow links directly.

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use super::registry::{require_str, truncate_for_model};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for a query and return the top results (title, url, short snippet). Powered by DuckDuckGo's HTML interface; no API key required."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query. Plain text; no need for advanced operators."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Optional cap on returned results, defaults to 5."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let query = require_str(&invocation.arguments, "query")?;
        let max_results = invocation
            .arguments
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(5);
        let client = Client::builder()
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15")
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        let response = client
            .post("https://html.duckduckgo.com/html/")
            .form(&[("q", query)])
            .header("Accept", "text/html")
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("search request failed: {e}")))?;
        let body = response
            .text()
            .await
            .map_err(|e| ToolError::Execution(format!("body read failed: {e}")))?;
        let results = parse_duckduckgo_html(&body, max_results);
        if results.is_empty() {
            return Ok(ToolResult::ok(
                "No results returned. Try a more specific query.",
            ));
        }
        let formatted = results
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                format!(
                    "[{}] {}\n    {}\n    {}",
                    i + 1,
                    r.title,
                    r.url,
                    r.snippet
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        Ok(ToolResult::ok(truncate_for_model(&formatted, 8_000)))
    }
}

struct Hit {
    title: String,
    url: String,
    snippet: String,
}

/// Extract result blocks from DuckDuckGo's HTML. The structure changes
/// often enough that a strict parser would break; this one is loose
/// (substring scans for `class="result__a"`, `class="result__snippet"`,
/// `class="result__url"`) and tolerates missing fields by leaving them
/// blank. Good enough for the model's purposes; it can re-issue with
/// a refined query if it needs richer metadata.
fn parse_duckduckgo_html(html: &str, max_results: usize) -> Vec<Hit> {
    let mut hits: Vec<Hit> = Vec::new();
    let mut cursor = 0;
    while let Some(title_start) = html[cursor..].find("result__a") {
        let abs_title_start = cursor + title_start;
        // The title lives inside the <a> tag's text content. Walk to the
        // closing > of the opening <a> tag.
        let after_tag = match html[abs_title_start..].find('>') {
            Some(o) => abs_title_start + o + 1,
            None => break,
        };
        let title_end = match html[after_tag..].find("</a>") {
            Some(o) => after_tag + o,
            None => break,
        };
        let raw_title = strip_tags(&html[after_tag..title_end]);
        let url = extract_href(&html[abs_title_start..title_end + 4])
            .unwrap_or_else(|| "(no url)".to_string());
        // Snippet: find the next result__snippet marker.
        let snippet = if let Some(s) = html[title_end..].find("result__snippet") {
            let s_start = title_end + s;
            let s_after = match html[s_start..].find('>') {
                Some(o) => s_start + o + 1,
                None => break,
            };
            let s_end = match html[s_after..].find("</a>") {
                Some(o) => s_after + o,
                None => match html[s_after..].find("</div>") {
                    Some(o) => s_after + o,
                    None => break,
                },
            };
            strip_tags(&html[s_after..s_end])
        } else {
            String::new()
        };
        hits.push(Hit {
            title: raw_title.trim().to_string(),
            url,
            snippet: snippet.trim().to_string(),
        });
        cursor = title_end + 4;
        if hits.len() >= max_results {
            break;
        }
    }
    hits
}

fn extract_href(tag_blob: &str) -> Option<String> {
    let href_idx = tag_blob.find("href=\"")?;
    let after = href_idx + "href=\"".len();
    let end = tag_blob[after..].find('"')? + after;
    Some(tag_blob[after..end].to_string())
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}
