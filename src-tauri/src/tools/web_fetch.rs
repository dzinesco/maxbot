//! `web_fetch` tool: HTTP GET, strip HTML, return plaintext content.

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use super::registry::{require_str, truncate_for_model};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        "web_fetch"
    }

    fn description(&self) -> &'static str {
        "Fetch a URL and return its content as plaintext. HTML is stripped down to headings, paragraphs, lists, and code blocks. Use this when the user gives you a link and wants you to read or summarize it."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The full URL to fetch, including http(s):// scheme."
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Optional cap on the returned content length. Defaults to 12000 characters."
                }
            },
            "required": ["url"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let url = require_str(&invocation.arguments, "url")?;
        let max_chars = invocation
            .arguments
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(12_000);
        let client = Client::builder()
            .user_agent("MaxBot/0.1 (+https://maxbot.app)")
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("fetch failed: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ToolError::Execution(format!(
                "fetch returned {}",
                status.as_u16()
            )));
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = response
            .text()
            .await
            .map_err(|e| ToolError::Execution(format!("body read failed: {e}")))?;
        let plaintext = if content_type.contains("html") {
            strip_html(&body)
        } else {
            body
        };
        Ok(ToolResult::ok(truncate_for_model(&plaintext, max_chars)))
    }
}

/// Minimal HTML → plaintext. Strips scripts, styles, tags, and collapses
/// whitespace. Not a full HTML parser — it intentionally errs on the
/// side of removing structure rather than producing HTML-shaped output.
fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    let mut in_script_or_style = false;
    let mut tag_buf = String::new();
    let mut last_was_space = false;
    let mut i = 0;
    while i < input.len() {
        let rest = &input[i..];
        if !in_tag && rest.starts_with('<') {
            in_tag = true;
            tag_buf.clear();
            tag_buf.push('<');
            i += 1;
            continue;
        }
        if in_tag {
            if rest.starts_with('>') {
                tag_buf.push('>');
                let lower = tag_buf.to_ascii_lowercase();
                if lower.starts_with("<script") || lower.starts_with("<style") {
                    in_script_or_style = true;
                } else if lower == "</script>" || lower == "</style>" {
                    in_script_or_style = false;
                } else if lower.starts_with("<br")
                    || lower.starts_with("<p")
                    || lower.starts_with("<div")
                    || lower.starts_with("<li")
                    || lower.starts_with("<h")
                    || lower.starts_with("<tr")
                {
                    out.push('\n');
                    last_was_space = false;
                }
                in_tag = false;
                i += 1;
                continue;
            }
            tag_buf.push(rest.chars().next().unwrap_or(' '));
            i += 1;
            continue;
        }
        if in_script_or_style {
            i += 1;
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        if c.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(c);
            last_was_space = false;
        }
        i += 1;
    }
    out.trim().to_string()
}
