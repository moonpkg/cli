use crate::util::Res;

pub const ARCHIVE_EXTS: &[&str] = &[
    ".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst", ".tar.lz", ".tar.lzma", ".tar.z", ".tgz",
    ".txz", ".tbz2", ".tbz", ".tzst", ".tar", ".zip", ".7z", ".appimage",
];

const NAME_EXTS: &[&str] = &[
    ".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst", ".tar.lz", ".tar.lzma", ".tar.z", ".tgz",
    ".txz", ".tbz2", ".tbz", ".tzst", ".tar", ".zip", ".7z", ".appimage", ".deb", ".moon",
];

const NOISE_TOKENS: &[&str] = &[
    "linux", "linux64", "x86", "x86_64", "x64", "amd64", "aarch64", "arm64", "arm", "gnu", "musl",
    "static", "portable", "bin", "release", "unknown", "pc", "generic", "glibc", "64bit", "64",
    "i386", "i686",
];

fn strip_ext(f: &str) -> String {
    let l = f.to_ascii_lowercase();
    for e in NAME_EXTS {
        if l.ends_with(e) {
            return f[..f.len() - e.len()].to_string();
        }
    }
    f.to_string()
}

pub fn guess_name(filename: &str) -> String {
    let mut keep: Vec<String> = Vec::new();
    for t in name_words(filename) {
        let tl = t.to_ascii_lowercase();
        let mut ch = tl.chars();
        let first = ch.next();
        let is_version = first.map_or(false, |c| c.is_ascii_digit())
            || (first == Some('v') && ch.next().map_or(false, |c| c.is_ascii_digit()));
        if is_version || is_noise(&tl) {
            break;
        }
        keep.push(t);
    }
    if keep.is_empty() {
        strip_ext(filename)
    } else {
        keep.join("-")
    }
}

const PLATFORM_TOKENS: &[&str] = &[
    "linux", "linux64", "unix", "gnu", "musl", "android", "freebsd",
    "windows", "win", "win32", "win64", "macos", "osx", "darwin",
    "x86", "x86_64", "x8664", "amd64", "x64", "i386", "i486", "i586", "i686", "ia32",
    "arm", "arm64", "armhf", "armv7", "armv7l", "aarch64",
    "mips", "mips64", "ppc", "ppc64", "ppc64el", "riscv", "riscv64", "s390x",
    "32", "64", "32bit", "64bit",
];

pub fn is_noise(t: &str) -> bool {
    NOISE_TOKENS.contains(&t) || PLATFORM_TOKENS.contains(&t)
}

pub fn name_words(filename: &str) -> Vec<String> {
    strip_ext(filename).split(['-', '_', '+']).filter(|t| !t.is_empty()).map(String::from).collect()
}

pub fn guess_version(filename: &str) -> Option<String> {
    for t in name_words(filename) {
        let mut ch = t.chars();
        let first = ch.next();
        let starts_num = first.map_or(false, |c| c.is_ascii_digit())
            || (first == Some('v') && ch.next().map_or(false, |c| c.is_ascii_digit()));
        if starts_num {
            let v = t.trim_start_matches(['v', 'V']);
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

pub fn guess_arch(filename: &str) -> Option<&'static str> {
    let w = name_words(filename);
    for pair in w.windows(2) {
        if pair[0] == "x86" && pair[1] == "64" {
            return Some("x86_64");
        }
    }
    w.iter().find_map(|t| match t.as_str() {
        "x86_64" | "x8664" | "amd64" | "x64" => Some("x86_64"),
        "i386" | "i486" | "i586" | "i686" | "ia32" => Some("i686"),
        "aarch64" | "arm64" => Some("aarch64"),
        "arm" | "armhf" | "armv7" | "armv7l" => Some("armv7"),
        "riscv64" => Some("riscv64"),
        "ppc64" | "ppc64el" => Some("ppc64"),
        "mips" | "mips64" => Some("mips"),
        _ => None,
    })
}

pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let tok = |s: &str| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut digits = false;
        for c in s.chars() {
            if c.is_ascii_digit() != digits {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                digits = c.is_ascii_digit();
            }
            cur.push(c);
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    };
    let (ta, tb) = (tok(a), tok(b));
    for (x, y) in ta.iter().zip(tb.iter()) {
        let (nx, ny) = (x.parse::<u64>().ok(), y.parse::<u64>().ok());
        let ord = match (nx, ny) {
            (Some(i), Some(j)) => i.cmp(&j),
            _ => x.cmp(y),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    ta.len().cmp(&tb.len())
}

pub fn sanitize(s: &str) -> Res<String> {
    let out: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' })
        .collect();
    let out = out.trim_matches(|c| c == '-' || c == '.').to_string();
    if out.is_empty() {
        Err(format!("cannot derive a valid app name from '{s}', use --name"))
    } else {
        Ok(out)
    }
}

pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
