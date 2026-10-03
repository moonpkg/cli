use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::{Config, ico};
use crate::paths::Dirs;
use crate::state::{manifest_path, now_secs};
use crate::util::{Res, fit_name};

struct InstallRec {
    when: u64,
    name: String,
    source: String,
}

fn history_path(d: &Dirs) -> PathBuf {
    d.apps.parent().unwrap_or(&d.apps).join("history")
}

pub fn push_history(d: &Dirs, name: &str, source: &str) {
    if let Some(p) = history_path(d).parent() {
        let _ = fs::create_dir_all(p);
    }
    let now = now_secs();

    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(history_path(d)) {
        use std::io::Write;
        let _ = writeln!(f, "{}\t{}\t{}", now, clean(name), clean(source));
    }
}

fn read_history(d: &Dirs) -> Vec<InstallRec> {
    let Ok(t) = fs::read_to_string(history_path(d)) else { return Vec::new() };
    let mut out: Vec<InstallRec> = t
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split('\t');
            InstallRec {
                when: it.next().unwrap_or("").parse().unwrap_or(0),
                name: it.next().unwrap_or("").to_string(),
                source: it.next().unwrap_or("").to_string(),
            }
        })
        .collect();
    out.reverse();
    out
}

fn local_stamps(stamps: &[u64]) -> Vec<(i64, u32, u32, u32, u32)> {
    let mut input = String::new();
    for s in stamps {
        input.push_str(&format!("{s}\n"));
    }
    let ok = Command::new("date")
        .args(["-f", "-", "+%Y|%m|%d|%H|%M"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut ch| {
            ch.stdin.take().unwrap().write_all(input.as_bytes())?;
            let out = ch.wait_with_output()?;
            if !out.status.success() {
                return Err(std::io::Error::other("date failed"));
            }
            let v: Vec<(i64, u32, u32, u32, u32)> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| {
                    let n: Vec<i64> = l.split('|').filter_map(|x| x.trim().parse().ok()).collect();
                    (n.len() == 5).then(|| (n[0], n[1] as u32, n[2] as u32, n[3] as u32, n[4] as u32))
                })
                .collect();
            if v.len() == stamps.len() {
                Ok(v)
            } else {
                Err(std::io::Error::other("unexpected date output"))
            }
        });
    if let Ok(v) = ok {
        return v;
    }

    let off = local_offset();
    stamps
        .iter()
        .map(|s| {
            let t = (*s as i64 + off) as i64;
            let days = t.div_euclid(86_400);
            let secs = t.rem_euclid(86_400);
            let (y, m, dd) = civil_from_days(days);
            (y, m, dd, (secs / 3600) as u32, ((secs % 3600) / 60) as u32)
        })
        .collect()
}

fn local_offset() -> i64 {
    Command::new("date")
        .arg("+%z")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .and_then(|s| {
            let (sign, rest) = s.split_at(1);
            let (h, m) = rest.split_at(2);
            let v = h.parse::<i64>().ok()? * 3600 + m.parse::<i64>().ok()? * 60;
            Some(if sign == "-" { -v } else { v })
        })
        .unwrap_or(0)
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn weekday_ymd(y: i64, m: u32, d: u32) -> String {
    let days = days_from_civil(y, m, d);

    let wd = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][(((days % 7) + 7 + 3) % 7) as usize];
    format!("{wd} {y:04}-{m:02}-{d:02}")
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

pub fn cmd_history(d: &Dirs, cfg: &Config, limit: Option<usize>, clear: bool) -> Res<()> {
    if clear {
        let _ = fs::remove_file(history_path(d));
        println!("{} history cleared", cfg.icon(ico::TRASH));
        return Ok(());
    }
    let mut recs = read_history(d);
    if let Some(n) = limit {
        recs.truncate(n);
    }
    if recs.is_empty() {
        println!("{} No installs recorded yet.", cfg.icon(ico::CLOCK));
        println!("  {} moon install anything and it shows up here", cfg.icon(ico::BULLET));
        return Ok(());
    }
    let stamps: Vec<u64> = recs.iter().map(|r| r.when).collect();
    let times = local_stamps(&stamps);
    let today = local_stamps(&[now_secs()]).pop().unwrap_or((1970, 1, 1, 0, 0));

    let files: Vec<String> = recs
        .iter()
        .map(|r| {
            Path::new(&r.source).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| r.source.clone())
        })
        .collect();
    let name_w = files.iter().map(|f| f.chars().count()).max().unwrap_or(0).min(40);

    println!();
    let mut last_day: Option<(i64, u32, u32)> = None;
    for ((r, t), file) in recs.iter().zip(times.iter()).zip(files.iter()) {
        let day = (t.0, t.1, t.2);
        if last_day != Some(day) {
            if last_day.is_some() {
                println!();
            }
            let header = if day == (today.0, today.1, today.2) {
                "Today".to_string()
            } else if today.2 > 1 && day == (today.0, today.1, today.2 - 1) {
                "Yesterday".to_string()
            } else {
                weekday_ymd(t.0, t.1, t.2)
            };
            println!("{header}");
            last_day = Some(day);
        }
        let mut line = format!("  {:02}:{:02}  {}", t.3, t.4, fit_name(file, name_w));

        let stem = Path::new(&r.source).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if !r.name.is_empty() && !r.name.eq_ignore_ascii_case(&stem) {
            line.push_str(&format!("  ({})", r.name));
        }
        if !manifest_path(d, &r.name).exists() {
            line.push_str(&format!("  {} removed later", cfg.icon(ico::WARN)));
        }
        println!("{line}");
    }
    println!();
    println!(
        "  {} {} install(s) recorded  {}",
        cfg.icon(ico::BULLET),
        recs.len(),
        format!("({})", history_path(d).display())
    );
    println!();
    Ok(())
}
