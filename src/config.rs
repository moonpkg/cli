use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::util::Res;

pub mod ico {
    pub const CHECK: &str = "\u{f00c}";
    pub const CROSS: &str = "\u{f00d}";
    pub const WARN: &str = "\u{f071}";
    pub const FOLDER: &str = "\u{f07b}";
    pub const COG: &str = "\u{f013}";
    pub const PACKAGE: &str = "\u{f487}";
    pub const ARROW: &str = "\u{f061}";
    pub const TRASH: &str = "\u{f1f8}";
    pub const LIST: &str = "\u{f03a}";
    pub const UNDO: &str = "\u{f0e2}";
    pub const BULLET: &str = "\u{f444}";
    pub const LINK: &str = "\u{f0c1}";
    pub const DRIVE: &str = "\u{f0a0}";
    pub const DOWNLOAD: &str = "\u{f019}";
    pub const CLOCK: &str = "\u{f017}";
    pub const INFO: &str = "\u{f05a}";
    pub const MAGIC: &str = "\u{f0d0}";
    pub const WRENCH: &str = "\u{f0ad}";
    pub const SEARCH: &str = "\u{f002}";
    pub const FILE: &str = "\u{f15b}";
    pub const SHIELD: &str = "\u{f132}";
    pub const REFRESH: &str = "\u{f021}";
    pub const SAVE: &str = "\u{f0c7}";
    pub const UP: &str = "\u{f077}";
    pub const MOON: &str = "\u{f186}";
    pub const EXAM: &str = "\u{f058}";
}

#[derive(Clone, Copy, PartialEq)]
pub enum IconMode {
    Auto,
    Always,
    Never,
}

pub struct Config {
    path: PathBuf,
    pub icons: IconMode,
    nerd_detected: bool,
}

impl Config {
    pub fn load() -> Self {
        let cfg = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".config"));
        let path = cfg.join("moon").join("config");
        let mut c = Config { path, icons: IconMode::Auto, nerd_detected: has_nerd_font() };
        if let Ok(t) = fs::read_to_string(&c.path) {
            for l in t.lines() {
                if let Some(v) = l.trim().strip_prefix("icons=") {
                    c.icons = match v.trim() {
                        "always" | "on" | "yes" => IconMode::Always,
                        "never" | "off" | "no" => IconMode::Never,
                        _ => IconMode::Auto,
                    };
                }
            }
        }
        c
    }

    fn save(&self) -> Res<()> {
        if let Some(p) = self.path.parent() {
            fs::create_dir_all(p).map_err(|e| format!("cannot create {}: {e}", p.display()))?;
        }
        let s = format!("# moon config\n# icons: auto | always | never\nicons={}\n", match self.icons {
            IconMode::Auto => "auto",
            IconMode::Always => "always",
            IconMode::Never => "never",
        });
        fs::write(&self.path, s).map_err(|e| format!("cannot write {}: {e}", self.path.display()))
    }

    pub fn icons_on(&self) -> bool {
        match self.icons {
            IconMode::Always => true,
            IconMode::Never => false,
            IconMode::Auto => self.nerd_detected,
        }
    }

    pub fn icon<'a>(&'a self, c: &'a str) -> &'a str {
        if self.icons_on() { c } else { "" }
    }
}

fn apps_hint(cfg: &Config) -> String {
    cfg.path
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("data/moon").display().to_string())
        .unwrap_or_default()
}

pub fn has_nerd_font() -> bool {
    let home = env::var_os("HOME").map(PathBuf::from);
    let mut roots: Vec<PathBuf> = vec![
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
        PathBuf::from("/run/current-system/sw/share/fonts"),
    ];
    if let Some(h) = &home {
        roots.push(h.join(".local/share/fonts"));
        roots.push(h.join(".fonts"));
    }
    let mut stack: Vec<PathBuf> = roots;
    let mut seen = 0;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let is_dir = e.file_type().map_or(false, |t| t.is_dir());
            if is_dir {
                if p.file_name().map_or(false, |n| n == "fonts.conf") {
                    continue;
                }
                if stack.len() < 512 {
                    stack.push(p);
                }
                continue;
            }
            seen += 1;
            if seen > 20_000 {
                return false;
            }
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
            if (name.contains("nerd") || name.contains("powerline") || name.contains("nerdfont"))
                && [".ttf", ".otf", ".ttc", ".woff2"].iter().any(|e| name.ends_with(e))
            {
                return true;
            }
        }
    }
    false
}

pub fn cmd_config(cfg: &mut Config, icons_set: bool) -> Res<()> {
    let interactive = {
        use std::io::IsTerminal;
        std::io::stdin().is_terminal()
    };
    let mode_str = |m: IconMode| match m {
        IconMode::Auto => "auto",
        IconMode::Always => "always",
        IconMode::Never => "never",
    };
    let detected = if cfg.nerd_detected { "found" } else { "not installed" };

    if icons_set && !interactive {
        cfg.save()?;
        println!("{} Icons: {} ({})", cfg.icon(ico::CHECK), mode_str(cfg.icons), detected);
        println!("  {} saved to {}", cfg.icon(ico::SAVE), cfg.path.display());
        return Ok(());
    }

    if !interactive {
        println!();
        println!("  {}  moon settings", cfg.icon(ico::COG));
        println!("  {}", "\u{2500}".repeat(40));
        println!("  {:<14}{} ({})", "Icons:", mode_str(cfg.icons), detected);
        println!("  {:<14}{}", "Config:", cfg.path.display());
        println!("  {:<14}{}", "Apps:", format!("{}/apps", apps_hint(cfg)));
        println!();
        println!("  {} set with:  moon config --icons auto|always|never", cfg.icon(ico::BULLET));
        println!();
        return Ok(());
    }

    if icons_set {
        cfg.save()?;
        println!("{} Icons: {} ({})", cfg.icon(ico::CHECK), mode_str(cfg.icons), detected);
        return Ok(());
    }

    loop {
        println!();
        println!("  {}  moon settings", cfg.icon(ico::COG));
        println!("  {}", "\u{2500}".repeat(40));
        println!("  {:<3}{:<20}{}", "1.", "Icons", format!("{} (Nerd Font {})", mode_str(cfg.icons), detected));
        println!("  {:<3}{}", "0.", "Cancel");
        println!();
        print!("  {} ", cfg.icon(ico::ARROW));
        let _ = std::io::stdout().flush();
        let mut s = String::new();
        if std::io::stdin().read_line(&mut s).is_err() {
            return Ok(());
        }
        match s.trim() {
            "1" => {
                println!();
                println!("  Icons:");
                println!("    1. auto     use them only if a Nerd Font is installed  ({detected})");
                println!("    2. always   use them even if none is installed");
                println!("    3. never    plain text, no glyphs");
                print!("  {} ", cfg.icon(ico::ARROW));
                let _ = std::io::stdout().flush();
                let mut c = String::new();
                if std::io::stdin().read_line(&mut c).is_err() {
                    return Ok(());
                }
                cfg.icons = match c.trim() {
                    "2" => IconMode::Always,
                    "3" => IconMode::Never,
                    _ => IconMode::Auto,
                };
                cfg.save()?;
                println!("  {} Saved: icons = {}", cfg.icon(ico::CHECK), mode_str(cfg.icons));
            }
            "0" | "" => return Ok(()),
            _ => println!("  Pick 0-1."),
        }
    }
}
