//! Turns raw bytes from a command into plain text lines.
//!
//! This is not a full terminal: colors and cursor movement codes are stripped,
//! and a carriage return (`\r`, used by progress bars) rewrites the current line.

use std::collections::VecDeque;

/// Oldest lines are dropped past this point, so a window left open all day
/// doesn't keep growing in memory (about 1 MB per window at typical line lengths).
pub const MAX_LINES: usize = 10_000;
/// Very long lines are broken here so drawing stays fast.
const MAX_LINE_CHARS: usize = 4_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Output from the command.
    Out,
    /// The command line the user ran (shown like a prompt).
    Cmd,
    /// Status notes such as "[exited: 0]".
    Info,
}

#[derive(Debug)]
pub struct Line {
    pub text: String,
    pub kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Esc {
    None,
    Esc,
    Csi,
    /// OSC / DCS / other string sequences, ended by BEL or ESC \
    Str,
    StrEsc,
    SkipOne,
}

pub struct Output {
    pub lines: VecDeque<Line>,
    /// Whether the last line is still receiving command output.
    open: bool,
    /// A `\r` was seen: the next printable character overwrites the line.
    cr: bool,
    esc: Esc,
    /// Bytes of a UTF-8 character that was split across two reads.
    partial: Vec<u8>,
}

impl Output {
    pub fn new() -> Self {
        Self {
            lines: VecDeque::new(),
            open: false,
            cr: false,
            esc: Esc::None,
            partial: Vec::new(),
        }
    }

    pub fn push_line(&mut self, kind: Kind, text: String) {
        self.close_line();
        self.push(Line { text, kind });
    }

    /// Ends the current output line (used before writing status lines).
    pub fn close_line(&mut self) {
        self.open = false;
        self.cr = false;
        self.esc = Esc::None;
        self.partial.clear();
    }

    fn push(&mut self, line: Line) {
        self.lines.push_back(line);
        while self.lines.len() > MAX_LINES {
            self.lines.pop_front();
        }
    }

    fn current(&mut self) -> &mut String {
        if !self.open {
            self.push(Line { text: String::new(), kind: Kind::Out });
            self.open = true;
        }
        &mut self.lines.back_mut().expect("line was just pushed").text
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        let mut data = std::mem::take(&mut self.partial);
        data.extend_from_slice(bytes);

        let mut rest: &[u8] = &data;
        loop {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    self.feed_str(s);
                    break;
                }
                Err(e) => {
                    let (good, bad) = rest.split_at(e.valid_up_to());
                    // SAFETY-free: `good` was just validated by from_utf8.
                    self.feed_str(std::str::from_utf8(good).unwrap_or_default());
                    match e.error_len() {
                        // Incomplete character at the end: keep it for the next read.
                        None => {
                            self.partial = bad.to_vec();
                            break;
                        }
                        Some(n) => {
                            self.feed_char('\u{FFFD}');
                            rest = &bad[n..];
                        }
                    }
                }
            }
        }
    }

    fn feed_str(&mut self, s: &str) {
        for c in s.chars() {
            self.feed_char(c);
        }
    }

    fn feed_char(&mut self, c: char) {
        match self.esc {
            Esc::None => {}
            Esc::Esc => {
                self.esc = match c {
                    '[' => Esc::Csi,
                    ']' | 'P' | 'X' | '^' | '_' => Esc::Str,
                    '(' | ')' | '*' | '+' | '#' | '%' => Esc::SkipOne,
                    _ => Esc::None,
                };
                return;
            }
            Esc::Csi => {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    self.esc = Esc::None;
                }
                return;
            }
            Esc::Str => {
                match c {
                    '\u{07}' => self.esc = Esc::None,
                    '\u{1b}' => self.esc = Esc::StrEsc,
                    _ => {}
                }
                return;
            }
            Esc::StrEsc => {
                self.esc = if c == '\\' { Esc::None } else { Esc::Str };
                return;
            }
            Esc::SkipOne => {
                self.esc = Esc::None;
                return;
            }
        }

        match c {
            '\u{1b}' => self.esc = Esc::Esc,
            '\u{9b}' => self.esc = Esc::Csi,
            '\n' => {
                if !self.open {
                    // An empty line in the output.
                    self.current();
                }
                self.open = false;
                self.cr = false;
            }
            '\r' => self.cr = true,
            '\u{08}' => {
                if self.open {
                    self.current().pop();
                }
            }
            '\t' => {
                let line = self.take_cr();
                let spaces = 8 - (line.chars().count() % 8);
                line.extend(std::iter::repeat_n(' ', spaces));
            }
            c if c.is_control() => {}
            c => {
                let line = self.take_cr();
                if line.len() >= MAX_LINE_CHARS {
                    self.open = false;
                }
                self.current().push(c);
            }
        }
    }

    /// Current line, cleared first if a carriage return is pending.
    fn take_cr(&mut self) -> &mut String {
        let cr = std::mem::take(&mut self.cr);
        let line = self.current();
        if cr {
            line.clear();
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(o: &Output) -> Vec<&str> {
        o.lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn strips_colors_and_handles_crlf() {
        let mut o = Output::new();
        o.feed(b"\x1b[32mhello\x1b[0m\r\nworld\r\n");
        assert_eq!(texts(&o), ["hello", "world"]);
    }

    #[test]
    fn carriage_return_overwrites_progress() {
        let mut o = Output::new();
        o.feed(b"10%\r50%\r\x1b[K100%\r\ndone\n");
        assert_eq!(texts(&o), ["100%", "done"]);
    }

    #[test]
    fn utf8_split_across_reads() {
        let mut o = Output::new();
        let s = "héllo ✓\n".as_bytes();
        o.feed(&s[..2]);
        o.feed(&s[2..9]);
        o.feed(&s[9..]);
        assert_eq!(texts(&o), ["héllo ✓"]);
    }

    #[test]
    fn osc_title_is_removed_and_blank_lines_kept() {
        let mut o = Output::new();
        o.feed(b"\x1b]0;title\x07a\n\nb\n");
        assert_eq!(texts(&o), ["a", "", "b"]);
    }

    #[test]
    fn caps_line_count() {
        let mut o = Output::new();
        for i in 0..(MAX_LINES + 50) {
            o.feed(format!("{i}\n").as_bytes());
        }
        assert_eq!(o.lines.len(), MAX_LINES);
        assert_eq!(o.lines.back().unwrap().text, (MAX_LINES + 49).to_string());
    }
}
