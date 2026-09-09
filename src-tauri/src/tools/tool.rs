//! Tool trait and shared execution types.

use async_trait::async_trait;
use serde_json::Value;

use crate::llm::provider::ToolDefinition;

/// What the model sees when it emits a tool call. The argument payload is
/// the model's chosen JSON object, already validated by the JSON Schema
/// declared in the tool's definition.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub name: String,
    pub arguments: Value,
    /// Stable id assigned by the model, used to thread the result back
    /// into the conversation as a `tool` role message.
    pub id: String,
}

/// What the executor returns to the model. Plain text is the cheapest
/// format; the chat loop turns the text into a `tool` role message. Long
/// outputs are truncated at the executor level so we don't blow the
/// model's context window on a single tool call.
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// Content delivered to the model. Always non-empty; an "I refused
    /// because…" message is still a content string.
    pub content: String,
    /// If the tool errored, the model sees a `tool` message that begins
    /// with this prefix (e.g. "[error]"). The chat loop sets it on the
    /// persisted tool message so the next turn can react to it.
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }
    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("execution failed: {0}")]
    Execution(String),
}

/// Optional side-band that the chat command can pass into a tool to
/// support consent prompts, cancellation, and progress reporting. Most
/// tools ignore it; the shell executor uses it to surface "user denied"
/// back to the caller.
#[derive(Default, Clone)]
pub struct ToolContext {
    /// Pre-fetched user consent. If the tool requires consent and this
    /// is false, the tool should return `ToolError::Execution` describing
    /// the user denial.
    pub consent_granted: bool,
    /// Optional one-line description of the action, used by the consent
    /// dialog (e.g. "run shell command `rm -rf build`"). Tools set this
    /// internally before asking the chat command to gate them.
    pub consent_prompt: Option<String>,
}

/// Common interface every tool implements. The `definition()` is the
/// shape the model sees; `execute()` is called by the agent loop when
/// the model emits a matching tool call.
#[async_trait]
pub trait Tool: Send + Sync {
    /// The tool's name, surfaced to the model in the function-calling
    /// schema. Returning `&str` (not `&'static str`) lets dynamic tools
    /// (MCP-backed, plugin-backed) have names computed at runtime.
    fn name(&self) -> &str;
    /// Human-readable description surfaced to the model.
    fn description(&self) -> &str;
    /// True for tools that touch the filesystem, shell, or anything else
    /// where the user should be asked before the action runs. The chat
    /// command surfaces a native consent dialog before invoking `execute`.
    fn requires_consent(&self) -> bool;
    /// JSON Schema for the tool's `parameters` object.
    fn parameters_schema(&self) -> Value;

    /// Concrete definition to send to the model.
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            kind: "function".to_string(),
            function: crate::llm::provider::ToolFunctionSpec {
                name: self.name().to_string(),
                description: self.description().to_string(),
                parameters: self.parameters_schema(),
            },
        }
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError>;
}
