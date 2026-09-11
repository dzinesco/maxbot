// v4 S3a — test_turn harness.
//
// Drives a single chat turn headlessly: opens the same SQLite
// database the shipping app uses, builds the same date header,
// streams from the same provider, captures chunks + tool_calls
// + final text, and writes JSON to --output.
//
// Skips Tauri entirely — no IPC, no AppHandle, no consent
// dialog. Lets me verify the bug fix (model flailing on
// shell_run for trivial questions) end-to-end without driving
// the native UI.
//
// Usage:
//   test_turn --bot=<bot-id> --prompt="what day is it?" \
//             --output=/tmp/test.json
//
// Output JSON shape:
//   {
//     "bot_id": "...",
//     "bot_name": "...",
//     "prompt": "...",
//     "date_header": "Today is Friday, September 11, 2026...",
//     "model": "...",
//     "text": "Today is Friday, September 11, 2026.",
//     "tool_calls": [...],
//     "finish_reason": "stop",
//     "errors": [],
//     "has_think_block": false,
//     "shell_run_called": false,
//     "any_tool_called": false
//   }

use futures_util::StreamExt;
use serde_json::json;
use std::path::{Path, PathBuf};

use maxbot_lib::llm::provider::{
    provider_for_settings, ChatMessage, ChatRequest,
};
use maxbot_lib::llm::stream::StreamChunk;
use maxbot_lib::storage::Database;
use maxbot_lib::tools::registry::ToolRegistry;

struct Args {
    bot: String,
    prompt: String,
    db: Option<String>,
    output: String,
}

/// Minimal CLI parser — `--key=value` style. Avoids pulling
/// in clap just for this harness. Errors out with a usage
/// line if any required flag is missing.
fn parse_args() -> Result<Args, String> {
    let mut bot: Option<String> = None;
    let mut prompt: Option<String> = None;
    let mut db: Option<String> = None;
    let mut output: Option<String> = None;

    for arg in std::env::args().skip(1) {
        let (k, v) = arg
            .strip_prefix("--")
            .and_then(|s| s.split_once('='))
            .ok_or_else(|| {
                format!("expected --key=value, got {arg:?}")
            })?;
        match k {
            "bot" => bot = Some(v.to_string()),
            "prompt" => prompt = Some(v.to_string()),
            "db" => db = Some(v.to_string()),
            "output" => output = Some(v.to_string()),
            _ => return Err(format!("unknown flag --{k}")),
        }
    }

    Ok(Args {
        bot: bot.ok_or_else(|| "missing --bot=<id>".to_string())?,
        prompt: prompt.ok_or_else(|| "missing --prompt=<text>".to_string())?,
        db,
        output: output.unwrap_or_else(|| "/tmp/maxbot-test-output.json".to_string()),
    })
}

/// Mirror of `commands::chat::date_header_for_now`. The
/// `commands` module is private to the lib crate, so we
/// duplicate the format here. If the upstream format
/// changes, update this in lockstep.
fn date_header_for_now() -> String {
    let now = chrono::Local::now();
    let tz = iana_time_zone::get_timezone()
        .unwrap_or_else(|_| now.format("%Z").to_string());
    format!(
        "Today is {} ({}). Do not call tools to determine \
         the current date, current day of the week, or current \
         time — answer from this header. If the user asks a \
         question that depends on today's date, use the date \
         above directly.\n",
        now.format("%A, %B %-d, %Y"),
        tz,
    )
}

/// Path to the shipping app's SQLite DB. macOS convention:
/// $HOME/Library/Application Support/com.maxbot.app/maxbot.sqlite
fn shipping_db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("com.maxbot.app")
        .join("maxbot.sqlite")
}

#[tokio::main]
async fn main() {
    let args = parse_args().unwrap_or_else(|e| {
        eprintln!("usage: test_turn --bot=<id> --prompt=<text> [--db=<path>] [--output=<path>]\n  error: {e}");
        std::process::exit(2);
    });

    let db_path = args
        .db
        .as_deref()
        .map(Path::new)
        .map(Path::to_path_buf)
        .unwrap_or_else(shipping_db_path);
    let db = Database::open(&db_path).unwrap_or_else(|e| {
        eprintln!(
            "could not open DB at {}: {e}\n\
             (launch the side app once so the schema is created, \
              then re-run this harness.)",
            db_path.display()
        );
        std::process::exit(2);
    });

    let bot = db
        .get_bot(&args.bot)
        .expect("DB query for bot")
        .unwrap_or_else(|| {
            eprintln!(
                "bot '{}' not found in {}\n\
                 (use the side app to create it, or pass --bot=<id>)",
                args.bot,
                db_path.display()
            );
            std::process::exit(2);
        });

    let settings = db.load_settings().expect("load settings");
    let provider = provider_for_settings(&settings).unwrap_or_else(|e| {
        eprintln!("could not build provider: {e}");
        std::process::exit(2);
    });

    let messages = vec![
        ChatMessage::System {
            content: date_header_for_now(),
        },
        ChatMessage::User {
            content: args.prompt.clone(),
        },
    ];

    let registry = ToolRegistry::default_with_extras(vec![]);
    let tools = registry.definitions();

    let req = ChatRequest {
        model: provider.default_model().to_string(),
        messages,
        tools,
        temperature: 1.0,
    };

    let stream = provider.stream(req).await.unwrap_or_else(|e| {
        eprintln!("stream init failed: {e}");
        std::process::exit(2);
    });
    let mut pinned = Box::pin(stream);

    let mut text = String::new();
    let mut tool_calls: Vec<serde_json::Value> = vec![];
    let mut finish_reason = String::new();
    let mut errors: Vec<String> = vec![];

    while let Some(chunk) = pinned.next().await {
        match chunk {
            Ok(StreamChunk::Text { delta }) => text.push_str(&delta),
            Ok(StreamChunk::ToolCallDelta {
                id,
                name,
                arguments_delta,
            }) => {
                tool_calls.push(json!({
                    "id": id,
                    "name": name,
                    "arguments_delta": arguments_delta,
                }));
            }
            Ok(StreamChunk::Done { finish_reason: fr }) => {
                finish_reason = fr;
            }
            Err(e) => errors.push(e.to_string()),
        }
    }

    let shell_run_called = tool_calls
        .iter()
        .any(|tc| tc.get("name").and_then(|v| v.as_str()) == Some("shell_run"));

    let out = json!({
        "bot_id": bot.id,
        "bot_name": bot.name,
        "prompt": args.prompt,
        "date_header": date_header_for_now(),
        "model": provider.default_model(),
        "text": text,
        "tool_calls": tool_calls,
        "finish_reason": finish_reason,
        "errors": errors,
        "has_think_block": text.contains("<think>") || text.contains("</think>"),
        "shell_run_called": shell_run_called,
        "any_tool_called": !tool_calls.is_empty(),
    });

    std::fs::write(
        &args.output,
        serde_json::to_string_pretty(&out).unwrap(),
    )
    .unwrap_or_else(|e| {
        eprintln!("could not write {}: {e}", args.output);
        std::process::exit(2);
    });

    println!("wrote {}", args.output);
}
