//! Minimal `.env` loader. Reads KEY=VALUE pairs from a small set of
//! well-known paths and returns them as a `HashMap`. The first file that
//! exists wins; later files are ignored. If `$MAXBOT_ENV` is set, that
//! file is checked first.
//!
//! We don't pull in `dotenvy` for this — the format is dead simple
//! (KEY=VALUE, one per line, `#` for comments, optional surrounding
//! quotes) and a hand-rolled parser keeps the dependency surface small.
//!
//! Candidate search order:
//! 1. `$MAXBOT_ENV` (full path to a `.env` file)
//! 2. `./.env` (current working directory — works for `cargo tauri dev`)
//! 3. `~/.maxbot.env` (general home-dir location)
//! 4. `~/dev/maxbot/maxbot/.env` (Tyler's dev project root)

use std::collections::HashMap;
use std::path::PathBuf;

pub fn load_dotenv() -> HashMap<String, String> {
    let mut out = HashMap::new();
    for path in candidate_paths() {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                log::info!("env_loader: loaded .env from {}", path.display());
                parse_env_text(&text, &mut out);
                return out;
            }
            Err(_) => continue,
        }
    }
    out
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("MAXBOT_ENV").map(PathBuf::from) {
        paths.push(p);
    }
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join(".env"));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        paths.push(home.join(".maxbot.env"));
        paths.push(home.join("dev/maxbot/maxbot/.env"));
    }
    paths
}

fn parse_env_text(text: &str, out: &mut HashMap<String, String>) {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim().to_string();
        if key.is_empty() {
            continue;
        }
        // Strip an optional `export ` prefix some folks leave in.
        let key = key.strip_prefix("export ").unwrap_or(&key).to_string();
        let value = v
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        out.insert(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_key_values() {
        let mut out = HashMap::new();
        parse_env_text(
            "FOO=bar\n# comment\nBAZ=\"qux\"\n   SPACED = 'hi'\n",
            &mut out,
        );
        assert_eq!(out.get("FOO").unwrap(), "bar");
        assert_eq!(out.get("BAZ").unwrap(), "qux");
        assert_eq!(out.get("SPACED").unwrap(), "hi");
    }

    #[test]
    fn strips_export_prefix() {
        let mut out = HashMap::new();
        parse_env_text("export FOO=bar\n", &mut out);
        assert_eq!(out.get("FOO").unwrap(), "bar");
    }
}
