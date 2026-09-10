//! v3.7.0 (Phase 8) — GitHub connector.
//!
//! Four tools: `github_list_issues`, `github_get_issue`,
//! `github_create_issue`, `github_add_comment`.
//!
//! ## Auth
//!
//! GitHub Personal Access Token (classic or fine-grained).
//! The user pastes it into the existing Settings panel
//! (new `github_pat` field). The connector sends it as
//! `Authorization: token <PAT>`. Per the brief, the
//! classic-token path is the simpler auth model; an
//! OAuth App / GitHub App path can land later if Tyler
//! ever wants it.
//!
//! ## Endpoint shape
//!
//! - `github_list_issues(repo, state)` —
//!   `GET /repos/{owner}/{repo}/issues?state=open|closed|all`
//! - `github_get_issue(repo, number)` —
//!   `GET /repos/{owner}/{repo}/issues/{number}`
//! - `github_create_issue(repo, title, body)` —
//!   `POST /repos/{owner}/{repo}/issues` with
//!   `{ title, body }`. Per-call consent.
//! - `github_add_comment(repo, number, body)` —
//!   `POST /repos/{owner}/{repo}/issues/{number}/comments`
//!   with `{ body }`. Per-call consent.
//!
//! `repo` is the `owner/name` slug (e.g. "denoland/deno").
//! The user can also pass a fully-qualified URL; we
//! strip the host prefix if so.

use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tauri::Manager;

use crate::AppState;
use crate::storage::Settings;
use crate::tools::registry::truncate_for_model;
use crate::tools::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

const GH_BASE: &str = "https://api.github.com";
const GH_API_VERSION: &str = "2022-11-28";
const DEFAULT_LIST_MAX: u32 = 20;
const MAX_LIST_MAX: u32 = 100;
const USER_AGENT: &str = "MaxBot/3.7";

fn http_client() -> Result<Client, ToolError> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| ToolError::Execution(format!("http client: {e}")))
}

fn require_state(context: &ToolContext) -> Result<Arc<AppState>, ToolError> {
    let app = context
        .app
        .as_ref()
        .ok_or_else(|| ToolError::Execution("connector requires an AppHandle".to_string()))?;
    let state: tauri::State<Arc<AppState>> = app.state();
    Ok(state.inner().clone())
}

fn require_github_pat(settings: &Settings) -> Result<String, ToolError> {
    settings
        .github_pat
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ToolError::Execution(
                "GitHub connector not configured: set github_pat in Settings".to_string(),
            )
        })
}

/// Strip a `https://github.com/` prefix (and a trailing
/// `.git`) from a repo argument so the user can paste a
/// GitHub URL or a plain `owner/name` slug interchangeably.
fn normalize_repo(input: &str) -> String {
    let mut s = input.trim().to_string();
    for prefix in [
        "https://github.com/",
        "http://github.com/",
        "github.com/",
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
            break;
        }
    }
    if let Some(stripped) = s.strip_suffix(".git") {
        s = stripped.to_string();
    }
    s
}

/// `github_list_issues` — list issues on a repo. The
/// default state is "open" since that's what most
/// queries want; "closed" / "all" are also valid.
/// Read-only; no consent.
pub struct GithubListIssuesTool;

#[async_trait]
impl Tool for GithubListIssuesTool {
    fn name(&self) -> &str {
        "github_list_issues"
    }

    fn description(&self) -> &str {
        "List issues on a GitHub repo. `repo` is `owner/name` (or a full GitHub URL). `state` is one of `open` (default), `closed`, `all`. Returns issue number, title, author, labels, updated_at. Read-only — no consent required."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "repo": {
                    "type": "string",
                    "description": "Repository as `owner/name` (e.g. 'denoland/deno') or a full GitHub URL."
                },
                "state": {
                    "type": "string",
                    "enum": ["open", "closed", "all"],
                    "default": "open",
                    "description": "Filter by issue state. Default `open`."
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 20,
                    "description": "Max issues to return. Default 20, max 100."
                }
            },
            "required": ["repo"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let repo_raw = invocation
            .arguments
            .get("repo")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `repo`".to_string()))?;
        let repo = normalize_repo(repo_raw);
        if !repo.contains('/') {
            return Err(ToolError::InvalidArguments(format!(
                "`repo` must be `owner/name`, got `{repo_raw}`"
            )));
        }
        let state = invocation
            .arguments
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("open");
        let max = invocation
            .arguments
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(DEFAULT_LIST_MAX)
            .clamp(1, MAX_LIST_MAX);

        let app_state = require_state(&context)?;
        let settings = app_state
            .db
            .load_settings()
            .map_err(|e| ToolError::Execution(format!("load settings: {e}")))?;
        let pat = require_github_pat(&settings)?;
        let client = http_client()?;
        let url = format!("{GH_BASE}/repos/{repo}/issues?state={state}&per_page={max}");
        let resp = client
            .get(&url)
            .header("X-GitHub-Api-Version", GH_API_VERSION)
            .header("Authorization", format!("token {pat}"))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("github list: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ToolError::Execution(format!(
                "github list returned {}: {}",
                status.as_u16(),
                truncate_for_model(&body, 400)
            )));
        }
        let items: Vec<Value> = resp
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("github list parse: {e}")))?;
        if items.is_empty() {
            return Ok(ToolResult::ok(format!("(no {state} issues on {repo})")));
        }
        let mut out = String::new();
        for item in &items {
            // GitHub's `/issues` endpoint returns both
            // issues and pull requests. PRs have a
            // `pull_request` field; we surface that so
            // the model can tell them apart.
            out.push_str(&format_issue_summary(item, repo.as_str()));
            out.push('\n');
        }
        Ok(ToolResult::ok(truncate_for_model(out.trim_end(), 8_000)))
    }
}

fn format_issue_summary(v: &Value, repo: &str) -> String {
    let number = v.get("number").and_then(|x| x.as_u64()).unwrap_or(0);
    let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("(no title)");
    let user = v
        .get("user")
        .and_then(|u| u.get("login"))
        .and_then(|x| x.as_str())
        .unwrap_or("?");
    let updated = v.get("updated_at").and_then(|x| x.as_str()).unwrap_or("?");
    let labels: Vec<String> = v
        .get("labels")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let labels_part = if labels.is_empty() {
        String::new()
    } else {
        format!(" [{}]", labels.join(", "))
    };
    let is_pr = v.get("pull_request").is_some();
    let kind = if is_pr { "PR" } else { "issue" };
    format!("{repo}#{number} ({kind}) {title}  by @{user}  updated {updated}{labels_part}")
}

/// `github_get_issue` — fetch a single issue. The
/// number can come from `github_list_issues` or from a
/// URL the user pastes. Read-only; no consent.
pub struct GithubGetIssueTool;

#[async_trait]
impl Tool for GithubGetIssueTool {
    fn name(&self) -> &str {
        "github_get_issue"
    }

    fn description(&self) -> &str {
        "Fetch a single GitHub issue by repo + number. Returns title, author, state, body, labels, comments count. Read-only — no consent required."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "repo": {"type": "string", "description": "Repository as `owner/name` or a full URL."},
                "number": {"type": "integer", "description": "Issue number (e.g. 42).", "minimum": 1}
            },
            "required": ["repo", "number"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let repo_raw = invocation
            .arguments
            .get("repo")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `repo`".to_string()))?;
        let repo = normalize_repo(repo_raw);
        let number = invocation
            .arguments
            .get("number")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ToolError::InvalidArguments("missing `number`".to_string()))?;
        let app_state = require_state(&context)?;
        let settings = app_state
            .db
            .load_settings()
            .map_err(|e| ToolError::Execution(format!("load settings: {e}")))?;
        let pat = require_github_pat(&settings)?;
        let client = http_client()?;
        let url = format!("{GH_BASE}/repos/{repo}/issues/{number}");
        let resp = client
            .get(&url)
            .header("X-GitHub-Api-Version", GH_API_VERSION)
            .header("Authorization", format!("token {pat}"))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("github get: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ToolError::Execution(format!(
                "github get returned {}: {}",
                status.as_u16(),
                truncate_for_model(&body, 400)
            )));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("github get parse: {e}")))?;
        let out = format_issue_full(&v, repo.as_str());
        Ok(ToolResult::ok(truncate_for_model(&out, 8_000)))
    }
}

fn format_issue_full(v: &Value, repo: &str) -> String {
    let number = v.get("number").and_then(|x| x.as_u64()).unwrap_or(0);
    let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("(no title)");
    let state = v.get("state").and_then(|x| x.as_str()).unwrap_or("?");
    let user = v
        .get("user")
        .and_then(|u| u.get("login"))
        .and_then(|x| x.as_str())
        .unwrap_or("?");
    let created = v.get("created_at").and_then(|x| x.as_str()).unwrap_or("?");
    let updated = v.get("updated_at").and_then(|x| x.as_str()).unwrap_or("?");
    let body = v.get("body").and_then(|x| x.as_str()).unwrap_or("");
    let labels: Vec<String> = v
        .get("labels")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let labels_part = if labels.is_empty() {
        String::new()
    } else {
        format!("\nLabels: {}", labels.join(", "))
    };
    let body_part = if body.is_empty() {
        String::new()
    } else {
        format!("\n\n{body}")
    };
    format!(
        "{repo}#{number} {title}\nState: {state}  Author: @{user}  Created: {created}  Updated: {updated}{labels_part}{body_part}"
    )
}

/// `github_create_issue` — file a new issue. Per-call
/// consent; mutating.
pub struct GithubCreateIssueTool;

#[async_trait]
impl Tool for GithubCreateIssueTool {
    fn name(&self) -> &str {
        "github_create_issue"
    }

    fn description(&self) -> &str {
        "Create a new GitHub issue. Per-call consent (this hits the `POST /repos/{owner}/{repo}/issues` endpoint). `title` is required; `body` is the issue body in Markdown."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "repo": {"type": "string", "description": "Repository as `owner/name` or a full URL."},
                "title": {"type": "string", "description": "Issue title."},
                "body": {"type": "string", "description": "Issue body in Markdown (optional)."}
            },
            "required": ["repo", "title"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let repo_raw = invocation
            .arguments
            .get("repo")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `repo`".to_string()))?;
        let repo = normalize_repo(repo_raw);
        let title = invocation
            .arguments
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `title`".to_string()))?;
        let body = invocation
            .arguments
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let app_state = require_state(&context)?;
        let settings = app_state
            .db
            .load_settings()
            .map_err(|e| ToolError::Execution(format!("load settings: {e}")))?;
        let pat = require_github_pat(&settings)?;
        let client = http_client()?;
        let url = format!("{GH_BASE}/repos/{repo}/issues");
        let resp = client
            .post(&url)
            .header("X-GitHub-Api-Version", GH_API_VERSION)
            .header("Authorization", format!("token {pat}"))
            .header("Accept", "application/vnd.github+json")
            .json(&json!({ "title": title, "body": body }))
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("github create: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ToolError::Execution(format!(
                "github create returned {}: {}",
                status.as_u16(),
                truncate_for_model(&body, 400)
            )));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("github create parse: {e}")))?;
        let number = v.get("number").and_then(|x| x.as_u64()).unwrap_or(0);
        let html_url = v.get("html_url").and_then(|x| x.as_str()).unwrap_or("?");
        Ok(ToolResult::ok(format!("created: {repo}#{number} ({html_url})")))
    }
}

/// `github_add_comment` — post a comment on an existing
/// issue. Per-call consent; mutating.
pub struct GithubAddCommentTool;

#[async_trait]
impl Tool for GithubAddCommentTool {
    fn name(&self) -> &str {
        "github_add_comment"
    }

    fn description(&self) -> &str {
        "Post a comment on an existing GitHub issue. Per-call consent (this hits the `POST /repos/{owner}/{repo}/issues/{number}/comments` endpoint)."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "repo": {"type": "string", "description": "Repository as `owner/name` or a full URL."},
                "number": {"type": "integer", "description": "Issue number to comment on.", "minimum": 1},
                "body": {"type": "string", "description": "Comment body in Markdown."}
            },
            "required": ["repo", "number", "body"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let repo_raw = invocation
            .arguments
            .get("repo")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `repo`".to_string()))?;
        let repo = normalize_repo(repo_raw);
        let number = invocation
            .arguments
            .get("number")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ToolError::InvalidArguments("missing `number`".to_string()))?;
        let body = invocation
            .arguments
            .get("body")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `body`".to_string()))?;
        let app_state = require_state(&context)?;
        let settings = app_state
            .db
            .load_settings()
            .map_err(|e| ToolError::Execution(format!("load settings: {e}")))?;
        let pat = require_github_pat(&settings)?;
        let client = http_client()?;
        let url = format!("{GH_BASE}/repos/{repo}/issues/{number}/comments");
        let resp = client
            .post(&url)
            .header("X-GitHub-Api-Version", GH_API_VERSION)
            .header("Authorization", format!("token {pat}"))
            .header("Accept", "application/vnd.github+json")
            .json(&json!({ "body": body }))
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("github comment: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ToolError::Execution(format!(
                "github comment returned {}: {}",
                status.as_u16(),
                truncate_for_model(&body, 400)
            )));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("github comment parse: {e}")))?;
        let id = v.get("id").and_then(|x| x.as_u64()).unwrap_or(0);
        Ok(ToolResult::ok(format!("commented: {id}")))
    }
}

/// Test the GitHub connection by calling `GET /user`.
/// Used by the BotEditor's "Test connection" button.
pub async fn test_connection(settings: &Settings) -> Result<String, String> {
    let pat = require_github_pat(settings).map_err(|e| e.to_string())?;
    let client = http_client().map_err(|e| e.to_string())?;
    let resp = client
        .get(format!("{GH_BASE}/user"))
        .header("X-GitHub-Api-Version", GH_API_VERSION)
        .header("Authorization", format!("token {pat}"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("github user: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "github user returned {}: {}",
            status.as_u16(),
            truncate_for_model(&body, 400)
        ));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("github user parse: {e}"))?;
    let login = v.get("login").and_then(|x| x.as_str()).unwrap_or("(unknown)");
    Ok(format!("GitHub OK: signed in as @{login}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with_pat(pat: &str) -> Settings {
        let mut s = Settings::default();
        s.github_pat = Some(pat.to_string());
        s
    }

    #[test]
    fn require_github_pat_returns_trimmed_value() {
        let s = settings_with_pat("  ghp_abc  ");
        let t = require_github_pat(&s).unwrap();
        assert_eq!(t, "ghp_abc");
    }

    #[test]
    fn require_github_pat_errors_when_unset() {
        let s = Settings::default();
        let err = require_github_pat(&s).unwrap_err();
        assert!(err.to_string().contains("github_pat"));
    }

    #[test]
    fn normalize_repo_handles_https_url() {
        assert_eq!(
            normalize_repo("https://github.com/denoland/deno"),
            "denoland/deno"
        );
        assert_eq!(
            normalize_repo("https://github.com/denoland/deno.git"),
            "denoland/deno"
        );
        assert_eq!(normalize_repo("denoland/deno"), "denoland/deno");
        assert_eq!(normalize_repo("  denoland/deno  "), "denoland/deno");
        assert_eq!(normalize_repo("http://github.com/a/b"), "a/b");
        assert_eq!(normalize_repo("github.com/a/b"), "a/b");
    }

    #[test]
    fn format_issue_summary_handles_pr_label() {
        let v = json!({
            "number": 7,
            "title": "fix bug",
            "user": {"login": "alice"},
            "updated_at": "2026-09-10T00:00:00Z",
            "labels": [{"name": "bug"}],
            "pull_request": {"url": "https://api.github.com/repos/a/b/pulls/7"}
        });
        let s = format_issue_summary(&v, "a/b");
        assert!(s.contains("a/b#7"));
        assert!(s.contains("(PR)"));
        assert!(s.contains("alice"));
        assert!(s.contains("[bug]"));
    }

    #[test]
    fn format_issue_summary_handles_issue_label() {
        let v = json!({
            "number": 1,
            "title": "open one",
            "user": {"login": "bob"},
            "updated_at": "2026-09-10T00:00:00Z",
            "labels": []
        });
        let s = format_issue_summary(&v, "a/b");
        assert!(s.contains("(issue)"));
        assert!(!s.contains("(PR)"));
    }

    #[test]
    fn format_issue_full_includes_body_and_labels() {
        let v = json!({
            "number": 1,
            "title": "hi",
            "state": "open",
            "user": {"login": "bob"},
            "created_at": "2026-09-10T00:00:00Z",
            "updated_at": "2026-09-10T00:00:00Z",
            "body": "the body",
            "labels": [{"name": "bug"}]
        });
        let s = format_issue_full(&v, "a/b");
        assert!(s.contains("a/b#1"));
        assert!(s.contains("bob"));
        assert!(s.contains("the body"));
        assert!(s.contains("Labels: bug"));
    }
}
