use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::util::Res;

const AR_MAGIC: &[u8] = b"!<arch>\n";

pub const DEB_SCRIPTS: &[&str] = &["preinst", "postinst", "prerm", "postrm", "config"];

#[derive(Clone)]
pub struct ArMember {
    pub name: String,
    pub off: u64,
    size: u64,
}

fn is_ar(p: &Path) -> bool {
    let mut b = [0u8; 8];
    fs::File::open(p).and_then(|mut f| f.read_exact(&mut b)).is_ok() && &b[..] == AR_MAGIC
}

pub fn is_deb(p: &Path, fname: &str) -> bool {
    fname.to_ascii_lowercase().ends_with(".deb") || is_ar(p)
}

pub fn ar_members(f: &mut fs::File) -> Res<Vec<ArMember>> {
    let mut magic = [0u8; 8];
    f.read_exact(&mut magic).map_err(|_| "not an ar archive (short file)".to_string())?;
    if &magic[..] != AR_MAGIC {
        return Err("not an ar archive (bad magic)".into());
    }
    let mut out = Vec::new();
    let mut pos: u64 = 8;
    loop {
        let mut h = [0u8; 60];
        if f.read_exact(&mut h).is_err() {
            break;
        }
        let name = String::from_utf8_lossy(&h[0..16]).trim_end().trim_end_matches('/').to_string();
        let size = String::from_utf8_lossy(&h[48..58]).trim().parse::<u64>().unwrap_or(0);
        if !name.is_empty() && !name.starts_with('/') {
            out.push(ArMember { name, off: pos + 60, size });
        }
        pos += 60 + size + (size % 2);
        f.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

pub fn ar_member_to(f: &mut fs::File, m: &ArMember, out: &Path) -> Res<()> {
    let mut r = f.try_clone().map_err(|e| e.to_string())?;
    r.seek(SeekFrom::Start(m.off)).map_err(|e| e.to_string())?;
    let mut w = fs::File::create(out).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    std::io::copy(&mut r.take(m.size), &mut w).map_err(|e| e.to_string())?;
    Ok(())
}
