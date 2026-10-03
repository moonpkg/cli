use std::env;
use std::io::Write;
use std::path::Path;

pub type Res<T> = Result<T, String>;

pub fn human_size(n: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < U.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.1} {}", U[i]) }
}

pub fn wrap_text(s: &str, width: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for w in s.split_whitespace() {
        if !line.is_empty() && vis_len(&line) + 1 + vis_len(w) > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(w);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

pub fn vis_len(s: &str) -> usize {
    let mut n = 0;
    let mut esc = false;
    for c in s.chars() {
        if esc {
            if c.is_ascii_alphabetic() {
                esc = false;
            }
            continue;
        }
        if c == '\x1b' {
            esc = true;
            continue;
        }
        n += 1;
    }
    n
}

pub fn clip_to(s: &str, w: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    let mut esc = false;
    for c in s.chars() {
        if esc {
            out.push(c);
            if c.is_ascii_alphabetic() {
                esc = false;
            }
            continue;
        }
        if c == '\x1b' {
            out.push(c);
            esc = true;
            continue;
        }
        if n >= w {
            break;
        }
        out.push(c);
        n += 1;
    }
    if esc {
        out.push_str("\x1b[0m");
    }
    out
}

pub fn pad_to(s: &str, w: usize) -> String {
    let mut out = s.to_string();
    let l = vis_len(s);
    if l < w {
        out.push_str(&" ".repeat(w - l));
    }
    out
}

pub fn fit_name(name: &str, w: usize) -> String {
    if name.chars().count() <= w {
        return name.to_string();
    }
    if w <= 3 {
        return name.chars().take(w).collect();
    }
    let tail: String = name.chars().skip(name.chars().count() - (w - 3)).collect();
    format!("\u{2026}{tail}")
}

pub fn shorten_path(p: &Path, w: usize) -> String {
    let full = p.display().to_string();
    let home = env::var("HOME").unwrap_or_default();
    let s = if !home.is_empty() {
        full.strip_prefix(&home).map_or(full.clone(), |r| format!("~{r}"))
    } else {
        full
    };
    if s.chars().count() <= w {
        s
    } else {
        let keep: String = s.chars().skip(s.chars().count() - w + 3).collect();
        format!("...{keep}")
    }
}

pub fn ago(secs: u64) -> String {
    let (n, u) = match secs {
        0..=59 => (secs, "s"),
        60..=3599 => (secs / 60, "min"),
        3600..=86_399 => (secs / 3600, "h"),
        _ => (secs / 86_400, "d"),
    };
    format!("{n}{u} ago")
}

pub fn confirm(prompt: &str, assume_yes: bool) -> bool {
    if assume_yes {
        return true;
    }
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        eprintln!("refusing to continue without a terminal; pass --yes");
        return false;
    }
    print!("{prompt} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    if std::io::stdin().read_line(&mut s).is_err() {
        return false;
    }
    matches!(s.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}
