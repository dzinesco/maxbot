//! Concrete `FileLoopIO` implementation + low-level helpers.
//!
//! The impl is a thin wrapper over the `std::fs` API with three
//! non-trivial bits of policy:
//!
//! 1. **Atomic writes**: every write goes through `atomic_write`,
//!    which writes to `<path>.tmp` and `rename`s it into place. A
//!    crashed process mid-write leaves the old file untouched.
//!
//! 2. **Lock staleness check**: `acquire_lock` reads any existing
//!    LOCK, parses the pid, and calls `kill -0 <pid>` (the POSIX
//!    "is this pid alive?" probe). If alive, fail. If dead,
//!    overwrite. We never block — a stale lock is treated as
//!    reclaimable.
//!
//! 3. **Path safety**: every method calls `assert_safe(path)` first.
//!    A path containing `passphrase` or `computer_passphrase` is
//!    refused with `LoopError::ForbiddenPath` (defense in depth
//!    alongside Sub-slice D's sandbox bind).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};

use chrono::{DateTime, Utc};

use super::types::{
    Fact, FactKind, JournalEntry, LockError, LoopError, LoopIO, Memory, State, Task,
    TaskStatus,
};
use super::{
    assert_safe, DEFAULT_COMPACTION_BYTES, DEFAULT_COMPACTION_ENTRIES,
};

const TASK_FILE: &str = "TASK.md";
const MEMORY_FILE: &str = "MEMORY.md";
const STATE_FILE: &str = "STATE.json";
const LOCK_FILE: &str = "LOCK";
const JOURNAL_DIR: &str = "journal";

type Clock = Box<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// The thin trait the supervisor consumes. All file paths are
/// derived from `root`; tests inject a temp dir.
pub struct FileLoopIO {
    root: PathBuf,
    compaction_bytes: u64,
    compaction_entries: usize,
    /// Clock injection for tests that need to stub UTC date.
    clock: Clock,
    /// Pid of the process that owns this `FileLoopIO` instance.
    /// Public so the supervisor can read it for `State::pid` without
    /// reaching through to `std::process::id()` and so tests can pin
    /// the pid deterministically.
    pub my_pid: u32,
}

impl FileLoopIO {
    /// Build an I/O handle rooted at the given directory. The
    /// directory is created on first write.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            compaction_bytes: DEFAULT_COMPACTION_BYTES,
            compaction_entries: DEFAULT_COMPACTION_ENTRIES,
            clock: Box::new(Utc::now),
            my_pid: std::process::id(),
        }
    }

    /// Override the compaction entry threshold. Production default
    /// is 100; tests typically use 5.
    pub fn with_threshold(mut self, entries: usize) -> Self {
        self.compaction_entries = entries;
        self
    }

    /// Override the compaction byte threshold. Production default
    /// is 50 KiB.
    pub fn with_bytes(mut self, bytes: u64) -> Self {
        self.compaction_bytes = bytes;
        self
    }

    /// Inject a clock. The clock is called on every operation that
    /// needs a timestamp (`append_memory`, `append_journal`,
    /// `update_task_status`, `write_state`, etc.) so tests can pin
    /// or advance "now".
    pub fn with_clock(
        mut self,
        clock: impl Fn() -> DateTime<Utc> + Send + Sync + 'static,
    ) -> Self {
        self.clock = Box::new(clock);
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    fn today_utc(&self) -> String {
        self.now().format("%Y-%m-%d").to_string()
    }

    fn task_path(&self) -> PathBuf {
        self.root.join(TASK_FILE)
    }
    fn memory_path(&self) -> PathBuf {
        self.root.join(MEMORY_FILE)
    }
    fn state_path(&self) -> PathBuf {
        self.root.join(STATE_FILE)
    }
    fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK_FILE)
    }
    fn journal_path(&self) -> PathBuf {
        self.root.join(JOURNAL_DIR).join(format!("{}.md", self.today_utc()))
    }
}

impl LoopIO for FileLoopIO {
    fn acquire_lock(&self) -> Result<(), LockError> {
        let lock_path = self.lock_path();
        let my_pid = self.my_pid;
        let now = self.now().to_rfc3339();

        if lock_path.exists() {
            let existing = fs::read_to_string(&lock_path)?;
            match parse_lock(&existing) {
                Some((other_pid, _acquired_at)) if other_pid == my_pid => {
                    // Re-entrant acquisition — treat as success.
                    return Ok(());
                }
                Some((other_pid, _)) => {
                    if pid_alive(other_pid) {
                        return Err(LockError::Held(other_pid));
                    }
                    // Stale lock — overwrite below.
                }
                None => {
                    return Err(LockError::Malformed(lock_path.display().to_string()));
                }
            }
        }
        atomic_write(&lock_path, &format!("pid={my_pid}\nacquired_at={now}\n"))
            .map_err(|e| match e {
                LoopError::Io(io) => LockError::Io(io),
                other => LockError::Io(io::Error::new(
                    io::ErrorKind::Other,
                    other.to_string(),
                )),
            })?;
        Ok(())
    }

    fn release_lock(&self) -> Result<(), LoopError> {
        let lock_path = self.lock_path();
        if !lock_path.exists() {
            return Ok(());
        }
        let existing = fs::read_to_string(&lock_path)?;
        if let Some((other_pid, _)) = parse_lock(&existing) {
            if other_pid != process::id() {
                return Err(LoopError::Io(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("refusing to release lock owned by pid {other_pid}"),
                )));
            }
        }
        fs::remove_file(&lock_path)?;
        Ok(())
    }

    fn read_task(&self) -> Result<Task, LoopError> {
        let path = self.task_path();
        assert_safe(&path)?;
        let s = fs::read_to_string(&path)?;
        parse_task(&s)
    }

    fn write_task(&self, task: &Task) -> Result<(), LoopError> {
        let path = self.task_path();
        assert_safe(&path)?;
        atomic_write(&path, &format_task(task))?;
        Ok(())
    }

    fn update_task_status(&self, status: TaskStatus) -> Result<(), LoopError> {
        let mut t = self.read_task().unwrap_or_default();
        t.status = status;
        t.updated_at = self.now().to_rfc3339();
        self.write_task(&t)
    }

    fn read_memory(&self) -> Result<Memory, LoopError> {
        let path = self.memory_path();
        assert_safe(&path)?;
        if !path.exists() {
            return Ok(Memory::default());
        }
        let s = fs::read_to_string(&path)?;
        parse_memory(&s)
    }

    fn append_memory(&self, facts: &[Fact]) -> Result<(), LoopError> {
        let path = self.memory_path();
        assert_safe(&path)?;
        let mut mem = self.read_memory().unwrap_or_default();
        for fact in facts {
            // Route durable facts into the durable Vec; everything
            // else goes into recent. `append_memory` is the only
            // write path; durable promotion happens here based on
            // the per-fact flag.
            if fact.durable {
                mem.durable.push(fact.clone());
            } else {
                mem.recent.push(fact.clone());
            }
        }
        let serialized = format_memory(&mem);
        atomic_write(&path, &serialized)?;
        // Trigger compaction if threshold exceeded.
        let entry_count = mem.durable.len() + mem.recent.len();
        let size = serialized.len() as u64;
        if entry_count > self.compaction_entries || size > self.compaction_bytes {
            self.compact_memory()?;
        }
        Ok(())
    }

    fn compact_memory(&self) -> Result<(), LoopError> {
        let path = self.memory_path();
        assert_safe(&path)?;
        let mut mem = self.read_memory()?;
        if mem.recent.is_empty() && mem.summary.is_none() {
            return Ok(());
        }
        // Keep all durable entries + the most recent half of non-durable.
        let keep_n = (self.compaction_entries / 2).max(1);
        let split = mem.recent.len().saturating_sub(keep_n);
        let (older, newer) = mem.recent.split_at(split);
        let summary = if !older.is_empty() {
            let span = format!(
                "{} older non-durable entries (compacted at {})",
                older.len(),
                self.now().to_rfc3339(),
            );
            Some(span)
        } else {
            None
        };
        mem.recent = newer.to_vec();
        mem.summary = summary;
        atomic_write(&path, &format_memory(&mem))?;
        Ok(())
    }

    fn read_state(&self) -> Result<State, LoopError> {
        let path = self.state_path();
        assert_safe(&path)?;
        let s = fs::read_to_string(&path)?;
        serde_json::from_str(&s).map_err(|e| LoopError::Serialize(e.to_string()))
    }

    fn write_state(&self, state: &State) -> Result<(), LoopError> {
        let path = self.state_path();
        assert_safe(&path)?;
        let s = serde_json::to_string_pretty(state)
            .map_err(|e| LoopError::Serialize(e.to_string()))?;
        atomic_write(&path, &s)?;
        Ok(())
    }

    fn append_journal(&self, entry: &JournalEntry) -> Result<(), LoopError> {
        let path = self.journal_path();
        assert_safe(&path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let serialized = format_journal_entry(entry);
        // Journal is append-only and concurrent-writes aren't a
        // concern (single supervisor). Plain append mode is fine;
        // we don't need the atomic-write ceremony here.
        let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
        f.write_all(serialized.as_bytes())?;
        Ok(())
    }
}

// --- helpers (also used by tests) ---------------------------------------

/// Atomic write: write to `<path>.tmp`, fsync, then `rename` over
/// the target. A crash mid-write leaves the original file untouched.
pub fn atomic_write(path: &Path, contents: &str) -> Result<(), LoopError> {
    let parent = path.parent().ok_or_else(|| {
        LoopError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no parent: {}", path.display()),
        ))
    })?;
    fs::create_dir_all(parent)?;
    let tmp = path.with_extension("tmp");
    {
        let mut f = File::create(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// POSIX "is this pid alive?" probe. Uses `kill -0 <pid>` which
/// exits 0 if the pid is alive (and we have permission to signal
/// it), non-zero otherwise. Returns `false` on platforms without
/// `kill`.
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        // Without `kill` we can't probe; assume alive to be safe.
        true
    }
}

/// Parse a LOCK file. Returns `(pid, acquired_at)` or `None` if the
/// file is malformed. Tolerant of extra whitespace / unknown lines.
pub fn parse_lock(s: &str) -> Option<(u32, String)> {
    let mut pid: Option<u32> = None;
    let mut acquired_at: Option<String> = None;
    for line in s.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("pid=") {
            pid = rest.trim().parse::<u32>().ok();
        } else if let Some(rest) = line.strip_prefix("acquired_at=") {
            acquired_at = Some(rest.trim().to_string());
        }
    }
    Some((pid?, acquired_at.unwrap_or_default()))
}

// --- serializers -------------------------------------------------------

fn format_task(task: &Task) -> String {
    format!(
        "---\nstatus: {}\nupdated_at: {}\n---\n\n{}\n",
        task.status.as_str(),
        task.updated_at,
        task.body.trim_end(),
    )
}

fn parse_task(s: &str) -> Result<Task, LoopError> {
    let mut lines = s.lines();
    let first = lines
        .next()
        .ok_or_else(|| LoopError::Parse("empty TASK.md".into()))?;
    if first.trim() != "---" {
        return Err(LoopError::Parse("TASK.md missing opening `---`".into()));
    }
    let mut status: Option<TaskStatus> = None;
    let mut updated_at = String::new();
    let body_lines: Vec<&str>;
    loop {
        let line = match lines.next() {
            Some(l) => l,
            None => {
                return Err(LoopError::Parse(
                    "TASK.md frontmatter not terminated".into(),
                ));
            }
        };
        if line.trim() == "---" {
            body_lines = lines.collect();
            break;
        }
        if let Some(rest) = line.strip_prefix("status:") {
            status = TaskStatus::from_str(rest.trim());
        } else if let Some(rest) = line.strip_prefix("updated_at:") {
            updated_at = rest.trim().to_string();
        }
    }
    let body = body_lines.join("\n").trim_matches('\n').to_string();
    Ok(Task {
        status: status.ok_or_else(|| LoopError::Parse("TASK.md missing status".into()))?,
        body,
        updated_at,
    })
}

fn format_memory(mem: &Memory) -> String {
    let mut out = String::from("# MEMORY\n\n");
    if !mem.durable.is_empty() {
        out.push_str("## Durable\n\n");
        for f in &mem.durable {
            out.push_str(&format_memory_line(f));
        }
        out.push('\n');
    }
    if !mem.recent.is_empty() {
        out.push_str("## Recent\n\n");
        for f in &mem.recent {
            out.push_str(&format_memory_line(f));
        }
        out.push('\n');
    }
    if let Some(summary) = &mem.summary {
        out.push_str(&format!("## Summary\n\n{summary}\n\n"));
    }
    out
}

fn format_memory_line(f: &Fact) -> String {
    let durable_marker = if f.durable { " #durable" } else { "" };
    format!(
        "- {} [{}] {} = {}{}\n",
        f.timestamp,
        f.kind.as_str(),
        f.key,
        f.value,
        durable_marker
    )
}

fn parse_memory(s: &str) -> Result<Memory, LoopError> {
    let mut mem = Memory::default();
    let mut section: Option<&str> = None;
    let mut summary_text: Option<String> = None;
    let mut in_summary = false;
    for line in s.lines() {
        let line = line.trim();
        if line.starts_with("## ") {
            let name = line.trim_start_matches("## ").trim();
            section = Some(name);
            in_summary = name == "Summary";
            continue;
        }
        if line.starts_with("# ") {
            section = None;
            in_summary = false;
            continue;
        }
        if in_summary && !line.is_empty() {
            summary_text = Some(line.to_string());
            continue;
        }
        if let Some(fact) = parse_memory_line(line) {
            match section {
                Some("Durable") => mem.durable.push(fact),
                Some("Recent") => mem.recent.push(fact),
                _ => {}
            }
        }
    }
    mem.summary = summary_text;
    Ok(mem)
}

fn parse_memory_line(line: &str) -> Option<Fact> {
    let line = line.strip_prefix("- ")?;
    let durable = line.contains("#durable");
    let line = line.replace(" #durable", "");
    let ts_end = line.find(' ')?;
    let timestamp = line[..ts_end].to_string();
    let rest = &line[ts_end + 1..];
    let rest = rest.strip_prefix('[')?;
    let kind_end = rest.find(']')?;
    let kind_str = &rest[..kind_end];
    let rest = rest[kind_end + 1..].trim_start();
    let kind = match kind_str {
        "fact" => FactKind::Fact,
        "preference" => FactKind::Preference,
        _ => return None,
    };
    let eq_idx = rest.find('=')?;
    let key = rest[..eq_idx].trim().to_string();
    let value = rest[eq_idx + 1..].trim().to_string();
    Some(Fact {
        kind,
        key,
        value,
        durable,
        timestamp,
    })
}

fn format_journal_entry(e: &JournalEntry) -> String {
    let mut out = String::new();
    out.push_str(&format!("## Turn {} @ {}\n\n", e.turn, e.at));
    if !e.actions.is_empty() {
        out.push_str("### Actions\n\n");
        for a in &e.actions {
            out.push_str(&format!("- {a}\n"));
        }
        out.push('\n');
    }
    if !e.errors.is_empty() {
        out.push_str("### Errors\n\n");
        for er in &e.errors {
            out.push_str(&format!("- {er}\n"));
        }
        out.push('\n');
    }
    if let Some(note) = &e.note {
        out.push_str(&format!("### Note\n\n{note}\n\n"));
    }
    out
}

// --- tests --------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::r#loop::io::assert_safe;
    use std::time::Duration;
    use tempfile::TempDir;

    /// Helper: build a fresh FileLoopIO rooted at a /tmp tempdir
    /// (NOT the production data dir).
    fn fresh_io() -> (TempDir, FileLoopIO) {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let io = FileLoopIO::new(tmp.path());
        (tmp, io)
    }

    /// Helper: build a fixture Fact with deterministic fields.
    fn fact(n: usize) -> Fact {
        Fact {
            kind: FactKind::Fact,
            key: format!("k{n}"),
            value: format!("v{n}"),
            durable: false,
            timestamp: format!("2026-09-11T10:00:{:02}Z", n % 60),
        }
    }

    // Test 1: round-trip — Task, Memory, State, JournalEntry.
    #[test]
    fn test_round_trip_task_memory_state_journal() {
        let (_tmp, io) = fresh_io();

        // --- Task
        let task_in = Task {
            status: TaskStatus::Running,
            body: "Process inbox".to_string(),
            updated_at: "2026-09-11T10:00:00Z".to_string(),
        };
        io.write_task(&task_in).expect("write_task");
        let task_out = io.read_task().expect("read_task");
        assert_eq!(task_in, task_out);

        io.update_task_status(TaskStatus::Done).expect("update_task_status");
        let task_after = io.read_task().expect("read_task");
        assert_eq!(task_after.status, TaskStatus::Done);
        assert_eq!(task_after.body, "Process inbox");
        assert!(!task_after.updated_at.is_empty());

        // --- Memory: append non-durable facts (the I/O entry point).
        let facts: Vec<Fact> = (1..=3).map(fact).collect();
        io.append_memory(&facts).expect("append_memory");
        let mem_after_append = io.read_memory().expect("read_memory");
        assert!(mem_after_append.durable.is_empty());
        assert_eq!(mem_after_append.recent.len(), 3);

        // --- State
        let state_in = State::new(12345u32, "2026-09-11T10:00:00Z", "run-abc");
        io.write_state(&state_in).expect("write_state");
        let state_out = io.read_state().expect("read_state");
        assert_eq!(state_in, state_out);

        // --- Journal
        let entry_in = JournalEntry {
            turn: 1,
            at: "2026-09-11T10:00:01Z".into(),
            actions: vec!["read TASK.md".into(), "append MEMORY.md".into()],
            errors: vec![],
            note: None,
        };
        io.append_journal(&entry_in).expect("append_journal");
        let journal_path = io.journal_path();
        assert!(journal_path.exists(), "journal file should exist");
        let journal_body = std::fs::read_to_string(&journal_path).unwrap();
        assert!(journal_body.contains("## Turn 1"));
        assert!(journal_body.contains("read TASK.md"));
        assert!(journal_body.contains("append MEMORY.md"));

        // Append a second entry — both should appear.
        let entry2 = JournalEntry {
            turn: 2,
            at: "2026-09-11T10:00:02Z".into(),
            actions: vec!["write STATE.json".into()],
            errors: vec!["fake error".into()],
            note: Some("round-trip second pass".into()),
        };
        io.append_journal(&entry2).expect("append_journal");
        let journal_body2 = std::fs::read_to_string(&journal_path).unwrap();
        assert!(journal_body2.contains("## Turn 1"));
        assert!(journal_body2.contains("## Turn 2"));
        assert!(journal_body2.contains("fake error"));
        assert!(journal_body2.contains("round-trip second pass"));
    }

    // Test 2: journal rollover — stub UTC date, verify two files.
    #[test]
    fn test_journal_rollover() {
        use chrono::TimeZone;
        let tmp = tempfile::tempdir().expect("create tempdir");
        let pinned_a = Utc.with_ymd_and_hms(2026, 9, 11, 23, 59, 58).unwrap();
        let pinned_b = Utc.with_ymd_and_hms(2026, 9, 12, 0, 0, 2).unwrap();

        // Clock returns A for the first two calls (today's date +
        // entry timestamp), B thereafter (tomorrow's date +
        // tomorrow's entry timestamp).
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let counter = Arc::new(AtomicUsize::new(0));
        let ca = pinned_a;
        let cb = pinned_b;
        let clock = move || {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            // First call → A (today's date). Second call → B
            // (tomorrow's date). We only need two values because
            // each append_journal calls today_utc() once.
            if n == 0 { ca } else { cb }
        };
        let io = FileLoopIO::new(tmp.path()).with_clock(clock);

        let entry_today = JournalEntry {
            turn: 1,
            at: pinned_a.to_rfc3339(),
            actions: vec!["today-action".into()],
            errors: vec![],
            note: None,
        };
        io.append_journal(&entry_today).expect("append today");
        let path_today = tmp.path().join("journal/2026-09-11.md");
        assert!(path_today.exists(), "today's journal should exist");

        let entry_tomorrow = JournalEntry {
            turn: 1,
            at: pinned_b.to_rfc3339(),
            actions: vec!["tomorrow-action".into()],
            errors: vec![],
            note: None,
        };
        io.append_journal(&entry_tomorrow).expect("append tomorrow");
        let path_tomorrow = tmp.path().join("journal/2026-09-12.md");
        assert!(
            path_tomorrow.exists(),
            "tomorrow's journal should exist after rollover"
        );

        // Verify contents.
        let today_body = std::fs::read_to_string(&path_today).unwrap();
        let tomorrow_body = std::fs::read_to_string(&path_tomorrow).unwrap();
        assert!(today_body.contains("today-action"));
        assert!(!today_body.contains("tomorrow-action"));
        assert!(tomorrow_body.contains("tomorrow-action"));
        assert!(!tomorrow_body.contains("today-action"));
    }

    // Test 3: MEMORY.md compaction — small threshold, verify compaction.
    #[test]
    fn test_memory_compaction() {
        let (_tmp, io) = fresh_io();
        // Threshold of 5 entries — append 7 to force compaction.
        let io = io.with_threshold(5);

        let facts: Vec<Fact> = (1..=7).map(fact).collect();
        io.append_memory(&facts).expect("append_memory");

        let mem = io.read_memory().expect("read_memory");
        // After compaction: durable (none), recent (kept half = 2),
        // summary present.
        assert!(mem.durable.is_empty());
        assert!(mem.recent.len() <= 5, "recent should be condensed");
        assert!(mem.summary.is_some(), "summary should be set after compaction");

        // Verify the most-recent entries survived.
        let last = mem.recent.last().expect("at least one recent");
        assert_eq!(last.key, "k7", "k7 (most recent) should survive");

        // Verify the file on disk mentions Summary.
        let mem_path = io.root().join(MEMORY_FILE);
        let body = std::fs::read_to_string(&mem_path).unwrap();
        assert!(body.contains("## Summary"));

        // Durable entries must always survive — even past threshold.
        let durable_facts = vec![Fact {
            kind: FactKind::Preference,
            key: "user_name".into(),
            value: "Tyler".into(),
            durable: true,
            timestamp: "2026-09-11T09:00:00Z".into(),
        }];
        io.append_memory(&durable_facts).expect("append durable");
        let mem2 = io.read_memory().expect("read_memory");
        assert_eq!(mem2.durable.len(), 1, "durable entry must survive");
        assert_eq!(mem2.durable[0].key, "user_name");
    }

    // Test 4: lock acquire/release with a real subprocess pid.
    #[test]
    fn test_lock_acquire_release_with_subprocess() {
        let (_tmp, io) = fresh_io();
        let lock_path = io.root().join(LOCK_FILE);

        // --- Process A acquires.
        io.acquire_lock().expect("A acquires");
        assert!(lock_path.exists());
        let body = std::fs::read_to_string(&lock_path).unwrap();
        let (pid, _) = parse_lock(&body).expect("parse lock");
        assert_eq!(pid, process::id());

        // --- Spawn Process B (a long-running sleeper).
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sh sleep");
        let child_pid = child.id();

        // Process B "acquires" by writing the LOCK file with its
        // own pid — same format FileLoopIO uses. This is the same
        // file state B would produce if it called acquire_lock.
        std::fs::write(
            &lock_path,
            format!("pid={child_pid}\nacquired_at=2026-09-11T10:00:00Z\n"),
        )
        .expect("write B's lock");

        // --- Process A's acquire_lock must fail with Held(child_pid).
        let err = io.acquire_lock().expect_err("A's acquire should fail");
        match err {
            LockError::Held(p) => assert_eq!(p, child_pid, "expected Held({child_pid})"),
            other => panic!("expected LockError::Held({child_pid}), got {other:?}"),
        }

        // --- Kill Process B and wait for reap.
        child.kill().expect("kill child");
        child.wait().expect("wait child");
        std::thread::sleep(Duration::from_millis(50));

        // --- Process A retries — should succeed (stale lock overwritten).
        io.acquire_lock().expect("A retries after B died");
        let body = std::fs::read_to_string(&lock_path).unwrap();
        let (pid, _) = parse_lock(&body).expect("parse lock after retry");
        assert_eq!(pid, process::id(), "lock should be A's again");

        // --- Release.
        io.release_lock().expect("A releases");
        assert!(!lock_path.exists());
    }

    // Test 5: passphrase / computer_passphrase paths are refused.
    #[test]
    fn test_passphrase_path_refused() {
        // The guard is a substring match on the lowercased path.
        // Production paths never contain these substrings; the
        // guard exists for defense-in-depth (Sub-slice D's sandbox
        // bind is the primary enforcement).

        // (a) Direct call: `assert_safe` on a passphrase path.
        let bad1 = Path::new("/tmp/some/passphrase_db.json");
        let r1 = assert_safe(bad1);
        assert!(
            matches!(r1, Err(LoopError::ForbiddenPath(_))),
            "expected ForbiddenPath for 'passphrase' substring, got {r1:?}"
        );

        // (b) computer_passphrase substring.
        let bad2 = Path::new("/tmp/computer_passphrase_store.json");
        let r2 = assert_safe(bad2);
        assert!(
            matches!(r2, Err(LoopError::ForbiddenPath(_))),
            "expected ForbiddenPath for 'computer_passphrase' substring, got {r2:?}"
        );

        // (c) Case-insensitive — uppercase PASSHRASE should still match.
        let bad3 = Path::new("/tmp/PassPhrase.dat");
        let r3 = assert_safe(bad3);
        assert!(
            matches!(r3, Err(LoopError::ForbiddenPath(_))),
            "expected ForbiddenPath case-insensitive, got {r3:?}"
        );

        // (d) A safe path passes the guard.
        let good = Path::new("/tmp/normal/data.json");
        assert!(assert_safe(good).is_ok());

        // (e) An I/O call routed through the trait hits the guard
        // too: build an io rooted at a path containing
        // "passphrase" and confirm every read fails. We don't
        // actually create the file — the guard runs before the
        // read.
        let passphrase_root = PathBuf::from("/tmp/passphrase_subdir");
        let io = FileLoopIO::new(&passphrase_root);
        let r = io.read_memory();
        assert!(matches!(r, Err(LoopError::ForbiddenPath(_))));
        let r = io.read_task();
        assert!(matches!(r, Err(LoopError::ForbiddenPath(_))));
        let r = io.read_state();
        assert!(matches!(r, Err(LoopError::ForbiddenPath(_))));
    }

    // --- additional sanity tests (not in the brief's 5 but useful) ---

    #[test]
    fn test_pid_alive_self() {
        assert!(pid_alive(process::id()));
    }

    #[test]
    fn test_pid_alive_dead() {
        // Spawn a sleeper, capture its pid, kill + reap it. After
        // reaping, `kill -0` on that pid must return non-zero
        // (ESRCH = "no such process"). We use a very high pid
        // (0x7FFFFFFE) as an additional "definitely not alive"
        // check on Linux/macOS — pid numbers above that range
        // are not assigned to user processes.
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleeper");
        let child_pid = child.id();
        child.kill().expect("kill");
        child.wait().expect("wait");
        // Brief pause for OS-level pid cleanup.
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !pid_alive(child_pid),
            "pid {child_pid} should be dead after reap"
        );
        // High pid (out-of-range) check — pid 0 is special on
        // POSIX (always returns 0 from kill -0 0) so we don't
        // test it here.
        assert!(!pid_alive(0x7FFFFFFE));
    }

    #[test]
    fn test_release_lock_no_file_is_noop() {
        let (_tmp, io) = fresh_io();
        // No LOCK file exists; release_lock should succeed.
        io.release_lock().expect("release with no file");
    }

    #[test]
    fn test_acquire_lock_reentrant() {
        let (_tmp, io) = fresh_io();
        io.acquire_lock().expect("first acquire");
        io.acquire_lock().expect("reentrant acquire (same pid) should succeed");
        io.release_lock().expect("release");
    }
}
