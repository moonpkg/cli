use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{Config, ico};
use crate::install::{InstallOpts, install as install_app};
use crate::paths::Dirs;
use crate::util::Res;

const TERMINALS: &[&str] = &[
    "xdg-terminal-exec",
    "x-terminal-emulator",
    "gnome-terminal",
    "konsole",
    "xfce4-terminal",
    "kitty",
    "alacritty",
    "foot",
    "wezterm",
    "xterm",
];

enum Target {
    Url(String),
    Github {
        repo: String,
        file: Option<String>,
        tag: Option<String>,
    },
}

pub fn is_uri(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.starts_with("moon://") || l.starts_with("moon:")
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let d = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (d(b[i + 1]), d(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn repo_slug(s: &str) -> Res<String> {
    let s = s.trim().trim_end_matches('/');
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    let s = s.strip_prefix("www.").unwrap_or(s);
    let s = s.strip_prefix("github.com/").unwrap_or(s);
    let s = s.strip_suffix(".git").unwrap_or(s);
    let mut parts = s.split('/');
    let owner = parts.next().unwrap_or("");
    let name = parts.next().unwrap_or("");
    let ok = |x: &str| {
        !x.is_empty()
            && x.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !ok(owner) || !ok(name) || parts.next().is_some() {
        return Err(format!("'{s}' is not a GitHub repo, use owner/name"));
    }
    Ok(format!("{owner}/{name}"))
}

fn parse(s: &str) -> Res<Target> {
    let rest = s
        .strip_prefix("moon://")
        .or_else(|| s.strip_prefix("moon:"))
        .unwrap_or(s);
    let rest = rest.trim_start_matches('/');

    if rest.starts_with("http://") || rest.starts_with("https://") {
        return Ok(Target::Url(rest.to_string()));
    }

    let cut = rest.find(['?', '=']).unwrap_or(rest.len());
    let action = rest[..cut].trim().to_ascii_lowercase();
    let mut value: Option<String> = None;
    let mut params: Vec<(String, String)> = Vec::new();
    let tail = &rest[cut..];

    if let Some(t) = tail.strip_prefix('=') {
        for (i, part) in t.split('&').enumerate() {
            match part.split_once('=') {
                Some((k, v)) => params.push((k.trim().to_ascii_lowercase(), decode(v))),
                None if i == 0 => value = Some(decode(part)),
                None => {}
            }
        }
    } else if let Some(t) = tail.strip_prefix('?') {
        for part in t.split('&') {
            if let Some((k, v)) = part.split_once('=') {
                params.push((k.trim().to_ascii_lowercase(), decode(v)));
            }
        }
    }

    let get = |keys: &[&str]| {
        params
            .iter()
            .find(|(k, v)| !v.is_empty() && keys.iter().any(|n| n == k))
            .map(|(_, v)| v.clone())
    };

    match action.as_str() {
        "" | "install" | "i" | "add" => {
            let url = value
                .filter(|v| !v.is_empty())
                .or_else(|| get(&["url", "source", "src"]))
                .unwrap_or_default();
            if url.is_empty() {
                return Err(
                    "moon://install needs a url, e.g. moon://install?url=https://example.com/app.tar.gz"
                        .into(),
                );
            }
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(format!("moon://install only takes http(s) URLs, got '{url}'"));
            }
            Ok(Target::Url(url))
        }
        "github" | "gh" => {
            let repo = value
                .filter(|v| !v.is_empty())
                .or_else(|| get(&["repo", "repository", "github", "gh"]))
                .unwrap_or_default();
            if repo.is_empty() {
                return Err(
                    "moon://github needs a repo, e.g. moon://github?repo=owner/name".into(),
                );
            }
            Ok(Target::Github {
                repo: repo_slug(&repo)?,
                file: get(&["file", "filename", "asset"]),
                tag: get(&["tag", "version", "release"]),
            })
        }
        other => Err(format!(
            "unknown moon:// action '{other}', use moon://install?url=... or moon://github?repo=owner/name"
        )),
    }
}

pub fn resolve(uri: &str) -> Res<String> {
    match parse(uri)? {
        Target::Url(u) => Ok(u),
        Target::Github { repo, file, tag } => resolve_release(&repo, tag.as_deref(), file.as_deref()),
    }
}

fn resolve_release(repo: &str, tag: Option<&str>, want: Option<&str>) -> Res<String> {
    let cfg = Config::load();
    let api = match tag {
        Some(t) => format!("https://api.github.com/repos/{repo}/releases/tags/{t}"),
        None => format!("https://api.github.com/repos/{repo}/releases/latest"),
    };
    println!(
        "{} {} Looking up {}",
        cfg.step(),
        cfg.icon(ico::SEARCH),
        match tag {
            Some(t) => format!("{repo} release {t}"),
            None => format!("the newest release of {repo}"),
        }
    );

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = env::temp_dir().join(format!("moon-uri-{}-{stamp}.json", std::process::id()));
    if let Err(e) = crate::deps::fetch_url(&api, &tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("cannot read the GitHub release of {repo}: {e}"));
    }
    let text = fs::read_to_string(&tmp).unwrap_or_default();
    let _ = fs::remove_file(&tmp);

    let assets = release_assets(&text);
    if assets.is_empty() {
        let msg = top_field(&text, "message").unwrap_or("");
        return Err(if msg.is_empty() {
            format!("{repo} has no release assets")
        } else {
            format!("GitHub: {msg}")
        });
    }

    let pool: Vec<(String, String)> = assets
        .iter()
        .filter(|(n, _)| n.to_ascii_lowercase().ends_with(".moon"))
        .cloned()
        .collect();
    if pool.is_empty() {
        return Err(format!(
            "{repo} has no .moon asset in that release (found: {})",
            assets
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if let Some(w) = want.filter(|w| !w.is_empty()) {
        let wl = w.to_ascii_lowercase();
        let exact = pool
            .iter()
            .find(|(n, _)| n.to_ascii_lowercase() == wl)
            .or_else(|| pool.iter().find(|(n, _)| n.to_ascii_lowercase().ends_with(&wl)))
            .cloned();
        let hit = exact.or_else(|| {
            let found: Vec<&(String, String)> = pool
                .iter()
                .filter(|(n, _)| n.to_ascii_lowercase().contains(&wl))
                .collect();
            if found.len() == 1 {
                Some(found[0].clone())
            } else {
                None
            }
        });
        return match hit {
            Some((_, url)) => Ok(url),
            None => Err(format!(
                "no .moon asset matches '{w}' in {repo} (available: {})",
                pool.iter()
                    .map(|(n, _)| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        };
    }

    if pool.len() == 1 {
        return Ok(pool[0].1.clone());
    }
    ask(repo, &pool, &cfg)
}

fn ask(repo: &str, assets: &[(String, String)], cfg: &Config) -> Res<String> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "{repo} has {} .moon assets ({}); add file=<name> to pick one",
            assets.len(),
            assets
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    println!();
    println!("  {}  Several .moon packages in {repo}:", cfg.icon(ico::PACKAGE));
    println!();
    for (i, (name, _)) in assets.iter().enumerate() {
        println!("  {:>2}  {name}", i + 1);
    }
    println!();

    loop {
        print!("  {} ", cfg.icon(ico::ARROW));
        let _ = std::io::stdout().flush();
        let mut s = String::new();
        if std::io::stdin().read_line(&mut s).is_err() {
            return Err("cancelled".into());
        }
        let pick = s.trim();
        let pick = if pick.is_empty() { "1" } else { pick };
        if let Ok(n) = pick.parse::<usize>() {
            if n >= 1 && n <= assets.len() {
                return Ok(assets[n - 1].1.clone());
            }
        }
        println!("  Pick a number between 1 and {}.", assets.len());
    }
}

pub fn cmd_url(d: &Dirs, cfg: &Config, remove: bool) -> Res<()> {
    d.ensure()?;
    let path = d.desktop.join("moon-url.desktop");

    if remove {
        match fs::remove_file(&path) {
            Ok(()) => println!("{} {} Removed {}", cfg.step(), cfg.icon(ico::TRASH), path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("{} {} No moon:// handler is registered", cfg.step(), cfg.icon(ico::INFO));
                return Ok(());
            }
            Err(e) => return Err(format!("cannot remove {}: {e}", path.display())),
        }
        crate::desktop::update_desktop_db(d);
        return Ok(());
    }

    let exe = env::current_exe().map_err(|e| format!("cannot find the moon binary: {e}"))?;
    write_entry(&path, &exe)?;
    crate::desktop::update_desktop_db(d);

    if let Some(config) = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    {
        let _ = fs::create_dir_all(config);
    }

    let defaulted = Command::new("xdg-mime")
        .args(["default", "moon-url.desktop", "x-scheme-handler/moon"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    println!();
    println!("{} {} Registered moon:// links", cfg.step(), cfg.icon(ico::LINK));
    println!("    {} {}", cfg.icon(ico::FILE), path.display());
    if !defaulted {
        println!(
            "    {} xdg-mime not found, run `xdg-mime default moon-url.desktop x-scheme-handler/moon` once",
            cfg.icon(ico::INFO)
        );
    }
    println!();
    println!("  {} moon://install?url=https://example.com/app.tar.gz", cfg.icon(ico::ARROW));
    println!("  {} moon://github?repo=owner/name", cfg.icon(ico::ARROW));
    println!();
    Ok(())
}

fn write_entry(path: &Path, exe: &Path) -> Res<()> {
    let text = format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName=moon URL\nComment=Install apps from moon:// links\nExec={} url %u\nTerminal=false\nNoDisplay=true\nMimeType=x-scheme-handler/moon;\nCategories=Utility;\n",
        crate::desktop::exec_quote(exe)
    );
    fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn repair(d: &Dirs) {
    let path = d.desktop.join("moon-url.desktop");
    let Ok(text) = fs::read_to_string(&path) else { return };
    if !text.contains("x-scheme-handler/moon") {
        return;
    }
    if !(text.contains(" install %u")
        || text.contains("Exec=moon ")
        || text.contains("Terminal=true"))
    {
        return;
    }
    let Ok(exe) = env::current_exe() else { return };
    let _ = write_entry(&path, &exe);
}

pub fn open(d: &Dirs, uri: &str) -> Res<()> {
    use std::io::IsTerminal;
    let mut opts = InstallOpts {
        source: uri.to_string(),
        ..Default::default()
    };
    if std::io::stdin().is_terminal() {
        return install_app(d, &mut opts);
    }
    if launch_terminal(uri).is_ok() {
        return Ok(());
    }
    install_app(d, &mut opts)
}

fn terminal_prefix(term: &str) -> Vec<&'static str> {
    let base = std::path::Path::new(term)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(term);
    match base {
        "gnome-terminal" => vec!["--"],
        "wezterm" => vec!["start", "--"],
        "xfce4-terminal" => vec!["-x"],
        "xdg-terminal-exec" | "kitty" | "foot" => Vec::new(),
        _ => vec!["-e"],
    }
}

fn launch_terminal(uri: &str) -> Res<()> {
    let exe = env::current_exe().map_err(|e| format!("cannot find the moon binary: {e}"))?;
    let mut terms: Vec<String> = Vec::new();
    if let Ok(t) = env::var("TERMINAL") {
        if !t.trim().is_empty() {
            terms.push(t);
        }
    }
    terms.extend(TERMINALS.iter().map(|t| t.to_string()));

    for term in terms {
        if start_in_terminal(&term, &exe, uri) {
            return Ok(());
        }
    }
    Err("no terminal emulator found".into())
}

fn start_in_terminal(term: &str, exe: &Path, uri: &str) -> bool {
    let script = "\"$1\" install \"$2\"; status=$?; printf '\\nPress Enter to close... '; read -r _; exit $status";
    let child = Command::new(term)
        .args(terminal_prefix(term))
        .arg("sh")
        .arg("-c")
        .arg(script)
        .arg("moon")
        .arg(exe)
        .arg(uri)
        .spawn();
    let Ok(mut child) = child else { return false };
    for _ in 0..10 {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {}
            Err(_) => return false,
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    true
}

fn release_assets(json: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(at) = json.find("\"assets\"") else { return out };
    let Some(open) = json[at..].find('[').map(|i| at + i) else { return out };
    let b = json.as_bytes();
    let mut i = open + 1;
    let mut depth = 0i32;
    while i < b.len() {
        match b[i] {
            b'"' => i = skip_str(b, i),
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                i += 1;
            }
            b'{' => {
                let (obj, next) = object_at(json, i);
                let name = top_field(obj, "name").unwrap_or("");
                let url = top_field(obj, "browser_download_url").unwrap_or("");
                if !name.is_empty() && !url.is_empty() {
                    out.push((name.to_string(), url.to_string()));
                }
                i = next;
            }
            _ => i += 1,
        }
    }
    out
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

fn object_at(s: &str, start: usize) -> (&str, usize) {
    let b = s.as_bytes();
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
                    return (&s[start..=i], i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    (&s[start..], b.len())
}

fn top_field<'a>(obj: &'a str, key: &str) -> Option<&'a str> {
    let b = obj.as_bytes();
    let mut i = 0usize;
    let mut depth = 0i32;
    while i < b.len() {
        match b[i] {
            b'"' => {
                let (txt, next) = read_str(obj, i);
                i = next;
                if depth == 1 {
                    let mut j = i;
                    while j < b.len() && b[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if j < b.len() && b[j] == b':' && txt == key {
                        let mut k = j + 1;
                        while k < b.len() && b[k].is_ascii_whitespace() {
                            k += 1;
                        }
                        if k < b.len() && b[k] == b'"' {
                            let (v, _) = read_str(obj, k);
                            return Some(v);
                        }
                        return None;
                    }
                }
            }
            b'{' | b'[' => {
                depth += 1;
                i += 1;
            }
            b'}' | b']' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    break;
                }
            }
            _ => i += 1,
        }
    }
    None
}
