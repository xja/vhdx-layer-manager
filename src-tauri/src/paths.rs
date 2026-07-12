use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::error::{AppError, Result};

#[derive(Debug, Clone)]
pub struct AppPaths {
    root: PathBuf,
}

impl AppPaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn base_dir(&self) -> PathBuf {
        self.root.join("disks")
    }

    // 必须要跟 base 目录相同，否则引导的时候会报错找不到 \Windows\System32\winload.efi
    pub fn diff_dir(&self) -> PathBuf {
        self.root.join("disks")
    }

    pub fn meta_dir(&self) -> PathBuf {
        self.root.join("meta")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.meta_dir().join("tmp")
    }

    pub fn locales_dir(&self) -> PathBuf {
        self.meta_dir().join("locales")
    }

    pub fn mount_root(&self) -> PathBuf {
        self.meta_dir().join("mnt")
    }

    pub fn vms_dir(&self) -> PathBuf {
        self.root.join("vms")
    }

    pub fn state_db_path(&self) -> PathBuf {
        self.meta_dir().join("state.db")
    }

    pub fn ops_log_path(&self) -> PathBuf {
        self.meta_dir().join("ops.log")
    }

    /// Ensure the expected directory layout exists.
    pub fn ensure_layout(&self) -> Result<()> {
        for dir in [
            self.root(),
            self.base_dir().as_path(),
            self.diff_dir().as_path(),
            self.meta_dir().as_path(),
            self.tmp_dir().as_path(),
            self.locales_dir().as_path(),
            self.mount_root().as_path(),
            self.vms_dir().as_path(),
        ] {
            fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

/// Reject empty paths and Windows drive roots such as `E:\`.
pub fn validate_workspace_root(root: &Path) -> Result<()> {
    let raw = root.to_string_lossy();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::Message("workspace root path is empty".into()));
    }

    let normalized = normalize_workspace_path(trimmed);
    if is_windows_drive_root(&normalized) {
        return Err(AppError::Message(
            "workspace root cannot be a drive root (for example E:\\); choose a normal folder such as E:\\test-vhdx"
                .into(),
        ));
    }

    Ok(())
}

fn normalize_workspace_path(path: &str) -> String {
    let mut normalized = path.trim().replace('/', "\\");
    if let Some(stripped) = normalized.strip_prefix(r"\\?\") {
        normalized = stripped.to_string();
    }
    while normalized.ends_with('\\') && normalized.len() > 3 {
        // Keep drive roots like E:\ intact for detection after trailing-slash trim below.
        normalized.pop();
    }
    normalized.trim_end_matches('\\').to_string()
}

fn is_windows_drive_root(path: &str) -> bool {
    let bytes = path.as_bytes();
    matches!(
        bytes,
        [drive, b':'] if drive.is_ascii_alphabetic()
    ) || matches!(
        bytes,
        [drive, b':', b'\\'] if drive.is_ascii_alphabetic()
    )
}

#[cfg(test)]
mod tests {
    use super::{is_windows_drive_root, normalize_workspace_path, validate_workspace_root};
    use std::path::Path;

    #[test]
    fn rejects_drive_roots() {
        for sample in [r"E:\", r"E:/", r"e:", r"\\?\E:\", r"D:\"] {
            assert!(
                validate_workspace_root(Path::new(sample)).is_err(),
                "should reject {sample}"
            );
        }
    }

    #[test]
    fn accepts_normal_folders() {
        for sample in [r"E:\test-vhdx", r"E:\backup", r"D:\Programs\vhdx-manager", r"\\?\E:\test-vhdx\"] {
            assert!(
                validate_workspace_root(Path::new(sample)).is_ok(),
                "should accept {sample}"
            );
        }
    }

    #[test]
    fn normalizes_trailing_slashes_for_detection() {
        assert!(is_windows_drive_root(&normalize_workspace_path(r"E:\")));
        assert!(is_windows_drive_root(&normalize_workspace_path(r"E:/")));
        assert!(!is_windows_drive_root(&normalize_workspace_path(
            r"E:\test-vhdx\"
        )));
    }
}
