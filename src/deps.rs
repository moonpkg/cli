//! Downloadable, per-distro dependencies for bundles.
//!
//! A bundle may declare the system packages it needs in per-distro sections:
//!
//! [arch]
//! webkit2gtk-4.1 gtk3
//!
//! [ubuntu]
//! libwebkit2gtk-4.1-0 libgtk-3-0
//!
//! When 'moon install' finds the bundle's program misses shared libraries and
//! a section matches the machine's distro, the packages are downloaded from
//! the distro's own repositories (no root required) and their shared objects
//! are unpacked into the app's 'lib/' folder next to the program, where an
//! '$ORIGIN/lib' rpath finds them. Nothing touches the system: the files
//! belong to the app and gets uninstalled with 'moon remove'.
//!
//! Supported systems and how they are fetched:
//!
//! * **deb** ('[debian]', '[ubuntu]', ...): 'apt-get download' + 'dpkg-deb -x'
//! * **arch**: the Arch package search JSON API + a mirror + 'tar --zstd'
//! * **fedora / opensuse**: 'dnf download' / 'zypper download' + 'rpm2cpio' + 'cpio'

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::probe::uname_m;
use crate::state::DepGroup;
use crate::util::Res;

pub fn os_release() -> (String, Vec<String>) {
    parse_os_release(&fs::read_to_string("/etc/os-release").unwrap_or_default())
}

pub fn parse_os_release(text: &str) -> (String, Vec<String>) {
    let mut id = String::new();
    let mut like: Vec<String> = Vec::new();
    for l in text.lines() {
        let Some((k, v)) = l.split_once('=') else { continue };
        let v = v.trim().trim_matches('"');
        match k {
            "ID" => id = v.to_string(),
            "ID_LIKE" => like = v.split_whitespace().map(|s| s.to_string()).collect(),
            _ => {}
        }
    }
    (id, like)
}

pub fn family(id: &str) -> Option<&'static str> {
    Some(match id.to_ascii_lowercase().as_str() {
        "arch" | "archarm" | "manjaro" | "endeavouros" | "cachyos" | "garuda" => "arch",
        "debian" | "ubuntu" | "linuxmint" | "pop" | "elementary" | "zorin" | "kali" | "raspbian" | "nobara" => "debian",
        "fedora" | "rhel" | "centos" | "rocky" | "almalinux" | "ol" => "fedora",
        "opensuse" | "opensuse-leap" | "opensuse-tumbleweed" | "opensuse-tumbleweed-kubic" | "sles" | "opensuse-microos" => "opensuse",
        "alpine" => "alpine",
        _ => return None,
    })
}

pub fn pick_group<'a>(deps: &'a [DepGroup], id: &str, id_like: &[String]) -> Option<&'a DepGroup> {
    let names: Vec<String> = std::iter::once(id.to_string())
        .chain(id_like.iter().cloned())
        .collect();
    for n in &names {
        if let Some(g) = deps.iter().find(|g| g.distro.eq_ignore_ascii_case(n)) {
            return Some(g);
        }
    }
    for n in &names {
        if let Some(f) = family(n) {
            if let Some(g) = deps.iter().find(|g| g.distro.eq_ignore_ascii_case(f)) {
                return Some(g);
            }
        }
    }
    None
}

pub fn fetch_deps(g: &DepGroup, libdir: &Path) -> Res<(usize, Vec<String>)> {
    let fam = family(&g.distro)
        .ok_or_else(|| format!("dependency section '{}' is not a system moon can fetch packages for", g.distro))?;
    fs::create_dir_all(libdir).map_err(|e| format!("cannot create {}: {e}", libdir.display()))?;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let work = std::env::temp_dir().join(format!("moon-deps-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&work).map_err(|e| format!("cannot create {}: {e}", work.display()))?;

    let res = match fam {
        "debian" => fetch_deb(g, &work, libdir),
        "arch" => fetch_arch(g, &work, libdir),
        "fedora" | "opensuse" => fetch_rpm(g, fam, &work, libdir),
        other => Err(format!("have no way to download packages for '{other}' yet")),
    };
    let _ = fs::remove_dir_all(&work);
    res
}

///
///
/// deb
///
///

fn fetch_deb(g: &DepGroup, work: &Path, libdir: &Path) -> Res<(usize, Vec<String>)> {
    let mut args: Vec<String> = vec!["download".into()];
    args.extend(g.packages.iter().cloned());
    run(work, "apt-get", &args.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        .map_err(|e| format!("{e} (make sure the apt package lists exist)"))?;

    let debs = list_files(work, |n| n.ends_with(".deb"));
    if debs.is_empty() {
        return Err("`apt-get download` produced no .deb files".into());
    }
    let mut names = Vec::new();
    for (i, deb) in debs.iter().enumerate() {
        let x = work.join(format!("deb{i}"));
        fs::create_dir_all(&x).map_err(|e| format!("cannot create {}: {e}", x.display()))?;
        run(
            &x,
            "dpkg-deb",
            &["-x", deb.to_str().unwrap_or(""), x.to_str().unwrap_or("")],
        )
        .map_err(|e| format!("cannot unpack {}: {e}", deb.display()))?;
        if let Some(n) = deb.file_name() {
            names.push(n.to_string_lossy().into_owned());
        }
    }
    let mut copied = 0;
    for i in 0..debs.len() {
        copied += copy_libs(&work.join(format!("deb{i}")).join("usr"), libdir);
    }
    Ok((copied, names))
}

///
///
/// arch
///
///

fn fetch_arch(g: &DepGroup, work: &Path, libdir: &Path) -> Res<(usize, Vec<String>)> {
    let mut copied = 0;
    let mut names = Vec::new();
    for pkg in &g.packages {
        let q = pkg.replace('+', "%2B");
        let url = format!("https://archlinux.org/packages/search/json/?q={q}");
        let json = work.join(format!("search-{pkg}.json"));
        if let Err(e) = fetch_url(&url, &json) {
            eprintln!("  moon: cannot query the Arch package database for {pkg}: {e}");
            continue;
        }
        let text = fs::read_to_string(&json).unwrap_or_default();
        let entries = arch_entries(&text, &uname_m());
        let Some((repo, file)) = pick_entry(&entries, pkg) else {
            eprintln!("  moon: no {} package named '{pkg}' found in the Arch repositories", uname_m());
            continue;
        };
        let dl = format!("https://geo.mirror.pkgbuild.com/{repo}/os/{}/{}", uname_m(), file);
        let dest = work.join(file);
        if let Err(e) = fetch_url(&dl, &dest) {
            eprintln!("  moon: cannot fetch {file}: {e}");
            continue;
        }
        let x = work.join("pkg");
        let _ = fs::remove_dir_all(&x);
        if fs::create_dir_all(&x).is_err() || !extract_tar(&dest, &x) {
            eprintln!("  moon: cannot unpack {file} (needs tar with zstd, or bsdtar)");
            continue;
        }
        copied += copy_libs(&x, libdir);
        names.push(pkg.clone());
    }
    if names.is_empty() {
        return Err("no Arch dependency package could be downloaded".into());
    }
    Ok((copied, names))
}

struct ArchEntry {
    name: String,
    repo: String,
    file: String,
}

fn arch_entries(json: &str, arch: &str) -> Vec<ArchEntry> {
    let mut out = Vec::new();
    for obj in result_objects(json) {
        let name = field(obj, "pkgname");
        let repo = field(obj, "repo");
        let file = field(obj, "filename");
        if !name.is_empty() && !repo.is_empty() && !file.is_empty() && field(obj, "arch") == arch {
            out.push(ArchEntry {
                name: name.to_string(),
                repo: repo.to_string(),
                file: file.to_string(),
            });
        }
    }
    out
}

fn pick_entry<'a>(entries: &'a [ArchEntry], pkg: &str) -> Option<(&'a str, &'a str)> {
    let e = entries
        .iter()
        .find(|e| e.name.as_str() == pkg)
        .or_else(|| if entries.len() == 1 { entries.first() } else { None })?;
    Some((e.repo.as_str(), e.file.as_str()))
}

fn result_objects(json: &str) -> Vec<&str> {
    let Some(at) = json.find("\"results\"") else { return Vec::new() };
    let Some(open) = json[at..].find('[').map(|i| at + i) else { return Vec::new() };
    let b = json.as_bytes();
    let mut out = Vec::new();
    let mut i = open + 1;
    while i < b.len() {
        match b[i] {
            b'"' => i = skip_str(b, i),
            b'{' => {
                let (o, next) = object_at(json, i);
                out.push(o);
                i = next;
            }
            b']' => break,
            _ => i += 1,
        }
    }
    out
}

fn object_at(json: &str, start: usize) -> (&str, usize) {
    let b = json.as_bytes();
    let mut depth = 0i32;
    let mut i = start;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i = skip_str(b, i);
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return (&json[start..=i], i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    (&json[start..], b.len())
}

fn skip_str(b: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    i
}

fn read_str(s: &str, start: usize) -> (&str, usize) {
    let b = s.as_bytes();
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return (&s[start + 1..i], i + 1),
            _ => i += 1,
        }
    }
    (&s[start + 1..], b.len())
}

fn field<'a>(obj: &'a str, key: &str) -> &'a str {
    let needle = format!("\"{key}\"");
    let Some(mut at) = obj.find(needle.as_str()).map(|i| i + needle.len()) else { return "" };
    let b = obj.as_bytes();
    while at < b.len() && (b[at] == b':' || b[at].is_ascii_whitespace()) {
        at += 1;
    }
    if at < b.len() && b[at] == b'"' {
        let (v, _) = read_str(obj, at);
        return v;
    }
    ""
}

///
///
/// rpm
///
///

fn fetch_rpm(g: &DepGroup, fam: &str, work: &Path, libdir: &Path) -> Res<(usize, Vec<String>)> {
    let dl = if fam == "opensuse" { "zypper" } else { "dnf" };
    let mut args: Vec<&str> = vec!["download"];
    args.extend(g.packages.iter().map(|s| s.as_str()));
    run(work, dl, &args).map_err(|e| format!("{e} (`{dl} download` needs the package repos configured)"))?;

    let rpms = list_files(work, |n| n.ends_with(".rpm"));
    if rpms.is_empty() {
        return Err(format!("`{dl} download` produced no .rpm files"));
    }
    let mut names = Vec::new();
    for (i, rpm) in rpms.iter().enumerate() {
        let x = work.join(format!("rpm{i}"));
        fs::create_dir_all(&x).map_err(|e| format!("cannot create {}: {e}", x.display()))?;
        let script = shell_pipe("rpm2cpio", rpm, "cpio -idm");
        let out = Command::new("/bin/sh")
            .args(["-c", &script])
            .current_dir(&x)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                return Err(format!("cannot unpack {}: {}", rpm.display(), err.trim().lines().last().unwrap_or("")));
            }
            Err(e) => return Err(format!("cannot run rpm2cpio/cpio: {e}")),
        }
        if let Some(n) = rpm.file_name() {
            names.push(n.to_string_lossy().into_owned());
        }
    }
    let mut copied = 0;
    for i in 0..rpms.len() {
        copied += copy_libs(&work.join(format!("rpm{i}")), libdir);
    }
    Ok((copied, names))
}

fn shell_pipe(cmd: &str, file: &Path, rest: &str) -> String {
    let f = file.to_string_lossy().replace('\'', "'\\''");
    format!("{cmd} '{f}' | {rest}")
}

// helpers

fn file_so(p: &Path) -> bool {
    p.file_name()
        .and_then(|f| f.to_str())
        .is_some_and(|f| f.starts_with("lib") && f.contains(".so"))
}

fn copy_libs(root: &Path, libdir: &Path) -> usize {
    let mut n = 0;
    for base in ["usr/lib", "usr/lib64", "lib", "lib64"] {
        n += copy_libs_from(&root.join(base), libdir, 0);
    }
    n
}

fn copy_libs_from(dir: &Path, libdir: &Path, depth: usize) -> usize {
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let p = e.path();
        let ft = match e.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_dir() {
            if depth < 6 {
                n += copy_libs_from(&p, libdir, depth + 1);
            }
            continue;
        }
        let Some(name) = p.file_name().and_then(|f| f.to_str()).map(String::from) else { continue };
        if !file_so(Path::new(&name)) {
            continue;
        }
        let dest = libdir.join(&name);
        if fs::symlink_metadata(&dest).is_ok() {
            continue;
        }
        if ft.is_symlink() {
            let Ok(target) = fs::read_link(&p) else { continue };
            let target = target.file_name().map(PathBuf::from).unwrap_or(target);
            if symlink(&target, &dest).is_ok() {
                n += 1;
            }
        } else if ft.is_file() && fs::copy(&p, &dest).is_ok() {
            let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o755));
            n += 1;
        }
    }
    n
}

fn list_files(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| p.file_name().and_then(|f| f.to_str()).is_some_and(&keep))
        .collect()
}

fn run(dir: &Path, cmd: &str, args: &[&str]) -> Res<()> {
    let out = Command::new(cmd)
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("cannot run `{cmd}`: {e} (is it installed?)"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail = err.trim().lines().last().unwrap_or("").to_string();
        return Err(format!("`{cmd} {}` failed: {tail}", args.join(" ")));
    }
    Ok(())
}

pub fn fetch_url(url: &str, dest: &Path) -> Res<()> {
    let d = dest.to_str().unwrap_or("");
    let curl = Command::new("curl").args(["-fsSL", "-o", d, url]).status();
    if matches!(curl, Ok(s) if s.success()) {
        return Ok(());
    }
    let wget = Command::new("wget").args(["-qO", d, url]).status();
    if matches!(wget, Ok(s) if s.success()) {
        return Ok(());
    }
    match curl {
        Ok(_) => Err(format!("cannot download {url} (curl and wget both failed)")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("cannot download {url}: neither curl nor wget is installed"))
        }
        Err(e) => Err(format!("cannot run curl: {e}")),
    }
}

fn extract_tar(file: &Path, dir: &Path) -> bool {
    let f = file.to_string_lossy().into_owned();
    let d = dir.to_string_lossy().into_owned();
    let zst = f.ends_with(".zst") || f.ends_with(".zstd");
    let mut tries: Vec<(&str, Vec<String>)> = Vec::new();
    if zst {
        tries.push(("tar", vec!["--zstd".into(), "-xf".into(), f.clone(), "-C".into(), d.clone()]));
    }
    tries.push(("tar", vec!["-xf".into(), f.clone(), "-C".into(), d.clone()]));
    if !zst {
        tries.push(("tar", vec!["--zstd".into(), "-xf".into(), f.clone(), "-C".into(), d.clone()]));
    }
    tries.push(("bsdtar", vec!["-xf".into(), f.clone(), "-C".into(), d.clone()]));
    for (cmd, args) in tries {
        if let Ok(s) = Command::new(cmd).args(&args).status() {
            if s.success() {
                return true;
            }
        }
    }
    false
}