use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{Config, ico};
use crate::paths::Dirs;
use crate::state::{manifest_path, read_manifest, write_manifest};
use crate::util::{Res, confirm, shorten_path};

#[derive(Clone, PartialEq)]
pub enum Problem {
    Missing { what: String, path: PathBuf },

    BrokenLink { link: PathBuf, target: PathBuf },

    OrphanLink { link: PathBuf, target: PathBuf },
}

pub struct Report {
    pub name: String,
    pub version: Option<String>,
    pub portable: Option<PathBuf>,
    pub problems: Vec<Problem>,
}

fn all_names(d: &Dirs) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(&d.manifests)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().to_str().and_then(|s| s.strip_suffix(".manifest").map(String::from)))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

pub fn scan(d: &Dirs) -> Res<Vec<Report>> {
    d.ensure()?;
    let names = all_names(d);
    let mut owned: BTreeMap<PathBuf, String> = BTreeMap::new();
    for n in &names {
        if let Some(m) = read_manifest(d, n) {
            for l in &m.links {
                owned.insert(l.clone(), n.clone());
            }
        }
    }

    let mut out = Vec::new();
    for n in &names {
        let m = read_manifest(d, n).unwrap_or_default();
        let mut problems = Vec::new();

        if let Some(dir) = &m.dir {
            if !dir.exists() {
                problems.push(Problem::Missing { what: "Application missing".into(), path: dir.clone() });
            }
        }
        if let Some(main) = &m.main {
            if m.dir.as_ref().is_some_and(|d| d.exists()) && !main.exists() {
                problems.push(Problem::Missing { what: "Executable missing".into(), path: main.clone() });
            }
        }

        for (i, l) in m.links.iter().enumerate() {
            match fs::read_link(l) {
                Ok(t) if !t.exists() => {
                    problems.push(Problem::BrokenLink { link: l.clone(), target: t });
                }
                Ok(_) => {}
                Err(_) => {
                    let _ = i;
                    problems.push(Problem::Missing { what: "Command link missing".into(), path: l.clone() });
                }
            }
        }

        if let Some(p) = &m.desktop {
            if !p.exists() {
                problems.push(Problem::Missing { what: "Menu entry missing".into(), path: p.clone() });
            }
        }

        out.push(Report {
            name: n.clone(),
            version: m.version.clone(),
            portable: m.portable.clone(),
            problems,
        });
    }

    let claimed: Vec<PathBuf> = out
        .iter()
        .filter_map(|r| read_manifest(d, &r.name))
        .filter_map(|m| m.dir)
        .collect();
    if let Ok(rd) = fs::read_dir(&d.apps) {
        let mut extra: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !claimed.contains(p))
            .collect();
        extra.sort();
        for p in extra {
            let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            out.push(Report {
                name,
                version: None,
                portable: None,
                problems: vec![Problem::Missing { what: "Left over, not tracked by moon".into(), path: p }],
            });
        }
    }

    let mut orphan: Vec<(String, PathBuf, PathBuf)> = Vec::new();
    if let Ok(rd) = fs::read_dir(&d.bin) {
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            let Some(t) = fs::read_link(&p).ok() else { continue };
            if !t.exists() && !owned.contains_key(&p) {
                orphan.push((owned.get(&p).cloned().unwrap_or_default(), p, t));
            }
        }
    }
    if !orphan.is_empty() {
        let mut r = Report { name: "command links".into(), version: None, portable: None, problems: Vec::new() };
        for (_, p, t) in orphan {
            r.problems.push(Problem::OrphanLink { link: p, target: t });
        }
        out.push(r);
    }

    Ok(out)
}

fn line(cfg: &Config, text: &str, path: &Path) {
    if path.as_os_str().is_empty() {
        println!("   {text}");
    } else {
        println!("   {} {text}: {}", cfg.icon(ico::BULLET), shorten_path(path, 64));
    }
}

pub fn print_report(cfg: &Config, reports: &[Report]) {
    let bad: Vec<&Report> = reports.iter().filter(|r| !r.problems.is_empty()).collect();
    if bad.is_empty() {
        println!();
        println!("  {} No broken entries. {} app(s) checked.", cfg.icon(ico::CHECK), reports.len());
        println!();
        return;
    }
    println!();
    println!("{} Broken entries:", cfg.icon(ico::WRENCH));
    println!();
    for r in bad {
        let ver = r.version.clone().unwrap_or_default();
        let tag = match &r.portable {
            Some(p) => format!("  {} portable: {}", cfg.icon(ico::DRIVE), shorten_path(p, 48)),
            None => String::new(),
        };
        println!(
            "{} {}{}{}",
            cfg.icon(ico::WARN),
            r.name,
            if ver.is_empty() { String::new() } else { format!(" {ver}") },
            tag
        );
        for p in &r.problems {
            match p {
                Problem::Missing { what, path } => line(cfg, what, path),
                Problem::BrokenLink { link, target } => {
                    println!("   {} Symlink exists", cfg.icon(ico::LINK));
                    line(cfg, "Target missing", link);
                    line(cfg, "Points at", target);
                }
                Problem::OrphanLink { link, target } => {
                    println!("   {} Command link not made by moon", cfg.icon(ico::WARN));
                    line(cfg, "Link", link);
                    line(cfg, "Target missing", target);
                }
            }
        }
        println!();
    }
}

fn fix_one(d: &Dirs, cfg: &Config, r: &Report) -> usize {
    let mut n = 0;
    let m = read_manifest(d, &r.name);
    for p in &r.problems {
        match p {
            Problem::Missing { path, .. } => {
                if fs::remove_file(path).is_ok() || (path.is_dir() && fs::remove_dir_all(path).is_ok()) {
                    println!("   {} removed {}", cfg.icon(ico::TRASH), shorten_path(path, 64));
                    n += 1;
                }
            }
            Problem::BrokenLink { link, .. } | Problem::OrphanLink { link, .. } => {
                if fs::remove_file(link).is_ok() {
                    println!("   {} removed broken link {}", cfg.icon(ico::TRASH), shorten_path(link, 64));
                    n += 1;
                }
            }
        }
    }
    if let Some(mut m) = m {
        let live = |p: &Path| p.exists();
        m.links.retain(|l| live(l));
        m.link_targets.truncate(m.links.len());
        if let Some(p) = &m.desktop {
            if !p.exists() {
                m.desktop = None;
            }
        }
        if m.dir.as_ref().is_some_and(|p| !p.exists()) {
            let _ = fs::remove_file(manifest_path(d, &r.name));
            println!("   {} forgot {}, the app itself is gone", cfg.icon(ico::TRASH), r.name);
            return n;
        }
        if write_manifest(d, &r.name, &m).is_ok() {
            println!("   {} updated the manifest", cfg.icon(ico::SAVE));
            n += 1;
        }
    }
    n
}

fn confirm_purge<'a, I: Iterator<Item = &'a Report>>(cfg: &Config, reports: I, yes: bool) -> bool {
    let gone: Vec<&Report> = reports
        .filter(|r| {
            r.problems.iter().any(|p| matches!(p, Problem::Missing { path, .. } if path.is_dir()))
        })
        .collect();
    if gone.is_empty() {
        return confirm(
            &format!("  {} Remove the leftovers listed above?", cfg.icon(ico::WARN)),
            yes,
        );
    }
    println!();
    println!("  {} These apps are gone, so moon will remove their leftovers:", cfg.icon(ico::WARN));
    for r in &gone {
        println!("    {} {}", cfg.icon(ico::BULLET), r.name);
    }
    println!();
    confirm(&format!("  {} Continue?", cfg.icon(ico::WARN)), yes)
}

pub fn cmd_doctor(d: &Dirs, cfg: &Config, fix: bool, yes: bool) -> Res<()> {
    let reports = scan(d)?;
    print_report(cfg, &reports);

    let bad: Vec<&Report> = reports.iter().filter(|r| !r.problems.is_empty()).collect();
    if bad.is_empty() {
        return Ok(());
    }

    println!("  {} app(s) with problems. Nothing was changed.", bad.len());
    if !fix {
        println!(
            "  {} Run: moon doctor --fix   (add --yes to skip the question)",
            cfg.icon(ico::ARROW)
        );
        println!();
        return Ok(());
    }

    println!();
    if !confirm_purge(cfg, bad.iter().copied(), yes) {
        println!("  {} Cancelled.", cfg.icon(ico::CROSS));
        return Ok(());
    }

    let mut fixed = 0;
    for r in &bad {
        let before = r.problems.len();
        let n = fix_one(d, cfg, r);
        if n > 0 {
            fixed += 1;
            println!(
                "   {} {name} ({before} problem(s))",
                cfg.icon(ico::CHECK),
                name = r.name
            );
        }
    }
    println!();
    if fixed > 0 {
        println!("==> {} Fixed {fixed} app(s)", cfg.icon(ico::WRENCH));
    } else {
        println!("==> {} Nothing could be fixed automatically", cfg.icon(ico::WARN));
    }

    let after = scan(d)?.into_iter().filter(|r| !r.problems.is_empty()).count();
    if after == 0 {
        println!("==> {} No broken entries left", cfg.icon(ico::CHECK));
    } else {
        println!(
            "==> {} {after} app(s) still have problems, run `moon doctor` to look",
            cfg.icon(ico::WARN)
        );
    }
    crate::desktop::update_desktop_db(d);
    println!();
    Ok(())
}
