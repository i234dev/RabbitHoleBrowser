//! Platform font directory scanning. Only the table directory and the
//! `name`/`OS/2`/`head` tables are read for each file.

use super::sfnt::{face_offset, parse_info_with, rd16, rd32, table_directory, TableRecord};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SystemFace {
    pub path: PathBuf,
    pub index: u32,
    pub family: String,
    pub full_name: String,
    pub weight: u16,
    pub italic: bool,
}

/// Font directories for the current platform.
pub fn font_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    if cfg!(target_os = "windows") {
        if let Some(w) = std::env::var_os("WINDIR") {
            dirs.push(PathBuf::from(w).join("Fonts"));
        } else {
            dirs.push(PathBuf::from("C:\\Windows\\Fonts"));
        }
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(l).join("Microsoft").join("Windows").join("Fonts"));
        }
    } else if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        dirs.push(PathBuf::from("/Library/Fonts"));
        if let Some(h) = &home {
            dirs.push(h.join("Library").join("Fonts"));
        }
    } else {
        dirs.push(PathBuf::from("/usr/share/fonts"));
        dirs.push(PathBuf::from("/usr/local/share/fonts"));
        if let Some(h) = &home {
            dirs.push(h.join(".fonts"));
            dirs.push(h.join(".local").join("share").join("fonts"));
        }
    }
    dirs
}

/// Scan all platform directories.
pub fn scan_system_fonts() -> Vec<SystemFace> {
    let mut out = Vec::new();
    for d in font_directories() {
        scan_dir(&d, 0, &mut out);
    }
    out
}

fn scan_dir(dir: &Path, depth: usize, out: &mut Vec<SystemFace>) {
    if depth > 6 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir(&path, depth + 1, out);
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default();
        if !matches!(ext.as_str(), "ttf" | "otf" | "ttc" | "otc") {
            continue;
        }
        out.extend(read_faces(&path));
    }
}

/// Read the face descriptors of a font file without loading glyph data.
pub fn read_faces(path: &Path) -> Vec<SystemFace> {
    let mut out = Vec::new();
    let Ok(mut f) = File::open(path) else { return out };
    let mut head = vec![0u8; 4096];
    let n = match f.read(&mut head) {
        Ok(n) => n,
        Err(_) => return out,
    };
    head.truncate(n);
    if head.len() < 12 {
        return out;
    }
    let face_count = if &head[0..4] == b"ttcf" { rd32(&head, 8).unwrap_or(0).min(64) } else { 1 };
    for index in 0..face_count {
        let Some(base) = face_offset(&head, index) else { continue };
        // Directory may lie beyond the first 4 KiB in collections.
        let dir: Option<Vec<TableRecord>> = if base + 12 <= head.len() {
            let n = rd16(&head, base + 4).unwrap_or(0) as usize;
            if base + 12 + n * 16 <= head.len() {
                table_directory(&head, base)
            } else {
                read_range(&mut f, base as u64, 12 + n * 16).and_then(|d| table_directory(&d, 0))
            }
        } else {
            read_range(&mut f, base as u64, 12).and_then(|d| {
                let n = rd16(&d, 4)? as usize;
                read_range(&mut f, base as u64, 12 + n * 16).and_then(|d| table_directory(&d, 0))
            })
        };
        let Some(dir) = dir else { continue };
        let info = parse_info_with(&dir, |off, len| {
            let end = off as usize + len as usize;
            if end <= head.len() {
                Some(head[off as usize..end].to_vec())
            } else {
                read_range(&mut f, off as u64, len as usize)
            }
        });
        if let Some(i) = info {
            out.push(SystemFace { path: path.to_path_buf(), index, family: i.family, full_name: i.full_name, weight: i.weight, italic: i.italic });
        }
    }
    out
}

fn read_range(f: &mut File, off: u64, len: usize) -> Option<Vec<u8>> {
    if len > 16 * 1024 * 1024 {
        return None;
    }
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}
