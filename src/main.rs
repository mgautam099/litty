//! litty — a tiny window for running long-lived commands.
//!
//! Each window: type a command, press Enter, watch its output.
//!   Up / Down   cycle through history (read from, and appended to, ~/.zsh_history)
//!   Ctrl+C      stop the running command (press again to force-kill)
//!   Cmd+N       open another window
//!   Cmd+W       close this window    Cmd+M   minimize
//!   Cmd+Q       quit (asks first), closing every window

mod history;
mod output;
mod process;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{
    self, text::CCursor, text_selection::CCursorRange, FontId, Id, Key, Label, Modifiers,
    RichText, TextStyle, TextWrapMode, ViewportBuilder, ViewportCommand, ViewportId,
};

use history::History;
use output::{Kind, Output};
use process::{PaneState, Proc};

const FONT_SIZE: f32 = 14.0;
const WINDOW_SIZE: [f32; 2] = [900.0, 560.0];
/// How far a new window sits right of and below the one it was opened from.
const CASCADE: f32 = 28.0;

/// State every window can see.
struct Shared {
    history: Mutex<History>,
    /// Set by any window on Cmd+N (where the new window should go); the main window opens it.
    new_window: Mutex<Option<ViewportBuilder>>,
    /// Set once the user confirmed Cmd+Q; the main window closes everything.
    quit: AtomicBool,
}

/// One window: its input line, history position and running command.
struct Pane {
    viewport: ViewportId,
    builder: ViewportBuilder,
    state: Arc<Mutex<PaneState>>,
    proc: Option<Proc>,
    input: String,
    /// Position while browsing history with Up/Down (None = editing a new line).
    hist_pos: Option<usize>,
    /// What was typed before Up was pressed, restored when coming back down.
    draft: String,
    pty_size: (u16, u16),
    title: String,
    closed: bool,
    /// Showing the "quit?" prompt after Cmd+Q.
    confirm_quit: bool,
}

impl Pane {
    fn new(viewport: ViewportId, builder: ViewportBuilder) -> Self {
        let cwd = std::env::var_os("HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"));
        Self {
            viewport,
            builder,
            state: Arc::new(Mutex::new(PaneState { output: Output::new(), running: false, cwd })),
            proc: None,
            input: String::new(),
            hist_pos: None,
            draft: String::new(),
            pty_size: (120, 40),
            title: String::new(),
            closed: false,
            confirm_quit: false,
        }
    }

    fn close(&mut self) {
        self.closed = true;
        self.proc = None; // hangs up the command
    }

    fn run(&mut self, ctx: &egui::Context, shared: &Shared) {
        let cmd = self.input.trim().to_string();
        self.input.clear();
        self.hist_pos = None;
        if cmd.is_empty() {
            return;
        }
        shared.history.lock().unwrap().record(cmd.clone());

        {
            let mut st = self.state.lock().unwrap();
            let line = format!("{} ❯ {}", short_path(&st.cwd), cmd);
            st.output.push_line(Kind::Cmd, line);
            st.running = true;
        }
        self.proc = None;

        let ctx = ctx.clone();
        let vp = self.viewport;
        let (cols, rows) = self.pty_size;
        match process::spawn(&cmd, Arc::clone(&self.state), cols, rows, move || {
            ctx.request_repaint_of(vp)
        }) {
            Ok(p) => self.proc = Some(p),
            Err(e) => {
                let mut st = self.state.lock().unwrap();
                st.output.push_line(Kind::Info, format!("[could not start: {e}]"));
                st.running = false;
            }
        }
    }

    fn history_step(&mut self, shared: &Shared, up: bool) {
        let hist = shared.history.lock().unwrap();
        let len = hist.entries.len();
        if len == 0 {
            return;
        }
        let next = match (self.hist_pos, up) {
            (None, true) => {
                self.draft = self.input.clone();
                Some(len - 1)
            }
            (None, false) => return,
            (Some(p), true) => Some(p.saturating_sub(1)),
            (Some(p), false) if p + 1 < len => Some(p + 1),
            (Some(_), false) => None,
        };
        self.hist_pos = next;
        self.input = match next {
            Some(p) => hist.entries[p].clone(),
            None => std::mem::take(&mut self.draft),
        };
    }

    fn ui(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        let ctx = ui.ctx().clone();
        let vp = self.viewport;
        let input_id = Id::unique(("litty-input", vp));

        let (new_window, ctrl_c, close, quit, minimize) = ctx.input_mut(|i| {
            let mut ctrl_c = i.consume_key(Modifiers::CTRL, Key::C);
            // On macOS copy is Cmd+C, so Ctrl+C arrives as a plain key (handled above).
            // Elsewhere Ctrl+C is turned into a copy event; treat it as Ctrl+C, like terminals do.
            if cfg!(not(target_os = "macos")) {
                let before = i.events.len();
                i.events.retain(|e| !matches!(e, egui::Event::Copy));
                ctrl_c |= i.events.len() != before;
            }
            (
                i.consume_key(Modifiers::COMMAND, Key::N),
                ctrl_c,
                i.consume_key(Modifiers::COMMAND, Key::W),
                i.consume_key(Modifiers::COMMAND, Key::Q),
                i.consume_key(Modifiers::COMMAND, Key::M),
            )
        });
        if new_window {
            *shared.new_window.lock().unwrap() = Some(next_window(&ctx.input(|i| i.viewport().clone())));
            ctx.request_repaint_of(ViewportId::ROOT);
        }
        if close {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        if quit {
            self.confirm_quit = true;
        }
        // While the prompt is up, Enter means "quit" and must not reach the input line.
        let confirm_enter =
            self.confirm_quit && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        if minimize {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        }

        let (running, cwd) = {
            let st = self.state.lock().unwrap();
            (st.running, st.cwd.clone())
        };
        if ctrl_c {
            if let (true, Some(p)) = (running, self.proc.as_mut()) {
                p.interrupt();
            } else {
                self.input.clear();
                self.hist_pos = None;
            }
        }

        // Window title, so the windows are easy to tell apart.
        let title = match (&self.proc, running) {
            (_, true) => format!("{} — litty", last_cmd(&self.state).unwrap_or_default()),
            _ => format!("litty — {}", short_path(&cwd)),
        };
        if title != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        // ---- input line ----
        egui::Panel::bottom(("litty-bottom", vp)).show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} ❯", short_path(&cwd)))
                        .monospace()
                        .color(ui.visuals().hyperlink_color),
                );

                if ui.memory(|m| m.has_focus(input_id)) {
                    let (up, down) = ui.input_mut(|i| {
                        (
                            i.consume_key(Modifiers::NONE, Key::ArrowUp),
                            i.consume_key(Modifiers::NONE, Key::ArrowDown),
                        )
                    });
                    if up || down {
                        self.history_step(shared, up);
                        // Put the cursor at the end of the recalled command.
                        if let Some(mut s) = egui::TextEdit::load_state(ui.ctx(), input_id) {
                            let end = CCursor::new(self.input.chars().count());
                            s.cursor.set_char_range(Some(CCursorRange::one(end)));
                            s.store(ui.ctx(), input_id);
                        }
                    }
                }

                let hint = if running { "running — Ctrl+C to stop" } else { "type a command" };
                let resp = ui.add_enabled(
                    !self.confirm_quit,
                    egui::TextEdit::singleline(&mut self.input)
                        .id(input_id)
                        .font(TextStyle::Monospace)
                        .frame(egui::Frame::NONE)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY),
                );
                if resp.changed() {
                    self.hist_pos = None;
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    if !running {
                        self.run(&ctx, shared);
                    }
                    resp.request_focus();
                }
                if ui.memory(|m| m.focused().is_none()) {
                    resp.request_focus();
                }
            });
            ui.add_space(4.0);
        });

        // ---- output ----
        egui::CentralPanel::default().show(ui, |ui| {
            let font = TextStyle::Monospace.resolve(ui.style());
            let row_h = ui.text_style_height(&TextStyle::Monospace);
            self.update_pty_size(ui, &font, row_h);

            let st = self.state.lock().unwrap();
            let lines = &st.output.lines;
            let colors = [
                ui.visuals().text_color(),
                ui.visuals().hyperlink_color,
                ui.visuals().weak_text_color(),
            ];
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::ScrollArea::both()
                .id_salt(("litty-output", vp))
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show_rows(ui, row_h, lines.len(), |ui, range| {
                    for line in lines.range(range) {
                        let color = match line.kind {
                            Kind::Out => colors[0],
                            Kind::Cmd => colors[1],
                            Kind::Info => colors[2],
                        };
                        let text = if line.text.is_empty() { " " } else { &line.text };
                        ui.add(
                            Label::new(RichText::new(text).monospace().color(color))
                                .wrap_mode(TextWrapMode::Extend),
                        );
                    }
                });
        });

        if self.confirm_quit {
            self.quit_prompt(&ctx, shared, confirm_enter);
        }
    }

    /// The Cmd+Q prompt. Enter or the button quits everything; Esc or Cancel goes back.
    fn quit_prompt(&mut self, ctx: &egui::Context, shared: &Shared, enter: bool) {
        let modal = egui::Modal::new(Id::unique(("litty-quit", self.viewport))).show(ctx, |ui| {
            ui.label("Close all windows and quit litty?");
            ui.label(RichText::new("Running commands will be stopped.").weak());
            ui.horizontal(|ui| (ui.button("Quit").clicked(), ui.button("Cancel").clicked())).inner
        });
        let (quit, cancel) = modal.inner;
        if enter || quit {
            shared.quit.store(true, Ordering::SeqCst);
            ctx.request_repaint_of(ViewportId::ROOT);
        }
        if enter || quit || cancel || modal.should_close() {
            self.confirm_quit = false;
        }
    }

    /// Tells the command how big the window is (in characters), like a real terminal.
    fn update_pty_size(&mut self, ui: &egui::Ui, font: &FontId, row_h: f32) {
        let glyph_w = ui.ctx().fonts_mut(|f| f.glyph_width(font, 'M')).max(1.0);
        let avail = ui.available_size();
        let size = (
            (avail.x / glyph_w).floor().clamp(20.0, 1000.0) as u16,
            (avail.y / row_h).floor().clamp(5.0, 500.0) as u16,
        );
        if size != self.pty_size {
            self.pty_size = size;
            if let Some(p) = &self.proc {
                p.resize(size.0, size.1);
            }
        }
    }
}

fn last_cmd(state: &Mutex<PaneState>) -> Option<String> {
    let st = state.lock().unwrap();
    st.output
        .lines
        .iter()
        .rev()
        .find(|l| l.kind == Kind::Cmd)
        .and_then(|l| l.text.split_once(" ❯ ").map(|(_, c)| c.to_string()))
}

fn short_path(p: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        if let Ok(rest) = p.strip_prefix(&home) {
            return if rest.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    p.display().to_string()
}

fn window_builder() -> ViewportBuilder {
    // An empty icon stops eframe from setting its own egui logo, so the app bundle's icon shows.
    ViewportBuilder::default()
        .with_title("litty")
        .with_inner_size(WINDOW_SIZE)
        .with_icon(egui::IconData::default())
}

/// Like Firefox: a new window opens just right of and below the current one,
/// or fullscreen / maximized if the current one is.
fn next_window(info: &egui::ViewportInfo) -> ViewportBuilder {
    let b = window_builder();
    if info.fullscreen == Some(true) {
        return b.with_fullscreen(true);
    }
    // egui stops updating `maximized` after startup on macOS, so also treat a window that
    // covers most of the screen (a zoomed window leaves only the menu bar and Dock) as maximized.
    let covers_screen = match (info.inner_rect, info.monitor_size) {
        (Some(w), Some(m)) => w.width() >= 0.9 * m.x && w.height() >= 0.8 * m.y,
        _ => false,
    };
    if info.maximized == Some(true) || covers_screen {
        return b.with_maximized(true);
    }
    let b = info.inner_rect.map_or(b.clone(), |r| b.with_inner_size(r.size()));
    info.outer_rect.map_or(b.clone(), |r| b.with_position(r.min + egui::vec2(CASCADE, CASCADE)))
}

struct App {
    shared: Arc<Shared>,
    /// The first window. If you close it while others are open, it is hidden
    /// (its command is stopped) and the app keeps running until the last window closes.
    root: Pane,
    root_hidden: bool,
    children: Vec<Arc<Mutex<Pane>>>,
    next_id: u64,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if self.shared.quit.load(Ordering::SeqCst) {
            self.root.close();
            for p in &self.children {
                p.lock().unwrap().close();
            }
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        self.children.retain(|p| !p.lock().unwrap().closed);

        if !self.root_hidden
            && ctx.input(|i| i.viewport().close_requested())
            && !self.children.is_empty()
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            self.root.close();
            self.root_hidden = true;
        }
        if self.root_hidden && self.children.is_empty() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        if let Some(builder) = self.shared.new_window.lock().unwrap().take() {
            self.next_id += 1;
            let id = ViewportId::from_hash_of(("litty-window", self.next_id));
            self.children.push(Arc::new(Mutex::new(Pane::new(id, builder))));
        }

        if !self.root_hidden {
            self.root.ui(ui, &self.shared);
        }

        for pane in &self.children {
            let (id, builder) = {
                let p = pane.lock().unwrap();
                (p.viewport, p.builder.clone())
            };
            let pane = Arc::clone(pane);
            let shared = Arc::clone(&self.shared);
            ctx.show_viewport_deferred(id, builder, move |ui, _class| {
                let mut p = pane.lock().unwrap();
                if p.closed {
                    return;
                }
                if ui.ctx().input(|i| i.viewport().close_requested()) {
                    p.close();
                    ui.ctx().request_repaint_of(ViewportId::ROOT);
                    return;
                }
                p.ui(ui, &shared);
            });
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.root.close();
        for p in &self.children {
            p.lock().unwrap().close();
        }
    }
}

fn main() -> eframe::Result {
    let shared = Arc::new(Shared {
        history: Mutex::new(History::load_zsh()),
        new_window: Mutex::new(None),
        quit: AtomicBool::new(false),
    });
    let options = eframe::NativeOptions {
        viewport: window_builder(),
        // macOS's built-in menu would make Cmd+Q quit the whole app; without it the key
        // reaches the window instead (Cmd+Q / Cmd+W close it, Cmd+M minimizes).
        #[cfg(target_os = "macos")]
        event_loop_builder: Some(Box::new(|b| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            b.with_default_menu(false);
        })),
        ..Default::default()
    };
    eframe::run_native(
        "litty",
        options,
        Box::new(|cc| {
            cc.egui_ctx.all_styles_mut(|s| {
                s.text_styles.insert(TextStyle::Monospace, FontId::monospace(FONT_SIZE));
                // No blinking cursor: the app does no work at all while idle.
                s.visuals.text_cursor.blink = false;
            });
            Ok(Box::new(App {
                shared,
                root: Pane::new(ViewportId::ROOT, window_builder()),
                root_hidden: false,
                children: Vec::new(),
                next_id: 0,
            }))
        }),
    )
}
