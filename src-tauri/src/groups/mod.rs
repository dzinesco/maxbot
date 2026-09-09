//! v2.4.0 — Multi-Bot group chat executor.
//!
//! A "group" is a 2-6 Bot collaboration. The user types
//! `@BotName` to route a message to a specific member; the
//! Bot's reply is persisted into the group's transcript
//! (`group_messages`) and may include a handoff to another
//! member of the group via the `<handoff to="BotName">…</handoff>`
//! convention.
//!
//! The executor reuses `crate::bots::executor::run_bot_once`
//! rather than re-implementing the LLM tool loop. The bot's
//! "user message" is delivered via the existing
//! `enqueue_bot_message` path (inbox), which the executor
//! already folds into the system prompt. After the bot run
//! completes, we look up the last assistant message in the
//! bot's underlying conversation, copy it into
//! `group_messages`, and run the handoff parse on it.

use std::sync::Arc;

use chrono::Utc;
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::bots::executor::run_bot_once;
use crate::bots::{Bot, BotMessage};
use crate::storage::db::Database;

/// Sentinel `from_bot_id` used for user-sent inbox messages.
/// The executor recognizes this and labels it "user" in the
/// system prompt. Mirrors the constant in
/// `commands::bots::USER_SENDER`; we hardcode the literal
/// here because the `commands` module is private.
const USER_SENDER: &str = "__user__";
use crate::storage::db::GroupChatWithMembers;
use crate::AppState;

/// Minimum number of Bots in a group (including the owner).
/// Plan: 2-6 Bots per group.
pub const MIN_GROUP_SIZE: usize = 2;
/// Maximum number of Bots in a group.
pub const MAX_GROUP_SIZE: usize = 6;
/// How many transcript messages to fold into the bot's
/// inbox context. The plan calls for the last 20.
const HISTORY_CONTEXT_LIMIT: u32 = 20;

/// Errors a group turn can surface. `String` would be
/// simpler, but a typed enum lets the unit tests in this
/// module assert on the variant without regex'ing an
/// English error message.
#[derive(Debug, PartialEq, Eq)]
pub enum GroupError {
    /// `create_group` was called with fewer than 2 or
    /// more than 6 total members (owner + additional).
    BadSize { total: usize },
    /// Group id doesn't match a row in `group_chats`.
    NotFound,
    /// `bot_id` isn't in the group's `group_members` rows.
    NotMember { bot_id: String, group_id: String },
    /// Bot id isn't in the `bots` table.
    BotNotFound { bot_id: String },
    /// No user message in the group's history. (We
    /// require a user message to run a turn.)
    NoUserMessage,
    /// The assistant's reply included a `<handoff to="X">`
    /// tag pointing at a name that doesn't resolve to a
    /// member of this group.
    UnknownHandoffTarget { name: String },
    /// Underlying DB error.
    Db(String),
}

impl std::fmt::Display for GroupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GroupError::BadSize { total } => write!(
                f,
                "group size must be between {MIN} and {MAX} (got {total})",
                MIN = MIN_GROUP_SIZE,
                MAX = MAX_GROUP_SIZE,
            ),
            GroupError::NotFound => write!(f, "group not found"),
            GroupError::NotMember { bot_id, group_id } => {
                write!(f, "bot {bot_id} is not a member of group {group_id}")
            }
            GroupError::BotNotFound { bot_id } => write!(f, "bot {bot_id} not found"),
            GroupError::NoUserMessage => write!(f, "no user message in group history"),
            GroupError::UnknownHandoffTarget { name } => write!(
                f,
                "handoff target '{name}' is not a member of this group"
            ),
            GroupError::Db(e) => write!(f, "db error: {e}"),
        }
    }
}

impl std::error::Error for GroupError {}

/// Validate a group's total member count. Used by both
/// `create_group` (Tauri command) and the unit tests in
/// this module — keeping the rule in one place stops a
/// future change from drifting.
pub fn validate_group_size(total: usize) -> Result<(), GroupError> {
    if total < MIN_GROUP_SIZE || total > MAX_GROUP_SIZE {
        Err(GroupError::BadSize { total })
    } else {
        Ok(())
    }
}

/// Parse a handoff tag out of an assistant reply.
///
/// Format: `<handoff to="BotName">content</handoff>`. The
/// tag is intentionally simple — it's parsed by a Bot
/// (LLM-generated) and the Rust side just needs to find
/// the *first* occurrence and extract the target name +
/// body. Multiple handoffs in one reply are an error case
/// the executor surfaces via the "first wins" rule, but
/// the LLM is expected to emit at most one.
///
/// Returns `None` when no tag is present. The caller is
/// responsible for resolving the name against the
/// group's member list.
pub fn parse_handoff(text: &str) -> Option<(String, String)> {
    // Find the start of the opening tag. The LLM is
    // expected to emit the tag verbatim with the exact
    // attribute order shown in the prompt; we don't try
    // to handle reordered attributes because the bot
    // would then have to do its own parsing, which
    // defeats the point of a fixed-tag convention.
    let open = text.find("<handoff to=\"")?;
    let after_attr = open + "<handoff to=\"".len();
    let close_quote = text[after_attr..].find('"')?;
    let target = &text[after_attr..after_attr + close_quote];
    // Skip past the closing quote + the literal `>` to
    // get to the body.
    let body_start = after_attr + close_quote + "\">".len();
    let end_tag = text[body_start..].find("</handoff>")?;
    let body = &text[body_start..body_start + end_tag];
    Some((target.to_string(), body.to_string()))
}

/// Strip the handoff tag (and any leading/trailing
/// whitespace around it) from the rendered assistant
/// text. The `content` persisted in `group_messages` for
/// `role='assistant'` is the stripped form, so the user
/// sees the natural-language reply without the structural
/// tag noise.
pub fn strip_handoff_tag(text: &str) -> String {
    let Some(open) = text.find("<handoff to=\"") else {
        return text.to_string();
    };
    // Find the end of the closing tag.
    let after_open = open + "<handoff to=\"".len();
    let Some(close_quote) = text[after_open..].find('"') else {
        return text.to_string();
    };
    let body_start = after_open + close_quote + "\">".len();
    let Some(close_rel) = text[body_start..].find("</handoff>") else {
        return text.to_string();
    };
    let body_end = body_start + close_rel + "</handoff>".len();
    let before = &text[..open];
    let after = &text[body_end..];
    // Trim trailing whitespace from `before` and
    // leading whitespace from `after` so we don't
    // leave a double space (or trailing space + leading
    // newline) in the rendered text.
    let before_trim = before.trim_end();
    let after_trim = after.trim_start();
    if before_trim.is_empty() {
        after_trim.to_string()
    } else if after_trim.is_empty() {
        before_trim.to_string()
    } else {
        format!("{before_trim} {after_trim}")
    }
}

/// Resolve a handoff target name to a Bot id. The lookup
/// is case-insensitive (LLMs sometimes capitalize the
/// first letter even when the user typed all-lowercase).
/// Returns `GroupError::UnknownHandoffTarget` if no
/// member matches.
fn resolve_handoff_target(
    name: &str,
    members: &[(String, String)],
) -> Result<String, GroupError> {
    let lower = name.to_lowercase();
    for (id, member_name) in members {
        if member_name.to_lowercase() == lower {
            return Ok(id.clone());
        }
    }
    Err(GroupError::UnknownHandoffTarget { name: name.to_string() })
}

/// Run a single turn for one Bot in a group.
///
/// Loads the group + last 20 messages, validates that
/// `bot_id` is a member, enqueues a context-rich inbox
/// message (so the executor folds the group transcript
/// into the bot's system prompt), then calls
/// `run_bot_once`. After the bot returns we copy the
/// final assistant text into `group_messages` (role =
/// `assistant`), and if the reply contained a handoff
/// tag we also persist a `role='handoff'` row pointing
/// at the resolved target Bot id.
///
/// Returns the new assistant message id, so the renderer
/// can scroll-to / highlight it.
pub async fn run_group_turn(
    app: AppHandle,
    state: Arc<AppState>,
    group_id: String,
    bot_id: String,
    handoff_from_message_id: Option<String>,
) -> Result<String, String> {
    // 1. Load the group.
    let group: GroupChatWithMembers = state
        .db
        .get_group(&group_id)
        .map_err(|e| GroupError::Db(e.to_string()).to_string())?
        .ok_or_else(|| GroupError::NotFound.to_string())?;
    // 2. Verify membership.
    if !group.member_bot_ids.contains(&bot_id) {
        return Err(GroupError::NotMember {
            bot_id: bot_id.clone(),
            group_id: group_id.clone(),
        }
        .to_string());
    }
    // 3. Load the Bot.
    let bot: Bot = state
        .db
        .get_bot(&bot_id)
        .map_err(|e| GroupError::Db(e.to_string()).to_string())?
        .ok_or_else(|| GroupError::BotNotFound { bot_id: bot_id.clone() }.to_string())?;
    // 4. Load the last 20 messages.
    let history = state
        .db
        .list_group_messages(&group_id, HISTORY_CONTEXT_LIMIT)
        .map_err(|e| GroupError::Db(e.to_string()).to_string())?;
    // 5. Find the most-recent user message.
    let last_user = history
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .ok_or_else(|| GroupError::NoUserMessage.to_string())?;
    // 6. Build the inbox body.
    let members_with_names = build_member_name_lookup(&state, &group.member_bot_ids);
    let inbox_body = build_inbox_body(
        &group,
        &members_with_names,
        &history,
        &last_user.content,
        handoff_from_message_id.as_deref(),
    );
    // 7. Enqueue the inbox message (executor will fold
    //    it into the bot's system prompt and mark it
    //    read).
    let bot_msg = BotMessage {
        id: Uuid::new_v4().to_string(),
        from_bot_id: USER_SENDER.to_string(),
        to_bot_id: bot_id.clone(),
        body: inbox_body,
        created_at: Utc::now(),
        read: false,
        conversation_id: None,
    };
    state
        .db
        .enqueue_bot_message(&bot_msg)
        .map_err(|e| GroupError::Db(e.to_string()).to_string())?;
    // 8. Run the bot.
    let output = run_bot_once(
        app.clone(),
        state.clone(),
        bot.clone(),
        CancellationToken::new(),
        None,
        None,
    )
    .await;
    // 9. Pull the bot's last assistant message from
    //    the conversation the executor wrote into.
    let convo_messages = state
        .db
        .list_messages(&output.conversation_id)
        .map_err(|e| GroupError::Db(e.to_string()).to_string())?;
    let last_assistant = convo_messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, crate::storage::MessageRole::Assistant))
        .ok_or_else(|| "bot finished but produced no assistant message".to_string())?;
    let raw_assistant_text = last_assistant.content.clone();
    // 10-12. Parse + strip the handoff and persist the
    //        assistant + handoff rows. Extracted into a
    //        pure helper so unit tests can verify the
    //        persistence shape without an LLM.
    persist_group_turn_result(
        &state.db,
        &group_id,
        &bot_id,
        &raw_assistant_text,
        &last_user.mentions,
        &members_with_names,
    )
    .map_err(|e| e.to_string())
}

/// Persist a finished group turn's rows. The caller
/// supplies the raw assistant text (from the bot's
/// underlying conversation), the mention list to stamp
/// on the assistant row, and the group's member lookup
/// for handoff target resolution. Returns the new
/// assistant row id so the renderer can scroll to it.
///
/// Extracted from `run_group_turn` so unit tests can
/// verify the handoff split without running an LLM.
pub fn persist_group_turn_result(
    db: &Database,
    group_id: &str,
    bot_id: &str,
    raw_assistant_text: &str,
    mentions: &[String],
    members: &[(String, String)],
) -> Result<String, GroupError> {
    let handoff = parse_handoff(raw_assistant_text);
    let rendered_text = strip_handoff_tag(raw_assistant_text);
    let assistant_row = db
        .append_group_message(
            group_id,
            Some(bot_id),
            "assistant",
            &rendered_text,
            mentions,
            None,
        )
        .map_err(|e| GroupError::Db(e.to_string()))?;
    if let Some((target_name, handoff_body)) = handoff {
        let target_id = resolve_handoff_target(&target_name, members)?;
        db.append_group_message(
            group_id,
            Some(bot_id),
            "handoff",
            &handoff_body,
            &[],
            Some(&target_id),
        )
        .map_err(|e| GroupError::Db(e.to_string()))?;
    }
    Ok(assistant_row.id)
}

/// Build the inbox body that gets folded into the bot's
/// system prompt. Kept as a free function so unit tests
/// can verify the format without a DB.
fn build_inbox_body(
    group: &GroupChatWithMembers,
    members: &[(String, String)],
    history: &[crate::storage::db::GroupMessage],
    last_user_body: &str,
    handoff_from_message_id: Option<&str>,
) -> String {
    let mut s = String::new();
    s.push_str("<GROUP_CTX>\n");
    s.push_str(&format!("Group: {}\n", group.chat.name));
    s.push_str("Members:\n");
    for (id, name) in members {
        s.push_str(&format!("- @{} (id {})\n", name, id));
    }
    s.push_str("\nRecent transcript (oldest first, last 20):\n");
    for m in history {
        let speaker = m.bot_id.as_deref().unwrap_or("user");
        let prefix = match m.role.as_str() {
            "handoff" => format!(
                "[handoff to {}] {}",
                m.handoff_to.as_deref().unwrap_or("?"),
                m.content
            ),
            "user" => format!("[user] {}", m.content),
            _ => format!("[@{}] {}", speaker, m.content),
        };
        s.push_str(&prefix);
        s.push('\n');
    }
    s.push_str(&format!("\nYour task: {}\n", last_user_body));
    if let Some(hid) = handoff_from_message_id {
        if let Some(h) = history.iter().find(|m| m.id == hid) {
            s.push_str(&format!(
                "\nYou are being handed off from another Bot. Their notes:\n{}\n",
                h.content
            ));
        }
    }
    s.push_str("</GROUP_CTX>\n");
    s.push_str("\nRespond in character. If you need to hand off to another group member, end your reply with exactly one tag of the form `<handoff to=\"BotName\">notes for the next Bot</handoff>`.");
    s
}

/// Resolve member ids to (id, name) pairs. Missing rows
/// are skipped silently — a stale member id is benign
/// here (the executor would have rejected it in
/// `run_group_turn` already).
fn build_member_name_lookup(state: &Arc<AppState>, ids: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Ok(Some(b)) = state.db.get_bot(id) {
            out.push((b.id, b.name));
        }
    }
    out
}

// ---- Tests ----

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_group_size_accepts_2_to_6() {
        assert!(validate_group_size(2).is_ok());
        assert!(validate_group_size(3).is_ok());
        assert!(validate_group_size(4).is_ok());
        assert!(validate_group_size(5).is_ok());
        assert!(validate_group_size(6).is_ok());
    }

    #[test]
    fn validate_group_size_rejects_outside_2_to_6() {
        // Plan acceptance: 1 and 7 must error.
        assert_eq!(validate_group_size(1), Err(GroupError::BadSize { total: 1 }));
        assert_eq!(validate_group_size(7), Err(GroupError::BadSize { total: 7 }));
        // 0 and 100 for good measure.
        assert_eq!(validate_group_size(0), Err(GroupError::BadSize { total: 0 }));
        assert_eq!(
            validate_group_size(100),
            Err(GroupError::BadSize { total: 100 })
        );
    }

    #[test]
    fn parse_handoff_returns_target_bot_id_and_content() {
        let text = "Here are the notes.\n\n<handoff to=\"Writer\">please polish</handoff>\n";
        let (target, body) = parse_handoff(text).expect("handoff present");
        assert_eq!(target, "Writer");
        assert_eq!(body, "please polish");
    }

    #[test]
    fn parse_handoff_returns_none_when_absent() {
        assert!(parse_handoff("no tag here").is_none());
        assert!(parse_handoff("").is_none());
    }

    #[test]
    fn strip_handoff_tag_removes_tag_and_joins_cleanly() {
        let text = "Here are the notes.\n\n<handoff to=\"Writer\">please polish</handoff>\n";
        let stripped = strip_handoff_tag(text);
        assert_eq!(stripped, "Here are the notes.");
    }

    #[test]
    fn strip_handoff_tag_returns_text_unchanged_when_no_tag() {
        assert_eq!(strip_handoff_tag("hello"), "hello");
    }

    #[test]
    fn resolve_handoff_target_is_case_insensitive() {
        let members = vec![
            ("b1".to_string(), "Researcher".to_string()),
            ("b2".to_string(), "Writer".to_string()),
        ];
        assert_eq!(resolve_handoff_target("writer", &members).unwrap(), "b2");
        assert_eq!(resolve_handoff_target("WRITER", &members).unwrap(), "b2");
        assert_eq!(resolve_handoff_target("Researcher", &members).unwrap(), "b1");
    }

    #[test]
    fn parse_handoff_rejects_unknown_target() {
        // Plan: a `<handoff to="NotAMember">` against a
        // group where no member has that name should
        // surface `GroupError::UnknownHandoffTarget`.
        let members = vec![("b1".to_string(), "Researcher".to_string())];
        let err = resolve_handoff_target("NotAMember", &members).unwrap_err();
        assert_eq!(
            err,
            GroupError::UnknownHandoffTarget {
                name: "NotAMember".to_string()
            }
        );
    }

    /// Build a fresh test DB so the persistence-level
    /// test can exercise the real `append_group_message`
    /// + `list_group_messages` round-trip. Mirrors the
    /// `fresh_db` helper in `db.rs` — duplicated here
    /// because the helpers are private to their module.
    fn fresh_db() -> Database {
        let dir = std::env::temp_dir().join(format!("maxbot-groups-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("groups-test.sqlite");
        Database::open(&path).expect("open test db")
    }

    /// Seed a minimal bot row. `append_group_message`
    /// uses `bot_id REFERENCES bots(id) ON DELETE SET NULL`
    /// so a missing bot is fine for the FK, but the
    /// `members` lookup the executor uses still needs
    /// real names. We create two Bots so the test can
    /// hand off between them.
    fn seed_bot(db: &Database, id: &str, name: &str) {
        let now = chrono::Utc::now();
        let bot = crate::bots::Bot {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            system_prompt: String::new(),
            default_model: "MiniMax-M3".to_string(),
            allowed_tools: vec![],
            icon: String::new(),
            color: String::new(),
            avatar_color: String::new(),
            last_active_at: None,
            state: crate::bots::BotState::Idle,
            created_at: now,
            updated_at: now,
        };
        db.upsert_bot(&bot).expect("upsert test bot");
    }

    /// Plan acceptance: a Bot reply that contains a
    /// `<handoff to="Writer">…</handoff>` tag should
    /// produce two rows in `group_messages` — one
    /// `role='assistant'` row with the stripped text,
    /// and one `role='handoff'` row pointing at the
    /// resolved target Bot id with the tag's body.
    #[test]
    fn run_group_turn_persists_assistant_and_handoff_rows() {
        let db = fresh_db();
        seed_bot(&db, "writer_id", "Writer");
        let group = db
            .create_group("Research pod", "writer_id", &[])
            .expect("create group");
        let members = vec![("writer_id".to_string(), "Writer".to_string())];
        let raw = "Here are the Q3 numbers.\n\n<handoff to=\"Writer\">please draft the email</handoff>\n";
        let mentions = vec!["writer_id".to_string()];
        let assistant_id = persist_group_turn_result(
            &db,
            &group.chat.id,
            "writer_id",
            raw,
            &mentions,
            &members,
        )
        .expect("persist should succeed");
        // The assistant row exists with the stripped text.
        let rows = db
            .list_group_messages(&group.chat.id, 0)
            .expect("list messages");
        assert_eq!(rows.len(), 2, "expected assistant + handoff rows");
        let assistant = rows
            .iter()
            .find(|m| m.role == "assistant")
            .expect("assistant row");
        assert_eq!(assistant.id, assistant_id);
        assert_eq!(assistant.bot_id.as_deref(), Some("writer_id"));
        assert_eq!(assistant.content, "Here are the Q3 numbers.");
        assert_eq!(assistant.handoff_to, None);
        assert_eq!(assistant.mentions, vec!["writer_id".to_string()]);
        let handoff = rows
            .iter()
            .find(|m| m.role == "handoff")
            .expect("handoff row");
        assert_eq!(handoff.bot_id.as_deref(), Some("writer_id"));
        assert_eq!(handoff.content, "please draft the email");
        assert_eq!(handoff.handoff_to.as_deref(), Some("writer_id"));
    }

    /// A reply with no handoff tag should produce a
    /// single `role='assistant'` row. The persistence
    /// helper must not invent a handoff row.
    #[test]
    fn run_group_turn_without_handoff_persists_only_assistant() {
        let db = fresh_db();
        seed_bot(&db, "writer_id", "Writer");
        let group = db
            .create_group("Research pod", "writer_id", &[])
            .expect("create group");
        let members = vec![("writer_id".to_string(), "Writer".to_string())];
        let raw = "Here are the Q3 numbers.";
        persist_group_turn_result(
            &db,
            &group.chat.id,
            "writer_id",
            raw,
            &[],
            &members,
        )
        .expect("persist should succeed");
        let rows = db
            .list_group_messages(&group.chat.id, 0)
            .expect("list messages");
        assert_eq!(rows.len(), 1, "no handoff tag → no handoff row");
        assert_eq!(rows[0].role, "assistant");
        assert_eq!(rows[0].content, "Here are the Q3 numbers.");
        assert_eq!(rows[0].handoff_to, None);
    }
}
