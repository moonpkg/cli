use std::env;
use std::fs;
use std::path::PathBuf;

use crate::config::{Config, ico};
use crate::util::Res;

#[derive(Clone)]
pub struct Dirs {
    pub local: PathBuf,
    pub apps: PathBuf,
    pub manifests: PathBuf,
    pub tmp: PathBuf,
    pub bin: PathBuf,
    pub desktop: PathBuf,
}

impl Dirs {
    pub fn new() -> Res<Self> {
        let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?);
        let data = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local/share"));
        let moon = data.join("moon");
        let local = home.join(".local");
        Ok(Dirs {
            bin: local.join("bin"),
            local,
            apps: moon.join("apps"),
            manifests: moon.join("manifests"),
            tmp: moon.join("tmp"),
            desktop: data.join("applications"),
        })
    }

    pub fn ensure(&self) -> Res<()> {
        for d in [&self.apps, &self.manifests, &self.tmp, &self.bin, &self.desktop] {
            fs::create_dir_all(d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
        }
        Ok(())
    }
}

pub struct Cleanup(pub PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn warn_path(d: &Dirs) {
    let path = env::var("PATH").unwrap_or_default();
    let bin = d.bin.to_string_lossy().into_owned();
    if !path.split(':').any(|p| p == bin) {
        let cfg = Config::load();
        eprintln!(
            "warning: {} {bin} is not in your PATH. Add to your shell rc:\n    export PATH=\"{bin}:$PATH\"",
            cfg.icon(ico::WARN)
        );
    }
}
