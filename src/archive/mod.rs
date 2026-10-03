use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::{Config, ico};
use crate::fsutil::copy_single;
use crate::naming::ARCHIVE_EXTS;
use crate::probe::{attempt, looks_like, sniff, try_capture, try_run};
use crate::util::Res;

fn unpack_error(src: &Path, tool: &str, stderr: &str) -> String {
    let file = src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let detail = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>();
    let cause = if detail.iter().any(|l| l.contains("not a tar archive") || l.contains("File format not recognized")) {
        format!("{file} is {}.", looks_like(src))
    } else if detail.iter().any(|l| l.contains("Unexpected EOF") || l.contains("unexpected end of file")) {
        format!("{file} is truncated - the download did not finish.")
    } else if detail.iter().any(|l| l.contains("Checksum") || l.contains("corrupt")) {
        format!("{file} is damaged (checksum error).")
    } else if let Some(l) = detail.iter().find(|l| !l.contains("tar:") && !l.contains("xz:")) {
        format!("{file}: {l}")
    } else {
        format!("`{tool}` could not unpack {file}.")
    };
    let extra = detail
        .iter()
        .filter(|l| !l.is_empty() && !l.starts_with(&format!("{file}: {l}")))
        .take(2)
        .map(|l| format!("\n  {tool}: {l}"))
        .collect::<String>();
    format!("{cause}{extra}")
}

pub fn extract(src: &Path, lname: &str, dest: &Path, name: &str) -> Res<()> {
    let os = |s: &'static str| OsStr::new(s);
    let kind = if lname.ends_with(".appimage") {
        "appimage"
    } else if lname.ends_with(".zip") {
        "zip"
    } else if lname.ends_with(".7z") {
        "7z"
    } else if ARCHIVE_EXTS.iter().any(|e| lname.ends_with(e)) {
        "tar"
    } else {
        sniff(src)
    };

    match kind {
        "appimage" => copy_single(src, dest, &format!("{name}.AppImage")),
        "elf" | "script" => copy_single(src, dest, name),
        "zip" => {
            let (s, d) = (src.as_os_str(), dest.as_os_str());
            let o = std::ffi::OsString::from(format!("-o{}", dest.display()));
            let tools: [(&str, Vec<&OsStr>); 3] = [
                ("unzip", vec![os("-q"), os("-o"), s, os("-d"), d]),
                ("bsdtar", vec![os("-xf"), s, os("-C"), d]),
                ("7z", vec![os("x"), os("-y"), s, &o]),
            ];
            let mut any_tool = false;
            let mut why = String::new();
            for (tool, argv) in tools {
                let (ok, found, err) = try_capture(tool, &argv);
                any_tool |= found;
                if ok {
                    return Ok(());
                }
                if !why.is_empty() {
                    why.push_str("; ");
                }
                why.push_str(&unpack_error(src, tool, &err));
            }
            if any_tool {
                Err(why)
            } else {
                Err("no unzip / bsdtar / 7z found, install one of them".into())
            }
        }
        "7z" => {
            let o = std::ffi::OsString::from(format!("-o{}", dest.display()));
            let mut any_tool = false;
            let mut why = String::new();
            for cmd in ["7z", "7zz", "7za", "7zr"] {
                let (ok, found, err) = try_capture(cmd, &[os("x"), os("-y"), src.as_os_str(), &o]);
                any_tool |= found;
                if ok {
                    return Ok(());
                }
                if !why.is_empty() {
                    why.push_str("; ");
                }
                why.push_str(&unpack_error(src, cmd, &err));
            }
            if any_tool {
                Err(why)
            } else {
                Err("no 7z/7zz/7za found, install p7zip".into())
            }
        }
        _ => {
            let (ok, found, err) = try_capture(
                "tar",
                &[os("-xf"), src.as_os_str(), os("-C"), dest.as_os_str(), os("--no-same-owner")],
            );
            if ok {
                Ok(())
            } else if found {
                Err(unpack_error(src, "tar", &err))
            } else {
                Err("`tar` not found".into())
            }
        }
    }
}

pub fn download(url: &str, work: &Path) -> Res<PathBuf> {
    let fname = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("download")
        .to_string();
    let out = work.join(&fname);
    println!("==> {} Downloading {url}", Config::load().icon(ico::DOWNLOAD));
    let os = OsStr::new;
    if try_run("curl", &[os("-fL"), os("--progress-bar"), os("-o"), out.as_os_str(), os(url)])?
        || try_run("wget", &[os("-O"), out.as_os_str(), os(url)])?
    {
        Ok(out)
    } else {
        Err("neither curl nor wget found".into())
    }
}

pub fn unpack_tar(src: &Path, dest: &Path) -> Res<()> {
    let os = OsStr::new;
    let (s, d) = (src.as_os_str(), dest.as_os_str());
    let mut last = String::from("no tar found");
    for (cmd, args) in [
        ("tar", vec![os("-xf"), s, os("-C"), d, os("--no-same-owner")]),
        ("bsdtar", vec![os("-xf"), s, os("-C"), d]),
    ] {
        match attempt(cmd, &args)? {
            Some(true) => return Ok(()),
            Some(false) => last = format!("`{cmd}` failed"),
            None => last = format!("`{cmd}` not found"),
        }
    }

    if is_zstd(src) && zstd_pipe_tar(src, dest).is_ok() {
        return Ok(());
    }
    Err(format!("cannot unpack {}: {last} (install tar / libarchive / zstd)", src.display()))
}

fn is_zstd(p: &Path) -> bool {
    let mut b = [0u8; 4];
    fs::File::open(p).and_then(|mut f| f.read_exact(&mut b)).is_ok()
        && (b[0] == 0x28 && b[1] == 0xB5 && b[2] == 0x2F && b[3] == 0xFD
            || (b[0] == 0x50 && b[1] == 0x2A && b[2] == 0x4D && b[3] == 0x18))
}

fn zstd_pipe_tar(src: &Path, dest: &Path) -> Res<()> {
    let os = OsStr::new;
    let mut z = Command::new("zstd")
        .args([os("-dcq"), src.as_os_str()])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let Some(out) = z.stdout.take() else { return Err("zstd gave no output".into()) };
    let mut t = Command::new("tar")
        .args([os("-xf"), os("-"), os("-C"), dest.as_os_str(), os("--no-same-owner")])
        .stdin(Stdio::from(out))
        .spawn()
        .map_err(|e| e.to_string())?;
    let _ = z.wait();
    if t.wait().map(|s| s.success()).unwrap_or(false) {
        Ok(())
    } else {
        Err("tar failed on the zstd stream".into())
    }
}
