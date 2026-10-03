use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::fsutil::is_exec_file;
use crate::naming::ARCHIVE_EXTS;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Dir,
    Supported,

    Unsupported,
}

impl Kind {
    fn rank(self) -> u8 {
        match self {
            Kind::Dir => 0,
            Kind::Supported => 1,
            Kind::Unsupported => 2,
        }
    }
    pub fn dimmed(self) -> bool {
        self == Kind::Unsupported
    }
}

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
}

fn sniff_head(prog: &str, args: &[&OsStr], want: usize) -> Option<Vec<u8>> {
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let mut buf = vec![0u8; want];
    let mut got = 0;
    while got < want {
        match out.read(&mut buf[got..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => got += k,
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    buf.truncate(got);
    (!buf.is_empty()).then_some(buf)
}

fn looks_runnable(head: &[u8]) -> bool {
    head.starts_with(b"\x7fELF") || head.starts_with(b"#!")
}

fn maybe_program(name: &str) -> bool {
    let base = name.rsplit('/').next().unwrap_or(name).to_ascii_lowercase();
    match base.rsplit_once('.') {
        None => !base.is_empty(),
        Some((_, ext)) => matches!(
            ext,
            "sh" | "bash" | "zsh" | "fish" | "run" | "bin" | "exe" | "py" | "pl" | "rb" | "js" | "appimage" | "elf"
        ),
    }
}

pub fn archive_is_runnable(path: &Path, lname: &str) -> bool {
    let os = OsStr::new;
    let listing = Command::new("bsdtar")
        .args([os("-tf"), path.as_os_str()])
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .or_else(|| {
            Command::new("unzip")
                .args([os("-Z1"), path.as_os_str()])
                .stdin(Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
        });
    let Some(listing) = listing else {
        return false;
    };
    let members: Vec<String> = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.ends_with('/'))
        .collect();
    if members.is_empty() {
        return false;
    }

    let check_all = members.len() <= 4;

    for m in members.iter().filter(|m| check_all || maybe_program(m)).take(8) {
        let m = OsStr::new(m);
        let head = sniff_head("bsdtar", &[os("-xOf"), path.as_os_str(), m], 4)
            .or_else(|| sniff_head("unzip", &[os("-p"), path.as_os_str(), m], 4))
            .or_else(|| sniff_head("7z", &[os("e"), os("-so"), path.as_os_str(), m], 4));
        if head.is_some_and(|h| looks_runnable(&h)) {
            return true;
        }
    }
    let _ = lname;
    false
}

fn classify(name: &str, path: &Path) -> Kind {
    let l = name.to_ascii_lowercase();
    if path.is_dir() {
        return Kind::Dir;
    }

    if l.ends_with(".deb") || l.ends_with(".appimage") {
        return Kind::Supported;
    }

    if l.ends_with(".zip") || l.ends_with(".7z") || l.ends_with(".rar") {
        return Kind::Unsupported;
    }
    if ARCHIVE_EXTS.iter().any(|e| l.ends_with(e)) {
        return Kind::Supported;
    }
    if is_exec_file(path) {
        return Kind::Supported;
    }
    Kind::Unsupported
}

pub fn list_dir(d: &Path, show_all: bool) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let Ok(rd) = fs::read_dir(d) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let kind = classify(&name, &p);
        if kind == Kind::Unsupported && !show_all {
            continue;
        }
        out.push(Entry { name, path: p, kind });
    }

    out.sort_by_key(|e| (e.kind.rank(), e.name.to_lowercase()));
    out
}
