use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{Config, ico};
use crate::naming::capitalize;
use crate::paths::Dirs;
use crate::util::Res;

#[derive(Default)]
struct Bundled {
    name: Option<String>,
    comment: Option<String>,
    categories: Option<String>,
    icon: Option<String>,
    wmclass: Option<String>,
    terminal: Option<String>,
    codes: Vec<String>,
}

fn parse_desktop(path: &Path) -> Bundled {
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut b = Bundled::default();
    let mut in_entry = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_entry = l == "[Desktop Entry]";
            continue;
        }
        if !in_entry || l.starts_with('#') {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        let v = v.trim().to_string();
        match k.trim() {
            "Name" => {
                b.name.get_or_insert(v);
            }
            "Comment" => {
                b.comment.get_or_insert(v);
            }
            "Categories" => {
                b.categories.get_or_insert(v);
            }
            "Icon" => {
                b.icon.get_or_insert(v);
            }
            "StartupWMClass" => {
                b.wmclass.get_or_insert(v);
            }
            "Terminal" => {
                b.terminal.get_or_insert(v);
            }
            "Exec" if b.codes.is_empty() => {
                b.codes = v
                    .split_whitespace()
                    .filter(|t| t.len() == 2 && t.starts_with('%') && *t != "%%")
                    .map(String::from)
                    .collect();
            }
            _ => {}
        }
    }
    b
}

fn best_icon(c: Vec<&PathBuf>) -> Option<String> {
    c.into_iter()
        .max_by_key(|p| {
            let svg = p.extension().map_or(false, |e| e.eq_ignore_ascii_case("svg"));
            (svg, fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        })
        .map(|p| p.to_string_lossy().into_owned())
}

fn resolve_icon(files: &[PathBuf], name: &str, hint: Option<&str>) -> Option<String> {
    let imgs: Vec<&PathBuf> = files
        .iter()
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).map_or(false, |e| {
                matches!(e.to_ascii_lowercase().as_str(), "png" | "svg" | "xpm")
            })
        })
        .collect();
    let stem_of = |p: &PathBuf| {
        p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase()
    };

    if let Some(h) = hint {
        if h.starts_with('/') {
            let fname = Path::new(h).file_name().map(|s| s.to_os_string());
            let c: Vec<&PathBuf> =
                imgs.iter().copied().filter(|p| p.file_name().map(|s| s.to_os_string()) == fname).collect();
            if let Some(i) = best_icon(c) {
                return Some(i);
            }
        } else {
            let hl = h.to_ascii_lowercase();
            let c: Vec<&PathBuf> = imgs.iter().copied().filter(|p| stem_of(p) == hl).collect();
            return best_icon(c).or_else(|| Some(h.to_string()));
        }
    }
    let n = name.to_ascii_lowercase();
    best_icon(imgs.iter().copied().filter(|p| stem_of(p).contains(&n)).collect())
}

fn exec_quote(p: &Path) -> String {
    let s = p.to_string_lossy();
    if s.chars().any(|c| " \t\n\"'\\><~|&;$*?#()`%".contains(c)) {
        let e = s
            .replace('\\', "\\\\\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$")
            .replace('%', "%%");
        format!("\"{e}\"")
    } else {
        s.into_owned()
    }
}

pub struct Entry {
    pub text: String,
    pub icon: Option<PathBuf>,
}

pub fn build_entry(
    name: &str,
    display: Option<&str>,
    files: &[PathBuf],
    main: &Path,
    force: bool,
) -> Option<Entry> {
    let n = name.to_ascii_lowercase();
    let deskfiles: Vec<&PathBuf> = files
        .iter()
        .filter(|p| p.extension().map_or(false, |e| e == "desktop"))
        .collect();
    let chosen = deskfiles
        .iter()
        .find(|p| p.file_stem().and_then(|s| s.to_str()).map_or(false, |s| s.to_ascii_lowercase().contains(&n)))
        .or(deskfiles.first())
        .map(|p| (*p).clone());
    let b = chosen.as_deref().map(parse_desktop).unwrap_or_default();
    let icon = resolve_icon(files, name, b.icon.as_deref());

    if chosen.is_none() && icon.is_none() && !force {
        return None;
    }

    let icon_src = icon.as_deref().filter(|i| i.starts_with('/')).map(PathBuf::from);

    let disp = display
        .map(String::from)
        .or(b.name.clone())
        .unwrap_or_else(|| capitalize(name));
    let mut exec = exec_quote(main);
    for c in &b.codes {
        exec.push(' ');
        exec.push_str(c);
    }

    let mut out = String::from("[Desktop Entry]\nType=Application\nVersion=1.0\n");
    out += &format!("Name={disp}\n");
    if let Some(c) = &b.comment {
        out += &format!("Comment={c}\n");
    }
    out += &format!("Exec={exec}\n");
    if let Some(i) = &icon {
        out += &format!("Icon={i}\n");
    }
    out += &format!("Terminal={}\n", b.terminal.as_deref().unwrap_or("false"));
    out += &format!("Categories={}\n", b.categories.as_deref().unwrap_or("Utility;"));
    if let Some(w) = &b.wmclass {
        out += &format!("StartupWMClass={w}\n");
    }
    Some(Entry { text: out, icon: icon_src })
}

pub fn write_desktop(
    d: &Dirs,
    name: &str,
    display: Option<&str>,
    files: &[PathBuf],
    main: &Path,
    force: bool,
) -> Res<Option<PathBuf>> {
    let Some(e) = build_entry(name, display, files, main, force) else {
        return Ok(None);
    };
    let path = d.desktop.join(format!("{name}.desktop"));
    fs::write(&path, e.text).map_err(|err| format!("cannot write {}: {err}", path.display()))?;
    Ok(Some(path))
}

pub fn update_desktop_db(d: &Dirs) {
    let cfg = Config::load();
    match Command::new("update-desktop-database").arg(&d.desktop).status() {
        Ok(s) if s.success() => println!("{} {} Desktop database updated", cfg.step(), cfg.icon(ico::CHECK)),
        Ok(_) => eprintln!(
            "warning: {} update-desktop-database failed (menu may need a relogin)",
            cfg.icon(ico::WARN)
        ),
        Err(_) => println!(
            "{} {} update-desktop-database not found (install desktop-file-utils); most menus refresh on their own",
            cfg.step(), cfg.icon(ico::INFO)
        ),
    }
}
