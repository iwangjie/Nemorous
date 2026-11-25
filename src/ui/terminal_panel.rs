use std::path::PathBuf;
use std::time::Duration;

use nsh_core::keyboard::{KeyCapture, KeyEvent};
use nsh_core::prelude::*;
use nsh_core::color::Rgba;

#[cfg(unix)]
use crate::workbench_terminal_unix::Terminal;
#[cfg(windows)]
use crate::workbench_terminal_windows::Terminal;

use super::workbench::{self, Panel, button};
use crate::state::{self, current_palette};

const CELL_WIDTH: f32 = 7.25;
const CELL_HEIGHT: f32 = 15.0;

struct Pty {
    terminal: Option<Terminal>,
    parser: vt100::Parser,
    rows: u16,
    columns: u16,
    wheel_remainder: f32,
}

impl Default for Pty {
    fn default() -> Self {
        Self {
            terminal: None,
            parser: vt100::Parser::new(14, 90, 2000),
            rows: 14,
            columns: 90,
            wheel_remainder: 0.0,
        }
    }
}

#[derive(Clone, Default)]
struct Display {
    active: bool,
    status: String,
    lines: Vec<Vec<Span>>,
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

pub(super) fn toggle() {
    if workbench::active() == Some(Panel::Terminal) {
        workbench::hide();
        return;
    }
    workbench::reveal(Panel::Terminal);
    if global::<Display>().with_untracked(|view| view.status.is_empty()) {
        start();
    }
}

pub(super) fn open_in_directory(directory: PathBuf) {
    workbench::reveal(Panel::Terminal);
    start_in(directory);
}

fn start() {
    start_in(workbench::workspace());
}

fn start_in(directory: PathBuf) {
    let shell = if cfg!(windows) {
        std::env::var_os("COMSPEC")
            .map(PathBuf::from)
            .unwrap_or_else(|| "cmd.exe".into())
    } else {
        std::env::var_os("SHELL")
            .map(PathBuf::from)
            .unwrap_or_else(|| "/bin/sh".into())
    };
    let mut result = None;
    workbench::update_resource::<Pty>(|pty| {
        pty.terminal.take();
        pty.parser = vt100::Parser::new(pty.rows, pty.columns, 2000);
        pty.wheel_remainder = 0.0;
        match Terminal::spawn(&directory, &shell, pty.rows, pty.columns) {
            Ok(terminal) => {
                pty.terminal = Some(terminal);
                result = Some(Ok(()));
            }
            Err(error) => result = Some(Err(error)),
        }
    });
    let status = match result {
        Some(Ok(())) => {
            workbench::command_log(format!("终端 · {}", directory.display()));
            Display {
                active: true,
                status: "Shell 已启动".into(),
                lines: Vec::new(),
            }
        }
        Some(Err(error)) => Display {
            active: false,
            status: error.to_string(),
            lines: Vec::new(),
        },
        None => Display::default(),
    };
    global::<Display>().set(status);
}

pub(super) fn shutdown() {
    workbench::update_resource::<Pty>(|pty| {
        pty.terminal.take();
    });
    global::<Display>().set(Display::default());
}

fn stop() {
    workbench::update_resource::<Pty>(|pty| {
        if let Some(terminal) = &mut pty.terminal {
            terminal.stop();
        }
        pty.terminal.take();
    });
    global::<Display>().update(|view| {
        view.status = "终端已关闭".into();
        view.active = false;
    });
}

fn paint(pty: &Pty) -> Vec<Vec<Span>> {
    let screen = pty.parser.screen();
    let cursor = screen.cursor_position();
    (0..pty.rows)
        .map(|row| {
            let mut spans: Vec<Span> = Vec::new();
            for col in 0..pty.columns {
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
                    spans.push(Span {
                        text: text.into(),
                        width,
                        fg,
                        bg,
                        bold,
                        flip,
                    });
                }
            }
            spans
        })
        .collect()
}

fn poll() {
    let mut frame = None;
    let mut ended = None;
    workbench::update_resource::<Pty>(|pty| {
        let Some(terminal) = &mut pty.terminal else {
            return;
        };
        let bytes = terminal.read_available();
        let result = terminal.exit_code();
        if !bytes.is_empty() {
            pty.parser.process(&bytes);
            frame = Some(paint(pty));
        }
        ended = match result {
            Ok(Some(code)) => Some(format!("Shell 已退出 · {code}")),
            Err(error) => Some(error.to_string()),
            Ok(None) => None,
        };
        if ended.is_some() {
            pty.terminal.take();
        }
    });
    if frame.is_some() || ended.is_some() {
        global::<Display>().update(|view| {
            if let Some(lines) = frame {
                view.lines = lines;
            }
            if let Some(status) = ended {
                view.status = status;
                view.active = false;
            }
        });
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
    let columns = ((bounds.size.width.0 / CELL_WIDTH).floor() as u16).clamp(8, 260);
    let mut frame = None;
    workbench::update_resource::<Pty>(|pty| {
        if pty.rows != rows || pty.columns != columns {
            pty.rows = rows;
            pty.columns = columns;
            if let Some(terminal) = &pty.terminal {
                terminal.resize(rows, columns);
            }
            pty.parser.screen_mut().set_size(rows, columns);
            frame = Some(paint(pty));
        }
    });
    if let Some(lines) = frame {
        global::<Display>().update(|view| view.lines = lines);
    }
}

fn write(bytes: &[u8]) {
    workbench::with_resource::<Pty, _>(|pty| {
        if let Some(terminal) = &pty.terminal {
            terminal.write(bytes);
        }
    });
}

fn nav_scrollback(lines: isize) {
    let mut frame = None;
    workbench::update_resource::<Pty>(|pty| {
        let screen = pty.parser.screen_mut();
        let previous = screen.scrollback();
        screen.set_scrollback(previous.saturating_add_signed(lines));
        if screen.scrollback() != previous {
            frame = Some(paint(pty));
        }
    });
    if let Some(lines) = frame {
        global::<Display>().update(|view| view.lines = lines);
    }
}

fn wheel_scrollback(event: &WheelEvent) {
    let delta = event.delta.points(px(CELL_HEIGHT)).y.0;
    if !delta.is_finite() || delta == 0.0 {
        return;
    }
    let mut lines = 0;
    workbench::update_resource::<Pty>(|pty| {
        let distance = pty.wheel_remainder - delta;
        lines = (distance / CELL_HEIGHT).trunc() as isize;
        pty.wheel_remainder = distance % CELL_HEIGHT;
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
                workbench::with_resource::<Pty, _>(|pty| set_clipboard(pty.parser.screen().contents()))
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
        workbench::with_resource::<Pty, _>(|pty| pty.parser.screen().application_cursor());
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
    let bracketed = workbench::with_resource::<Pty, _>(|pty| pty.parser.screen().bracketed_paste());
    if bracketed {
        write(format!("\x1b[200~{data}\x1b[201~").as_bytes());
    } else {
        write(data.as_bytes());
    }
}

fn color(color: vt100::Color, default: Rgba) -> Rgba {
    const BASIC: [u32; 16] = [
        0x16171a, 0xe76868, 0x72c491, 0xe6bb60, 0x75a9ed, 0xc997ed, 0x67c3c9, 0xd0d3d9, 0x6d7380,
        0xff9393, 0x92e8ae, 0xf5d891, 0xaac9ff, 0xe1b1ff, 0x99e7eb, 0xf6f7fa,
    ];
    let value = match color {
        vt100::Color::Default => return default,
        vt100::Color::Rgb(r, g, b) => u32::from_be_bytes([0, r, g, b]),
        vt100::Color::Idx(i @ 0..=15) => BASIC[usize::from(i)],
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
        .child(
            view()
                .flex_row()
                .h(px(24.0))
                .flex_none()
                .gap(px(5.0))
                .items_center()
                .child(button("重启 Shell", "terminal-restart", start))
                .child(button("关闭 Shell", "terminal-stop", stop))
                .child(
                    text(|| global::<Display>().with(|view| view.status.clone()))
                        .text_color(p.text_muted)
                        .text_size(px(10.0)),
                ),
        )
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
                .bg(rgba(0x16171aff))
                .flex_1()
                .min_h(px(0.0))
                .overflow_hidden()
                .flex_col()
                .child(dynamic(|| {
                    let lines = global::<Display>().with(|view| view.lines.clone());
                    view()
                        .flex_col()
                        .children(lines.into_iter().map(|line| {
                            view().flex_row().h(px(CELL_HEIGHT)).flex_none().children(
                                line.into_iter().map(|span| {
                                    let mut fg = color(span.fg, rgba(0xe5e6ebff));
                                    let mut bg = color(span.bg, rgba(0x16171aff));
                                    if span.flip {
                                        std::mem::swap(&mut fg, &mut bg);
                                    }
                                    view()
                                        .w(px(span.width as f32 * CELL_WIDTH))
                                        .h(px(CELL_HEIGHT))
                                        .flex_none()
                                        .bg(bg)
                                        .child(
                                            text(span.text)
                                                .font_family("monospace")
                                                .text_size(px(12.0))
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
