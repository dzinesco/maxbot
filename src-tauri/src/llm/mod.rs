//! LLM client surface. The `Provider` trait is the single seam between
//! the chat command and any of the four supported backends: MiniMax,
//! OpenAI, Anthropic, and xAI. MiniMax/OpenAI/xAI share an OpenAI-
//! compatible wire format; Anthropic is a separate translation layer
//! that still surfaces the same `StreamChunk`s to callers.

pub mod anthropic;
pub mod minimax;
pub mod openai;
pub mod openai_compat;
pub mod provider;
pub mod stream;
pub mod xai;

pub use provider::{
    provider_for_settings, ChatMessage, ChatRequest, ChatResponse, Provider, ProviderKind,
    ToolCall, ToolDefinition, ToolFunctionSpec,
};
pub use stream::{StreamChunk, StreamError};
