use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn exec_quote(p: &Path) -> String {
    let s = p.to_string_lossy();
    if s.chars().any(|c| " \t\n\"'\\><~|&;$*?#()`%".contains(c)) {
        let e = s
            .replace('\\', "\\\\\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$")
            .replace('%', "%%");
        format!("\"{e}\"")
    } else {
        s.into_owned()
    }
}

fn main() {
    if env::var("PROFILE").ok().as_deref() != Some("release") {
        return;
    }

    let Some(apps) = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .map(|d| d.join("applications"))
    else {
        return;
    };
    if fs::create_dir_all(&apps).is_err() {
        return;
    }

    let path = apps.join("moon-url.desktop");
    let stale = match fs::read_to_string(&path) {
        Err(_) => true,
        Ok(t) => {
            t.contains("x-scheme-handler/moon")
                && (t.contains(" install %u")
                    || t.contains("Exec=moon ")
                    || t.contains("Terminal=true"))
        }
    };
    if !stale {
        return;
    }

    let exe = env::var_os("CARGO_INSTALL_ROOT")
        .map(PathBuf::from)
        .map(|r| r.join("bin/moon"))
        .or_else(|| env::var_os("CARGO_HOME").map(|c| PathBuf::from(c).join("bin/moon")))
        .unwrap_or_else(|| PathBuf::from("moon"));
    let text = format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName=moon URL\nComment=Install apps from moon:// links\nExec={} url %u\nTerminal=false\nNoDisplay=true\nMimeType=x-scheme-handler/moon;\nCategories=Utility;\n",
        exec_quote(&exe)
    );
    if fs::write(&path, text).is_err() {
        return;
    }

    if let Some(config) = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    {
        let _ = fs::create_dir_all(config);
    }

    let _ = Command::new("update-desktop-database").arg(&apps).status();
    let _ = Command::new("xdg-mime")
        .args(["default", "moon-url.desktop", "x-scheme-handler/moon"])
        .status();
}
