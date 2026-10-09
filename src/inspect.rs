use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::bundle::{bundle_manifest, icon_in};
use crate::config::{Config, ico};
use crate::deps;
use crate::fsutil::{desktop_and_icon, is_exec_file, pick_main, tree_size, walk};
use crate::naming::{capitalize, guess_arch, guess_name, guess_version, is_noise, name_words, sanitize};
use crate::paths::{Cleanup, Dirs};
use crate::probe::{archive_format, elf_arch, file_kind, head_bytes, is_archive_name, is_compressed, missing_libs, uname_m};
use crate::state::{manifest_path, read_last, read_manifest};
use crate::util::{Res, ago, human_size, shorten_path, vis_len, wrap_text};
use crate::archive::{download, extract, unpack_tar};
use crate::deb::ar::{DEB_SCRIPTS, ar_member_to, ar_members, is_deb};
use crate::deb::control::parse_control;
use crate::deb::tree::{Node, deb_bin_name, deb_rel, deb_target, scan_tree};

struct Report {
    title: String,
    kind: String,
    format: String,
    label: String,
    #[allow(dead_code)]
    name: String,
    display: String,
    version: Option<String>,
    arch: String,
    arch_ok: bool,
    main: Option<PathBuf>,
    files: usize,
    size: u64,
    desktop: bool,
    icon: bool,
    libs: Option<(bool, Vec<String>)>,
    checks_title: String,
    name_label: String,
    from_name: Option<String>,
    notes: Vec<String>,
    app_dir: PathBuf,
    install_to: Vec<String>,
    creates: Vec<String>,
    relocate: Vec<String>,
    existing: Option<String>,
}

fn report_deb(d: &Dirs, src: &Path, work: &Path, fname: &str, root_mode: bool) -> Res<Report> {
    let mut ar = fs::File::open(src).map_err(|e| format!("cannot open {}: {e}", src.display()))?;
    let members = ar_members(&mut ar)?;
    let pick = |pfx: &str| {
        members
            .iter()
            .find(|m| m.name == pfx || m.name.starts_with(&format!("{pfx}.")))
            .cloned()
    };
    let Some(cm) = pick("control.tar") else { return Err("not a Debian package (no control.tar)".into()) };
    let Some(dm) = pick("data.tar") else { return Err("not a Debian package (no data.tar)".into()) };
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
        .ok_or("no control file")?;
    let meta = parse_control(&ctext);
    let name = match meta.get("Package") {
        Some(p) => sanitize(p)?,
        _ => sanitize(&guess_name(fname))?,
    };
    let version = meta.get("Version").map(String::from);
    let deb_arch = meta.get("Architecture").unwrap_or("?").to_string();
    let app_dir = d.apps.join(&name);

    let nodes = scan_tree(&ddir)?;
    let mut creates: Vec<String> = Vec::new();
    let mut relocate: Vec<String> = Vec::new();
    let mut main: Option<PathBuf> = None;
    let mut bin_names: Vec<String> = Vec::new();
    let mut desktop = false;
    let mut icon = false;
    let mut total = 0u64;
    let mut nfiles = 0usize;
    let mut seen: HashSet<PathBuf> = HashSet::new();

    for (p, node) in &nodes {
        let rel = deb_rel(p, &ddir)?;
        let (target, _) = deb_target(&rel, root_mode, &d.local, &app_dir);
        let under_usr = rel == "usr" || rel.starts_with("usr/");
        match node {
            Node::Dir => {
                seen.insert(target.clone());
                if !under_usr {
                    continue;
                }
            }
            Node::File => {
                nfiles += 1;
                total += fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                if !under_usr {
                    relocate.push(rel.clone());
                }
                if let Some(bin) = deb_bin_name(&rel) {
                    bin_names.push(bin.to_string());
                }
                if main.is_none() && is_exec_file(p) && deb_bin_name(&rel).is_some() {
                    main = Some(target.clone());
                }
                if target.extension().map_or(false, |e| e == "desktop")
                    && target.parent().map_or(false, |d| d.ends_with("applications"))
                {
                    desktop = true;
                }
                if target
                    .extension()
                    .and_then(|e| e.to_str())
                    .map_or(false, |e| matches!(e.to_ascii_lowercase().as_str(), "png" | "svg" | "xpm"))
                {
                    icon = true;
                }
                if let Some(par) = target.parent() {
                    let mut a = par;
                    while a != d.local && a.parent().is_some() && seen.insert(a.to_path_buf()) {
                        creates.push(format!("{}  (dir)", a.display()));
                        a = a.parent().unwrap();
                    }
                }
                creates.push(target.display().to_string());
            }
            Node::Link(_) => {
                if !under_usr {
                    relocate.push(rel.clone());
                }
                creates.push(format!("{}  (symlink)", target.display()));
            }
            _ => {}
        }
    }
    if main.is_none() {
        let n = name.to_ascii_lowercase();
        let mut best: Option<(i64, PathBuf)> = None;
        for (p, node) in &nodes {
            if !matches!(node, Node::File) || !is_exec_file(p) {
                continue;
            }
            let Ok(rel) = deb_rel(p, &ddir) else { continue };
            let fname = rel.rsplit('/').next().unwrap_or("").to_ascii_lowercase();
            let mut s: i64 = 0;
            if fname == n || fname == format!("{n}-bin") {
                s += 100;
            } else if fname.contains(&n) {
                s += 60;
            }
            if ["uninstall", "crashpad", "updater", "sandbox", "helper", "wrapper"]
                .iter()
                .any(|k| fname.contains(k))
            {
                s -= 80;
            }
            s -= rel.matches('/').count() as i64 * 3;
            s += (fs::metadata(p).map(|m| m.len()).unwrap_or(0) / 1_000_000) as i64;
            let (target, _) = deb_target(&rel, root_mode, &d.local, &app_dir);
            if best.as_ref().map_or(true, |(bs, _)| s > *bs) {
                best = Some((s, target));
            }
        }
        main = best.map(|(_, t)| t);
    }
    let install_to = if root_mode {
        vec!["/usr, /opt, /etc  (--root: real system paths)".into()]
    } else {
        vec![
            format!("{}/  (the package's /usr)", d.local.display()),
            format!("{}/  (metadata + non-/usr payload)", app_dir.display()),
        ]
    };
    if !root_mode && !bin_names.is_empty() {
        creates.push(format!("{}/  (command links: {})", d.bin.display(), bin_names.join(", ")));
    }
    if desktop {
        creates.push(format!("{}/{}.desktop  (menu entry)", d.desktop.display(), name));
    }

    let existing = if manifest_path(d, &name).exists() {
        let v = read_manifest(d, &name).and_then(|m| m.version);
        let same = version.as_deref() == v.as_deref();
        Some(format!(
            "{name}{} is already installed{}",
            v.map(|x| format!(" {x}")).unwrap_or_default(),
            if same { " (same version, would be reinstalled)" } else { "" }
        ))
    } else {
        None
    };

    let deb_arch_lc = deb_arch.to_ascii_lowercase();
    let (arch, arch_ok) = if deb_arch_lc == "all" {
        ("any (runs anywhere)".to_string(), true)
    } else {
        let host = uname_m();
        let fits = match deb_arch_lc.as_str() {
            "amd64" | "x86-64" => host == "x86_64",
            "arm64" | "aarch64" => host == "aarch64",
            "i386" | "i686" | "x86" => host == "x86" || host == "i686",
            "armhf" => host.starts_with("arm"),
            other => other == host,
        };
        (
            if fits {
                format!("{deb_arch} (this machine: {host})")
            } else {
                format!("{deb_arch} - not this machine ({host})")
            },
            fits,
        )
    };

    let mut notes: Vec<String> = Vec::new();
    if desktop {
        notes.push("desktop entry paths and icon will be rewritten to point at ~/.local".into());
    }
    if let Some(dep) = meta.get("Depends") {
        let deps = dep
            .split(',')
            .map(|s| s.split_whitespace().next().unwrap_or("").to_string())
            .filter(|s| s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        if !deps.is_empty() {
            notes.push(format!("depends on: {deps} (moon does not install dependencies)"));
        }
    }
    let has_scripts = DEB_SCRIPTS.iter().any(|s| cdir.join(s).is_file());
    if has_scripts {
        notes.push("has maintainer scripts; moon will not run them unless you pass --run-scripts".into());
    }
    if !relocate.is_empty() {
        notes.push(format!(
            "{} path(s) outside /usr are not written to their real location - use --root to do that",
            relocate.len()
        ));
    }

    let title = capitalize(&name);
    Ok(Report {
        title: title.clone(),
        kind: "Debian package".into(),
        format: format!(
            "deb ({} control, {} data)",
            archive_format(&cm.name),
            archive_format(&dm.name)
        ),
        label: format!(
            "Package: {}{}",
            meta.get("Package").unwrap_or(&name),
            version.clone().map(|v| format!(" {v}")).unwrap_or_default()
        ),
        name,
        display: meta.get("Summary").unwrap_or("").to_string(),
        version,
        arch,
        arch_ok,
        main: main.clone(),
        files: nfiles,
        size: total,
        desktop,
        icon,
        libs: main.as_ref().map(|m| {
            let miss = missing_libs(std::slice::from_ref(m));
            (miss.is_empty(), miss)
        }),
        checks_title: "Package contains".into(),
        name_label: "Installs as".into(),
        from_name: None,
        notes,
        app_dir: app_dir.clone(),
        install_to,
        creates,
        relocate,
        existing,
    })
}

fn report_archive(d: &Dirs, src: &Path, lname: &str, work: &Path) -> Res<Report> {
    let stage = work.join("stage");
    fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    extract(src, lname, &stage, "app")?;
    let entries: Vec<_> = fs::read_dir(&stage).map_err(|e| e.to_string())?.flatten().collect();
    if entries.is_empty() {
        return Err("archive is empty".into());
    }
    let root = match entries.as_slice() {
        [e] if e.file_type().map_or(false, |t| t.is_dir()) => e.path(),
        _ => stage.clone(),
    };
    let name = sanitize(&guess_name(lname))?;
    let version = guess_version(lname);
    let files = walk(&root, 8);
    let main = pick_main(&root, &files, &name);
    let size = tree_size(&root);
    let (desktop, icon) = desktop_and_icon(&files, &name);
    let app_dir = d.apps.join(&name);

    let mut creates: Vec<String> = vec![format!("{}/", app_dir.display())];
    let mut bin_names: Vec<String> = Vec::new();
    for bd in ["bin", "usr/bin"].iter().map(|p| root.join(p)).filter(|p| p.is_dir()) {
        if let Ok(rd) = fs::read_dir(&bd) {
            for e in rd.flatten() {
                let p = e.path();
                if is_exec_file(&p) {
                    bin_names.push(p.file_name().unwrap_or_default().to_string_lossy().into_owned());
                }
            }
        }
    }
    if !bin_names.is_empty() {
        creates.push(format!("{}/  (command links: {})", d.bin.display(), bin_names.join(", ")));
    }
    if desktop || icon {
        creates.push(format!("{}/{}.desktop  (menu entry)", d.desktop.display(), name));
    }
    let existing = if manifest_path(d, &name).exists() {
        let cur = read_manifest(d, &name).and_then(|m| m.version);
        Some(match &version {
            Some(v) if cur.as_deref() == Some(v.as_str()) => {
                format!("{name} {v} is already installed (same version, would be reinstalled)")
            }
            _ => format!("{name} is already installed (would be replaced)"),
        })
    } else {
        None
    };

    let file_arch = main
        .as_ref()
        .and_then(|m| fs::File::open(m).ok().and_then(|mut f| {
            let mut b = [0u8; 20];
            f.read(&mut b).ok().map(|n| elf_arch(&b[..n]))
        }))
        .flatten()
        .or_else(|| guess_arch(lname));
    let host = uname_m();
    let (arch, arch_ok) = match file_arch {
        Some(a) if a == host => (format!("{a} (this machine)"), true),
        Some(a) => (format!("{a} - not this machine ({host})"), false),
        None => (format!("unknown - assuming {host}"), true),
    };

    let mut notes: Vec<String> = Vec::new();
    if !desktop && !icon {
        notes.push("no icon and no .desktop file: this looks like a command line tool, so moon will skip the menu entry".into());
    } else {
        notes.push("menu entry will be rewritten with the final executable and icon paths".into());
    }
    if version.is_none() {
        notes.push("no version in the file name, so moon will record the install as unversioned".into());
    }

    let libs = main.as_ref().map(|m| {
        let miss = missing_libs(std::slice::from_ref(m));
        (miss.is_empty(), miss)
    });
    let title = capitalize(&name);
    Ok(Report {
        title: title.clone(),
        kind: "Archive".into(),
        format: archive_format(lname),
        label: format!("Archive: {lname}"),
        name,
        display: String::new(),
        version,
        arch,
        arch_ok,
        main: main.clone(),
        files: files.len(),
        size,
        desktop,
        icon,
        libs,
        checks_title: "Archive contains".into(),
        name_label: "Would install as".into(),
        from_name: Some(lname.to_string()),
        notes,
        app_dir: app_dir.clone(),
        install_to: vec![format!("{}/", app_dir.display())],
        creates,
        relocate: Vec::new(),
        existing,
    })
}

fn work_prefix() -> PathBuf {
    Dirs::new().map_or_else(|_| PathBuf::from("/nonexistent"), |d| d.tmp)
}

fn print_report(cfg: &Config, r: &Report) {
    let yes = cfg.icon(ico::CHECK);
    let no = cfg.icon(ico::CROSS);
    let mark = |ok: bool| if ok { yes } else { no };
    let row = |k: &str, v: &str| {
        let pad = 15usize.saturating_sub(vis_len(k) + 1);
        println!("  {k}:{}{v}", " ".repeat(pad));
    };

    println!();
    println!("  {}  {}", cfg.icon(ico::PACKAGE), r.title);
    println!("  {}", "\u{2500}".repeat(vis_len(&r.title) + 4));
    println!("  {}", r.label);

    row("Type", &r.kind);
    row("Format", &r.format);
    let arch_pad = 15usize.saturating_sub(vis_len("Architecture") + 1);
    println!("  Architecture:{}{} {}", " ".repeat(arch_pad), mark(r.arch_ok), r.arch);
    match &r.version {
        Some(v) => row("Version", v),
        None => row("Version", "not stated"),
    }

    println!();
    println!("  {} {}", cfg.icon(ico::LIST), r.checks_title);
    let check = |ok: bool, what: &str, detail: &str| {
        let head = if what.ends_with(':') { what.to_string() } else { format!("{what}:") };
        let pad = 18usize.saturating_sub(vis_len(&head));
        println!("    {} {head}{}{detail}", mark(ok), " ".repeat(pad));
    };
    match &r.main {
        Some(m) => {
            let shown = m
                .strip_prefix(work_prefix())
                .map_or_else(|_| m.clone(), |rel| r.app_dir.join(rel));
            let shown = shorten_path(&shown, 46);
            check(true, "executable", &format!("  {shown}"));
        }
        None => check(false, "executable", "  none found - moon would need --bin"),
    }
    check(r.icon, "icon", if r.icon { "  found" } else { "  none in the package" });
    check(r.desktop, "desktop entry", if r.desktop { "  found" } else { "  none in the package" });
    if let Some((ok, miss)) = &r.libs {
        if *ok {
            check(true, "libraries", "  all found");
        } else {
            check(false, "libraries", &format!("  missing: {}", miss.join(", ")));
        }
    }

    let extra = r
        .files
        .saturating_sub(usize::from(r.main.is_some()) + usize::from(r.icon) + usize::from(r.desktop));
    if extra > 0 {
        let what = if extra == 1 { "supporting file" } else { "supporting files" };
        check(true, what, &format!("  {}, {}", extra, human_size(r.size)));
    }

    if let Some(e) = &r.existing {
        println!();
        println!("  {} {}", cfg.icon(ico::WARN), e);
    }

    println!();
    println!("  {} {} {}", cfg.icon(ico::CHECK), r.name_label, r.title);
    if !r.display.is_empty() && r.display != r.title {
        println!("      {}", r.display);
    }

    if !r.notes.is_empty() {
        println!();
        for n in &r.notes {
            for (i, line) in wrap_text(n, 64).iter().enumerate() {
                if i == 0 {
                    println!("  {} note: {line}", cfg.icon(ico::INFO));
                } else {
                    println!("       {line}");
                }
            }
        }
    }

    if let Some(from) = &r.from_name {
        let words = name_words(from);
        let trimmed = words.iter().filter(|w| !is_noise(&w.to_ascii_lowercase())).cloned().collect::<Vec<_>>().join("-");
        if !trimmed.is_empty() && trimmed != words.join("-") {
            println!();
            println!("  {} From the file name:", cfg.icon(ico::PACKAGE));
            println!("      {from}");
            let mut bits = vec![format!("name: {}", r.title)];
            if let Some(v) = &r.version {
                bits.push(format!("version: {v}"));
            }
            bits.push(format!("architecture: {}", r.arch.split(" (").next().unwrap_or(&r.arch)));
            println!("      {}", bits.join("  "));
        }
    }

    println!();
    println!("  {} Would install to:", cfg.icon(ico::FOLDER));
    for d in &r.install_to {
        println!("    {d}");
    }
    if !r.relocate.is_empty() {
        let mut tops: Vec<String> = r
            .relocate
            .iter()
            .filter_map(|x| x.split('/').next())
            .map(String::from)
            .collect();
        tops.sort();
        tops.dedup();
        println!(
            "  {} {} path(s) outside /usr ({}) get relocated, not placed at their real path",
            cfg.icon(ico::WARN),
            r.relocate.len(),
            tops.join(", ")
        );
    }
    println!();
    println!("  {} Files to create:", cfg.icon(ico::LIST));
    for c in r.creates.iter().take(24) {
        println!("    {c}");
    }
    if r.creates.len() > 24 {
        println!("    ... and {} more", r.creates.len() - 24);
    }
    println!();
}

fn report_program(d: &Dirs, src: &Path, fname: &str) -> Res<Report> {
    let (kind, format) = file_kind(src, fname);
    let name = sanitize(&guess_name(fname))?;
    let app_dir = d.apps.join(&name);
    let main = src.to_path_buf();
    let size = fs::metadata(src).map(|m| m.len()).unwrap_or(0);

    let head = {
        let mut b = [0u8; 20];
        let n = fs::File::open(src).and_then(|mut f| f.read(&mut b)).unwrap_or(0);
        b[..n].to_vec()
    };
    let host = uname_m();
    let (arch, arch_ok) = match elf_arch(&head) {
        Some(a) if a == host => (format!("{a} (this machine)"), true),
        Some(a) => (format!("{a} - not this machine ({host})"), false),
        None => (format!("unknown - assuming {host}"), true),
    };

    let missing = missing_libs(std::slice::from_ref(&main));
    let is_appimage = format == "AppImage";
    let is_gui = is_appimage;

    let mut notes: Vec<String> = Vec::new();
    if kind.starts_with("Windows") {
        notes.push("this is a Windows program and will not run on Linux".into());
    } else if kind == "Not a program" || kind == "Unknown" {
        notes.push(format!("this file is {format}, not something moon can install"));
    } else if kind == "Download failure" {
        notes.push("this looks like a failed download - the server sent a web page instead of the file".into());
    }
    if kind == "Command line program" && !is_gui {
        notes.push("a script or small program: moon will link it into ~/.local/bin without a menu entry (use --desktop for one)".into());
    }
    if !missing.is_empty() {
        notes.push(format!(
            "missing libraries: {} - the program will not start until they are installed",
            missing.join(", ")
        ));
    }
    if is_appimage {
        notes.push("AppImages need FUSE to mount; if it fails to start, run it with --appimage-extract-and-run".into());
    }

    let installed = manifest_path(d, &name).exists().then(|| format!("{name} is already installed (would be replaced)"));
    let cmd = format!("{}/{name}  ->  {}", d.bin.display(), main.display());
    let cmd_link = format!("{}/{name}", d.bin.display());
    let title = capitalize(&name);
    Ok(Report {
        title: title.clone(),
        kind,
        format,
        label: format!("File: {fname}"),
        name,
        display: String::new(),
        version: guess_version(fname),
        arch,
        arch_ok,
        main: Some(main.clone()),
        files: 1,
        size,
        desktop: false,
        icon: false,
        libs: Some((missing.is_empty(), missing)),
        checks_title: "Checks".into(),
        name_label: "Would install as".into(),
        from_name: Some(fname.to_string()),
        notes,
        app_dir: app_dir.clone(),
        install_to: vec![format!("{}/", app_dir.display()), cmd_link],
        creates: vec![format!("{}/", app_dir.display()), cmd],
        relocate: Vec::new(),
        existing: installed,
    })
}

pub fn cmd_inspect(d: &Dirs, cfg: &Config, target: &str) -> Res<()> {
    if target == "last" || target == "--last" {
        return inspect_last(d, cfg);
    }

    let work = d.tmp.join(format!("inspect-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let _guard = Cleanup(work.clone());

    let is_url = target.starts_with("http://") || target.starts_with("https://");
    let src = if is_url { download(target, &work)? } else { PathBuf::from(target) };
    if !src.is_file() {
        return Err(format!("'{}' is not a file", src.display()));
    }
    let fname = src.file_name().and_then(|s| s.to_str()).unwrap_or("app").to_string();
    let lname = fname.to_ascii_lowercase();
    let root_mode = env::args().any(|a| a == "--root");
    let r = if is_deb(&src, &lname) {
        report_deb(d, &src, &work, &lname, root_mode)?
    } else if crate::bundle::is_bundle_name(&lname) {
        report_bundle(d, &src, &work)?
    } else if is_archive_name(&lname) || is_compressed(&head_bytes(&src)) {
        report_archive(d, &src, &lname, &work)?
    } else {
        report_program(d, &src, &fname)?
    };
    print_report(cfg, &r);
    Ok(())
}

fn report_bundle(d: &Dirs, src: &Path, work: &Path) -> Res<Report> {
    let stage = work.join("bundle");
    fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    extract(src, "app.moon", &stage, "bundle")?;
    let (name, bm) = bundle_manifest(&stage)?;
    let app = stage.join("app");
    if !app.is_dir() {
        return Err("not a moon bundle: no app/ directory inside".into());
    }
    let files = walk(&app, 8);
    let size = tree_size(&app);
    let main_rel = bm
        .main
        .as_ref()
        .and_then(|p| p.strip_prefix("app").ok())
        .map(|p| p.to_path_buf())
        .or_else(|| pick_main(&app, &files, &name));
    let main = main_rel.as_ref().map(|r| app.join(r));
    let fname = src.file_name().and_then(|s| s.to_str()).unwrap_or("app.moon");
    let version = bm.version.clone().filter(|v| !v.is_empty()).or_else(|| guess_version(fname));
    let app_dir = d.apps.join(&name);

    let cmds: Vec<String> = bm
        .links
        .iter()
        .map(|l| l.file_name().unwrap_or_default().to_string_lossy().into_owned())
        .collect();
    let mut creates: Vec<String> = vec![format!("{}/", app_dir.display())];
    if !cmds.is_empty() {
        creates.push(format!("{}/  (command links: {})", d.bin.display(), cmds.join(", ")));
    }
    if bm.desktop.is_some() {
        creates.push(format!("{}/{}.desktop  (menu entry)", d.desktop.display(), name));
    }
    let existing = if manifest_path(d, &name).exists() {
        Some(format!("{name} is already installed (would be replaced)"))
    } else {
        None
    };

    let file_arch = main
        .as_ref()
        .and_then(|m| fs::File::open(m).ok().and_then(|mut f| {
            let mut b = [0u8; 20];
            f.read(&mut b).ok().map(|n| elf_arch(&b[..n]))
        }))
        .flatten();
    let host = uname_m();
    let (arch, arch_ok) = match file_arch {
        Some(a) if a == host => (format!("{a} (this machine)"), true),
        Some(a) => (format!("{a} - not this machine ({host})"), false),
        None => (format!("unknown - assuming {host}"), true),
    };

    let mut notes: Vec<String> = vec![
        "a .moon bundle is a plain tar.gz: `tar xf` opens it, nothing else is needed".into(),
        "the menu entry and icon inside are rewritten to this machine's paths".into(),
        format!("no server: the file itself is the whole package ({})", human_size(size)),
    ];
    if !bm.deps.is_empty() {
        let (id, id_like) = deps::os_release();
        let txt = match deps::pick_group(&bm.deps, &id, &id_like) {
            Some(g) => format!("{} ({})", g.packages.join(", "), g.distro),
            None => {
                let names: Vec<&str> = bm.deps.iter().map(|d| d.distro.as_str()).collect();
                format!("declared for {} - not this system ({id})", names.join(", "))
            }
        };
        notes.push(format!(
            "declares downloadable dependencies: {txt} (moon fetches them from the distro's own repos, no root, nothing system-wide)"
        ));
    }

    Ok(Report {
        title: capitalize(&name),
        kind: "Moon bundle".into(),
        format: "single file, carries its own app, icon and menu entry".into(),
        label: format!("Bundle: {} ({} file(s), {})", name, files.len(), human_size(size)),
        name: name.clone(),
        display: name.clone(),
        version,
        arch,
        arch_ok,
        main: main_rel.as_ref().map(|r| app_dir.join(r)),
        files: files.len(),
        size,
        desktop: bm.desktop.is_some(),
        icon: icon_in(&stage.join("icon")).is_some(),
        libs: None,
        checks_title: "Checks".into(),
        name_label: "Would install as".into(),
        from_name: Some(format!("{name}.moon")),
        notes,
        install_to: vec![format!("{}/", app_dir.display())],
        app_dir,
        creates,
        relocate: Vec::new(),
        existing,
    })
}

fn inspect_last(d: &Dirs, cfg: &Config) -> Res<()> {
    let Some(l) = read_last(d) else {
        return Err("nothing has been installed with moon yet".into());
    };
    let m = read_manifest(d, &l.name).unwrap_or_default();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let row = |k: &str, v: String| println!("  {k:<14}{v}");
    println!();
    println!("  {}  Last install: {} {}", cfg.icon(ico::PACKAGE), l.name, m.version.clone().unwrap_or_default());
    println!("  {}", "\u{2500}".repeat(40));
    if !l.source.is_empty() {
        row("Source:", l.source.clone());
    }
    if l.when > 0 && now > l.when {
        row("Installed:", ago(now - l.when));
    }
    row(
        "Commands:",
        if m.cmds.is_empty() { "(none)".into() } else { m.cmds.join(", ") },
    );
    row("Location:", m.dir.clone().unwrap_or_default().display().to_string());
    row(
        "Paths:",
        format!(
            "{} created, {}",
            m.files.len() + m.links.len(),
            if m.root { "system-wide" } else { "per-user" }
        ),
    );
    if let Some(dk) = &m.desktop {
        row("Menu entry:", dk.display().to_string());
    }
    if m.portable.is_some() {
        row("Portable:", "yes, lives outside ~/.local".to_string());
    }
    println!();
    println!("  {} Undo it with:  moon undo", cfg.icon(ico::UNDO));
    println!();
    Ok(())
}
