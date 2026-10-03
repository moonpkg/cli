mod browse;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use crate::config::{Config, ico};
use crate::paths::Dirs;
use crate::util::{Res, clip_to, fit_name, pad_to, shorten_path, vis_len};
use crate::tui::browse::{Kind, archive_is_runnable, list_dir};

struct RawMode(pub Option<String>);

impl RawMode {
    fn enter() -> Option<Self> {
        let saved = Command::new("stty")
            .args(["-g"])
            .stdin(std::process::Stdio::inherit())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        let ok = Command::new("stty")
            .args(["raw", "-echo"])
            .stdin(std::process::Stdio::inherit())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            println!("error: this terminal does not support the interactive picker");
            return None;
        }

        print!("\x1b[?1049h\x1b[?25l");
        let _ = std::io::stdout().flush();
        Some(RawMode(saved))
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        print!("\x1b[?25h\x1b[?1049l");
        let _ = std::io::stdout().flush();
        match &self.0 {
            Some(s) if !s.is_empty() => {
                let _ = Command::new("stty").arg(s).status();
            }
            _ => {
                let _ = Command::new("stty").args(["sane"]).status();
            }
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Quit,
    HomeKey,
    EndKey,
    Char(u8),
    Other,
}

fn read_key() -> Key {
    let mut b = [0u8; 1];
    if std::io::stdin().read_exact(&mut b).is_err() {
        return Key::Quit;
    }
    match b[0] {
        b'\r' | b'\n' => return Key::Enter,
        b'q' | b'Q' | 0x03 => return Key::Quit,
        b'k' => return Key::Up,
        b'j' => return Key::Down,
        b'h' => return Key::Left,
        b'l' => return Key::Right,
        0x7f | 8 => return Key::Left,
        b'g' => return Key::HomeKey,
        b'G' => return Key::EndKey,
        27 => {}
        other => return Key::Char(other),
    }

    if std::io::stdin().read_exact(&mut b).is_err() {
        return Key::Left;
    }
    let final_byte = match b[0] {
        b'[' => {
            loop {
                if std::io::stdin().read_exact(&mut b).is_err() {
                    return Key::Left;
                }
                if b[0].is_ascii_digit() || b[0] == b';' {
                    continue;
                }
                break;
            }
            b[0]
        }
        b'O' => {
            if std::io::stdin().read_exact(&mut b).is_err() {
                return Key::Left;
            }
            b[0]
        }
        _ => return Key::Left,
    };
    match final_byte {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::HomeKey,
        b'F' => Key::EndKey,
        _ => Key::Other,
    }
}

fn term_size() -> (u16, u16) {
    let parsed = Command::new("stty")
        .arg("size")
        .stdin(std::process::Stdio::inherit())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            let v: Vec<u16> = String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .map(|x| x.parse().unwrap_or(0))
                .collect();
            (v.first().copied().unwrap_or(0), v.get(1).copied().unwrap_or(0))
        });

    match parsed {
        Some((r, c)) if r > 0 && c > 0 => (r, c),
        _ => (24, 80),
    }
}

pub fn tui_pick(d: &Dirs, cfg: &Config, start: &Path) -> Res<Option<PathBuf>> {
    let raw = match RawMode::enter() {
        Some(r) => r,
        None => return Ok(None),
    };

    let mut cwd = start.to_path_buf();
    let mut sel = 0usize;
    let mut off = 0usize;
    let mut force_ask: Option<PathBuf> = None;
    let mut note = String::new();
    let mut show_all = false;

    let picked: Option<PathBuf> = loop {
            let ents = list_dir(&cwd, show_all);
            if sel >= ents.len() {
                sel = ents.len().saturating_sub(1);
            }
            let n_ok = ents.iter().filter(|e| e.kind == Kind::Supported).count();
            let n_no = ents.iter().filter(|e| e.kind.dimmed()).count();

            let (rows, cols) = term_size();
            let panel_w = (cols as usize).saturating_sub(4).min(78).max(30);
            let visible = (rows as usize).saturating_sub(8).max(3);
            if sel < off {
                off = sel;
            }
            if sel >= off + visible {
                off = sel + 1 - visible;
            }

            let mut s = String::new();
            s.push_str("\x1b[H\x1b[2J");
            let top = 1usize;
            let left = ((cols as usize).saturating_sub(panel_w)) / 2;
            let last = (rows as usize).saturating_sub(1);

            let put = |s: &mut String, row: usize, col: usize, text: &str| {
                if row >= last || col >= cols as usize {
                    return;
                }
                s.push_str(&format!("\x1b[{};{}H", row + 1, col + 1));
                s.push_str(&clip_to(text, (cols as usize).saturating_sub(col)));
            };

            let title = format!(" {} moon install ", cfg.icon(ico::FOLDER));
            put(&mut s, top, left, &pad_to(&format!("\x1b[7m{title}\x1b[0m"), panel_w));

            let path_disp = shorten_path(&cwd, panel_w.saturating_sub(8));
            put(&mut s, top + 1, left, &format!("  {}", path_disp));
            if !note.is_empty() {
                let n = clip_to(&format!("\x1b[33m{note}\x1b[0m"), panel_w.saturating_sub(4));
                put(&mut s, top + 1, left + panel_w.saturating_sub(vis_len(&n)), &n);
            }

            let sep = format!("  {}", "\u{2500}".repeat(panel_w.saturating_sub(2)));
            put(&mut s, top + 2, left, &sep);

            let tag = "  \u{00b7} unsupported";
            let name_w = panel_w
                .saturating_sub(3)
                .saturating_sub(if cfg.icons_on() { 2 } else { 0 })
                .saturating_sub(if ents.iter().any(|e| e.kind.dimmed()) { tag.chars().count() } else { 0 });
            for (i, e) in ents.iter().enumerate().skip(off).take(visible) {
                let row = top + 3 + (i - off);
                let (mark, name) = match e.kind {
                    Kind::Dir => (cfg.icon(ico::FOLDER), format!("{}/", e.name)),
                    Kind::Supported => (cfg.icon(ico::CHECK), e.name.clone()),
                    Kind::Unsupported => (cfg.icon(ico::CROSS), e.name.clone()),
                };
                let mut line = format!(" {mark} {}", fit_name(&name, name_w));
                if e.kind.dimmed() {
                    line.push_str(tag);
                }
                let styled = if i == sel {
                    format!("\x1b[7m{}\x1b[0m", line)
                } else if e.kind.dimmed() {
                    format!("\x1b[2m{line}\x1b[0m")
                } else {
                    line
                };
                put(&mut s, row, left, &pad_to(&styled, panel_w));
            }
            if ents.is_empty() {
                put(&mut s, top + 3, left, "  (empty directory)");
            }

            let foot = top + 3 + visible + 1;
            let keys = if cfg.icons_on() {
                "\u{2191}\u{2193} move  \u{2190} back  \u{2192} open  enter install  u all  q quit"
            } else {
                "up/down move  esc back  right open  enter install  u all  q quit"
            };
            let hint = if show_all {
                format!(" {}/{} installable   {keys}   (showing everything)", n_ok, n_ok + n_no)
            } else {
                format!(" {n_ok} installable   {keys}")
            };
            put(&mut s, foot, left, &pad_to(&format!("\x1b[2m{hint}\x1b[0m"), panel_w));

            print!("{s}");
            let _ = std::io::stdout().flush();

            let k = read_key();
            match k {
                Key::Quit => break None,
                Key::Up => sel = sel.saturating_sub(1),
                Key::Down => sel = (sel + 1).min(ents.len().saturating_sub(1)),
                Key::HomeKey => sel = 0,
                Key::EndKey => sel = ents.len().saturating_sub(1),
                Key::Left => {
                    note.clear();
                    if force_ask.is_some() {
                        force_ask = None;
                    } else if let Some(p) = cwd.parent() {
                        if p != cwd {
                            cwd = p.to_path_buf();
                            sel = 0;
                            off = 0;
                        }
                    }
                }
                Key::Right | Key::Enter => {
                    note.clear();
                    let Some(e) = ents.get(sel).cloned() else { continue };
                    if e.kind == Kind::Dir {
                        if e.path.is_dir() {
                            cwd = e.path.clone();
                            sel = 0;
                            off = 0;
                        }
                        continue;
                    }
                    if e.kind.dimmed() {
                        if archive_is_runnable(&e.path, &e.name.to_ascii_lowercase()) {
                            break Some(e.path);
                        }
                        force_ask = Some(e.path.clone());
                        note = format!("No program inside {}. Force install? (y/n)", fit_name(&e.name, 22));
                        continue;
                    }
                    break Some(e.path);
                }
                Key::Char(c) => {
                    if let Some(p) = force_ask.take() {
                        match c {
                            b'y' | b'Y' => break Some(p),
                            _ => {
                                note = "cancelled".into();
                            }
                        }
                        continue;
                    }

                    if c == b'u' || c == b'U' {
                        show_all = !show_all;
                        sel = 0;
                        off = 0;
                        note = if show_all {
                            "showing every file (u hides them again)".into()
                        } else {
                            "showing installable files only".into()
                        };
                    }
                }
                Key::Other => {}
            }
        let _ = d;
    };

    drop(raw);
    Ok(picked)
}
