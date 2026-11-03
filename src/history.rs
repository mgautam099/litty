//! Command history, seeded from your zsh history file and shared by all windows.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_ENTRIES: usize = 5_000;

pub struct History {
    /// Oldest first.
    pub entries: Vec<String>,
}

impl History {
    pub fn load_zsh() -> Self {
        let mut h = Self { entries: Vec::new() };
        if let Some(path) = zsh_history_path() {
            if let Ok(bytes) = std::fs::read(&path) {
                for cmd in parse_zsh_history(&bytes) {
                    h.add(cmd);
                }
            }
        }
        h
    }

    /// Adds a command and appends it to the zsh history file, so it also shows up
    /// in other shells (iTerm2) and after a reboot. A failed write is ignored.
    pub fn record(&mut self, cmd: String) {
        let cmd = cmd.trim().to_string();
        if self.add(cmd.clone()) {
            if let Some(path) = zsh_history_path() {
                let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
                let _ = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .mode(0o600)
                    .open(path)
                    .and_then(|mut f| f.write_all(&history_line(&cmd, secs)));
            }
        }
    }

    /// Adds a command in memory, skipping it if it repeats the previous one (like zsh does).
    /// Returns whether it was added.
    fn add(&mut self, cmd: String) -> bool {
        let cmd = cmd.trim().to_string();
        if cmd.is_empty() || self.entries.last() == Some(&cmd) {
            return false;
        }
        self.entries.push(cmd);
        if self.entries.len() > MAX_ENTRIES {
            let extra = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..extra);
        }
        true
    }
}

fn zsh_history_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("HISTFILE") {
        return Some(PathBuf::from(p));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".zsh_history"))
}

/// One extended-format history line (`: 1700000000:0;command`), metafied the way zsh expects.
fn history_line(cmd: &str, secs: u64) -> Vec<u8> {
    let mut line = format!(": {secs}:0;").into_bytes();
    for &b in cmd.as_bytes() {
        if b == 0 || (0x83..=0xa2).contains(&b) {
            line.extend([0x83, b ^ 0x20]);
        } else {
            line.push(b);
        }
    }
    line.push(b'\n');
    line
}

/// zsh stores some bytes "metafied": 0x83 followed by the real byte XOR 0x20.
fn unmetafy(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut it = bytes.iter();
    while let Some(&b) = it.next() {
        if b == 0x83 {
            if let Some(&n) = it.next() {
                out.push(n ^ 0x20);
            }
        } else {
            out.push(b);
        }
    }
    out
}

/// Parses both plain and extended (`: 1700000000:0;command`) zsh history.
/// Multi-line commands are skipped, since the input box is a single line.
pub fn parse_zsh_history(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(&unmetafy(bytes)).into_owned();
    let mut out = Vec::new();
    let mut current = String::new();
    let mut multiline = false;

    for raw in text.split('\n') {
        let line = if current.is_empty() { strip_extended(raw) } else { raw };
        // zsh writes embedded newlines as a trailing backslash.
        if let Some(cont) = line.strip_suffix('\\') {
            current.push_str(cont);
            current.push('\n');
            multiline = true;
            continue;
        }
        current.push_str(line);
        if !multiline && !current.trim().is_empty() {
            out.push(current.trim().to_string());
        }
        current.clear();
        multiline = false;
    }
    out
}

fn strip_extended(line: &str) -> &str {
    // ": <timestamp>:<duration>;<command>"
    if let Some(rest) = line.strip_prefix(": ") {
        if let Some(idx) = rest.find(';') {
            let meta = &rest[..idx];
            if meta.chars().all(|c| c.is_ascii_digit() || c == ':') {
                return &rest[idx + 1..];
            }
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_extended_and_plain() {
        let data = b": 1700000000:0;npm run dev\n: 1700000001:0;cd ~/proj\nls -la\n";
        assert_eq!(parse_zsh_history(data), ["npm run dev", "cd ~/proj", "ls -la"]);
    }

    #[test]
    fn skips_multiline_commands() {
        let data = b": 1:0;echo a \\\necho b\n: 2:0;git status\n";
        assert_eq!(parse_zsh_history(data), ["git status"]);
    }

    #[test]
    fn unmetafies_utf8() {
        // 0x83 0xA3 is the metafied form of the byte 0x83.
        assert_eq!(unmetafy(&[b'a', 0x83, 0xA3, b'b']), vec![b'a', 0x83, b'b']);
    }

    #[test]
    fn written_line_reads_back() {
        let line = history_line("echo ƒ ✓", 1700000000);
        assert!(line.starts_with(b": 1700000000:0;echo "));
        assert_eq!(parse_zsh_history(&line), ["echo ƒ ✓"]);
    }

    #[test]
    fn dedupes_consecutive() {
        let mut h = History { entries: vec![] };
        h.add("a".into());
        h.add("a".into());
        h.add("b".into());
        h.add("a".into());
        assert_eq!(h.entries, ["a", "b", "a"]);
    }
}
