//! Runs one command through zsh on a pseudo-terminal (PTY).
//!
//! A PTY is used (instead of plain pipes) so programs flush their output line by
//! line as they would in iTerm2, and so Ctrl+C reaches them the normal way.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::output::{Kind, Output};

// ioctl numbers. The libc crate doesn't reliably export these for macOS,
// so the fixed macOS values (from <sys/ttycom.h>) are spelled out here.
#[cfg(target_os = "macos")]
const PTY_SET_CTTY: libc::c_ulong = 0x2000_7461; // TIOCSCTTY
#[cfg(target_os = "macos")]
const PTY_SET_SIZE: libc::c_ulong = 0x8008_7467; // TIOCSWINSZ
#[cfg(not(target_os = "macos"))]
const PTY_SET_CTTY: libc::c_ulong = libc::TIOCSCTTY as libc::c_ulong;
#[cfg(not(target_os = "macos"))]
const PTY_SET_SIZE: libc::c_ulong = libc::TIOCSWINSZ as libc::c_ulong;

/// State shared between a window and the thread reading its command's output.
pub struct PaneState {
    pub output: Output,
    pub running: bool,
    pub cwd: PathBuf,
}

pub struct Proc {
    master: Arc<File>,
    pid: i32,
    interrupts: u32,
    /// Cleared once the command has exited, so we never signal a reused pid.
    alive: Arc<AtomicBool>,
}

/// Runs the command, saves the final directory so `cd` carries over, and
/// keeps the command's exit code.
const SCRIPT: &str = r#"eval "$LITTY_CMD"; __lt_rc=$?; pwd >| "$LITTY_CWD_FILE" 2>/dev/null; exit $__lt_rc"#;

fn shell() -> String {
    if Path::new("/bin/zsh").exists() {
        return "/bin/zsh".into();
    }
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}

static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn spawn(
    cmd: &str,
    state: Arc<Mutex<PaneState>>,
    cols: u16,
    rows: u16,
    wake: impl Fn() + Send + 'static,
) -> io::Result<Proc> {
    let cwd = state.lock().unwrap().cwd.clone();

    let mut master_fd: libc::c_int = -1;
    let mut slave_fd: libc::c_int = -1;
    let mut ws = winsize(cols, rows);
    // SAFETY: plain libc call with valid out-pointers.
    let rc = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut ws,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty succeeded, so both fds are open and owned by us.
    let master = unsafe { OwnedFd::from_raw_fd(master_fd) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave_fd) };
    set_cloexec(&master);
    set_cloexec(&slave);

    let cwd_file = std::env::temp_dir().join(format!(
        "litty-{}-{}.cwd",
        std::process::id(),
        RUN_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let mut command = Command::new(shell());
    command
        .args(["-l", "-i", "-c", SCRIPT])
        .env("LITTY_CMD", cmd)
        .env("LITTY_CWD_FILE", &cwd_file)
        // We show plain text, so ask programs not to draw fancy terminal UI,
        // and never open a pager (it would wait for keys we don't send).
        .env("TERM", "dumb")
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat")
        .current_dir(&cwd)
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, PTY_SET_CTTY as _, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    drop(command); // closes our copies of the slave end

    let master = Arc::new(File::from(master));
    let pid = child.id() as i32;

    let reader = Arc::clone(&master);
    let alive = Arc::new(AtomicBool::new(true));
    let alive_reader = Arc::clone(&alive);
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match (&*reader).read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    state.lock().unwrap().output.feed(&buf[..n]);
                    wake();
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break, // EIO once every process has closed the PTY
            }
        }
        let status = child.wait();
        alive_reader.store(false, Ordering::SeqCst);
        let new_cwd = std::fs::read_to_string(&cwd_file)
            .ok()
            .map(|s| PathBuf::from(s.trim_end_matches('\n')))
            .filter(|p| p.is_dir());
        let _ = std::fs::remove_file(&cwd_file);

        // Success prints nothing, like a normal terminal.
        let note = match status {
            Ok(s) => match (s.code(), s.signal()) {
                (Some(0), _) => None,
                (Some(c), _) => Some(format!("[exited with code {c}]")),
                (None, Some(sig)) => Some(format!("[stopped by signal {sig}]")),
                _ => Some("[exited]".to_string()),
            },
            Err(e) => Some(format!("[could not read exit status: {e}]")),
        };
        let mut st = state.lock().unwrap();
        if let Some(note) = note {
            st.output.push_line(Kind::Info, note);
        }
        if let Some(dir) = new_cwd {
            st.cwd = dir;
        }
        st.running = false;
        drop(st);
        wake();
    });

    Ok(Proc { master, pid, interrupts: 0, alive })
}

impl Proc {
    /// First Ctrl+C sends an interrupt like a normal terminal.
    /// Pressing it again force-kills the command.
    pub fn interrupt(&mut self) {
        self.interrupts += 1;
        if self.interrupts == 1 {
            let _ = (&*self.master).write_all(&[0x03]);
        } else {
            self.signal(libc::SIGKILL);
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let ws = winsize(cols, rows);
        // SAFETY: valid fd and pointer.
        unsafe {
            libc::ioctl(fd(&self.master), PTY_SET_SIZE as _, &ws);
        }
    }

    fn signal(&self, sig: libc::c_int) {
        if !self.alive.load(Ordering::SeqCst) {
            return;
        }
        // SAFETY: plain libc calls; failures (process already gone) are ignored.
        unsafe {
            let fg = libc::tcgetpgrp(fd(&self.master));
            if fg > 0 {
                libc::killpg(fg, sig);
            }
            libc::killpg(self.pid, sig);
        }
    }
}

impl Drop for Proc {
    /// Closing a window hangs up its command, the same as closing an iTerm2 tab.
    fn drop(&mut self) {
        self.signal(libc::SIGHUP);
    }
}

fn fd(f: &File) -> libc::c_int {
    use std::os::fd::AsRawFd;
    f.as_raw_fd()
}

fn set_cloexec(f: &OwnedFd) {
    use std::os::fd::AsRawFd;
    // SAFETY: valid fd.
    unsafe {
        libc::fcntl(f.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
    }
}

fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize { ws_row: rows.max(1), ws_col: cols.max(1), ws_xpixel: 0, ws_ypixel: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn state() -> Arc<Mutex<PaneState>> {
        Arc::new(Mutex::new(PaneState {
            output: Output::new(),
            running: true,
            cwd: PathBuf::from("/"),
        }))
    }

    fn wait_done(st: &Arc<Mutex<PaneState>>, secs: u64) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(secs) {
            if !st.lock().unwrap().running {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn text(st: &Arc<Mutex<PaneState>>) -> Vec<String> {
        st.lock().unwrap().output.lines.iter().map(|l| l.text.clone()).collect()
    }

    #[test]
    fn runs_command_and_tracks_cd() {
        let st = state();
        let _p = spawn("echo hello; printf 'a\\tb\\n'; cd /tmp; ls -d .", Arc::clone(&st), 80, 24, || {}).unwrap();
        assert!(wait_done(&st, 10));
        let t = text(&st);
        assert!(t.contains(&"hello".to_string()), "{t:?}");
        assert!(t.contains(&"a       b".to_string()), "{t:?}");
        assert_eq!(t.last().unwrap(), "."); // no trailing status line on success
        assert_eq!(st.lock().unwrap().cwd, PathBuf::from("/tmp"));
    }

    #[test]
    fn reports_exit_code_and_window_size() {
        let st = state();
        let _p = spawn("stty size; exit 3", Arc::clone(&st), 99, 33, || {}).unwrap();
        assert!(wait_done(&st, 10));
        let t = text(&st);
        assert!(t.contains(&"33 99".to_string()), "{t:?}");
        assert_eq!(t.last().unwrap(), "[exited with code 3]");
    }

    #[test]
    fn ctrl_c_stops_long_running_command() {
        let st = state();
        let mut p = spawn("echo start; sleep 60; echo never", Arc::clone(&st), 80, 24, || {}).unwrap();
        let t0 = Instant::now();
        while !text(&st).contains(&"start".to_string()) {
            assert!(t0.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(20));
        }
        p.interrupt();
        assert!(wait_done(&st, 5), "Ctrl+C did not stop it: {:?}", text(&st));
        assert!(!text(&st).contains(&"never".to_string()));
    }

    #[test]
    fn closing_hangs_up_command() {
        let st = state();
        let p = spawn("sleep 60", Arc::clone(&st), 80, 24, || {}).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        drop(p);
        assert!(wait_done(&st, 5));
    }
}
