use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::util::Res;

#[derive(Clone, Debug)]
pub enum Node {
    Dir,
    File,
    Link(PathBuf),
    Other,
}

pub fn scan_tree(root: &Path) -> Res<Vec<(PathBuf, Node)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("cannot read {}: {e}", dir.display())),
        };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                out.push((p.clone(), Node::Link(fs::read_link(&p).unwrap_or_default())));
            } else if ft.is_dir() {
                out.push((p.clone(), Node::Dir));
                stack.push(p);
            } else if ft.is_file() {
                out.push((p, Node::File));
            } else {
                out.push((p, Node::Other));
            }
        }
    }
    out.sort_by_key(|(p, _)| (p.components().count(), p.clone()));
    Ok(out)
}

pub fn deb_rel(p: &Path, root: &Path) -> Res<String> {
    let mut s = p.strip_prefix(root).map_err(|_| format!("unexpected path {}", p.display()))?
        .to_string_lossy()
        .replace('\\', "/");
    while let Some(t) = s.strip_prefix("./") {
        s = t.to_string();
    }
    let s = s.trim_start_matches('/').to_string();
    if s.is_empty() {
        return Err(format!("empty path in {}", p.display()));
    }
    if s.split('/').any(|c| c == ".." || c == "." || c.is_empty()) {
        return Err(format!("refusing suspicious path '{s}' from the package"));
    }
    Ok(s)
}

pub fn deb_target(rel: &str, root_mode: bool, local: &Path, app_dir: &Path) -> (PathBuf, bool) {
    if root_mode {
        return (Path::new("/").join(rel), true);
    }
    let r = rel.strip_prefix("usr/local/").or_else(|| rel.strip_prefix("usr/"));
    match r {
        Some(rest) => (local.join(rest), true),
        None if rel == "usr" || rel == "usr/local" => (local.to_path_buf(), true),

        None => (app_dir.join("system").join(rel), true),
    }
}

pub fn deb_bin_name(rel: &str) -> Option<&str> {
    let r = rel.strip_prefix("usr/local/").or_else(|| rel.strip_prefix("usr/"))?;
    let (dir, file) = r.rsplit_once('/')?;
    if matches!(dir, "bin" | "sbin" | "games") && !file.is_empty() && !file.starts_with('.') {
        Some(file)
    } else {
        None
    }
}
