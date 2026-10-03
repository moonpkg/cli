use std::collections::BTreeSet;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::fsutil::is_exec_file;
use crate::naming::ARCHIVE_EXTS;
use crate::util::Res;

pub fn try_run(cmd: &str, args: &[&OsStr]) -> Res<bool> {
    match Command::new(cmd).args(args).status() {
        Ok(s) if s.success() => Ok(true),
        Ok(s) => Err(format!("`{cmd}` failed ({s})")),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("cannot run `{cmd}`: {e}")),
    }
}

pub fn looks_like(src: &Path) -> &'static str {
    let mut buf = [0u8; 512];
    let n = fs::File::open(src).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
    let head = &buf[..n];
    if n >= 5 && (head.starts_with(b"<!DOC") || head.starts_with(b"<html") || head.starts_with(b"<HTML")) {
        "an HTML page (a failed download, not an archive)"
    } else if n >= 8 && head.starts_with(b"\x89PNG\r\n\x1a\n") {
        "a PNG image"
    } else if n >= 6 && (head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a")) {
        "a GIF image"
    } else if n >= 3 && head.starts_with(b"%PDF") {
        "a PDF document"
    } else if n >= 3 && head.starts_with(b"\xff\xd8\xff") {
        "a JPEG image"
    } else if n >= 2 && head.starts_with(b"PK") {
        "a zip file"
    } else if n >= 4 && head.starts_with(b"\x7fELF") {
        "a Linux binary"
    } else if n >= 2 && head.starts_with(b"#!") {
        "a script"
    } else if n == 0 {
        "an empty file"
    } else if head.iter().take(64).all(|b| *b >= 0x09 && *b <= 0x0d || *b == 0x20) {
        "a text file"
    } else {
        "data of an unknown kind"
    }
}

pub fn try_capture(cmd: &str, args: &[&OsStr]) -> (bool, bool, String) {
    match Command::new(cmd).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).output() {
        Ok(o) => (o.status.success(), true, String::from_utf8_lossy(&o.stderr).into_owned()),
        Err(e) => (false, e.kind() != ErrorKind::NotFound, format!("{e}")),
    }
}

pub fn elf_arch(head: &[u8]) -> Option<&'static str> {
    if head.len() < 20 || &head[..4] != b"\x7fELF" {
        return None;
    }
    let bits = match head[4] {
        2 => 64,
        1 => 32,
        _ => return None,
    };
    let machine = match head[5] {
        1 => u16::from_le_bytes([head[18], head[19]]),
        2 => u16::from_be_bytes([head[18], head[19]]),
        _ => return None,
    };
    Some(match machine {
        0x03 => "i686",
        0x08 => "mips",
        0x14 => "ppc",
        0x15 => "ppc64",
        0x16 => "s390x",
        0x28 => if bits == 64 { "aarch64" } else { "armv7" },
        0x3E => "x86_64",
        0xB7 => "aarch64",
        0xF3 => if bits == 64 { "riscv64" } else { "riscv" },
        _ => return None,
    })
}

pub fn file_kind(src: &Path, fname: &str) -> (String, String) {
    let mut buf = [0u8; 8];
    let n = fs::File::open(src).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
    let head = &buf[..n];
    let l = fname.to_ascii_lowercase();

    if n >= 4 && &head[..4] == b"\x7fELF" {
        if l.ends_with(".appimage") || is_appimage(src) {
            return ("Linux application".into(), "AppImage".into());
        }
        return ("Linux application".into(), "ELF executable".into());
    }
    if n >= 2 && &head[..2] == b"#!" {
        let mut line = String::new();
        if let Ok(fl) = fs::File::open(src) {
            let mut rd = std::io::BufReader::new(fl);
            let _ = rd.read_line(&mut line);
        }
        let interp = line.trim_start_matches("#!").trim();
        let short = interp.rsplit('/').next().unwrap_or("sh");
        return (
            "Command line program".into(),
            format!("{short} script"),
        );
    }
    if n >= 4 && &head[..4] == b"PK\x03\x04" {
        return ("Archive".into(), "zip archive".into());
    }
    if n >= 6 && &head[..6] == b"7z\xbc\xaf\x27\x1c" {
        return ("Archive".into(), "7-zip archive".into());
    }
    if n >= 2 && &head[..2] == b"!<ar" {
        return ("Debian package".into(), "ar archive".into());
    }
    if n >= 2 && &head[..2] == b"MZ" {
        return (
            "Windows program".into(),
            "PE executable - cannot run on Linux".into(),
        );
    }
    if n >= 5 && (head.starts_with(b"<!DOC") || head.starts_with(b"<html") || head.starts_with(b"<HTML")) {
        return ("Download failure".into(), "an HTML page, not a program".into());
    }
    if ARCHIVE_EXTS.iter().any(|e| l.ends_with(e)) || is_compressed(&buf[..n]) {
        return ("Archive".into(), archive_format(&l));
    }
    match looks_like(src) {
        "a text file" => ("Not a program".into(), "plain text".into()),
        "an empty file" => ("Not a program".into(), "empty".into()),
        "data of an unknown kind" => ("Unknown".into(), "data of an unknown kind".into()),
        other => ("Not a program".into(), other.trim_start_matches("a ").to_string()),
    }
}

fn is_appimage(src: &Path) -> bool {
    const MAGIC: &[u8] = b"hsqs";
    let Ok(f) = fs::File::open(src) else { return false };
    let mut r = std::io::BufReader::with_capacity(1 << 20, f);
    let mut win = vec![0u8; 1 << 20];
    loop {
        let Ok(n) = r.read(&mut win) else { return false };
        if n == 0 {
            return false;
        }
        if win[..n].windows(MAGIC.len()).any(|w| w == MAGIC) {
            return true;
        }
    }
}

pub fn is_compressed(head: &[u8]) -> bool {
    head.len() >= 2
        && (head.starts_with(b"\x1f\x8b")
            || head.starts_with(b"\xfd7zXZ")
            || head.starts_with(b"BZh")
            || head.starts_with(b"\x28\xb5\x2f\xfd")
            || head.starts_with(b"7z\xbc\xaf\x27\x1c")
            || head.starts_with(b"PK"))
}

pub fn archive_format(lname: &str) -> String {
    const KINDS: &[(&str, &str)] = &[
        (".tar.gz", "tar archive, gzip"),
        (".tgz", "tar archive, gzip"),
        (".tar.xz", "tar archive, xz"),
        (".txz", "tar archive, xz"),
        (".tar.bz2", "tar archive, bzip2"),
        (".tbz2", "tar archive, bzip2"),
        (".tbz", "tar archive, bzip2"),
        (".tar.zst", "tar archive, zstd"),
        (".tzst", "tar archive, zstd"),
        (".tar.lzma", "tar archive, lzma"),
        (".tar.lz", "tar archive, lzip"),
        (".tar.z", "tar archive, compress"),
        (".tar", "tar archive"),
        (".zip", "zip archive"),
        (".7z", "7-zip archive"),
        (".appimage", "AppImage"),
    ];
    KINDS.iter().find(|(e, _)| lname.ends_with(e)).map_or_else(|| "archive".to_string(), |(_, k)| (*k).to_string())
}

pub fn sniff(path: &Path) -> &'static str {
    let mut buf = [0u8; 4];
    let n = fs::File::open(path).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
    if n == 4 && &buf == b"\x7fELF" {
        "elf"
    } else if n == 4 && &buf == b"PK\x03\x04" {
        "zip"
    } else if n >= 2 && &buf[..2] == b"#!" {
        "script"
    } else {
        "tar"
    }
}

pub fn uname_m() -> String {
    match env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        "x86" => "i686",
        "arm" => "arm",
        "powerpc64" => "ppc64",
        a => a,
    }
    .to_string()
}

pub fn head_bytes(p: &Path) -> Vec<u8> {
    let mut buf = [0u8; 8];
    let n = fs::File::open(p).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
    buf[..n].to_vec()
}

pub fn is_archive_name(lname: &str) -> bool {
    ARCHIVE_EXTS.iter().any(|e| lname.ends_with(e))
}

pub fn attempt(cmd: &str, args: &[&OsStr]) -> Res<Option<bool>> {
    match Command::new(cmd).args(args).status() {
        Ok(s) => Ok(Some(s.success())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot run `{cmd}`: {e}")),
    }
}

pub fn missing_libs(bins: &[PathBuf]) -> Vec<String> {
    let mut miss = BTreeSet::new();
    for b in bins.iter().take(64) {
        if !is_exec_file(b) {
            continue;
        }
        let Ok(out) = Command::new("ldd").arg(b).output() else { return Vec::new() };
        let text = String::from_utf8_lossy(&out.stdout);
        let text = format!("{text}\n{}", String::from_utf8_lossy(&out.stderr));
        for l in text.lines() {
            if let Some((pre, _)) = l.split_once("=> not found") {
                miss.insert(pre.trim().to_string());
            }
        }
    }
    miss.into_iter().collect()
}
