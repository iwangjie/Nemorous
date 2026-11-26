use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use nsh_core::color::Rgba;
use nsh_core::element::desktop::MenuItem;
use nsh_core::keyboard::{KeyCapture, KeyEvent};
use nsh_core::prelude::*;

#[cfg(unix)]
use crate::workbench_terminal_unix::Terminal;
#[cfg(windows)]
use crate::workbench_terminal_windows::Terminal;

use super::icons::ICO_PLUS;
use super::left_tool_window::button as left_button;
use super::workbench::{self, Panel};
use crate::state::{self, current_palette};

const FONT_SIZE: f32 = 13.0;
const DEFAULT_CELL_WIDTH: f32 = 7.8;
const CELL_HEIGHT: f32 = 17.0;

static CELL_WIDTH_BITS: AtomicU32 = AtomicU32::new(0);

fn cell_width() -> f32 {
    match CELL_WIDTH_BITS.load(Ordering::Relaxed) {
        0 => DEFAULT_CELL_WIDTH,
        bits => f32::from_bits(bits),
    }
}

fn measure_cell_width() {
    const COPIES: usize = 64;
    let width = measure_text(&format!("{FONT_SIZE}px monospace"), &"M".repeat(COPIES)).0 / COPIES as f32;
    if width.is_finite() && width > 1.0 {
        CELL_WIDTH_BITS.store(width.to_bits(), Ordering::Relaxed);
    }
}

struct Session {
    terminal: Option<Terminal>,
    parser: vt100::Parser,
    wheel_remainder: f32,
    title: String,
    status: String,
}

struct Pty {
    sessions: Vec<Session>,
    current: usize,
    rows: u16,
    columns: u16,
}

impl Default for Pty {
    fn default() -> Self {
        Self { sessions: Vec::new(), current: 0, rows: 14, columns: 90 }
    }
}

impl Pty {
    fn session(&self) -> Option<&Session> {
        self.sessions.get(self.current)
    }

    fn session_mut(&mut self) -> Option<&mut Session> {
        self.sessions.get_mut(self.current)
    }

    fn snapshot(&self) -> Display {
        let session = self.session();
        Display {
            active: self.sessions.iter().any(|s| s.terminal.is_some()),
            status: session.map(|s| s.status.clone()).unwrap_or_default(),
            lines: session.map(|s| paint(&s.parser, self.rows, self.columns)).unwrap_or_default(),
            tabs: self.sessions.iter().map(|s| s.title.clone()).collect(),
            current: self.current,
        }
    }
}

#[derive(Clone, Default)]
struct Display {
    active: bool,
    status: String,
    lines: Vec<Vec<Span>>,
    tabs: Vec<String>,
    current: usize,
}

#[derive(Clone)]
struct Span {
    text: String,
    width: usize,
    fg: vt100::Color,
    bg: vt100::Color,
    bold: bool,
    flip: bool,
}

fn publish() {
    let mut display = None;
    workbench::with_resource::<Pty, _>(|pty| display = Some(pty.snapshot()));
    if let Some(display) = display {
        global::<Display>().set(display);
    }
}

pub(super) fn toggle() {
    if workbench::active() == Some(Panel::Terminal) {
        workbench::hide();
        return;
    }
    workbench::reveal(Panel::Terminal);
    if workbench::with_resource::<Pty, _>(|pty| pty.sessions.is_empty()) {
        new_session(workbench::workspace());
    }
}

pub(super) fn open_in_directory(directory: PathBuf) {
    workbench::reveal(Panel::Terminal);
    new_session(directory);
}

fn shell_path() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("COMSPEC")
            .map(PathBuf::from)
            .unwrap_or_else(|| "cmd.exe".into())
    } else {
        std::env::var_os("SHELL")
            .map(PathBuf::from)
            .unwrap_or_else(|| "/bin/sh".into())
    }
}

fn spawn_session(directory: &PathBuf, rows: u16, columns: u16, number: usize) -> Session {
    let name = directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "终端".into());
    let title = format!("{name} · {number}");
    let parser = vt100::Parser::new(rows, columns, 2000);
    match Terminal::spawn(directory, &shell_path(), rows, columns) {
        Ok(terminal) => {
            workbench::command_log(format!("终端 · {}", directory.display()));
            Session { terminal: Some(terminal), parser, wheel_remainder: 0.0, title, status: "Shell 已启动".into() }
        }
        Err(error) => Session { terminal: None, parser, wheel_remainder: 0.0, title, status: error.to_string() },
    }
}

fn new_session(directory: PathBuf) {
    workbench::update_resource::<Pty>(|pty| {
        let number = pty.sessions.len() + 1;
        let session = spawn_session(&directory, pty.rows, pty.columns, number);
        pty.sessions.push(session);
        pty.current = pty.sessions.len() - 1;
    });
    publish();
}

fn restart_session(index: usize) {
    let directory = workbench::workspace();
    workbench::update_resource::<Pty>(|pty| {
        if index >= pty.sessions.len() {
            return;
        }
        let (rows, columns) = (pty.rows, pty.columns);
        pty.sessions[index] = spawn_session(&directory, rows, columns, index + 1);
    });
    publish();
}

fn select(index: usize) {
    workbench::update_resource::<Pty>(|pty| {
        if index < pty.sessions.len() {
            pty.current = index;
        }
    });
    publish();
}

fn close(index: usize) {
    workbench::update_resource::<Pty>(|pty| {
        if index < pty.sessions.len() {
            pty.sessions.remove(index);
            pty.current = pty.current.min(pty.sessions.len().saturating_sub(1));
            if index < pty.current {
                pty.current -= 1;
            }
        }
    });
    publish();
}

pub(super) fn shutdown() {
    workbench::replace_resource(Pty::default());
    global::<Display>().set(Display::default());
}

fn paint(parser: &vt100::Parser, rows: u16, columns: u16) -> Vec<Vec<Span>> {
    let screen = parser.screen();
    let cursor = screen.cursor_position();
    (0..rows)
        .map(|row| {
            let mut spans: Vec<Span> = Vec::new();
            for col in 0..columns {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let width = if cell.is_wide() { 2 } else { 1 };
                let flip = cell.inverse()
                    ^ (!screen.hide_cursor() && screen.scrollback() == 0 && cursor == (row, col));
                let (fg, bg, bold) = (cell.fgcolor(), cell.bgcolor(), cell.bold());
                let text = if cell.contents().is_empty() {
                    " "
                } else {
                    cell.contents()
                };
                if let Some(last) = spans
                    .last_mut()
                    .filter(|last| (last.fg, last.bg, last.bold, last.flip) == (fg, bg, bold, flip))
                {
                    last.text.push_str(text);
                    last.width += width;
                } else {
                    spans.push(Span { text: text.into(), width, fg, bg, bold, flip });
                }
            }
            spans
        })
        .collect()
}

fn poll() {
    let mut changed = false;
    workbench::update_resource::<Pty>(|pty| {
        let current = pty.current;
        for (index, session) in pty.sessions.iter_mut().enumerate() {
            let Some(terminal) = &mut session.terminal else {
                continue;
            };
            let bytes = terminal.read_available();
            let result = terminal.exit_code();
            if !bytes.is_empty() {
                session.parser.process(&bytes);
                changed |= index == current;
            }
            let ended = match result {
                Ok(Some(code)) => Some(format!("Shell 已退出 · {code}")),
                Err(error) => Some(error.to_string()),
                Ok(None) => None,
            };
            if let Some(status) = ended {
                session.status = status;
                session.terminal.take();
                changed = true;
            }
        }
    });
    if changed {
        publish();
    }
}

pub(super) fn setup() {
    let active = computed(|| global::<Display>().with(|view| view.active));
    effect(move || {
        if active.get() {
            let timer = set_interval(Duration::from_millis(30), poll);
            cleanup(move || timer.cancel());
        }
    });
}

fn resize(bounds: &Bounds<Points>) {
    let rows = ((bounds.size.height.0 / CELL_HEIGHT).floor() as u16).clamp(2, 80);
    let columns = ((bounds.size.width.0 / cell_width()).floor() as u16).clamp(8, 260);
    let mut resized = false;
    workbench::update_resource::<Pty>(|pty| {
        if pty.rows != rows || pty.columns != columns {
            pty.rows = rows;
            pty.columns = columns;
            for session in &mut pty.sessions {
                if let Some(terminal) = &session.terminal {
                    terminal.resize(rows, columns);
                }
                session.parser.screen_mut().set_size(rows, columns);
            }
            resized = true;
        }
    });
    if resized {
        publish();
    }
}

fn write(bytes: &[u8]) {
    workbench::with_resource::<Pty, _>(|pty| {
        if let Some(terminal) = pty.session().and_then(|s| s.terminal.as_ref()) {
            terminal.write(bytes);
        }
    });
}

fn nav_scrollback(lines: isize) {
    let mut changed = false;
    workbench::update_resource::<Pty>(|pty| {
        let Some(session) = pty.session_mut() else { return };
        let screen = session.parser.screen_mut();
        let previous = screen.scrollback();
        screen.set_scrollback(previous.saturating_add_signed(lines));
        changed = screen.scrollback() != previous;
    });
    if changed {
        publish();
    }
}

fn wheel_scrollback(event: &WheelEvent) {
    let delta = event.delta.points(px(CELL_HEIGHT)).y.0;
    if !delta.is_finite() || delta == 0.0 {
        return;
    }
    let mut lines = 0;
    workbench::update_resource::<Pty>(|pty| {
        let Some(session) = pty.session_mut() else { return };
        let distance = session.wheel_remainder - delta;
        lines = (distance / CELL_HEIGHT).trunc() as isize;
        session.wheel_remainder = distance % CELL_HEIGHT;
    });
    if lines != 0 {
        nav_scrollback(lines);
    }
}

fn handle_key(event: &KeyEvent) {
    let key = event.key.as_ref();
    if event.modifiers.cmd {
        match key.to_ascii_lowercase().as_str() {
            "v" => state::spawn_from_root(async {
                paste(&clipboard_string().await);
            }),
            "c" => {
                workbench::with_resource::<Pty, _>(|pty| {
                    if let Some(session) = pty.session() {
                        set_clipboard(session.parser.screen().contents())
                    }
                })
            }
            _ => {}
        }
        return;
    }
    if event.modifiers.shift {
        match key {
            "PageUp" => {
                nav_scrollback(12);
                return;
            }
            "PageDown" => {
                nav_scrollback(-12);
                return;
            }
            _ => {}
        }
    }
    if event.modifiers.ctrl && event.modifiers.shift && key.eq_ignore_ascii_case("v") {
        state::spawn_from_root(async { paste(&clipboard_string().await); });
        return;
    }
    if event.modifiers.ctrl {
        let key = event.code.strip_prefix("Key").unwrap_or(key);
        let c = key
            .as_bytes()
            .first()
            .copied()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if c.is_ascii_lowercase() {
            write(&[c - b'a' + 1]);
        }
        return;
    }
    let application_cursor =
        workbench::with_resource::<Pty, _>(|pty| pty.session().is_some_and(|s| s.parser.screen().application_cursor()));
    let literal: &[u8] = match key {
        "Enter" => b"\r",
        "Backspace" => b"\x7f",
        "Tab" => b"\t",
        "Escape" => b"\x1b",
        "ArrowUp" if application_cursor => b"\x1bOA",
        "ArrowDown" if application_cursor => b"\x1bOB",
        "ArrowRight" if application_cursor => b"\x1bOC",
        "ArrowLeft" if application_cursor => b"\x1bOD",
        "ArrowUp" => b"\x1b[A",
        "ArrowDown" => b"\x1b[B",
        "ArrowRight" => b"\x1b[C",
        "ArrowLeft" => b"\x1b[D",
        "Home" => b"\x1b[H",
        "End" => b"\x1b[F",
        "PageUp" => b"\x1b[5~",
        "PageDown" => b"\x1b[6~",
        "Delete" => b"\x1b[3~",
        other if other.chars().count() == 1 => other.as_bytes(),
        _ => return,
    };
    write(literal);
}

fn paste(data: &str) {
    let data = data.trim_end_matches('\0');
    if data.len() > 4096 {
        state::show_toast("warn", "终端单次粘贴上限为 4096 字节");
        return;
    }
    let bracketed = workbench::with_resource::<Pty, _>(|pty| pty.session().is_some_and(|s| s.parser.screen().bracketed_paste()));
    if bracketed {
        write(format!("\x1b[200~{data}\x1b[201~").as_bytes());
    } else {
        write(data.as_bytes());
    }
}

fn color(color: vt100::Color, default: Rgba, dark: bool) -> Rgba {
    const LIGHT: [u32; 16] = [
        0x2b2d33, 0xc93c3c, 0x2e8b57, 0xa8741a, 0x2f6fd1, 0x9a4fc9, 0x1f8f95, 0x5d6370, 0x6d7380,
        0xe05252, 0x3aa56c, 0xc08a1f, 0x4a86e8, 0xb06ae0, 0x2ba3a9, 0x2b2d33,
    ];
    const DARK: [u32; 16] = [
        0x16171a, 0xe76868, 0x72c491, 0xe6bb60, 0x75a9ed, 0xc997ed, 0x67c3c9, 0xd0d3d9, 0x6d7380,
        0xff9393, 0x92e8ae, 0xf5d891, 0xaac9ff, 0xe1b1ff, 0x99e7eb, 0xf6f7fa,
    ];
    let value = match color {
        vt100::Color::Default => return default,
        vt100::Color::Rgb(r, g, b) => u32::from_be_bytes([0, r, g, b]),
        vt100::Color::Idx(i @ 0..=15) => (if dark { DARK } else { LIGHT })[usize::from(i)],
        vt100::Color::Idx(i @ 16..=231) => {
            let i = i - 16;
            let cube = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            u32::from_be_bytes([0, cube(i / 36), cube(i / 6 % 6), cube(i % 6)])
        }
        vt100::Color::Idx(i) => {
            let gray = 8 + 10 * (i - 232);
            u32::from_be_bytes([0, gray, gray, gray])
        }
    };
    rgba(value << 8 | 0xff)
}

pub(super) fn body() -> impl IntoElement {
    measure_cell_width();
    let p = current_palette();
    let area = element_ref();
    let focus = area.clone();
    after_answer(move || focus.focus());
    let focus = area.clone();
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .gap(px(4.0))
        .child(dynamic(|| {
            let p = current_palette();
            let (tabs, current) = global::<Display>().with(|view| (view.tabs.clone(), view.current));
            view()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(px(2.0))
                .pb(px(3.0))
                .border_b_1()
                .border_color(p.border)
                .children(tabs.into_iter().enumerate().map(|(index, title)| {
                    let active = index == current;
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .h(px(24.0))
                        .px(px(8.0))
                        .rounded(px(4.0))
                        .bg(if active { p.accent_soft } else { transparent_black() })
                        .on_context_menu(move |event: &ContextMenuEvent| {
                            state::show_popup_menu_at(
                                vec![
                                    MenuItem::new("重启 Shell")
                                        .on_select(move || restart_session(index)),
                                    MenuItem::new("关闭终端").on_select(move || close(index)),
                                ],
                                event.page_position,
                            );
                        })
                        .child(
                            view()
                                .test_id(&format!("terminal-tab-{index}"))
                                .label(title.clone())
                                .cursor(Cursor::Pointer)
                                .on_press(move || select(index))
                                .child(
                                    text(title)
                                        .text_size(px(11.5))
                                        .text_color(if active { p.accent_soft_text } else { p.text_secondary }),
                                ),
                        )
                        .child(
                            view()
                                .test_id(&format!("terminal-tab-close-{index}"))
                                .label("关闭终端标签")
                                .cursor(Cursor::Pointer)
                                .on_press(move || close(index))
                                .child(text("×").text_size(px(12.0)).text_color(p.text_muted)),
                        )
                }))
                .child(left_button(
                    "terminal-new",
                    "新建终端",
                    ICO_PLUS,
                    p,
                    false,
                    true,
                    || new_session(workbench::workspace()),
                ))
                .child(view().flex_1())
                .child(
                    text(|| global::<Display>().with(|view| view.status.clone()))
                        .text_color(p.text_muted)
                        .text_size(px(10.5)),
                )
                .into_element()
        }))
        .child(
            view()
                .test_id("terminal-screen")
                .element_ref(&area)
                .focusable(true)
                .capture_keys(KeyCapture::All)
                .on_press(move || focus.focus())
                .on_key_down(handle_key)
                .on_wheel(wheel_scrollback)
                .on_layout(resize)
                .label("终端输出与输入")
                .bg(p.surface)
                .flex_1()
                .min_h(px(0.0))
                .overflow_hidden()
                .flex_col()
                .child(dynamic(|| {
                    let p = current_palette();
                    let (default_fg, default_bg) = (Rgba::from(p.text_primary), Rgba::from(p.surface));
                    let cell = cell_width();
                    let lines = global::<Display>().with(|view| view.lines.clone());
                    view()
                        .flex_col()
                        .children(lines.into_iter().map(|line| {
                            view().flex_row().h(px(CELL_HEIGHT)).flex_none().children(
                                line.into_iter().map(|span| {
                                    let mut fg = color(span.fg, default_fg, p.is_dark);
                                    let mut bg = color(span.bg, default_bg, p.is_dark);
                                    if span.flip {
                                        std::mem::swap(&mut fg, &mut bg);
                                    }
                                    view()
                                        .w(px(span.width as f32 * cell))
                                        .h(px(CELL_HEIGHT))
                                        .flex_none()
                                        .bg(bg)
                                        .child(
                                            text(span.text)
                                                .font_family("monospace")
                                                .text_size(px(FONT_SIZE))
                                                .line_height(px(CELL_HEIGHT))
                                                .font_weight(if span.bold {
                                                    FontWeight::SEMIBOLD
                                                } else {
                                                    FontWeight::NORMAL
                                                })
                                                .text_color(fg)
                                                .number_of_lines(1u32),
                                        )
                                }),
                            )
                        }))
                        .into_element()
                })),
        )
}
