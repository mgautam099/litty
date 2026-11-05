# litty

A tiny window for running long-lived commands. Each window runs one command at a time through zsh and shows its output as plain text. It is not a full terminal emulator: no colors, no cursor control, no interactive programs.

## Keys

| Key | What it does |
|---|---|
| Enter | Run the command you typed |
| Up / Down | Step through history (loaded from `~/.zsh_history`; commands you run here are appended to it, so iTerm2 sees them too) |
| Ctrl+C | Stop the running command. Press again to force-kill it |
| Cmd+N | Open a new window (just right of and below this one; fullscreen or maximized if this one is) |
| Cmd+W | Close this window (the app quits when the last one closes) |
| Cmd+Q | Ask "close all windows?", then quit (Enter = quit, Esc = cancel) |
| Cmd+M | Minimize this window |
| Cmd+C / Cmd+V | Copy / paste, using the macOS clipboard. Output text is copyable after selecting it with the mouse |
| Scroll | Look back through output (the last 10,000 lines per window are kept) |

Closing a window stops its command, the same as closing an iTerm2 tab.

The macOS menu bar is turned off (it would make Cmd+Q quit without asking), so Cmd+H (hide) and the app menu are not available.

## Requirements

- macOS only. Developed and tested on an Apple Silicon Mac (M4); Intel Macs are untested.
- zsh (`/bin/zsh`, which macOS ships).
- To build from source: Rust 1.85 or newer (the crate uses the 2024 edition). Developed with 1.96.

## Download

For Apple Silicon Macs, macOS 11 or newer.

1. Download the zip from the [latest release](https://github.com/mgautam099/litty/releases/latest) and unzip it.
2. Move `litty.app` to Applications.
3. Open it. macOS will refuse, because litty is not notarized by Apple.
4. Go to System Settings > Privacy & Security, scroll down and click Open Anyway.

Or skip steps 3 and 4 by running this once in a terminal:

```sh
xattr -dr com.apple.quarantine ~/Applications/litty.app
```

Use `/Applications/litty.app` if that's where you put it.

## Build from source

1. Install Rust (official installer, takes a minute or two):

   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

   Accept the default option, then open a new terminal tab so `cargo` is on your PATH. (Homebrew's `rust` also works.)

2. Get the code, then build and install the app:

   ```sh
   git clone https://github.com/mgautam099/litty.git
   cd litty
   ./make-app.sh
   ```

   The first build downloads dependencies and takes a few minutes. It installs `~/Applications/litty.app`, which you can open from Spotlight. Quit a running copy before reinstalling.

To run it without making an app: `cargo run --release`.

To change the app icon, replace `icon.png` (1024×1024, transparent corners) and run `./make-app.sh` again. The current icon is a placeholder.

## How commands run

Each command runs as `zsh -l -i -c ...` in a pseudo-terminal, so your `.zprofile`, `.zshrc`, aliases and PATH work as in iTerm2, and programs print output live.

- `cd` carries over to the next command in the same window. Other shell state (like `export FOO=1`) does not, because each command starts a fresh zsh. Use `export FOO=1 && mycommand` on one line instead.
- Colors and cursor movement are stripped, so output is plain text. Progress bars that redraw a line are shown as their latest state.
- A command that succeeds prints nothing extra. A failure adds a line such as `[exited with code 1]`.
- Interactive programs (vim, htop, prompts that wait for typed input) are not supported. Pagers are turned off (`PAGER=cat`) so `git log` and similar don't hang.

## Memory

Measured with `footprint` on an M4 Mac, one idle window: about 50 MB (iTerm2 on the same machine showed 156 MB at the time). Almost all of it is macOS itself (window buffer, OpenGL/Metal); egui and litty's own code are about 2 MB. Each extra window adds its own window buffer: with 3 windows each running a long command, usage hovered around 70-79 MB (reported from Activity Monitor). The cursor doesn't blink, so the app uses no CPU while idle.

Check it yourself:

```sh
footprint -p $(pgrep -f litty.app) | sed -n 2p
```

## Files

- `src/main.rs` — windows, input line, keys
- `src/process.rs` — runs the command on a PTY, Ctrl+C, cleanup
- `src/output.rs` — turns raw output into plain lines
- `src/history.rs` — reads and appends zsh history
- `make-app.sh` — builds and installs `~/Applications/litty.app`
- `icon.png` — app icon source

## License

[MIT](LICENSE). The bundled egui fonts have their own licenses (OFL-1.1 and Ubuntu Font License); see the `epaint_default_fonts` crate.
