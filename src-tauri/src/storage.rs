use std::path::Path;

use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::sys::{run_elevated_powershell, run_powershell};

const GPT_BASIC_DATA_PARTITION: &str = "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct VhdLayout {
    pub path: String,
    pub attached: bool,
    pub partitions: Vec<VhdPartition>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct VhdPartition {
    pub partition_number: u32,
    /// PowerShell Storage emits this as `Type` (partition type name).
    #[serde(rename = "Type")]
    pub kind: String,
    pub gpt_type: Option<String>,
    pub size_mb: u64,
    pub drive_letter: Option<String>,
    pub file_system: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct VhdAttachmentState {
    attached: bool,
}

pub fn inspect_vhd_layout(vhd_path: &Path) -> Result<VhdLayout> {
    let path = vhd_path.to_string_lossy();
    let script = format!(
        r#"$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Import-Module Storage
$path = '{path}'
$img = Get-DiskImage -ImagePath $path
$mountedHere = -not [bool]$img.Attached
if ($mountedHere) {{
    Mount-DiskImage -ImagePath $path -NoDriveLetter | Out-Null
    Start-Sleep -Milliseconds 300
}}
try {{
    $disk = $null
    for ($i = 0; $i -lt 10 -and -not $disk; $i++) {{
        $disk = Get-Disk | Where-Object {{ $_.Location -eq $path }} | Select-Object -First 1
        if (-not $disk) {{
            Start-Sleep -Milliseconds 200
        }}
    }}
    if (-not $disk) {{ throw "disk not found for $path" }}
    $parts = foreach ($p in ($disk | Get-Partition | Sort-Object PartitionNumber)) {{
        $vol = $null
        try {{ $vol = $p | Get-Volume -ErrorAction Stop }} catch {{}}
        [pscustomobject]@{{
            PartitionNumber = [int]$p.PartitionNumber
            Type = [string]$p.Type
            Kind = [string]$p.Type
            GptType = if ($p.GptType) {{ [string]$p.GptType }} else {{ $null }}
            SizeMB = [UInt64][math]::Round($p.Size / 1MB)
            DriveLetter = if ($p.DriveLetter) {{ [string]$p.DriveLetter }} else {{ $null }}
            FileSystem = if ($vol) {{ [string]$vol.FileSystem }} else {{ $null }}
            Label = if ($vol) {{ [string]$vol.FileSystemLabel }} else {{ $null }}
        }}
    }}
    [pscustomobject]@{{
        Path = $path
        Attached = [bool](Get-DiskImage -ImagePath $path).Attached
        Partitions = @($parts)
    }} | ConvertTo-Json -Compress -Depth 5
}} finally {{
    if ($mountedHere) {{
        Dismount-DiskImage -ImagePath $path | Out-Null
    }}
}}"#,
        path = ps_escape_single(path.as_ref()),
    );
    let output = run_elevated_powershell(&script, None)?;
    if output.exit_code.unwrap_or(-1) != 0 {
        return Err(AppError::Message(format!(
            "inspect_vhd_layout failed: {}",
            compact_command_output(&output)
        )));
    }
    serde_json::from_str(output.stdout.trim()).map_err(Into::into)
}

pub fn is_vhd_attached(vhd_path: &Path) -> Result<bool> {
    let path = vhd_path.to_string_lossy();
    let script = format!(
        r#"$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Import-Module Storage
$path = '{path}'
[pscustomobject]@{{
    Attached = [bool](Get-DiskImage -ImagePath $path).Attached
}} | ConvertTo-Json -Compress"#,
        path = ps_escape_single(path.as_ref()),
    );
    let output = run_powershell(&script, None)?;
    if output.exit_code.unwrap_or(-1) != 0 {
        return Err(AppError::Message(format!(
            "is_vhd_attached failed: {}",
            compact_command_output(&output)
        )));
    }
    let state: VhdAttachmentState = serde_json::from_str(output.stdout.trim())?;
    Ok(state.attached)
}

pub fn find_system_partition_number(layout: &VhdLayout) -> Option<u32> {
    layout
        .partitions
        .iter()
        .find(|part| matches_gpt_type(part.gpt_type.as_deref(), GPT_BASIC_DATA_PARTITION))
        .map(|part| part.partition_number)
        .or_else(|| {
            layout
                .partitions
                .iter()
                .filter(|part| part.kind.eq_ignore_ascii_case("Basic"))
                .max_by_key(|part| part.size_mb)
                .map(|part| part.partition_number)
        })
        .or_else(|| {
            layout
                .partitions
                .iter()
                .max_by_key(|part| part.size_mb)
                .map(|part| part.partition_number)
        })
}

fn matches_gpt_type(candidate: Option<&str>, expected: &str) -> bool {
    candidate
        .map(normalize_guid)
        .map(|guid| guid == expected)
        .unwrap_or(false)
}

fn normalize_guid(guid: &str) -> String {
    guid.trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .to_ascii_lowercase()
}

fn compact_command_output(output: &crate::sys::CommandOutput) -> String {
    let stderr = output.stderr.trim();
    let stdout = output.stdout.trim();
    if !stderr.is_empty() {
        format!("stderr={stderr}")
    } else if !stdout.is_empty() {
        format!("stdout={stdout}")
    } else {
        "no output".into()
    }
}

fn ps_escape_single(input: &str) -> String {
    input.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::{find_system_partition_number, VhdLayout, VhdPartition};
    use serde_json;

    fn sample_layout() -> VhdLayout {
        VhdLayout {
            path: r"E:\workspace\disks\0003-base-child.vhdx".into(),
            attached: true,
            partitions: vec![
                VhdPartition {
                    partition_number: 1,
                    kind: "Reserved".into(),
                    gpt_type: Some("{e3c9e316-0b5c-4db8-817d-f92df00215ae}".into()),
                    size_mb: 16,
                    drive_letter: None,
                    file_system: None,
                    label: None,
                },
                VhdPartition {
                    partition_number: 2,
                    kind: "System".into(),
                    gpt_type: Some("{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}".into()),
                    size_mb: 100,
                    drive_letter: None,
                    file_system: Some("FAT32".into()),
                    label: Some("EFI".into()),
                },
                VhdPartition {
                    partition_number: 3,
                    kind: "Reserved".into(),
                    gpt_type: Some("{e3c9e316-0b5c-4db8-817d-f92df00215ae}".into()),
                    size_mb: 16,
                    drive_letter: None,
                    file_system: None,
                    label: None,
                },
                VhdPartition {
                    partition_number: 4,
                    kind: "Basic".into(),
                    gpt_type: Some("{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}".into()),
                    size_mb: 61295,
                    drive_letter: None,
                    file_system: Some("NTFS".into()),
                    label: Some("System".into()),
                },
            ],
        }
    }

    #[test]
    fn deserializes_powershell_partition_type_field() {
        let json = r#"{
            "Path":"E:\\workspace\\disks\\0002-base.vhdx",
            "Attached":true,
            "Partitions":[
                {
                    "PartitionNumber":1,
                    "Type":"Reserved",
                    "GptType":"{e3c9e316-0b5c-4db8-817d-f92df00215ae}",
                    "SizeMB":16,
                    "DriveLetter":null,
                    "FileSystem":null,
                    "Label":null
                },
                {
                    "PartitionNumber":4,
                    "Type":"Basic",
                    "GptType":"{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}",
                    "SizeMB":61295,
                    "DriveLetter":"T",
                    "FileSystem":"NTFS",
                    "Label":"System"
                }
            ]
        }"#;
        let layout: VhdLayout = serde_json::from_str(json).expect("layout json should deserialize");
        assert_eq!(layout.partitions.len(), 2);
        assert_eq!(layout.partitions[0].kind, "Reserved");
        assert_eq!(layout.partitions[1].kind, "Basic");
        assert_eq!(find_system_partition_number(&layout), Some(4));
    }

    #[test]
    fn finds_partitions_by_gpt_type() {
        let layout = sample_layout();
        assert_eq!(find_system_partition_number(&layout), Some(4));
    }

    #[test]
    fn falls_back_to_largest_partition_for_system_volume() {
        let mut layout = sample_layout();
        layout.partitions[3].gpt_type = None;
        layout.partitions[3].kind = "Unknown".into();
        assert_eq!(find_system_partition_number(&layout), Some(4));
    }
}
