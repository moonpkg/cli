use std::fs;
use std::io::Read;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use crate::util::Res;

pub fn walk(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if depth < max_depth {
                    stack.push((p, depth + 1));
                }
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

pub fn is_exec_file(p: &Path) -> bool {
    let Ok(m) = fs::metadata(p) else { return false };
    if !m.is_file() || m.permissions().mode() & 0o111 == 0 {
        return false;
    }
    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if name.ends_with(".so") || name.contains(".so.") {
        return false;
    }
    let Ok(mut f) = fs::File::open(p) else { return false };
    let mut buf = [0u8; 4];
    let n = f.read(&mut buf).unwrap_or(0);
    (n == 4 && &buf == b"\x7fELF") || (n >= 2 && &buf[..2] == b"#!")
}

pub fn pick_main(root: &Path, files: &[PathBuf], name: &str) -> Option<PathBuf> {
    const BAD: &[&str] = &["sandbox", "crashpad", "uninstall", "helper", "updater", "chrome_"];
    let n = name.to_ascii_lowercase();
    files
        .iter()
        .filter(|p| is_exec_file(p))
        .map(|p| {
            let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
            let stem = fname.strip_suffix(".sh").unwrap_or(&fname).to_string();
            let depth = p.strip_prefix(root).map(|r| r.components().count()).unwrap_or(9) as i64;
            let mut s: i64 = 0;
            if stem == n {
                s += 100;
            } else if stem.len() >= 3 && (stem.contains(&n) || n.contains(&stem)) {
                s += 50;
            }
            if p.parent().and_then(|d| d.file_name()).map_or(false, |d| d == "bin") {
                s += 20;
            }
            s -= depth * 3;
            if BAD.iter().any(|k| fname.contains(k)) {
                s -= 40;
            }
            if fname.ends_with(".sh") && s < 50 {
                s -= 10;
            }
            let size = fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            (s, size, p.clone())
        })
        .max_by_key(|(s, size, _)| (*s, *size))
        .map(|t| t.2)
}

pub fn copy_single(src: &Path, dest: &Path, file_name: &str) -> Res<()> {
    let target = dest.join(file_name);
    fs::copy(src, &target).map_err(|e| format!("copy failed: {e}"))?;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())
}

pub fn tree_size(p: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![p.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else {
                total += m.len();
            }
        }
    }
    total
}

pub fn desktop_and_icon(files: &[PathBuf], name: &str) -> (bool, bool) {
    let n = name.to_ascii_lowercase();
    let desk = files.iter().any(|p| {
        p.extension().map_or(false, |e| e == "desktop")
            && p.parent().map_or(false, |d| d.ends_with("applications"))
    });
    let icon = files.iter().any(|p| {
        p.extension()
            .and_then(|e| e.to_str())
            .map_or(false, |e| matches!(e.to_ascii_lowercase().as_str(), "png" | "svg" | "xpm"))
            && p.file_stem()
                .and_then(|s| s.to_str())
                .map_or(false, |s| s.to_ascii_lowercase().contains(&n))
    });
    (desk, icon || (desk && !icon))
}

pub fn copy_tree(src: &Path, dst: &Path) -> Res<u64> {
    if !src.is_dir() {
        return Err(format!("{} is not a directory", src.display()));
    }
    fs::create_dir_all(dst).map_err(|e| format!("cannot create {}: {e}", dst.display()))?;
    let mut total = 0u64;
    for e in fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
        let p = e.path();
        let t = dst.join(e.file_name());
        let Ok(m) = e.metadata() else { continue };
        if m.is_dir() {
            total += copy_tree(&p, &t)?;
            continue;
        }
        let len = m.len();
        copy_any(&p, &t)?;
        total += len;
    }
    Ok(total)
}

pub fn copy_any(src: &Path, dst: &Path) -> Res<()> {
    if fs::symlink_metadata(src).map_or(false, |m| m.file_type().is_symlink()) {
        let t = fs::read_link(src).map_err(|e| e.to_string())?;
        let _ = fs::remove_file(dst);
        return symlink(t, dst).map_err(|e| e.to_string());
    }
    fs::copy(src, dst).map_err(|e| format!("cannot back up {}: {e}", src.display()))?;
    Ok(())
}
