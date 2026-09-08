//! LLM client surface. Currently only the MiniMax provider is implemented;
//! the trait is generic so other providers can plug in later without churning
//! the command layer.

pub mod minimax;
pub mod provider;
pub mod stream;

pub use provider::{ChatRequest, ChatResponse, Provider};
pub use stream::{StreamChunk, StreamError};
