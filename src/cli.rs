use std::env;
use std::path::PathBuf;

use crate::config::{Config, IconMode, cmd_config, ico};
use crate::desktop::update_desktop_db;
use crate::doctor::cmd_doctor;
use crate::history::cmd_history;
use crate::inspect::cmd_inspect;
use crate::install::{InstallOpts, cmd_undo, install, list, remove_app};
use crate::paths::Dirs;
use crate::util::Res;
use crate::tui::tui_pick;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn usage() {
    println!(
        "moon {VERSION} - install archives, AppImages and .deb packages into ~/.local

USAGE:
    moon install <archive|url|deb> [opts]  install or upgrade an app
    moon inspect <archive|url|deb|last>    show what installing would do
    moon list                              show installed apps
    moon remove <name>...                  uninstall (binaries, menu entry, files)
    moon undo [name]                       remove the app installed last (asks first)
    moon doctor [--fix]                    find apps you deleted by hand, repair them
    moon export <name> [dir]               copy an installed app as a portable bundle
    moon history [-n N] [--clear]          what was installed, and when
    moon config [--check]                  settings (icons, Nerd Font, update)
    moon help | --version

INSTALL OPTIONS:
    --name <name>       app/command name (default: guessed from the file name)
    --bin <rel/path>    main executable inside the archive (default: auto-detected)
    --desktop           always create a menu entry
    --no-desktop        never create a menu entry
    --force             overwrite entries not managed by moon
    --portable-dir <p>  keep the app on another disk (USB stick) instead of ~/.local

.deb OPTIONS:
    --root              install to the real /usr, /opt, ... (needs root, dpkg-style)
    --run-scripts       run the package's preinst/postinst (never run by default)
    --no-rewrite        don't rewrite absolute paths in desktop files/wrappers
    --dry-run, -n       show what would be installed, change nothing

EXPORT OPTIONS:
    --force             overwrite an existing bundle

UNDO / CONFIG OPTIONS:
    --yes, -y           don't ask for confirmation
    --icons <mode>      auto | always | never   (default: auto = if Nerd Font found)
    --check             only report the newest release, install nothing
    --update            update without opening the menu
    --script <path|url> use another installer instead of the official one

UPDATE:
    moon config                 pick 2: fetch docs/scripts/install.sh into the tmp dir and
                                run it - moon closes itself first, so the new binary can
                                take over ~/.local/bin/moon

SUPPORTED: .tar.gz .tar.xz .tar.bz2 .tar.zst .tgz .tar .zip .7z .AppImage .deb,
           or a bare executable. Apps live in ~/.local/share/moon/apps/<name>,
           commands are symlinked into ~/.local/bin."
    );
}

pub fn run() -> Res<()> {
    let all: Vec<String> = env::args().skip(1).collect();
    let Some(cmd) = all.first().map(String::as_str) else {
        usage();
        return Ok(());
    };
    let mut cfg = Config::load();

    let mut icons_set = false;
    let mut args: Vec<String> = Vec::with_capacity(all.len());
    let mut i = 0;
    while i < all.len() {
        if all[i] == "--icons" {
            if let Some(v) = all.get(i + 1) {
                cfg.icons = match v.as_str() {
                    "always" | "on" | "yes" => IconMode::Always,
                    "never" | "off" | "no" => IconMode::Never,
                    _ => IconMode::Auto,
                };
            }
            icons_set = true;
            i += 2;
            continue;
        }
        args.push(all[i].clone());
        i += 1;
    }
    match cmd {
        "help" | "-h" | "--help" => {
            usage();
            Ok(())
        }
        "-V" | "--version" | "version" => {
            println!("{} moon {VERSION}", cfg.icon(ico::MOON));
            Ok(())
        }
        "config" | "settings" | "prefs" => cmd_config(&mut cfg, icons_set, &args[1..]),
        "inspect" | "show" | "what" => {
            let d = Dirs::new()?;
            match args[1..].iter().find(|a| !a.starts_with('-')) {
                Some(t) => cmd_inspect(&d, &cfg, t),
                None => Err("usage: moon inspect <archive|url|deb|last>".into()),
            }
        }
        "undo" | "u" => {
            let d = Dirs::new()?;
            let name = args[1..].iter().find(|a| !a.starts_with('-')).map(|s| s.as_str());
            cmd_undo(&d, &cfg, name, args.iter().any(|a| a == "-y" || a == "--yes"))
        }
        "history" | "log" | "hist" => {
            let d = Dirs::new()?;
            let mut limit = None;
            let mut clear = false;
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--clear" => clear = true,
                    "-n" | "--limit" => {
                        limit = it.next().and_then(|v| v.parse().ok());
                    }
                    _ => {}
                }
            }
            cmd_history(&d, &cfg, limit, clear)
        }
        "list" | "ls" => list(&Dirs::new()?),
        "remove" | "uninstall" | "rm" => {
            if args.len() < 2 {
                return Err("usage: moon remove <name>...".into());
            }
            let d = Dirs::new()?;
            let mut failed = false;
            for n in &args[1..] {
                if let Err(e) = remove_app(&d, n, false) {
                    eprintln!("error: {} {e}", cfg.icon(ico::CROSS));
                    failed = true;
                }
            }
            update_desktop_db(&d);
            if failed { Err("some removals failed".into()) } else { Ok(()) }
        }
        "install" | "i" | "add" => {
            let mut o = InstallOpts {
                source: String::new(),
                name: None,
                bin: None,
                no_desktop: false,
                force_desktop: false,
                force: false,
                root: false,
                run_scripts: false,
                no_rewrite: false,
                dry_run: false,
                portable: None,
            };
            let mut deb_only: Vec<&str> = Vec::new();
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--name" => o.name = Some(it.next().ok_or("--name needs a value")?.clone()),
                    "--bin" => o.bin = Some(it.next().ok_or("--bin needs a value")?.clone()),
                    "--no-desktop" => o.no_desktop = true,
                    "--desktop" => o.force_desktop = true,
                    "--force" | "-f" => o.force = true,
                    "--root" => {
                        o.root = true;
                        deb_only.push("--root");
                    }
                    "--run-scripts" => {
                        o.run_scripts = true;
                        deb_only.push("--run-scripts");
                    }
                    "--no-rewrite" => {
                        o.no_rewrite = true;
                        deb_only.push("--no-rewrite");
                    }
                    "--dry-run" | "-n" => {
                        o.dry_run = true;
                        deb_only.push("--dry-run");
                    }
                    "--portable-dir" => {
                        let v = it.next().ok_or("--portable-dir needs a path")?;
                        o.portable = Some(PathBuf::from(v));
                    }
                    s if s.starts_with('-') => return Err(format!("unknown option '{s}'")),
                    s => {
                        if !o.source.is_empty() {
                            return Err("only one archive at a time".into());
                        }
                        o.source = s.to_string();
                    }
                }
            }
            if o.source.is_empty() && o.portable.is_none() {
                let d = Dirs::new()?;
                let start = env::current_dir().map_err(|e| e.to_string())?;
                let Some(picked) = tui_pick(&d, &cfg, &start)? else {
                    println!("{} Cancelled.", cfg.icon(ico::CROSS));
                    return Ok(());
                };
                o.source = picked.display().to_string();
            }
            if o.source.is_empty() {
                return Err("usage: moon install <archive|url|deb> [opts]".into());
            }
            install(&Dirs::new()?, &mut o)
        }
        "doctor" | "check" | "verify" | "repair" => {
            let d = Dirs::new()?;
            cmd_doctor(
                &d,
                &cfg,
                args.iter().any(|a| a == "--fix"),
                args.iter().any(|a| a == "-y" || a == "--yes"),
            )
        }
        "export" | "unexport" => {
            let pos: Vec<&String> = args[1..].iter().filter(|a| !a.starts_with('-')).collect();
            let Some(name) = pos.first().map(|s| s.as_str()) else {
                return Err(format!("usage: moon {cmd} <name> [dir]"));
            };
            let d = Dirs::new()?;
            let yes = args.iter().any(|a| a == "-y" || a == "--yes");
            let force = args.iter().any(|a| a == "--force" || a == "-f");
            match pos.get(1) {
                Some(p) => crate::portable::cmd_export(&d, &cfg, name, &PathBuf::from(p.as_str()), force),
                None => crate::portable::cmd_unexport(
                    &d,
                    &cfg,
                    name,
                    None,
                    yes || force,
                ),
            }
        }
        other => Err(format!("unknown command '{other}', try `moon help`")),
    }
}
