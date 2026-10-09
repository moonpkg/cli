pub mod ar;
pub mod control;
pub mod tree;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use crate::config::{Config, ico};
use crate::desktop::{update_desktop_db, write_desktop};
use crate::fsutil::{copy_any, is_exec_file};
use crate::history::push_history;
use crate::install::{ConflictChoice, InstallOpts, remove_app, resolve_conflict};
use crate::naming::{guess_name, sanitize};
use crate::paths::{Dirs, warn_path};
use crate::probe::missing_libs;
use crate::state::{LastInstall, Manifest, manifest_path, read_manifest, write_last, write_manifest};
use crate::util::Res;
use crate::archive::unpack_tar;
use crate::deb::ar::{DEB_SCRIPTS, ar_member_to, ar_members};
use crate::deb::control::parse_control;
use crate::deb::tree::{Node, deb_bin_name, deb_rel, deb_target, scan_tree};

fn same_content(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (fs::metadata(a), fs::metadata(b)) else { return false };
    if ma.len() != mb.len() {
        return false;
    }
    let (Ok(mut fa), Ok(mut fb)) = (fs::File::open(a), fs::File::open(b)) else { return false };
    let (mut ba, mut bb) = ([0u8; 64 * 1024], [0u8; 64 * 1024]);
    loop {
        let na = match fa.read(&mut ba) {
            Ok(0) => 0,
            Ok(n) => n,
            Err(_) => return false,
        };
        let nb = match fb.read(&mut bb) {
            Ok(0) => 0,
            Ok(n) => n,
            Err(_) => return false,
        };
        if na != nb || ba[..na] != bb[..nb] {
            return false;
        }
        if na == 0 {
            return true;
        }
    }
}

fn replace_path(text: &str, old: &str, new: &str) -> (String, usize) {
    let mut out = String::new();
    let mut rest = text;
    let mut hits = 0;
    while let Some(i) = rest.find(old) {
        let (a, b) = rest.split_at(i);
        let boundary = b[old.len()..]
            .chars()
            .next()
            .map_or(true, |c| !(c.is_alphanumeric() || "._-/".contains(c)));
        out.push_str(a);
        if boundary {
            out.push_str(new);
            hits += 1;
        } else {
            out.push_str(old);
        }
        rest = b;
        out.push_str(rest);
        return (out, hits);
    }
    out.push_str(rest);
    (out, hits)
}

fn rewrite_paths(text: &str, moved: &[(String, String)]) -> (String, usize) {
    let mut t = text.to_string();
    let mut hits = 0;
    for (old, new) in moved {
        let (s, n) = replace_path(&t, old, new);
        t = s;
        hits += n;
    }
    (t, hits)
}

#[derive(Clone, Copy, PartialEq)]
enum Act {
    Fresh,
    Owned,
    Shared,
    Conflict,
}

fn same_node(src: &Path, node: &Node, target: &Path) -> bool {
    match node {
        Node::File => same_content(src, target),
        Node::Link(t) => fs::read_link(target).map_or(false, |x| &x == t),
        _ => true,
    }
}

fn claimed(d: &Dirs) -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    let Ok(rd) = fs::read_dir(&d.manifests) else { return out };
    for e in rd.flatten() {
        let Some(m) = read_manifest(d, &e.file_name().to_string_lossy().replace(".manifest", ""))
        else {
            continue;
        };
        for p in m.files.iter().chain(&m.links).chain(&m.subdirs) {
            out.insert(p.clone());
        }
    }
    out
}

fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

fn run_deb_script(dir: &Path, name: &str, pkg: &str, ver: &str, arch: &str) -> Res<()> {
    let p = dir.join(name);
    if !p.is_file() {
        return Ok(());
    }
    let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o755));
    let head = fs::read(&p).map(|b| b.starts_with(b"#!")).unwrap_or(false);
    let mut cmd = if head {
        Command::new(&p)
    } else {
        let mut c = Command::new("sh");
        c.arg(&p);
        c
    };
    let st = cmd
        .env("PACKAGE", pkg)
        .env("VERSION", ver)
        .env("ARCH", arch)
        .env("DEBIAN_VERSION", "12")
        .env("DEBARCH", arch)
        .current_dir("/")
        .status()
        .map_err(|e| format!("cannot run maintainer script {name}: {e}"))?;
    if !st.success() {
        return Err(format!("maintainer script {name} failed ({st})"));
    }
    Ok(())
}

fn rollback(st: &DebState, app_dir: &Path) {
    for p in st.files.iter().chain(&st.links) {
        let _ = fs::remove_file(p);
    }
    for (bak, orig) in &st.backups {
        if bak.is_file() {
            let _ = fs::rename(bak, orig);
        }
    }
    let _ = fs::remove_dir_all(app_dir);
}

#[derive(Default)]
struct DebState {
    files: Vec<PathBuf>,
    links: Vec<PathBuf>,
    subdirs: Vec<PathBuf>,
    backups: Vec<(PathBuf, PathBuf)>,
    desktop: Vec<PathBuf>,
    cmds: Vec<String>,
    bin_paths: Vec<PathBuf>,
    shared: Vec<String>,
    conflicts: Vec<String>,
    skipped: Vec<String>,
    rewritten: usize,
    main: Option<PathBuf>,
}

struct PlanEntry {
    rel: String,
    src: PathBuf,
    node: Node,
    target: PathBuf,
    act: Act,
}

pub fn install_deb(d: &Dirs, o: &InstallOpts, src: &Path, work: &Path, fname: &str) -> Res<()> {
    let root_mode = o.root;
    if root_mode && !is_root() {
        return Err("--root installs into the real /usr, /opt, /etc ... - re-run it with sudo".into());
    }
    let mut dd = d.clone();
    if root_mode {
        dd.desktop = PathBuf::from("/usr/share/applications");
    }

    let mut ar = fs::File::open(src).map_err(|e| format!("cannot open {}: {e}", src.display()))?;
    let members = ar_members(&mut ar)?;
    let pick = |pfx: &str| {
        members
            .iter()
            .find(|m| m.name == pfx || m.name.starts_with(&format!("{pfx}.")))
            .cloned()
    };
    let Some(cm) = pick("control.tar") else {
        return Err("no control.tar.* member - not a Debian package".into());
    };
    let Some(dm) = pick("data.tar") else {
        return Err("no data.tar.* member - not a Debian package".into());
    };
    if let Some(db) = members.iter().find(|m| m.name == "debian-binary") {
        let mut f = fs::File::open(src).map_err(|e| e.to_string())?;
        f.seek(SeekFrom::Start(db.off)).map_err(|e| e.to_string())?;
        let mut v = String::new();
        let _ = f.take(16).read_to_string(&mut v);
        let v = v.trim().to_string();
        if !v.starts_with('2') {
            println!("{} {} note: unexpected debian-binary version '{v}'", Config::load().step(), Config::load().icon(ico::INFO));
        }
    }

    let ctar = work.join(&cm.name);
    let dtar = work.join(&dm.name);
    ar_member_to(&mut ar, &cm, &ctar)?;
    ar_member_to(&mut ar, &dm, &dtar)?;
    let cdir = work.join("control");
    let ddir = work.join("data");
    fs::create_dir_all(&cdir).map_err(|e| e.to_string())?;
    fs::create_dir_all(&ddir).map_err(|e| e.to_string())?;
    unpack_tar(&ctar, &cdir)?;
    unpack_tar(&dtar, &ddir)?;

    let ctext = ["control", "./control"]
        .iter()
        .find_map(|p| fs::read_to_string(cdir.join(p)).ok())
        .ok_or("package has no control file - not a Debian package")?;
    let meta = parse_control(&ctext);
    let deb_arch = meta.get("Architecture").unwrap_or("all").to_ascii_lowercase();
    let host = match env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "i386",
        "arm" => "armhf",
        "powerpc64" => "ppc64el",
        a => a,
    };
    if !matches!(deb_arch.as_str(), "all" | "any") && deb_arch != host && !o.force {
        return Err(format!(
            "package is for {deb_arch}, this machine is {host} (use --force to install anyway)"
        ));
    }

    let name = match &o.name {
        Some(n) => sanitize(n)?,
        None => match meta.get("Package") {
            Some(p) => sanitize(p)?,
            _ => sanitize(&guess_name(fname))?,
        },
    };
    let cfg = Config::load();
    let version = meta.get("Version").unwrap_or("").to_string();
    println!(
        "{} {} Installing '{name}'{} from {fname}",
        cfg.step(), cfg.icon(ico::PACKAGE),
        if version.is_empty() { String::new() } else { format!(" {version}") }
    );
    if let Some(s) = meta.get("Summary").or_else(|| meta.get("Description")) {
        if let Some(first) = s.lines().next() {
            println!("    {first}");
        }
    }

    let app_dir = d.apps.join(&name);
    let scripts: Vec<String> = DEB_SCRIPTS
        .iter()
        .filter(|n| cdir.join(n).is_file())
        .map(|s| (*s).to_string())
        .collect();
    if !scripts.is_empty() && !o.run_scripts {
        println!(
            "{} {} Package ships {} (skipped, use --run-scripts to run them)",
            cfg.step(), cfg.icon(ico::WARN),
            scripts.join(", ")
        );
    }
    if o.run_scripts {
        run_deb_script(&cdir, "preinst", &name, &version, &deb_arch)?;
    }

    let owned = claimed(d);
    let mut plan: Vec<PlanEntry> = Vec::new();
    let mut relocated: Vec<String> = Vec::new();
    for (p, node) in scan_tree(&ddir)? {
        let rel = deb_rel(&p, &ddir)?;
        if rel == "." {
            continue;
        }
        let under_usr = rel == "usr" || rel.starts_with("usr/");
        if !under_usr && !matches!(node, Node::Dir) {
            relocated.push(rel.clone());
        }
        let (target, in_home) = deb_target(&rel, root_mode, &d.local, &app_dir);
        if !in_home {
            continue;
        }
        plan.push(PlanEntry { rel, src: p, node, target, act: Act::Fresh });
    }

    let mut moved: Vec<(String, String)> = plan
        .iter()
        .map(|e| (format!("/{}", e.rel), e.target.to_string_lossy().into_owned()))
        .collect();
    moved.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    let moved_map: HashMap<String, String> = moved.iter().cloned().collect();

    for e in &mut plan {
        let Ok(mt) = fs::symlink_metadata(&e.target) else {
            e.act = Act::Fresh;
            continue;
        };
        e.act = if owned.contains(&e.target) {
            Act::Owned
        } else if matches!(e.node, Node::File) && mt.is_dir() {
            Act::Conflict
        } else if same_node(&e.src, &e.node, &e.target) {
            Act::Shared
        } else {
            Act::Conflict
        };
    }
    let mut hard_link_conflicts: Vec<PathBuf> = Vec::new();

    let mut force = o.force;
    if !o.dry_run && (app_dir.exists() || manifest_path(d, &name).exists()) {
        match resolve_conflict(d, &Config::load(), &name, src, Some(&version))? {
            ConflictChoice::Keep => return Ok(()),
            ConflictChoice::Force => force = true,
            ConflictChoice::Replace => {}
        }
    }
    let o = InstallOpts { force, ..(*o).clone() };
    let conflict_for = |e: &PlanEntry| matches!(e.act, Act::Conflict) && !o.force;
    let hard_conflict: Vec<&str> =
        plan.iter().filter(|e| conflict_for(e)).map(|e| e.rel.as_str()).collect();

    if o.dry_run {
        println!("{} {} Dry run: nothing written ({} entries)", cfg.step(), cfg.icon(ico::EXAM), plan.len());
        for e in plan.iter().filter(|e| matches!(e.node, Node::File)) {
            let (tag, mark) = match e.act {
                Act::Fresh => ("add", cfg.icon(ico::CHECK)),
                Act::Owned => ("update", cfg.icon(ico::REFRESH)),
                Act::Shared => ("keep (identical)", cfg.icon(ico::SHIELD)),
                Act::Conflict => ("CONFLICT (needs --force)", cfg.icon(ico::WARN)),
            };
            println!("  {mark} {tag:<20} {} -> {}", e.rel, e.target.display());
        }
        for e in plan.iter().filter(|e| matches!(e.node, Node::Link(_))) {
            println!("  {} link  {} -> {}", cfg.icon(ico::LINK), e.rel, e.target.display());
        }
        for ln in &hard_link_conflicts {
            println!("  {} link  {} (conflict)", cfg.icon(ico::WARN), ln.display());
        }
        if !relocated.is_empty() {
            println!(
                "{} {} {} paths outside /usr are relocated to {}/system (use --root for the real paths)",
                cfg.step(), cfg.icon(ico::WARN),
                relocated.len(),
                app_dir.display()
            );
        }
        return Ok(());
    }

    if !hard_conflict.is_empty() {
        return Err(format!(
            "{} file(s) already exist and are not managed by moon ({} ...). Use --force to overwrite, or --dry-run to look first.",
            hard_conflict.len(),
            hard_conflict.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        ));
    }

    if app_dir.exists() || manifest_path(d, &name).exists() {
        println!("{} {} Replacing existing install", cfg.step(), cfg.icon(ico::REFRESH));
        remove_app(d, &name, true)?;
    }
    fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;
    let mut st = DebState::default();

    let mut do_place = || -> Res<()> {
        for e in &plan {
            match (&e.node, e.act) {
                (Node::Other, _) => st.skipped.push(e.rel.clone()),
                (Node::Dir, Act::Conflict) => st.conflicts.push(e.rel.clone()),
                (Node::Dir, _) => {
                    if let Some(p) = e.target.parent() {
                        let _ = fs::create_dir_all(p);
                    }
                    let mode = fs::metadata(&e.src)
                        .map(|m| m.permissions().mode() & 0o7777)
                        .unwrap_or(0o755)
                        | 0o700;
                    fs::create_dir_all(&e.target)
                        .map_err(|err| format!("{}: {err}", e.target.display()))?;
                    fs::set_permissions(&e.target, fs::Permissions::from_mode(mode))
                        .map_err(|e| e.to_string())?;
                    st.subdirs.push(e.target.clone());
                }
                (_, Act::Shared) => st.shared.push(e.rel.clone()),
                (Node::File, _) => {
                    if let Some(p) = e.target.parent() {
                        fs::create_dir_all(p).map_err(|e| e.to_string())?;
                    }
                    if fs::symlink_metadata(&e.target).is_ok() {
                        if e.act == Act::Conflict {
                            let bak = app_dir.join("backup").join(&e.rel);
                            if let Some(p) = bak.parent() {
                                let _ = fs::create_dir_all(p);
                            }
                            copy_any(&e.target, &bak)?;
                            st.backups.push((bak, e.target.clone()));
                            println!("{} {} Backed up {}", cfg.step(), cfg.icon(ico::SAVE), e.target.display());
                        }
                        let _ = fs::remove_file(&e.target);
                    }
                    fs::copy(&e.src, &e.target)
                        .map_err(|err| format!("cannot install {}: {err}", e.target.display()))?;
                    let mode = fs::metadata(&e.src)
                        .map(|m| m.permissions().mode() & 0o7777)
                        .unwrap_or(0o644);

                    let mode = if is_exec_file(&e.src) { mode | 0o755 } else { mode & !0o111 };
                    fs::set_permissions(&e.target, fs::Permissions::from_mode(mode))
                        .map_err(|e| e.to_string())?;
                    st.files.push(e.target.clone());
                }
                (Node::Link(t), _) => {
                    if let Some(p) = e.target.parent() {
                        let _ = fs::create_dir_all(p);
                    }

                    let nt = moved_map
                        .get(&t.to_string_lossy().into_owned())
                        .map(PathBuf::from)
                        .unwrap_or_else(|| t.clone());
                    if let Ok(cur) = fs::read_link(&e.target) {
                        if &cur == &nt {
                            st.shared.push(e.rel.clone());
                            continue;
                        }
                        let _ = fs::remove_file(&e.target);
                    }
                    symlink(&nt, &e.target)
                        .map_err(|err| format!("cannot link {}: {err}", e.target.display()))?;
                    st.links.push(e.target.clone());
                }
            }
        }
        Ok(())
    };
    if let Err(e) = do_place() {
        rollback(&st, &app_dir);
        return Err(e);
    }

    let mut bin_targets: Vec<(String, PathBuf)> = Vec::new();
    for e in &plan {
        let Some(file) = deb_bin_name(&e.rel) else { continue };
        if !matches!(e.node, Node::File) || e.act == Act::Shared {
            continue;
        }
        if !is_exec_file(&e.target) {
            continue;
        }
        bin_targets.push((file.to_string(), e.target.clone()));
    }
    st.bin_paths = bin_targets.iter().map(|(_, p)| p.clone()).collect();
    if !root_mode {
        for (file, target) in &bin_targets {
            let l = d.bin.join(file);
            if l == *target {
                st.cmds.push(file.clone());
                continue;
            }
            match fs::read_link(&l) {
                Ok(cur) if cur == *target => {
                    st.cmds.push(file.clone());
                    continue;
                }
                Ok(_) => {
                    let _ = fs::remove_file(&l);
                }
                Err(_) => {
                    if fs::symlink_metadata(&l).is_ok() {
                        if !o.force {
                            hard_link_conflicts.push(l.clone());
                            continue;
                        }
                        let _ = fs::remove_file(&l);
                    }
                }
            }
            symlink(target, &l).map_err(|e| format!("cannot link {}: {e}", l.display()))?;
            println!("{} {} Linked  {}  ->  {}", cfg.step(), cfg.icon(ico::LINK), l.display(), target.display());
            st.links.push(l);
            st.cmds.push(file.clone());
        }
    } else {
        st.cmds = bin_targets.iter().map(|(f, _)| f.clone()).collect();
    }
    if !hard_link_conflicts.is_empty() {
        let names: Vec<String> =
            hard_link_conflicts.iter().map(|p| p.display().to_string()).collect();
        rollback(&st, &app_dir);
        return Err(format!(
            "{} entry/entries in {} exist and are not managed by moon ({}). Use --force.",
            hard_link_conflicts.len(),
            d.bin.display(),
            names.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        ));
    }

    let main = match &o.bin {
        Some(b) => {
            let b = b.trim_start_matches('/');
            let found = moved_map
                .get(&format!("/{b}"))
                .cloned()
                .or_else(|| plan.iter().find(|e| e.rel == b).map(|e| e.target.to_string_lossy().into_owned()));
            match found {
                Some(p) if Path::new(&p).is_file() => Some(PathBuf::from(p)),
                _ => {
                    rollback(&st, &app_dir);
                    return Err(format!("--bin '{b}' is not in the package"));
                }
            }
        }
        None => st
            .cmds
            .first()
            .map(|c| d.bin.join(c))
            .filter(|p| p.is_file())
            .or_else(|| bin_targets.first().map(|(_, p)| p.clone())),
    };
    st.main = main.clone();

    let placed: Vec<PathBuf> = plan
        .iter()
        .filter(|e| matches!(e.node, Node::File) && e.act != Act::Shared)
        .map(|e| e.target.clone())
        .collect();
    if !o.no_rewrite {
        for e in &plan {
            if !matches!(e.node, Node::File) || e.act == Act::Shared {
                continue;
            }
            let is_desktop = e.target
                .file_name()
                .and_then(|s| s.to_str())
                .map_or(false, |s| s.ends_with(".desktop"))
                && e.target.parent().map_or(false, |p| p.ends_with("applications"));
            let is_wrapper = deb_bin_name(&e.rel).is_some();
            if !is_desktop && !is_wrapper {
                continue;
            }
            let Ok(text) = fs::read_to_string(&e.target) else { continue };
            let (new, hits) = rewrite_paths(&text, &moved);
            if hits > 0 {
                let _ = fs::write(&e.target, new);
                st.rewritten += hits;
            }
        }
    }
    for e in &plan {
        if e.target.extension().map_or(false, |x| x == "desktop")
            && e.target.parent().map_or(false, |p| p.ends_with("applications"))
        {
            st.desktop.push(e.target.clone());
        }
    }
    st.desktop.sort();
    if st.desktop.is_empty() && o.force_desktop && !o.no_desktop {
        if let Some(m) = &st.main {
            if let Some(p) = write_desktop(&dd, &name, o.name.as_deref(), &placed, m, true)? {
                st.desktop.push(p);
            }
        }
    }

    if o.run_scripts {
        run_deb_script(&cdir, "postinst", &name, &version, &deb_arch)?;
    }
    st.files.sort();
    st.links.sort();
    st.subdirs.sort();
    let res = (|| -> Res<()> {
        write_manifest(
            d,
            &name,
            &Manifest {
                dir: Some(app_dir.clone()),
                main: st.main.clone(),
                version: if version.is_empty() { None } else { Some(version.clone()) },
                cmds: st.cmds.clone(),
                files: st.files.clone(),
                links: st.links.clone(),
                link_targets: Vec::new(),
                subdirs: st.subdirs.clone(),
                desktop: st.desktop.first().cloned(),
                deb: true,
                root: root_mode,
                portable: o.portable.clone(),
                deps: Vec::new(),
            },
        )?;
        if !st.desktop.is_empty() {
            update_desktop_db(&dd);
        }
        Ok(())
    })();
    if let Err(e) = res {
        rollback(&st, &app_dir);
        return Err(e);
    }
    write_last(d, &LastInstall { name: name.clone(), source: o.source.clone(), when: 0 });
    push_history(d, &name, &o.source);

    for s in &st.shared {
        println!(
            "{} {} Kept   /{s} (identical file already present, moon will not remove it)",
            cfg.step(), cfg.icon(ico::SHIELD)
        );
    }
    if !relocated.is_empty() {
        let mut uniq: Vec<String> = relocated.iter().map(|r| r.split('/').next().unwrap_or(r).to_string()).collect();
        uniq.sort();
        uniq.dedup();
        println!(
            "{} {} {} path(s) outside /usr ({} ...) live in {}/system, not at their real path. Re-run with --root (sudo) for a system-wide install.",
            cfg.step(), cfg.icon(ico::WARN),
            relocated.len(),
            uniq.iter().take(3).cloned().collect::<Vec<_>>().join(", "),
            app_dir.display()
        );
    }
    if st.rewritten > 0 {
        println!(
            "{} {} Rewrote {} absolute path(s) to the new locations",
            cfg.step(), cfg.icon(ico::MAGIC),
            st.rewritten
        );
    }
    if !st.skipped.is_empty() {
        println!(
            "{} {} Skipped {} special file(s) ({})",
            cfg.step(), cfg.icon(ico::WARN),
            st.skipped.len(),
            st.skipped.join(", ")
        );
    }
    let miss = missing_libs(&bin_targets.iter().map(|(_, p)| p.clone()).collect::<Vec<_>>());
    if !miss.is_empty() {
        eprintln!(
            "warning: {} {} shared librar{} missing: {}\n    install the matching Arch packages, e.g.  sudo pacman -S <lib>",
            cfg.icon(ico::WARN),
            miss.len(),
            if miss.len() == 1 { "y is" } else { "ies are" },
            miss.join(", ")
        );
    }
    if let Some(sz) = meta.get("Installed-Size").and_then(|s| s.split_whitespace().next()) {
        println!("{} {} Package size on disk: {sz} KiB", cfg.step(), cfg.icon(ico::DRIVE));
    }
    if !st.desktop.is_empty() {
        println!("{} {} Menu entry {}", cfg.step(), cfg.icon(ico::LIST), st.desktop[0].display());
    } else {
        println!("{} {} No .desktop file in the package, no menu entry", cfg.step(), cfg.icon(ico::INFO));
    }

    let mut roots: BTreeMap<String, usize> = BTreeMap::new();
    for f in &st.files {
        let key = f
            .strip_prefix(if root_mode { Path::new("/") } else { &d.local })
            .ok()
            .and_then(|r| r.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()))
            .unwrap_or_else(|| f.display().to_string());
        *roots.entry(key).or_default() += 1;
    }
    if !st.files.is_empty() {
        let base = if root_mode { "/".to_string() } else { d.local.display().to_string() };
        let mut parts: Vec<String> = roots.iter().map(|(k, n)| format!("{k}/ ({n})")).collect();
        parts.truncate(6);
        if roots.len() > 6 {
            parts.push(format!("+{} more", roots.len() - 6));
        }
        println!(
            "{} {} {} file(s) in {}: {}",
            cfg.step(), cfg.icon(ico::FOLDER),
            st.files.len(),
            base,
            parts.join(", ")
        );
    }
    for c in &st.cmds {
        println!("{} {} Command  {c}", cfg.step(), cfg.icon(ico::LINK));
    }
    if st.links.is_empty() && st.files.is_empty() {
        println!("\n{} Nothing to install: the package contains no regular files.", cfg.icon(ico::INFO));
        return Ok(());
    }
    if !root_mode && !st.cmds.is_empty() {
        warn_path(d);
    }
    match st.cmds.first() {
        Some(c) => println!("\n{} Done. Run it with:  {c}", cfg.icon(ico::CHECK)),
        None => {
            let stray: Vec<&PathBuf> = st
                .files
                .iter()
                .filter(|f| is_exec_file(f) && !st.bin_paths.contains(f))
                .collect();
            if stray.is_empty() {
                println!("\n{} Done. The package contains no executable.", cfg.icon(ico::CHECK));
            } else {
                println!("\n{} Done. No command added to PATH. The package's executable(s) are at:", cfg.icon(ico::WARN));
                for f in stray.iter().take(5) {
                    println!("    {}", f.display());
                }
                if stray.len() > 5 {
                    println!("    {} ... and {} more", cfg.icon(ico::BULLET), stray.len() - 5);
                }
            }
        }
    }
    Ok(())
}
