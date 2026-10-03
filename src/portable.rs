use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{Config, ico};
use crate::desktop::build_entry;
use crate::fsutil::{copy_single, copy_tree, tree_size, walk};
use crate::install::remove_app;
use crate::naming::capitalize;
use crate::paths::Dirs;
use crate::state::read_manifest;
use crate::util::{Res, confirm, human_size};

pub fn bundle_dir(root: &Path, name: &str) -> PathBuf {
    root.join(name)
}

pub fn bundle_desktop(
    root: Option<&Path>,
    name: &str,
    display: Option<&str>,
    main: &Path,
    force: bool,
) -> Res<Option<PathBuf>> {
    let Some(root) = root else { return Ok(None) };
    let dir = bundle_dir(root, name);
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    let files = walk(&dir, 4);
    let Some(entry) = build_entry(name, display, &files, main, force) else {
        return Ok(None);
    };

    let icon = entry.icon.as_deref().and_then(|s| {
        s.file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .filter(|f| !f.is_empty())
    });
    if let (Some(src), Some(fname)) = (&entry.icon, &icon) {
        let _ = copy_single(src, &dir, fname);
    }

    let path = dir.join(format!("{name}.desktop"));
    fs::write(&path, &entry.text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(Some(path))
}

fn show_tree(cfg: &Config, root: &Path, name: &str, max: usize) {
    let dir = bundle_dir(root, name);
    println!();
    println!("  {} {}/", cfg.icon(ico::FOLDER), root.display());
    println!("  \u{2514}\u{2500}\u{2500} {name}/");
    let mut files = walk(&dir, 3);
    files.sort_by_key(|p| p.components().count());
    for (i, f) in files.iter().enumerate() {
        let rel = f.strip_prefix(&dir).unwrap_or(f);
        let last = i + 1 == files.len();
        let ext = rel.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let mark = match ext.to_ascii_lowercase().as_str() {
            "png" | "svg" | "xpm" | "ico" => cfg.icon(ico::MAGIC),
            "desktop" => cfg.icon(ico::LIST),
            _ => cfg.icon(ico::FILE),
        };
        println!(
            "     {}{} {}  {}",
            if last { "\u{2514}\u{2500}\u{2500} " } else { "\u{251c}\u{2500}\u{2500} " },
            mark,
            rel.display(),
            human_size(fs::metadata(f).map(|m| m.len()).unwrap_or(0))
        );
    }
    if files.len() > max {
        println!("     {} ... and {} more", cfg.icon(ico::BULLET), files.len() - max);
    }
}

pub fn cmd_export(d: &Dirs, cfg: &Config, name: &str, dest: &Path, force: bool) -> Res<()> {
    let m = read_manifest(d, name).ok_or_else(|| format!("'{name}' is not installed"))?;
    let src = m.dir.clone().ok_or_else(|| format!("no app directory recorded for '{name}'"))?;
    if !src.exists() {
        return Err(format!("'{}' is missing, run `moon doctor`", src.display()));
    }

    fs::create_dir_all(dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
    let bundle = bundle_dir(dest, name);
    if bundle.exists() {
        if !force {
            return Err(format!("{} already exists (use --force to overwrite)", bundle.display()));
        }
        fs::remove_dir_all(&bundle).map_err(|e| format!("cannot replace {}: {e}", bundle.display()))?;
    }

    let bytes = copy_tree(&src, &bundle)?;
    let main = match &m.main {
        Some(p) if p.starts_with(&src) => bundle.join(p.strip_prefix(&src).unwrap_or(p)),
        Some(p) => p.clone(),
        None => return Err(format!("no main executable recorded for '{name}'")),
    };

    let display = m.cmds.first().map(|s| capitalize(s));
    let written = bundle_desktop(Some(dest), name, display.as_deref(), &main, true)?;

    let version = m.version.clone().unwrap_or_default();
    println!();
    println!(
        "  {} Portable bundle for {}{}",
        cfg.icon(ico::PACKAGE),
        name,
        if version.is_empty() { String::new() } else { format!(" {version}") }
    );
    show_tree(cfg, dest, name, 12);
    println!();
    println!("    {} size: {}", cfg.icon(ico::DRIVE), human_size(bytes));

    if let Some(w) = &written {
        println!("    {} menu entry in the bundle: {}", cfg.icon(ico::LIST), w.display());
    }

    let mut cmds = m.cmds.clone();
    if cmds.is_empty() {
        cmds = m
            .links
            .iter()
            .filter_map(|l| l.file_name().map(|f| f.to_string_lossy().into_owned()))
            .collect();
    }
    println!();
    println!("  {} The bundle is self-contained: copy the folder anywhere.", cfg.icon(ico::BULLET));
    println!(
        "  {} The desktop file uses absolute paths, so re-export to the new location",
        cfg.icon(ico::BULLET)
    );
    println!("    if you move it:  moon export {name} <new parent dir>");
    println!(
        "  {} '{}' is still installed in {}. Uninstall it when the copy works: moon remove {name}",
        cfg.icon(ico::BULLET),
        name,
        src.display()
    );
    if !cmds.is_empty() {
        println!("  {} commands: {}", cfg.icon(ico::BULLET), cmds.join(", "));
    }
    println!();
    Ok(())
}

pub fn cmd_unexport(d: &Dirs, cfg: &Config, name: &str, root: Option<&Path>, yes: bool) -> Res<()> {
    let m = read_manifest(d, name).ok_or_else(|| format!("'{name}' is not installed"))?;
    let root = match root {
        Some(p) => p.to_path_buf(),
        None => m
            .portable
            .clone()
            .ok_or(format!("'{name}' was not installed in portable mode, say where the bundle is: moon unexport {name} <dir>"))?,
    };
    let dir = bundle_dir(&root, name);
    if !dir.exists() {
        return Err(format!("no portable bundle at {}", dir.display()));
    }
    println!();
    println!("  {} Remove portable bundle {}", cfg.icon(ico::TRASH), dir.display());
    println!("    {} size: {}", cfg.icon(ico::DRIVE), human_size(tree_size(&dir)));
    println!();
    if !confirm(&format!("  {} Delete {}?", cfg.icon(ico::TRASH), dir.display()), yes) {
        println!("  {} Cancelled.", cfg.icon(ico::CROSS));
        return Ok(());
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("cannot remove {}: {e}", dir.display()))?;
    println!("==> {} Removed {}", cfg.icon(ico::TRASH), dir.display());
    if let Some(m) = read_manifest(d, name) {
        if m.dir.as_deref() == Some(dir.as_path()) {
            println!("==> {} Forgetting the manifest for {name}", cfg.icon(ico::TRASH));
            let _ = remove_app(d, name, true);
        }
    }
    Ok(())
}
