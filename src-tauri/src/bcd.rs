use std::path::Path;

use crate::error::Result;
use crate::sys::{run_elevated_command, CommandOutput};

/// Run bcdboot using the host's default system BCD store (omit /s and /f).
pub fn run_bcdboot(system_dir: &Path) -> Result<CommandOutput> {
    let sys_path = system_dir
        .to_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| system_dir.to_string_lossy().to_string());
    let sys_arg = format!("{sys_path}\\Windows");
    run_elevated_command("bcdboot", &[&sys_arg, "/d"], None)
}

/// Run bcdboot targeting a specific EFI partition while still using UEFI firmware.
pub fn run_bcdboot_to_efi(system_dir: &Path, efi_dir: &Path) -> Result<CommandOutput> {
    let sys_path = system_dir
        .to_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| system_dir.to_string_lossy().to_string());
    let sys_arg = format!("{sys_path}\\Windows");
    let efi_arg = efi_dir
        .to_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| efi_dir.to_string_lossy().to_string());
    run_elevated_command(
        "bcdboot",
        &[&sys_arg, "/s", &efi_arg, "/f", "UEFI", "/d"],
        None,
    )
}

pub fn bcdedit_enum_all() -> Result<CommandOutput> {
    run_elevated_command("bcdedit", &["/enum", "all", "/v"], None)
}

pub fn bcdedit_boot_sequence(guid: &str) -> Result<CommandOutput> {
    run_elevated_command("bcdedit", &["/bootsequence", guid], None)
}

pub fn bcdedit_delete(guid: &str) -> Result<CommandOutput> {
    run_elevated_command("bcdedit", &["/delete", guid], None)
}

pub fn bcdedit_set_description(guid: &str, desc: &str) -> Result<CommandOutput> {
    run_elevated_command("bcdedit", &["/set", guid, "description", desc], None)
}

/// Extract the identifier (GUID) for an entry whose device path references the given VHD path.
pub fn extract_guid_for_vhd(bcd_output: &str, vhd_path: &str) -> Option<String> {
    let needle = normalize_vhd_path(vhd_path);
    bcd_sections(bcd_output).into_iter().find_map(|section| {
        let guid = extract_section_identifier(&section)?;
        section
            .iter()
            .copied()
            .filter_map(parse_vhd_device_path)
            .map(|path| normalize_vhd_path(&path))
            .find(|candidate| candidate == &needle)
            .map(|_| guid)
    })
}

/// Extract identifier whose device/osdevice references a specific partition letter (e.g., "partition=U:").
pub fn extract_guid_for_partition_letter(bcd_output: &str, letter: char) -> Option<String> {
    let needle = format!("partition={}:", letter.to_ascii_lowercase());
    bcd_sections(bcd_output).into_iter().find_map(|section| {
        let guid = extract_section_identifier(&section)?;
        section
            .iter()
            .copied()
            .any(|line| line.to_ascii_lowercase().contains(&needle))
            .then_some(guid)
    })
}

/// Extract raw VHD path from a device/osdevice line; strips trailing ",locate=..." if present.
fn parse_vhd_device_path(line: &str) -> Option<String> {
    let before_comma = line.split_once(',').map(|(h, _)| h).unwrap_or(line);
    let lower_before = before_comma.to_ascii_lowercase();
    let pos = lower_before.find("vhd=")?;
    let path_part = before_comma[pos + 4..].trim();
    let token = path_part.split_whitespace().next().unwrap_or("");
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Normalize VHD paths for comparison: remove brackets, unify separators, drop \\?\ prefix, lowercase.
fn normalize_vhd_path(path: &str) -> String {
    let mut normalized = path.trim().trim_start_matches("\\\\?\\").replace('/', "\\");
    if normalized.starts_with('[') {
        if let Some(end) = normalized.find(']') {
            let drive = &normalized[1..end];
            let rest = &normalized[end + 1..];
            normalized = format!("{drive}{rest}");
        }
    }
    normalized.replace(['[', ']'], "").to_ascii_lowercase()
}

fn bcd_sections(output: &str) -> Vec<Vec<&str>> {
    let mut sections = Vec::new();
    let mut current = Vec::new();
    for line in output.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                sections.push(current);
                current = Vec::new();
            }
            continue;
        }
        current.push(line);
    }
    if !current.is_empty() {
        sections.push(current);
    }
    sections
}

fn extract_section_identifier(section: &[&str]) -> Option<String> {
    section.iter().copied().find_map(find_braced_token)
}

fn find_braced_token(line: &str) -> Option<String> {
    line.split_whitespace().find_map(|token| {
        let trimmed = token.trim_matches(|ch: char| matches!(ch, ',' | ';' | '"' | '\''));
        (trimmed.starts_with('{') && trimmed.ends_with('}') && trimmed.len() > 2)
            .then(|| trimmed.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::{extract_guid_for_partition_letter, extract_guid_for_vhd};

    #[test]
    fn extracts_guid_for_vhd_from_english_output() {
        let output = r#"
Windows Boot Loader
-------------------
identifier              {11111111-1111-1111-1111-111111111111}
device                  vhd=[E:]\workspace\base\0001-base.vhdx,locate=custom:22000002
path                    \Windows\system32\winload.efi

Windows Boot Loader
-------------------
identifier              {22222222-2222-2222-2222-222222222222}
device                  partition=C:
"#;

        assert_eq!(
            extract_guid_for_vhd(output, r"E:\workspace\base\0001-base.vhdx").as_deref(),
            Some("{11111111-1111-1111-1111-111111111111}")
        );
    }

    #[test]
    fn extracts_guid_for_partition_letter_from_localized_output() {
        let output = r#"
Windows Boot Loader
-------------------
标识符                  {33333333-3333-3333-3333-333333333333}
设备                    partition=S:
路径                    \Windows\system32\winload.efi

Windows Boot Loader
-------------------
标识符                  {44444444-4444-4444-4444-444444444444}
设备                    partition=C:
"#;

        assert_eq!(
            extract_guid_for_partition_letter(output, 'S').as_deref(),
            Some("{33333333-3333-3333-3333-333333333333}")
        );
    }

    #[test]
    fn ignores_non_identifier_guids_after_first_guid_in_section() {
        let output = r#"
Windows Boot Loader
-------------------
????                    {55555555-5555-5555-5555-555555555555}
????                    partition=Z:
resumeobject            {66666666-6666-6666-6666-666666666666}
"#;

        assert_eq!(
            extract_guid_for_partition_letter(output, 'Z').as_deref(),
            Some("{55555555-5555-5555-5555-555555555555}")
        );
    }

    #[test]
    fn extracts_guid_from_real_logged_bcd_output() {
        let output = r#"
Windows Boot Manager
--------------------
identifier              {a5a30fa2-3d06-4e9f-b5f4-a01df9d1fcba}
displayorder            {9dea862c-5cdd-4e70-acc1-f32b344d4795}
                        {31331cf8-4e8c-11f0-b258-806e6f6e6963}
timeout                 2

Windows ����������
-------------------
��ʶ��                  {10eeff43-7d2e-11f1-b31c-a8595f5436cb}
device                  vhd=[E:]\workspace\disks\0002-base.vhdx,locate=custom:12000002
path                    \Windows\system32\winload.efi
description             Windows 11
locale                  en-us
inherit                 {6efb52bf-1766-41db-a6b3-0ee5eff72bd7}
isolatedcontext         Yes
allowedinmemorysettings 0x15000075
osdevice                vhd=[E:]\workspace\disks\0002-base.vhdx,locate=custom:22000002
systemroot              \Windows
resumeobject            {10eeff42-7d2e-11f1-b31c-a8595f5436cb}
nx                      OptIn
bootmenupolicy          Standard
"#;

        assert_eq!(
            extract_guid_for_vhd(output, r"E:\workspace\disks\0002-base.vhdx").as_deref(),
            Some("{10eeff43-7d2e-11f1-b31c-a8595f5436cb}")
        );
    }
}
