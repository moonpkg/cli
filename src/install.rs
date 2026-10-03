use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use crate::config::{Config, ico};
use crate::desktop::{update_desktop_db, write_desktop};
use crate::fsutil::{is_exec_file, pick_main, walk};
use crate::history::push_history;
use crate::inspect::cmd_inspect;
use crate::naming::{guess_name, guess_version, sanitize, version_cmp};
use crate::paths::{Cleanup, Dirs, warn_path};
use crate::portable::bundle_desktop;

use crate::state::{LastInstall, Manifest, link_is_ours, manifest_path, moon_owned_link, read_last, read_manifest, state_path, write_last, write_manifest};
use crate::util::{Res, confirm};
use crate::archive::{download, extract};
use crate::deb::ar::is_deb;
use crate::deb::install_deb;

#[derive(PartialEq, Clone, Copy)]
pub enum ConflictChoice {
    Replace,

    Force,

    Keep,
}

pub fn resolve_conflict(
    d: &Dirs,
    cfg: &Config,
    name: &str,
    src: &Path,
    incoming: Option<&str>,
) -> Res<ConflictChoice> {
    let installed = read_manifest(d, name).and_then(|m| m.version).filter(|v| !v.is_empty());
    let incoming = incoming.map(str::to_string).filter(|v| !v.is_empty());

    let interactive = {
        use std::io::IsTerminal;
        std::io::stdin().is_terminal()
    };
    if !interactive {
        let rel = match (&installed, &incoming) {
            (Some(a), Some(b)) => match version_cmp(b, a) {
                std::cmp::Ordering::Greater => "upgrade",
                std::cmp::Ordering::Equal => "reinstall of the same version",
                std::cmp::Ordering::Less => "DOWNGRADE",
            },
            _ => "reinstall",
        };
        println!("==> {} Replacing existing install ({rel})", cfg.icon(ico::REFRESH));
        return Ok(ConflictChoice::Replace);
    }

    let newer = match (&installed, &incoming) {
        (Some(a), Some(b)) => version_cmp(b, a) == std::cmp::Ordering::Greater,
        _ => false,
    };
    let same = installed.is_some() && installed == incoming;

    println!();
    println!("  {}  {name} is already installed.", cfg.icon(ico::WARN));
    println!();
    println!("  {:<11}{}", "Installed:", installed.as_deref().unwrap_or("unknown"));
    println!("  {:<11}{}", "Downloaded:", incoming.as_deref().unwrap_or("unknown"));
    println!();
    if same {
        println!("  {} Same version: reinstalling will replace the files that are there now.", cfg.icon(ico::INFO));
    } else if installed.is_some() && incoming.is_some() && !newer {
        println!("  {} This is an OLDER version than the one installed.", cfg.icon(ico::WARN));
    }
    println!();

    let (first, upgrade_label, plain_label) = if newer {
        (1, "Upgrade      ", "Reinstall    ")
    } else if same {
        (1, "Reinstall    ", "Reinstall    ")
    } else {
        (3, "Replace      ", "Replace      ")
    };
    println!("  1  {upgrade_label} {} remove the old one, then install this", cfg.icon(ico::UP));
    println!("  2  {plain_label} {} install anyway, overwriting anything in the way", cfg.icon(ico::DOWNLOAD));
    println!("  3  Keep existing  {} change nothing", cfg.icon(ico::SHIELD));
    println!("  4  Inspect        {} look inside this file first", cfg.icon(ico::SEARCH));
    println!();

    loop {
        print!("  {} ", cfg.icon(ico::ARROW));
        let _ = std::io::stdout().flush();
        let mut s = String::new();
        if std::io::stdin().read_line(&mut s).is_err() {
            println!("  Keeping the existing install.");
            return Ok(ConflictChoice::Keep);
        }
        let pick = s.trim();
        match if pick.is_empty() { first.to_string() } else { pick.to_string() }.as_str() {
            "1" => return Ok(ConflictChoice::Replace),
            "2" => return Ok(ConflictChoice::Force),
            "3" => {
                println!("  Kept the existing {name}. Nothing was changed.");
                return Ok(ConflictChoice::Keep);
            }
            "4" => {
                println!();
                let _ = cmd_inspect(d, cfg, &src.display().to_string());
                println!();
                println!("  {name} is still installed. 1 replaces it, 3 keeps it, 4 inspects again.");
                continue;
            }
            _ => println!("  Pick 1, 2, 3 or 4."),
        }
    }
}

pub fn cmd_undo(d: &Dirs, cfg: &Config, name: Option<&str>, yes: bool) -> Res<()> {
    let (name, source) = match name {
        Some(n) => (n.to_string(), String::new()),
        None => {
            let Some(l) = read_last(d) else {
                return Err("nothing to undo: no app has been installed with moon yet".into());
            };
            (l.name, l.source)
        }
    };
    if !manifest_path(d, &name).exists() && !d.apps.join(&name).exists() {
        return Err(format!("'{name}' is not installed"));
    }
    let m = read_manifest(d, &name).unwrap_or_default();
    println!();
    println!(
        "  {} Undo install of {} {}",
        cfg.icon(ico::UNDO),
        name,
        m.version.clone().unwrap_or_default()
    );
    if !source.is_empty() {
        println!("    {} from {source}", cfg.icon(ico::FILE));
    }
    println!(
        "    {} {} file(s), {} command(s){}",
        cfg.icon(ico::LIST),
        m.files.len() + m.links.len(),
        m.cmds.len(),
        if m.desktop.is_some() { ", menu entry" } else { "" }
    );
    if m.portable.is_some() {
        println!("    {} portable, the app lives outside ~/.local", cfg.icon(ico::DRIVE));
    }
    println!();
    if !confirm(&format!("  {} Remove {name}?", cfg.icon(ico::TRASH)), yes) {
        println!("  {} Cancelled.", cfg.icon(ico::CROSS));
        return Ok(());
    }
    remove_app(d, &name, false)?;
    let _ = fs::remove_file(state_path(d));
    update_desktop_db(d);
    println!();
    Ok(())
}

#[derive(Clone)]
pub struct InstallOpts {
    pub source: String,
    pub name: Option<String>,
    pub bin: Option<String>,
    pub no_desktop: bool,
    pub force_desktop: bool,
    pub force: bool,

    pub root: bool,
    pub run_scripts: bool,
    pub no_rewrite: bool,
    pub dry_run: bool,
    pub portable: Option<PathBuf>,
}

pub fn remove_app(d: &Dirs, name: &str, quiet: bool) -> Res<()> {
    let m = read_manifest(d, name);
    let dir = m.as_ref().and_then(|m| m.dir.clone()).unwrap_or_else(|| d.apps.join(name));
    if m.is_none() && !dir.exists() {
        return Err(format!("'{name}' is not installed"));
    }
    if let Some(m) = &m {
        for l in &m.links {
            if link_is_ours(l, m, d) {
                let _ = fs::remove_file(l);
            }
        }
        if let Some(p) = &m.desktop {
            let _ = fs::remove_file(p);
        }

        if !m.files.is_empty() {
            let others: HashSet<PathBuf> = fs::read_dir(&d.manifests)
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.file_name() != format!("{name}.manifest").as_str())
                        .filter_map(|e| {
                            let n = e.file_name().to_string_lossy().replace(".manifest", "");
                            read_manifest(d, &n)
                        })
                        .flat_map(|o| o.files.into_iter().chain(o.links))
                        .collect()
                })
                .unwrap_or_default();
            let mut left = 0;
            for f in m.files.iter().chain(&m.links) {
                if others.contains(f) {
                    continue;
                }

                if fs::symlink_metadata(f).map(|s| s.file_type().is_symlink()).unwrap_or(false) {
                    let _ = fs::remove_file(f);
                } else if fs::remove_file(f).is_err() {
                    left += 1;
                }
            }
            if left > 0 && !quiet {
                eprintln!("warning: {left} file(s) of '{name}' could not be removed (permissions?)");
            }
        }
        let mut sd: Vec<&PathBuf> = m.subdirs.iter().collect();
        sd.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
        for p in sd {
            let _ = fs::remove_dir(p);
        }
    }
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_file(manifest_path(d, name));
    if !quiet {
        println!("==> {} Removed {name}", Config::load().icon(ico::TRASH));
    }
    Ok(())
}

pub fn install(d: &Dirs, o: &mut InstallOpts) -> Res<()> {
    d.ensure()?;
    let work = d.tmp.join(format!("job-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let _guard = Cleanup(work.clone());

    let is_url = o.source.starts_with("http://") || o.source.starts_with("https://");
    let src = if is_url {
        download(&o.source, &work)?
    } else {
        PathBuf::from(&o.source)
    };
    if !src.is_file() {
        return Err(format!("'{}' is not a file", src.display()));
    }
    let fname = src.file_name().and_then(|s| s.to_str()).unwrap_or("app").to_string();

    if is_deb(&src, &fname) {
        return install_deb(d, o, &src, &work, &fname);
    }

    let name = match &o.name {
        Some(n) => sanitize(n)?,
        None => sanitize(&guess_name(&fname))?,
    };
    let cfg_now = Config::load();
    println!("==> {} Installing '{name}' from {}", cfg_now.icon(ico::PACKAGE), fname);

    let stage = work.join("stage");
    fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    extract(&src, &fname.to_ascii_lowercase(), &stage, &name)?;

    let entries: Vec<_> = fs::read_dir(&stage).map_err(|e| e.to_string())?.flatten().collect();
    let root = match entries.as_slice() {
        [e] if e.file_type().map_or(false, |t| t.is_dir()) => e.path(),
        _ => stage.clone(),
    };
    if entries.is_empty() {
        return Err("archive is empty".into());
    }

    let final_dir = match &o.portable {
        Some(p) => crate::portable::bundle_dir(p, &name),
        None => d.apps.join(&name),
    };
    if !o.dry_run && (final_dir.exists() || manifest_path(d, &name).exists()) {
        let incoming = guess_version(&fname);
        match resolve_conflict(d, &Config::load(), &name, &src, incoming.as_deref())? {
            ConflictChoice::Keep => return Ok(()),
            ConflictChoice::Force => o.force = true,
            ConflictChoice::Replace => {}
        }
    }
    if final_dir.exists() || manifest_path(d, &name).exists() {
        println!("==> {} Replacing existing install", cfg_now.icon(ico::REFRESH));
        remove_app(d, &name, true)?;
    }
    if let Some(p) = &o.portable {
        fs::create_dir_all(p).map_err(|e| format!("cannot create {}: {e}", p.display()))?;
        println!(
            "==> {} Portable install, everything lives in {}",
            cfg_now.icon(ico::DRIVE),
            final_dir.display()
        );
    }
    fs::rename(&root, &final_dir).map_err(|e| format!("cannot move into place: {e}"))?;

    match finish_install(d, o, &name, &final_dir, &fname) {
        Ok(()) => {
            write_last(d, &LastInstall { name: name.clone(), source: o.source.clone(), when: 0 });
            push_history(d, &name, &o.source);
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&final_dir);
            Err(e)
        }
    }
}

fn finish_install(d: &Dirs, o: &InstallOpts, name: &str, dir: &Path, src_file_name: &str) -> Res<()> {
    let cfg = Config::load();
    let files = walk(dir, 8);

    let main = match &o.bin {
        Some(rel) => {
            let p = dir.join(rel);
            if !p.is_file() {
                return Err(format!("--bin '{rel}' not found inside the archive"));
            }
            let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o755));
            p
        }
        None => pick_main(dir, &files, name).ok_or(
            "no executable found in the archive (use --bin <relative/path> to point at it)",
        )?,
    };

    let mut links: Vec<(String, PathBuf)> = Vec::new();
    if let Some(bd) = ["bin", "usr/bin"].iter().map(|p| dir.join(p)).find(|p| p.is_dir()) {
        let mut ents: Vec<PathBuf> = fs::read_dir(&bd).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
        ents.sort();
        for p in ents {
            let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
            if !fname.starts_with('.') && is_exec_file(&p) {
                links.push((fname, p));
            }
        }
    }
    if !links.iter().any(|(_, p)| p == &main) {
        links.push((name.to_string(), main.clone()));
    }

    for (ln, target) in &links {
        let t = d.bin.join(ln);
        if fs::symlink_metadata(&t).is_ok()
            && !moon_owned_link(&t, d)
            && fs::read_link(&t).map_or(true, |x| x != *target)
            && !o.force
        {
            return Err(format!(
                "{} already exists and is not managed by moon (use --force to overwrite)",
                t.display()
            ));
        }
    }

    let mut made = Vec::new();
    let mut targets = Vec::new();
    for (ln, target) in &links {
        let l = d.bin.join(ln);
        let _ = fs::remove_file(&l);
        symlink(target, &l).map_err(|e| format!("cannot link {}: {e}", l.display()))?;
        println!("==> {} Linked  {}  ->  {}", Config::load().icon(ico::LINK), l.display(), target.display());
        made.push(l);
        targets.push(target.clone());
    }

    let portable = o.portable.as_deref();
    let mut desktop = None;
    if !o.no_desktop {
        let is_appimage = main.extension().map_or(false, |e| e.eq_ignore_ascii_case("appimage"));
        desktop = write_desktop(d, name, o.name.as_deref(), &files, &main, o.force_desktop || is_appimage)?;
        match &desktop {
            Some(p) => println!("==> {} Menu entry {}", cfg.icon(ico::LIST), p.display()),
            None => println!(
                "==> {} No icon or .desktop file in the archive, looks like a CLI tool: skipped menu entry (use --desktop to force)",
                cfg.icon(ico::INFO)
            ),
        }
        if portable.is_some() {
            let bundle = bundle_desktop(portable, name, o.name.as_deref(), &main, o.force_desktop || is_appimage)?;
            if let Some(p) = &bundle {
                println!("==> {} Portable bundle {}", cfg.icon(ico::DRIVE), p.display());
            }
        }
    }

    write_manifest(
        d,
        name,
        &Manifest {
            dir: Some(dir.to_path_buf()),
            links: made,
            link_targets: targets,
            desktop: desktop.clone(),
            main: Some(main.clone()),
            version: guess_version(src_file_name),
            portable: portable.map(|p| p.to_path_buf()),
            ..Default::default()
        },
    )?;

    if desktop.is_some() {
        update_desktop_db(d);
    }

    warn_path(d);

    let cmd = &links[links.len() - 1].0;
    let cmd = links.iter().find(|(_, p)| p == &main).map(|(n, _)| n).unwrap_or(cmd);
    println!("\n{} Done. Run it with:  {}", cfg.icon(ico::CHECK), cmd);
    Ok(())
}

pub fn list(d: &Dirs) -> Res<()> {
    let mut names: Vec<String> = fs::read_dir(&d.manifests)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().to_str().and_then(|s| s.strip_suffix(".manifest").map(String::from)))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    if names.is_empty() {
        let cfg = Config::load();
        println!("{} No apps installed with moon.", cfg.icon(ico::MOON));
        println!("  {} moon install <archive|AppImage|deb|url>", cfg.icon(ico::ARROW));
    }
    for n in names {
        let m = read_manifest(&d, &n).unwrap_or_default();
        let mut cmds = m.cmds.clone();
        if cmds.is_empty() {
            cmds = m
                .links
                .iter()
                .filter_map(|l| l.file_name().map(|f| f.to_string_lossy().into_owned()))
                .collect();
        }
        let ver = m.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
        let kind = match (m.deb, m.root) {
            (true, true) => " [deb, system]",
            (true, false) => " [deb]",
            _ => "",
        };
        let menu = if m.desktop.is_some() { " [menu]" } else { "" };
        let portable = if m.portable.is_some() { " [portable]" } else { "" };
        println!(
            "{n}{ver}{menu}{kind}{portable}\n    cmds: {}\n    dir:  {}",
            cmds.join(", "),
            m.dir.map(|p| p.display().to_string()).unwrap_or_default()
        );
    }
    Ok(())
}
