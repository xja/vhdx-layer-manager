use std::io::Read;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::error::Result;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

const VHDX_METADATA_READ_LIMIT: usize = 8 * 1024 * 1024;
const LOCATOR_KEYS: [&str; 5] = [
    "absolute_win32_path",
    "relative_path",
    "volume_path",
    "parent_linkage2",
    "parent_linkage",
];

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ParentLocator {
    absolute_win32_path: Option<String>,
    relative_path: Option<String>,
    volume_path: Option<String>,
}

pub fn inspect_vhd_parent_path(vhd_path: &Path) -> Result<Option<String>> {
    // Differencing VHDX files persist parent locators near the start of the file.
    let text = read_vhdx_locator_text(vhd_path)?;
    let locator = parse_parent_locator(&text);
    Ok(locator.resolve(vhd_path))
}

fn read_vhdx_locator_text(vhd_path: &Path) -> Result<String> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(vhd_path)?;
    let mut buf = vec![0u8; VHDX_METADATA_READ_LIMIT];
    let read = file.read(&mut buf)?;
    buf.truncate(read - (read % 2));
    Ok(String::from_utf16_lossy(
        &buf.chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>(),
    ))
}

fn parse_parent_locator(text: &str) -> ParentLocator {
    ParentLocator {
        absolute_win32_path: extract_locator_value(text, "absolute_win32_path"),
        relative_path: extract_locator_value(text, "relative_path"),
        volume_path: extract_locator_value(text, "volume_path"),
    }
}

fn extract_locator_value(text: &str, key: &str) -> Option<String> {
    let start = text.find(key)? + key.len();
    let end = LOCATOR_KEYS
        .iter()
        .filter_map(|candidate| text[start..].find(candidate).map(|idx| start + idx))
        .min()
        .unwrap_or(text.len());
    let raw = text[start..end].trim_matches('\0');
    sanitize_locator_value(raw)
}

fn sanitize_locator_value(raw: &str) -> Option<String> {
    let trimmed = raw.trim_matches(|ch: char| ch.is_control() || ch.is_whitespace());
    let path_like = trimmed
        .trim_start_matches(|ch: char| !matches!(ch, '.' | '\\') && !ch.is_ascii_alphabetic());
    let path_like = path_like.trim_matches(|ch: char| ch.is_control() || ch.is_whitespace());
    (!path_like.is_empty()).then(|| path_like.replace('/', "\\"))
}

impl ParentLocator {
    fn resolve(&self, child_path: &Path) -> Option<String> {
        let mut candidates = Vec::new();
        if let Some(relative) = self.relative_path.as_deref() {
            if let Some(resolved) = resolve_relative_parent(child_path, relative) {
                candidates.push(resolved);
            }
        }
        if let Some(absolute) = self.absolute_win32_path.clone() {
            candidates.push(PathBuf::from(absolute));
        }
        if let Some(volume) = self.volume_path.clone() {
            candidates.push(PathBuf::from(volume));
        }

        candidates
            .iter()
            .find(|candidate| candidate.exists())
            .or_else(|| candidates.first())
            .map(|path| path.to_string_lossy().to_string())
    }
}

fn resolve_relative_parent(child_path: &Path, relative_path: &str) -> Option<PathBuf> {
    let base = child_path.parent()?;
    let relative = relative_path
        .strip_prefix(".\\")
        .or_else(|| relative_path.strip_prefix("./"))
        .unwrap_or(relative_path);
    Some(base.join(relative))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::parse_parent_locator;

    #[test]
    fn prefers_relative_parent_when_absolute_is_stale() {
        let locator = parse_parent_locator(
            "parent_linkage{guid}absolute_win32_pathE:\\disks\\0002-base.vhdxrelative_path.\\0002-base.vhdxvolume_path\\\\?\\Volume{guid}\\disks\\0002-base.vhdxparent_linkage2{00000000-0000-0000-0000-000000000000}",
        );

        let resolved = locator.resolve(Path::new(r"E:\backup\disks\0003-init.vhdx"));
        assert_eq!(
            resolved.as_deref(),
            Some(r"E:\backup\disks\0002-base.vhdx")
        );
    }

    #[test]
    fn falls_back_to_absolute_parent_without_relative_path() {
        let locator = parse_parent_locator(
            "parent_linkage{guid}absolute_win32_pathE:\\workspace\\disks\\0002-base.vhdxparent_linkage2{00000000-0000-0000-0000-000000000000}",
        );

        let resolved = locator.resolve(Path::new(r"E:\workspace\disks\0003-child.vhdx"));
        assert_eq!(
            resolved.as_deref(),
            Some(r"E:\workspace\disks\0002-base.vhdx")
        );
    }

    #[test]
    fn returns_none_when_no_parent_locator_exists() {
        let locator = parse_parent_locator("random text without locator");
        let resolved = locator.resolve(Path::new(r"E:\workspace\disks\0002-base.vhdx"));
        assert_eq!(resolved, None);
    }
}
