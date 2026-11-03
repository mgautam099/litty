# litty

Very lightweight macOS app in Rust (eframe/egui 0.37, glow backend, libc) that replaces iTerm2 for one use: after each reboot the owner opens ~3 windows, runs one long-running command in each, and leaves them until shutdown. Low memory use is the reason it exists.

It is deliberately NOT a terminal emulator: an input line plus plain-text command output. See `README.md` for behavior and keys.

## Working rules (from the owner)

- Honesty first: say plainly what works, what doesn't, and what is unverified.
- If something is unclear, ask. Don't assume.
- No extra features. Keep it minimal and lightweight. New dependencies need a reason.
- Don't commit or push unless asked. Public repo: https://github.com/mgautam099/litty (MIT). The local repo has its own git identity (GitHub noreply email); don't change it.
- Commit messages: one line, `<gitmoji> <type>(<scope>): <description>`, e.g. `🐛 fix(ui): the distortion of certain elements`. Gitmoji: ✨ feat, 🐛 fix, 📝 docs, ♻️ refactor, 🔧 chore/config, ✅ test. Lowercase description, scope optional. No body, no extra detail, no `Co-Authored-By` trailer.
- Ask before running `./make-app.sh` (it replaces `~/Applications/litty.app`) or touching `~/.zsh_history` by hand.

## Commands

```sh
cargo test                 # 14 tests, includes real zsh runs (output, cd, exit codes, PTY size, Ctrl+C, hang-up)
cargo build --release
cargo run --release        # run without installing
./make-app.sh              # build + install ~/Applications/litty.app (builds icns from icon.png, ad-hoc codesign)
```

Rust comes from Homebrew (no rustup). The Mac is an Apple Silicon M4.

## Files

- `src/main.rs` — App, Pane (one per window), keys, window placement, quit prompt
- `src/process.rs` — PTY spawn, reader thread, Ctrl+C, resize, cleanup; process tests
- `src/output.rs` — byte-to-line parser (strips ANSI, `\r` rewrites line); tests
- `src/history.rs` — zsh history read/append (extended format, metafied bytes); tests
- `make-app.sh`, `icon.png` — packaging; the icon is a generated placeholder
- `LICENSE` (MIT), `README.md` (public, written for people who clone the repo)

## Design decisions and why

- Each command runs as `/bin/zsh -l -i -c` on a real PTY so `.zshrc`, aliases and PATH work and output isn't buffered. TERM=dumb, PAGER=cat, GIT_PAGER=cat.
- `cd` carries over via a temp file the wrapper script writes (`pwd`); `export` does not.
- Window 1 is the root viewport, others are deferred viewports. Closing the root while others are open hides it (CancelClose + Visible(false)); the app exits when the last window closes.
- Cmd+N copies the current window's placement: cascade +28pt, or fullscreen/maximized. egui stops updating `maximized` after startup on macOS, so "maximized" is detected as covering >=90% width and >=80% height of the monitor (see `next_window`).
- macOS default menu is disabled (`with_default_menu(false)` in main.rs, needs the macOS-only `winit` dep). Reason: the menu's Cmd+Q terminates the app and can't be intercepted; with it off, Cmd+Q arrives as a key event and shows a confirmation. Side effects: no Cmd+H, no menu bar.
- `eframe` is built with `default-features = false`. The `clipboard` feature MUST stay on, otherwise copy/paste only works inside litty (egui falls back to an in-app clipboard).
- The `[done]` line is not printed on success (normal terminals don't). Non-zero exit and signals still print a note.
- Commands run are appended to `~/.zsh_history` (owner's choice, since litty replaces iTerm2). Writes are best-effort.
- `icon.png` + `with_icon(IconData::default())`: the empty icon stops eframe from setting the egui logo at runtime, so the bundle icon shows.
- Cursor blink is off so the app uses no CPU when idle.

## Verified vs not

Verified on the owner's Mac: tests pass, release build, running the app and the installed bundle, new-window cascade, Cmd+Q prompt, copy/paste with the system clipboard, history append.

Memory, reported by the owner (not measured by me): 3 windows each running a long command hovered at 70-79 MB. One idle window measured by me at ~50 MB (`footprint`), so each extra window adds roughly 10-15 MB. The owner's tool was presumably Activity Monitor, which may differ slightly from `footprint`.

Icon confirmed by the owner in Finder, Dock and the application menu.

Not verified: a window full of 10,000 lines (memory estimated at 1-2 MB, not measured). Quit-while-hidden-root and Dock > Quit paths are lightly tested at best; Dock > Quit bypasses the prompt.

## Memory notes

Measured with `footprint -p <pid>`: ~50 MB, nearly all macOS (OpenGL/Metal driver, AppKit, window IOSurface). egui + our code ~2 MB (font atlas). Dropping fonts, opt-level changes or a smaller binary save <1 MB because fonts are read-only data in the binary. A lower-resolution window buffer would blur text. The remaining lever is a smaller default window (`WINDOW_SIZE`); not done.

## Possible later work (not requested)

- Replace the placeholder icon with the owner's own.
- Cmd+H / a minimal menu, only if the owner wants it back.
