//! Tool registry for MaxBot's function-calling flow.
//!
//! Each tool is a self-contained unit with a JSON-Schema-ish declaration
//! sent to the model plus a synchronous `execute` implementation. The
//! `ToolRegistry` is the central lookup: `definitions()` returns the array
//! of `ToolDefinition`s to pass into the chat request, and `execute()`
//! dispatches a model-emitted tool call by name and returns the result.
//!
//! Tools that touch the filesystem or shell include a confirmation flag so
//! the chat command can pop a native consent dialog before the action runs.

pub mod apple_script;
pub mod apple_script_exec;
pub mod app_launcher;
pub mod browsers;
pub mod calendar;
pub mod clipboard;
pub mod file_read;
pub mod file_write;
pub mod mail;
pub mod notes;
pub mod registry;
pub mod reminders;
pub mod shell_run;
pub mod system;
pub mod tool;
pub mod tts;
pub mod web_fetch;
pub mod web_search;
pub mod window;

pub use registry::ToolRegistry;
pub use tool::{Tool, ToolContext, ToolError, ToolResult};
