use std::ffi::OsStr;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{Config, ico};
use crate::paths::Dirs;
use crate::probe::try_run;
use crate::util::{Res, confirm};

const PACKED_INSTALL_SH: &str = include_str!("../docs/scripts/install.sh");
const INSTALL_SH_URL: &str = "https://moonpkg.github.io/cli/scripts/install.sh";
const RELEASES_API: &str = "https://api.github.com/repos/moonpkg/cli/releases/latest";

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn cmd_check(cfg: &Config) -> Res<()> {
    println!("{} moon {VERSION}", cfg.icon(ico::MOON));
    match latest_release() {
        Ok(Some(tag)) if newer_than(VERSION, &tag) => {
            println!("  {} update available: {tag}", cfg.icon(ico::UP));
        }
        Ok(Some(tag)) => println!("  {} up to date ({tag})", cfg.icon(ico::CHECK)),
        Ok(None) => println!("  {} GitHub did not answer, cannot tell", cfg.icon(ico::WARN)),
        Err(e) => println!("  {} {e}", cfg.icon(ico::WARN)),
    }
    println!("  {} to update: open `moon config` and pick 2", cfg.icon(ico::BULLET));
    Ok(())
}

pub fn cmd_update(d: &Dirs, cfg: &Config, script: Option<&str>, assume_yes: bool) -> Res<()> {
    use std::os::unix::process::CommandExt;

    d.ensure()?;
    let src = script.unwrap_or(INSTALL_SH_URL);

    let work = d.tmp.join(format!("update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| format!("cannot create {}: {e}", work.display()))?;

    println!("{} Updating moon {VERSION}", cfg.icon(ico::REFRESH));
    let (path, origin) = fetch(src, &work, cfg)?;
    println!("  {} installer {} ({src})", cfg.icon(ico::CHECK), origin);

    let latest = match latest_release() {
        Ok(Some(tag)) if newer_than(VERSION, &tag) => {
            println!("  {} {} is out there", cfg.icon(ico::UP), tag);
            Some(tag)
        }
        Ok(Some(tag)) => {
            println!("  {} {VERSION} is the newest one", cfg.icon(ico::CHECK));
            Some(tag)
        }
        _ => None,
    };
    let target = latest.clone().unwrap_or_else(|| "latest".to_string());

    if !confirm(&format!("{} Install {target} over moon {VERSION}?", cfg.icon(ico::MAGIC)), assume_yes) {
        println!("{} Cancelled.", cfg.icon(ico::CROSS));
        let _ = fs::remove_dir_all(&work);
        return Ok(());
    }

    println!("\n{} moon closes now, the installer takes over.", cfg.icon(ico::DOWNLOAD));
    let _ = std::io::stdout().flush();

    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(format!(
            "trap 'rm -rf -- {}' EXIT; bash {}",
            sh_quote(&work.to_string_lossy()),
            sh_quote(&path.to_string_lossy())
        ));
    if let Some(tag) = latest {
        cmd.env("MOON_VERSION", tag);
    }
    let e = cmd.exec();
    let _ = fs::remove_dir_all(&work);
    Err(format!("cannot hand over to bash: {e}"))
}

pub fn latest_release() -> Res<Option<String>> {
    let out = match Command::new("curl").args(["-fsSL", RELEASES_API]).output() {
        Ok(o) if o.status.success() => o,
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            match Command::new("wget").args(["-qO-", RELEASES_API]).output() {
                Ok(o) if o.status.success() => o,
                _ => return Ok(None),
            }
        }
        Err(_) => return Ok(None),
    };
    Ok(tag_name(&String::from_utf8_lossy(&out.stdout)))
}

fn tag_name(json: &str) -> Option<String> {
    let key = "\"tag_name\"";
    let rest = json.get(json.find(key)? + key.len()..)?;
    let start = rest.find('"')? + 1;
    let end = rest.get(start..)?.find('"')? + start;
    let tag = rest.get(start..end)?.trim().to_string();
    (!tag.is_empty()).then_some(tag)
}

pub fn newer_than(current: &str, candidate: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim()
            .trim_start_matches(['v', 'V'])
            .split(|c: char| !c.is_ascii_digit())
            .take_while(|p| !p.is_empty())
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (cur, new) = (parts(current), parts(candidate));
    if new.is_empty() {
        return false;
    }
    for i in 0..new.len().max(cur.len()) {
        if new.get(i).copied().unwrap_or(0) != cur.get(i).copied().unwrap_or(0) {
            return new.get(i).copied().unwrap_or(0) > cur.get(i).copied().unwrap_or(0);
        }
    }
    false
}

fn fetch(src: &str, work: &Path, cfg: &Config) -> Res<(PathBuf, &'static str)> {
    let path = work.join("install.sh");
    let is_url = src.starts_with("http://") || src.starts_with("https://");
    if is_url {
        println!("{} {} Downloading {src}", cfg.step(), cfg.icon(ico::DOWNLOAD));
        let os = OsStr::new;
        let got = try_run(
            "curl",
            &[os("-fsSL"), os(src), os("-o"), path.as_os_str()],
        )
        .unwrap_or(false)
            || try_run(
                "wget",
                &[os("-q"), os(src), os("-O"), path.as_os_str()],
            )
            .unwrap_or(false);
        if got && fs::metadata(&path).is_ok_and(|m| m.len() > 0) {
            return Ok((path, "downloaded"));
        }
        println!(
            "  {} {} unreachable, using the copy packed into moon",
            cfg.icon(ico::WARN),
            src
        );
        fs::write(&path, PACKED_INSTALL_SH).map_err(|e| format!("cannot write the packed installer: {e}"))?;
        return Ok((path, "packed copy"));
    }

    fs::copy(src, &path).map_err(|e| format!("cannot read {src}: {e}"))?;
    Ok((path, "local file"))
}

fn sh_quote(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}