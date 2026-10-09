use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::paths::Dirs;
use crate::util::Res;

#[derive(Default)]
pub struct Manifest {
    pub dir: Option<PathBuf>,
    pub links: Vec<PathBuf>,
    pub link_targets: Vec<PathBuf>,
    pub desktop: Option<PathBuf>,
    pub main: Option<PathBuf>,

    pub files: Vec<PathBuf>,
    pub subdirs: Vec<PathBuf>,
    pub version: Option<String>,
    pub cmds: Vec<String>,
    pub deb: bool,
    pub root: bool,
    pub portable: Option<PathBuf>,
    pub deps: Vec<DepGroup>,
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct DepGroup {
    pub distro: String,
    pub packages: Vec<String>,
}

pub fn manifest_path(d: &Dirs, name: &str) -> PathBuf {
    d.manifests.join(format!("{name}.manifest"))
}

pub fn read_manifest(d: &Dirs, name: &str) -> Option<Manifest> {
    Some(parse_manifest(&fs::read_to_string(manifest_path(d, name)).ok()?))
}

pub fn parse_manifest(text: &str) -> Manifest {
    let mut m = Manifest::default();
    let mut sec: Option<String> = None;
    let mut pkgs: Vec<String> = Vec::new();
    let flush = |sec: &mut Option<String>, pkgs: &mut Vec<String>, out: &mut Vec<DepGroup>| {
        if let Some(d) = sec.take() {
            if !pkgs.is_empty() {
                out.push(DepGroup { distro: d, packages: std::mem::take(pkgs) });
            }
        }
    };
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with('[') && t.ends_with(']') {
            flush(&mut sec, &mut pkgs, &mut m.deps);
            sec = Some(t[1..t.len() - 1].trim().to_string());
            continue;
        }
        if sec.is_some() {
            for w in t.split_whitespace() {
                pkgs.push(w.to_string());
            }
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        let p = PathBuf::from(v);
        match k {
            "dir" => m.dir = Some(p),
            "link" => m.links.push(p),
            "to" => m.link_targets.push(p),
            "desktop" => m.desktop = Some(p),
            "main" => m.main = Some(p),
            "file" => m.files.push(p),
            "subdir" => m.subdirs.push(p),
            "version" => m.version = Some(v.to_string()),
            "cmd" => m.cmds.push(v.to_string()),
            "deb" => m.deb = true,
            "root" => m.root = true,
            "portable" => m.portable = Some(p),
            _ => {}
        }
    }
    flush(&mut sec, &mut pkgs, &mut m.deps);
    m
}

pub fn write_manifest(d: &Dirs, name: &str, m: &Manifest) -> Res<()> {
    fs::write(manifest_path(d, name), render_manifest(m)).map_err(|e| format!("cannot write manifest: {e}"))
}

pub fn render_manifest(m: &Manifest) -> String {
    let mut s = String::new();
    if let Some(p) = &m.dir {
        s += &format!("dir={}\n", p.display());
    }
    if let Some(p) = &m.main {
        s += &format!("main={}\n", p.display());
    }
    if let Some(v) = &m.version {
        s += &format!("version={v}\n");
    }
    if m.deb {
        s += "deb=1\n";
    }
    if m.root {
        s += "root=1\n";
    }
    if let Some(p) = &m.portable {
        s += &format!("portable={}\n", p.display());
    }
    for l in &m.links {
        s += &format!("link={}\n", l.display());
    }
    for t in &m.link_targets {
        s += &format!("to={}\n", t.display());
    }
    for c in &m.cmds {
        s += &format!("cmd={c}\n");
    }
    for f in &m.files {
        s += &format!("file={}\n", f.display());
    }

    let mut sd: Vec<&PathBuf> = m.subdirs.iter().collect();
    sd.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for p in sd {
        s += &format!("subdir={}\n", p.display());
    }
    if let Some(p) = &m.desktop {
        s += &format!("desktop={}\n", p.display());
    }
    for g in &m.deps {
        s += &format!("\n[{}]\n{}\n", g.distro, g.packages.join(" "));
    }
    s
}

pub fn moon_owned_link(p: &Path, d: &Dirs) -> bool {
    fs::read_link(p).map_or(false, |t| t.starts_with(&d.apps))
}

pub fn link_target(p: &Path) -> Option<PathBuf> {
    fs::read_link(p).ok()
}

pub fn link_is_ours(p: &Path, m: &Manifest, d: &Dirs) -> bool {
    if moon_owned_link(p, d) {
        return true;
    }
    match link_target(p) {
        Some(t) => m.dir.as_ref().is_some_and(|root| t.starts_with(root)),
        None => false,
    }
}

pub struct LastInstall {
    pub name: String,
    pub source: String,
    pub when: u64,
}

pub fn state_path(d: &Dirs) -> PathBuf {
    d.apps.parent().unwrap_or(&d.apps).join("last")
}

pub fn read_last(d: &Dirs) -> Option<LastInstall> {
    let t = fs::read_to_string(state_path(d)).ok()?;
    let mut l = LastInstall { name: String::new(), source: String::new(), when: 0 };
    for line in t.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "name" => l.name = v.to_string(),
            "source" => l.source = v.to_string(),
            "when" => l.when = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    (!l.name.is_empty()).then_some(l)
}

pub fn write_last(d: &Dirs, l: &LastInstall) {
    if let Some(p) = state_path(d).parent() {
        let _ = fs::create_dir_all(p);
    }
    let _ = fs::write(
        state_path(d),
        format!("name={}\nsource={}\nwhen={}\n", l.name, l.source, if l.when > 0 { l.when } else { now_secs() }),
    );
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
