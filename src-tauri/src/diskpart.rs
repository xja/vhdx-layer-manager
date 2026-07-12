use std::path::Path;

use crate::error::Result;
use crate::sys::{run_elevated_command, CommandOutput};

/// Run a diskpart script stored at `script_path`.
pub fn run_diskpart_script(script_path: &Path) -> Result<CommandOutput> {
    run_elevated_command(
        "diskpart",
        &["/s", script_path.to_string_lossy().as_ref()],
        None,
    )
}

/// Generate script to create and partition a base VHDX with GPT + EFI/MSR/Primary.
pub fn base_diskpart_script(
    vhd_path: &Path,
    size_gb: u64,
    efi_letter: char,
    sys_letter: char,
) -> String {
    let size_mb = size_gb * 1024;
    format!(
        r#"
create vdisk file="{vhd}" maximum={size_mb} type=expandable
select vdisk file="{vhd}"
attach vdisk
convert gpt
create partition efi size=100
format quick fs=fat32 label="EFI"
assign letter={efi_letter}
create partition msr size=16
create partition primary
format quick fs=ntfs label="System"
assign letter={sys_letter}
list volume
list partition
"#,
        vhd = vhd_path.display(),
        size_mb = size_mb,
        efi_letter = efi_letter,
        sys_letter = sys_letter
    )
}

/// Script to create a differencing VHDX and list partitions (no letter assignment).
pub fn diff_attach_list_script(child: &Path, parent: &Path) -> String {
    format!(
        r#"
create vdisk file="{child}" parent="{parent}"
select vdisk file="{child}"
attach vdisk
list volume
list partition
"#,
        child = child.display(),
        parent = parent.display()
    )
}

/// Attach an existing VHD and list its partitions/volumes.
pub fn attach_list_vdisk_script(vhd_path: &Path) -> String {
    format!(
        r#"
select vdisk file="{vhd}"
attach vdisk
list partition
list volume
"#,
        vhd = vhd_path.display()
    )
}

/// Script to assign letters to specific partitions on the currently attached VHD.
pub fn assign_partitions_script(vhd_path: &Path, assignments: &[(u32, char)]) -> String {
    let mut lines = Vec::new();
    lines.push(format!(r#"select vdisk file="{}""#, vhd_path.display()));
    for (part_idx, letter) in assignments {
        lines.push(format!("select partition {part_idx}"));
        lines.push(format!("assign letter={letter} noerr"));
    }
    lines.push("list volume".into());
    lines.join("\n")
}

pub fn detach_vdisk_script(vhd_path: &Path, letters: &[char], detach_vdisk: bool) -> String {
    let mut lines = Vec::new();
    let select_vhd = format!(r#"select vdisk file="{}""#, vhd_path.display());
    lines.push(select_vhd.clone());
    for letter in letters {
        lines.push(format!("select volume {letter}"));
        lines.push(format!("remove letter={letter} noerr"));
    }
    if detach_vdisk {
        lines.push(select_vhd);
        lines.push("detach vdisk".into());
    }
    lines.join("\n")
}
