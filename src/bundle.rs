//! `.moon` bundles: one `tar.gz` holding an app, its manifest, its icon and its
//! menu entry. Nothing else - `tar xf app.moon` still works, and the manifest
//! inside is the same key=value format moon writes itself.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use crate::config::{Config, ico};
use crate::desktop::update_desktop_db;
use crate::fsutil::{copy_tree, pick_main, walk};
use crate::history::push_history;
use crate::install::{ConflictChoice, InstallOpts, remove_app, resolve_conflict};
use crate::naming::sanitize;
use crate::paths::{Cleanup, Dirs, warn_path};
use crate::probe::try_capture;
use crate::state::{LastInstall, Manifest, manifest_path, moon_owned_link, parse_manifest, read_manifest, render_manifest, write_last, write_manifest};
use crate::util::{Res, human_size};

pub const BUNDLE_EXT: &str = ".moon";
const APP_DIR: &str = "app";

pub fn is_bundle_name(lname: &str) -> bool {
    lname.to_ascii_lowercase().ends_with(BUNDLE_EXT)
}

fn field(text: &str, key: &str) -> Option<String> {
    let mut in_entry = false;
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            in_entry = t == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some(v) = t.strip_prefix(&format!("{key}=")) {
            return Some(v.trim().to_string());
        }
    }
    None
}

fn retarget(text: &str, main: &Path, icon: Option<&Path>) -> String {
    let mut out = String::new();
    for l in text.lines() {
        if let Some(exec) = l.strip_prefix("Exec=") {
            let exec = exec.trim();
            let args = match exec.find(char::is_whitespace) {
                Some(i) => &exec[i..],
                None => "",
            };
            let mut line = format!("Exec={}", crate::desktop::exec_quote(main));
            if !args.is_empty() {
                line.push(' ');
                line.push_str(args);
            }
            out.push_str(&line);
        } else if l.starts_with("Icon=") {
            match icon {
                Some(p) => out.push_str(&format!("Icon={}", p.display())),
                None => out.push_str(l),
            }
        } else {
            out.push_str(l);
        }
        out.push('\n');
    }
    out
}

pub fn bundle_manifest(stage: &Path) -> Res<(String, Manifest)> {
    let mut hits: Vec<(String, Manifest)> = fs::read_dir(stage)
        .map_err(|e| format!("cannot read the bundle: {e}"))?
        .flatten()
        .filter_map(|e| {
            let fname = e.file_name().to_string_lossy().into_owned();
            let stem = fname.strip_suffix(".manifest")?.to_string();
            let text = fs::read_to_string(e.path()).ok()?;
            Some((stem, parse_manifest(&text)))
        })
        .collect();
    if hits.len() > 1 {
        hits.sort_by(|a, b| a.0.cmp(&b.0));
        return Err(format!(
            "the bundle holds {} manifests ({}), expected exactly one",
            hits.len(),
            hits.iter().map(|(n, _)| format!("{n}.manifest")).collect::<Vec<_>>().join(", ")
        ));
    }
    hits.pop()
        .ok_or_else(|| "not a moon bundle: no .manifest at the top".to_string())
}

pub fn icon_in(dir: &Path) -> Option<PathBuf> {
    let mut hits: Vec<PathBuf> = walk(dir, 2)
        .into_iter()
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map_or(false, |e| matches!(e.to_ascii_lowercase().as_str(), "png" | "svg" | "xpm"))
        })
        .collect();
    hits.sort();
    hits.into_iter().next()
}

fn pack(dir: &Path, out: &Path) -> Res<()> {
    let os = OsStr::new;
    let tar = try_capture("tar", &[os("-czf"), out.as_os_str(), os("-C"), dir.as_os_str(), os(".")]);
    if tar.0 {
        return Ok(());
    }
    let why_tar = if tar.1 { format!("`tar` failed: {}", tar.2.trim()) } else { String::new() };
    let bsd = try_capture("bsdtar", &[os("-czf"), out.as_os_str(), os("-C"), dir.as_os_str(), os(".")]);
    if bsd.0 {
        return Ok(());
    }
    let _ = fs::remove_file(out);
    Err(if why_tar.is_empty() && !bsd.1 {
        "no tar or bsdtar found, cannot build the bundle".into()
    } else {
        let b = if bsd.1 { format!("`bsdtar` failed: {}", bsd.2.trim()) } else { "`bsdtar` not found".into() };
        if why_tar.is_empty() { b } else { format!("{why_tar}\n  {b}") }
    })
}

pub fn cmd_bundle(d: &Dirs, cfg: &Config, name: &str, out: Option<&Path>, force: bool) -> Res<()> {
    d.ensure()?;
    let given = Path::new(name);
    if given.is_dir() {
        return pack_folder(cfg, given, out, force);
    }
    let m = read_manifest(d, name).ok_or_else(|| {
        if name.contains('/') || is_bundle_name(name) {
            format!("'{}' is a file, not a bundle folder - point moon at the folder that holds the manifest", name)
        } else {
            format!("'{name}' is not installed, nothing to bundle (`moon list` shows what is, or pass a folder)")
        }
    })?;
    let src = m
        .dir
        .clone()
        .filter(|p| p.is_dir())
        .ok_or_else(|| format!("the files of '{name}' are gone, run `moon doctor`"))?;
    let main = m
        .main
        .as_ref()
        .and_then(|p| p.strip_prefix(&src).ok())
        .map(|p| p.to_path_buf())
        .ok_or_else(|| format!("no main executable recorded for '{name}'"))?;

    let out = match out {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(format!("{name}{BUNDLE_EXT}")),
    };
    if out.exists() && !force {
        return Err(format!("{} already exists (use --force to overwrite)", out.display()));
    }
    if let Some(p) = out.parent() {
        fs::create_dir_all(p).map_err(|e| format!("cannot create {}: {e}", p.display()))?;
    }

    let work = d.tmp.join(format!("bundle-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| format!("cannot create {}: {e}", work.display()))?;
    let _guard = Cleanup(work.clone());

    let app = work.join(APP_DIR);
    let bytes = copy_tree(&src, &app)?;

    let mut links: Vec<(String, String)> = m
        .links
        .iter()
        .zip(&m.link_targets)
        .filter_map(|(l, t)| {
            let cmd = l.file_name()?.to_string_lossy().into_owned();
            let rel = t.strip_prefix(&src).ok()?;
            Some((cmd, rel.to_string_lossy().into_owned()))
        })
        .collect();
    if links.is_empty() {
        links.push((name.to_string(), main.to_string_lossy().into_owned()));
    }

    let mut desktop = None;
    let mut icon = None;
    if let Some(dk) = &m.desktop {
        if let Ok(text) = fs::read_to_string(dk) {
            let ddir = work.join("desktop");
            fs::create_dir_all(&ddir).map_err(|e| e.to_string())?;
            let rel = format!("desktop/{name}.desktop");
            fs::write(work.join(&rel), &text).map_err(|e| format!("cannot stage the menu entry: {e}"))?;
            if let Some(icon_value) = field(&text, "Icon") {
                let ip = PathBuf::from(&icon_value);
                if ip.is_file() {
                    let idir = work.join("icon");
                    fs::create_dir_all(&idir).map_err(|e| e.to_string())?;
                    let iname = ip
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_else(|| format!("{name}.png"));
                    fs::copy(&ip, idir.join(&iname)).map_err(|e| format!("cannot stage the icon: {e}"))?;
                    icon = Some(format!("icon/{iname}"));
                }
            }
            desktop = Some(rel);
        }
    }

    let bm = Manifest {
        dir: Some(PathBuf::from(APP_DIR)),
        main: Some(PathBuf::from(APP_DIR).join(&main)),
        version: m.version.clone(),
        desktop: desktop.clone().map(PathBuf::from),
        links: links.iter().map(|(c, _)| PathBuf::from(c)).collect(),
        link_targets: links.iter().map(|(_, t)| PathBuf::from(APP_DIR).join(t)).collect(),
        cmds: links.iter().map(|(c, _)| c.clone()).collect(),
        ..Default::default()
    };
    let manifest_text = render_manifest(&bm);
    fs::write(work.join(format!("{name}.manifest")), &manifest_text)
        .map_err(|e| format!("cannot write the bundle manifest: {e}"))?;

    let _ = fs::remove_file(&out);
    pack(&work, &out)?;

    let members: Vec<(String, String)> = vec![
        (
            ico::PACKAGE.to_string(),
            format!("application  {} file(s), {}", walk(&app, 12).len(), human_size(bytes)),
        ),
        (
            ico::LIST.to_string(),
            format!("metadata     {name}.manifest, {}", human_size(manifest_text.len() as u64)),
        ),
        (
            if desktop.is_some() { ico::MAGIC } else { ico::CROSS }.to_string(),
            desktop.clone().unwrap_or_else(|| "menu entry   none, this is a command line tool".into()),
        ),
        (
            if icon.is_some() { ico::MAGIC } else { ico::CROSS }.to_string(),
            icon.clone().unwrap_or_else(|| "icon         none".into()),
        ),
    ];
    let size = human_size(fs::metadata(&out).map(|m| m.len()).unwrap_or(0));
    report(cfg, name, m.version.as_deref(), &members, &out, size);
    Ok(())
}

fn report(cfg: &Config, name: &str, version: Option<&str>, members: &[(String, String)], out: &Path, size: String) {
    println!();
    println!(
        "  {} Bundle for {}{}",
        cfg.icon(ico::PACKAGE),
        name,
        version.filter(|v| !v.is_empty()).map(|v| format!(" {v}")).unwrap_or_default()
    );
    println!("    {} {}", cfg.icon(ico::DRIVE), size);
    for (mark, member) in members {
        println!("    {} {}", cfg.icon(mark), member);
    }
    println!();
    println!("  {} Written: {}", cfg.icon(ico::SAVE), out.display());
    println!("  {} Install it anywhere with:  moon install {}", cfg.icon(ico::ARROW), out.display());
    println!("  {} No server, no account, just the file.", cfg.icon(ico::BULLET));
    println!();
}

fn pack_folder(cfg: &Config, dir: &Path, out: Option<&Path>, force: bool) -> Res<()> {
    let (name, m) = bundle_manifest(dir).map_err(|e| {
        format!("{} is not a bundle folder: {e} (expected one *.manifest and an app/ directory)", dir.display())
    })?;
    let app_dir = match &m.dir {
        Some(p) if dir.join(p).is_dir() => dir.join(p),
        _ => dir.join(APP_DIR),
    };
    if !app_dir.is_dir() {
        return Err(format!(
            "{} has a manifest but no app/ directory, there is nothing to pack",
            dir.display()
        ));
    }
    let out = match out {
        Some(p) => p.to_path_buf(),
        None => dir.with_extension("moon"),
    };
    if out.exists() && !force {
        return Err(format!("{} already exists (use --force to overwrite)", out.display()));
    }
    if let Some(p) = out.parent() {
        fs::create_dir_all(p).map_err(|e| format!("cannot create {}: {e}", p.display()))?;
    }
    let _ = fs::remove_file(&out);
    pack(dir, &out)?;

    let files = walk(&app_dir, 12);
    let bytes: u64 = files.iter().map(|f| fs::metadata(f).map(|x| x.len()).unwrap_or(0)).sum();
    let members: Vec<(String, String)> = vec![
        (
            ico::PACKAGE.to_string(),
            format!("application  {} file(s), {}", files.len(), human_size(bytes)),
        ),
        (
            ico::LIST.to_string(),
            format!("metadata     {name}.manifest, {}", human_size(fs::metadata(dir.join(format!("{name}.manifest"))).map(|x| x.len()).unwrap_or(0))),
        ),
        (
            if m.desktop.is_some() { ico::MAGIC } else { ico::CROSS }.to_string(),
            m.desktop
                .as_ref()
                .map(|p| format!("menu entry   {}", p.display()))
                .unwrap_or_else(|| "menu entry   none, this is a command line tool".into()),
        ),
        (
            if icon_in(&dir.join("icon")).is_some() { ico::MAGIC } else { ico::CROSS }.to_string(),
            icon_in(&dir.join("icon"))
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
                .map(|f| format!("icon         icon/{f}"))
                .unwrap_or_else(|| "icon         none".into()),
        ),
    ];
    let size = human_size(fs::metadata(&out).map(|x| x.len()).unwrap_or(0));
    report(cfg, &name, m.version.as_deref(), &members, &out, size);
    Ok(())
}

pub fn install_bundle(d: &Dirs, o: &mut InstallOpts, src: &Path, work: &Path, fname: &str) -> Res<()> {
    let cfg = Config::load();
    let stage = work.join("bundle");
    fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    crate::archive::extract(src, "app.moon", &stage, "bundle")?;

    let (mname, m) = bundle_manifest(&stage)?;
    let app_src = stage.join(APP_DIR);
    if !app_src.is_dir() {
        return Err(format!("{fname} is not a moon bundle: no {APP_DIR}/ directory inside"));
    }

    let name = match &o.name {
        Some(n) => sanitize(n)?,
        None => sanitize(&mname)?,
    };
    let main_rel = match &o.bin {
        // the caller knows better than the manifest
        Some(rel) => {
            let p = PathBuf::from(rel);
            if !app_src.join(&p).is_file() {
                return Err(format!("--bin '{rel}' is not inside {APP_DIR}/ of the bundle"));
            }
            p
        }
        None => m
            .main
            .as_ref()
            .and_then(|p| p.strip_prefix(APP_DIR).ok())
            .map(|p| p.to_path_buf())
            .or_else(|| {
                let files = walk(&app_src, 8);
                pick_main(&app_src, &files, &name)
                    .map(|p| p.strip_prefix(&app_src).ok().map(Path::to_path_buf))?
            })
            .ok_or_else(|| "the bundle has no main executable recorded (use --bin <path-in-app/>)".to_string())?,
    };
    let version = m.version.clone().unwrap_or_default();
    println!();
    println!(
        "{} {} Installing '{name}'{} from {fname}",
        cfg.step(),
        cfg.icon(ico::PACKAGE),
        if version.is_empty() { String::new() } else { format!(" {version}") }
    );

    let mut links: Vec<(String, String)> = m
        .links
        .iter()
        .zip(&m.link_targets)
        .filter_map(|(l, t)| {
            let cmd = l.file_name()?.to_string_lossy().into_owned();
            let rel = t.strip_prefix(APP_DIR).ok()?.to_string_lossy().into_owned();
            Some((cmd, rel))
        })
        .collect();
    if links.is_empty() {
        links.push((name.clone(), main_rel.to_string_lossy().into_owned()));
    }

    let files: Vec<PathBuf> = walk(&app_src, 12);
    let total: u64 = files.iter().map(|f| fs::metadata(f).map(|m| m.len()).unwrap_or(0)).sum();
    println!(
        "    {:<12}{} file(s), {}",
        "Contents:",
        files.len(),
        human_size(total)
    );
    println!("    {:<12}{}", "Commands:", links.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>().join(", "));
    if let Some(dk) = &m.desktop {
        println!("    {:<12}{}", "Menu entry:", dk.display());
    }

    if o.dry_run {
        println!();
        println!("  {} Dry run: nothing was written.", cfg.icon(ico::EXAM));
        println!();
        return Ok(());
    }

    let final_dir = match &o.portable {
        Some(p) => crate::portable::bundle_dir(p, &name),
        None => d.apps.join(&name),
    };
    if final_dir.exists() || manifest_path(d, &name).exists() {
        let incoming = if version.is_empty() { None } else { Some(version.as_str()) };
        match resolve_conflict(d, &cfg, &name, src, incoming)? {
            ConflictChoice::Keep => return Ok(()),
            ConflictChoice::Force => o.force = true,
            ConflictChoice::Replace => {}
        }
        println!();
        println!("{} {} Replacing existing install", cfg.step(), cfg.icon(ico::REFRESH));
        remove_app(d, &name, true)?;
    }
    if let Some(p) = &o.portable {
        fs::create_dir_all(p).map_err(|e| format!("cannot create {}: {e}", p.display()))?;
    }
    if let Some(parent) = final_dir.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    fs::rename(&app_src, &final_dir).map_err(|e| format!("cannot move into place: {e}"))?;

    let main = final_dir.join(&main_rel);
    let _ = fs::set_permissions(&main, fs::Permissions::from_mode(0o755));

    let mut icon_dest = None;
    if let Some(icon) = m.desktop.as_ref().and_then(|_| icon_in(&stage.join("icon"))) {
        let dest = final_dir.join(icon.file_name().unwrap_or_default());
        if fs::copy(&icon, &dest).is_ok() {
            icon_dest = Some(dest);
        }
    }

    for (cmd, rel) in &links {
        let target = final_dir.join(rel);
        if !target.exists() {
            continue;
        }
        let link = d.bin.join(cmd);
        if fs::symlink_metadata(&link).is_ok()
            && !moon_owned_link(&link, d)
            && fs::read_link(&link).map_or(true, |x| x != target)
            && !o.force
        {
            return Err(format!(
                "{} already exists and is not managed by moon (use --force to overwrite)",
                link.display()
            ));
        }
    }

    let mut made = Vec::new();
    let mut targets = Vec::new();
    for (cmd, rel) in &links {
        let target = final_dir.join(rel);
        if !target.exists() {
            continue;
        }
        let link = d.bin.join(cmd);
        let _ = fs::remove_file(&link);
        symlink(&target, &link).map_err(|e| format!("cannot link {}: {e}", link.display()))?;
        println!(
            "{} {} Linked  {}  {}  {}",
            cfg.step(),
            cfg.icon(ico::LINK),
            link.display(),
            cfg.glyph(ico::ARROW, "->"),
            target.display()
        );
        made.push(link.clone());
        targets.push(target);
    }

    let mut desktop = None;
    if !o.no_desktop {
        if let Some(dk) = &m.desktop {
            if let Ok(text) = fs::read_to_string(stage.join(dk)) {
                let out = retarget(&text, &main, icon_dest.as_deref());
                let path = d.desktop.join(format!("{name}.desktop"));
                fs::write(&path, out).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                println!("{} {} Menu entry {}", cfg.step(), cfg.icon(ico::LIST), path.display());
                desktop = Some(path);
            }
        }
    }

    write_manifest(
        d,
        &name,
        &Manifest {
            dir: Some(final_dir.clone()),
            links: made.clone(),
            link_targets: targets.clone(),
            desktop: desktop.clone(),
            main: Some(main.clone()),
            version: m.version.clone(),
            cmds: links.iter().map(|(c, _)| c.clone()).collect(),
            ..Default::default()
        },
    )?;

    if desktop.is_some() {
        update_desktop_db(d);
    }
    warn_path(d);
    write_last(d, &LastInstall { name: name.clone(), source: o.source.clone(), when: 0 });
    push_history(d, &name, &o.source);

    if !m.desktop.is_some() {
        println!();
        println!(
            "  {} The bundle has no menu entry, this looks like a command line tool.",
            cfg.icon(ico::INFO)
        );
    }
    println!();
    let cmd = links
        .iter()
        .find(|(_, rel)| *rel == main_rel.to_string_lossy())
        .map(|(c, _)| c.clone())
        .or_else(|| links.first().map(|(c, _)| c.clone()))
        .unwrap_or_else(|| name.clone());
    println!("{} Done. Run it with:  {}", cfg.icon(ico::CHECK), cmd);
    println!();
    Ok(())
}