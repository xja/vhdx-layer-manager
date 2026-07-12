use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::iter::once;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use tracing::info;
use uuid::Uuid;

use crate::bcd::{
    bcdedit_boot_sequence, bcdedit_delete, bcdedit_enum_all, bcdedit_set_description,
    extract_guid_for_partition_letter, extract_guid_for_vhd, run_bcdboot, run_bcdboot_to_efi,
};
use crate::db::Database;
use crate::diskpart::{
    assign_partitions_script, attach_list_vdisk_script, base_diskpart_script, detach_vdisk_script,
    diff_attach_list_script, run_diskpart_script,
};
use crate::dism::{apply_image, list_images};
use crate::error::{AppError, Result};
use crate::models::{Node, NodeStatus, WimImageInfo};
use crate::paths::AppPaths;
use crate::state::SharedState;
use crate::storage::{find_system_partition_number, inspect_vhd_layout, is_vhd_attached};
use crate::sys::{run_elevated_command, CommandOutput};
use crate::temp::TempManager;
use crate::vhdx::inspect_vhd_parent_path;
use windows_sys::Win32::Storage::FileSystem::{GetLogicalDrives, QueryDosDeviceW};

pub struct WorkspaceService {
    state: SharedState,
}

impl WorkspaceService {
    pub fn new(state: SharedState) -> Self {
        Self { state }
    }

    fn db(&self) -> Result<std::sync::Arc<Database>> {
        self.state.db()
    }

    fn paths(&self) -> Result<AppPaths> {
        self.state.paths()
    }

    pub fn scan(&self) -> Result<Vec<Node>> {
        let paths = self.paths()?;
        paths.ensure_layout()?;
        let db = self.db()?;

        let existing_nodes = db.fetch_nodes()?;
        let mut existing_paths: HashMap<String, Node> = existing_nodes
            .iter()
            .map(|n| (normalize_path(&n.path), n.clone()))
            .collect();

        let disks_dir = paths.base_dir();
        let vhd_paths = collect_vhdx_files(&disks_dir)?;
        reconcile_scanned_paths(paths.root(), &db, &mut existing_paths, &vhd_paths)?;

        let needs_bcd_lookup = vhd_paths.iter().any(|path| {
            let normalized = normalize_path(path.to_string_lossy().as_ref());
            existing_paths
                .get(&normalized)
                .map(|node| node.bcd_guid.is_none())
                .unwrap_or(true)
        });
        let bcd_enum = if !needs_bcd_lookup {
            None
        } else {
            bcdedit_enum_all().ok()
        };
        let mut scanned = Vec::new();

        for path in vhd_paths {
            let path_str = path.to_string_lossy().to_string();
            let normalized = normalize_path(&path_str);
            let created_at = file_time_or_now(&path);
            let attached = is_vhd_attached(&path).unwrap_or(false);

            let mut parent_normalized = None;
            let mut detail_ok = true;
            match inspect_vhd_parent_path(&path) {
                Ok(parent) => {
                    parent_normalized = parent.map(|p| normalize_path(&p));
                }
                Err(err) => {
                    detail_ok = false;
                    info!("inspect_vhd_parent_path failed path={} err={err}", path_str);
                }
            }

            let bcd_guid = existing_paths
                .get(&normalized)
                .and_then(|node| node.bcd_guid.clone())
                .or_else(|| {
                    bcd_enum
                        .as_ref()
                        .and_then(|out| extract_guid_for_vhd(&out.stdout, &path_str))
                });

            scanned.push(ScannedVhd {
                path: path_str,
                normalized,
                parent_normalized,
                detail_ok,
                created_at,
                bcd_guid,
                attached,
            });
        }

        // Assign IDs for all discovered VHDX files (reuse existing where possible).
        let mut path_to_id: HashMap<String, String> = existing_paths
            .iter()
            .map(|(p, n)| (p.clone(), n.id.clone()))
            .collect();
        for info in &scanned {
            path_to_id
                .entry(info.normalized.clone())
                .or_insert_with(|| Uuid::new_v4().to_string());
        }

        // Insert newly discovered nodes.
        for info in &scanned {
            if existing_paths.contains_key(&info.normalized) {
                continue;
            }
            let id = path_to_id
                .get(&info.normalized)
                .cloned()
                .expect("id must exist for scanned path");
            let node = Node {
                id: id.clone(),
                parent_id: None,
                name: derive_name_from_path(&info.path),
                path: info.path.clone(),
                bcd_guid: info.bcd_guid.clone(),
                desc: None,
                created_at: info.created_at,
                status: NodeStatus::Normal,
                boot_files_ready: info.bcd_guid.is_some(),
            };
            db.insert_node(&node)?;
            db.insert_op(
                &Uuid::new_v4().to_string(),
                Some(&id),
                "import_vhdx",
                "ok",
                &format!("path={}", node.path),
            )?;
            existing_paths.insert(info.normalized.clone(), node);
        }

        // Update parent linkage and BCD info for existing records.
        for info in &scanned {
            if let Some(node_id) = path_to_id.get(&info.normalized) {
                let target_parent = info
                    .parent_normalized
                    .as_ref()
                    .and_then(|p| path_to_id.get(p).cloned());
                if let Some(existing) = existing_paths.get_mut(&info.normalized) {
                    if existing.parent_id != target_parent {
                        db.update_node_parent(node_id, target_parent.as_deref())?;
                        existing.parent_id = target_parent.clone();
                    }
                    if let Some(guid) = info.bcd_guid.as_ref() {
                        if existing.bcd_guid.as_deref() != Some(guid.as_str()) {
                            db.update_node_bcd(node_id, guid)?;
                            existing.bcd_guid = Some(guid.clone());
                            existing.boot_files_ready = true;
                        }
                    }
                }
            }
        }

        let latest_nodes = db.fetch_nodes()?;
        let detail_lookup: HashMap<String, (Option<String>, bool, bool)> = scanned
            .into_iter()
            .map(|info| {
                (
                    info.normalized,
                    (info.parent_normalized, info.detail_ok, info.attached),
                )
            })
            .collect();
        let id_by_path: HashMap<String, String> = latest_nodes
            .iter()
            .map(|n| (normalize_path(&n.path), n.id.clone()))
            .collect();

        for n in latest_nodes.iter() {
            let normalized = normalize_path(&n.path);
            let mut status = NodeStatus::Normal;
            if !Path::new(&n.path).exists() {
                status = NodeStatus::MissingFile;
            } else if let Some((parent_path, detail_ok, attached)) = detail_lookup.get(&normalized) {
                if !detail_ok {
                    status = NodeStatus::Error;
                } else if *attached {
                    status = NodeStatus::Mounted;
                } else if let Some(parent_norm) = parent_path {
                    match id_by_path.get(parent_norm) {
                        Some(pid) if n.parent_id.as_deref() == Some(pid.as_str()) => {}
                        Some(_) | None => status = NodeStatus::MissingParent,
                    }
                } else if n.parent_id.is_some() {
                    status = NodeStatus::MissingParent;
                }
            }
            db.update_node_status(&n.id, status.clone())?;
            info!("scan node={} status={:?}", n.id, status);
        }

        Ok(db.fetch_nodes()?)
    }

    /// Lightweight fetch without validation; used by UI refresh to avoid slow diskpart checks.
    pub fn list_nodes(&self) -> Result<Vec<Node>> {
        self.db()?.fetch_nodes()
    }

    pub fn list_wim_images(&self, image_path: &str) -> Result<Vec<WimImageInfo>> {
        list_images(image_path)
    }

    pub fn create_base(
        &self,
        name: &str,
        desc: Option<String>,
        wim_file: &str,
        wim_index: u32,
        size_gb: u64,
    ) -> Result<Node> {
        let paths = self.paths()?;
        paths.ensure_layout()?;
        let db = self.db()?;
        let seq = db.next_seq()?;
        let id = Uuid::new_v4().to_string();
        let filename = format!("{seq:04}-{slug}.vhdx", slug = name.to_lowercase());
        let vhd_path = paths.base_dir().join(filename);

        let temp = TempManager::new(paths.tmp_dir())?;
        fs::create_dir_all(paths.mount_root())?;
        let letters = pick_free_letters(2).ok_or_else(|| {
            AppError::Message("no free drive letter available between S: and Z:".into())
        })?;
        let efi_letter = letters[0];
        let sys_letter = letters[1];

        let script = base_diskpart_script(&vhd_path, size_gb, efi_letter, sys_letter);
        let script_path = temp.write_script("create_base.txt", &script)?;
        log_diskpart_script(&script_path);
        let create_res = run_diskpart_script(&script_path)?;
        log_command("diskpart create base", &create_res, Some(&script_path));

        if create_res.exit_code.unwrap_or(-1) != 0 {
            self.rescan_after_failure("create_base");
            return Err(command_error(
                "diskpart create base",
                &create_res,
                Some(&script_path),
            ));
        }

        let guid = match (|| -> Result<String> {
            let _attach_guard = AttachedVhdGuard::new(
                temp.clone(),
                vhd_path.clone(),
                vec![sys_letter, efi_letter],
                "detach_base.txt",
                "diskpart detach base",
                true,
                true,
            );
            let dism_res = apply_image(wim_file, wim_index, &format!("{sys_letter}:\\"))?;
            log_command("dism apply", &dism_res, None);
            if dism_res.exit_code.unwrap_or(-1) != 0 {
                return Err(command_error("dism apply", &dism_res, None));
            }

            let sys_mount = PathBuf::from(format!("{sys_letter}:"));
            let efi_mount = PathBuf::from(format!("{efi_letter}:"));
            let bcd_efi_res = run_bcdboot_to_efi(&sys_mount, &efi_mount)?;
            log_command("bcdboot efi", &bcd_efi_res, None);
            if bcd_efi_res.exit_code.unwrap_or(-1) != 0 {
                return Err(command_error("bcdboot", &bcd_efi_res, None));
            }

            let bcd_res = run_bcdboot(&sys_mount)?;
            log_command("bcdboot", &bcd_res, None);
            if bcd_res.exit_code.unwrap_or(-1) != 0 {
                return Err(command_error("bcdboot", &bcd_res, None));
            }

            let bcd_enum = bcdedit_enum_all()?;
            log_command("bcdedit enum", &bcd_enum, None);
            Ok(
                extract_guid_for_vhd(&bcd_enum.stdout, vhd_path.to_str().unwrap_or_default())
                .or_else(|| extract_guid_for_partition_letter(&bcd_enum.stdout, sys_letter))
                .unwrap_or_default(),
            )
        })() {
            Ok(guid) => guid,
            Err(err) => {
                self.rescan_after_failure("create_base");
                return Err(err);
            }
        };

        let node = Node {
            id: id.clone(),
            parent_id: None,
            name: name.to_string(),
            path: vhd_path.to_string_lossy().to_string(),
            bcd_guid: if guid.is_empty() {
                None
            } else {
                Some(guid.clone())
            },
            desc,
            created_at: Utc::now(),
            status: NodeStatus::Normal,
            boot_files_ready: !guid.is_empty(),
        };

        db.insert_node(&node)?;
        db.insert_op(
            &Uuid::new_v4().to_string(),
            Some(&id),
            "create_base",
            "ok",
            "",
        )?;
        info!("create_base id={id} path={}", node.path);
        Ok(node)
    }

    pub fn create_diff(&self, parent_id: &str, name: &str, desc: Option<String>) -> Result<Node> {
        self.scan()?;
        let db = self.db()?;
        let parent = db
            .fetch_node(parent_id)?
            .ok_or_else(|| AppError::Message("parent not found".into()))?;
        if is_vhd_attached(Path::new(&parent.path)).unwrap_or(false) {
            return Err(AppError::Message(
                "selected parent vhdx must be detached before creating a differencing disk"
                    .into(),
            ));
        }
        self.ensure_no_attached_descendants(parent_id)?;
        let paths = self.paths()?;
        paths.ensure_layout()?;
        let seq = db.next_seq()?;
        let id = Uuid::new_v4().to_string();
        let filename = format!("{seq:04}-{slug}.vhdx", slug = name.to_lowercase());

        let parent_path = Path::new(&parent.path);
        let parent_dir = parent_path
            .parent()
            .ok_or_else(|| AppError::Message(format!("invalid parent path: {}", parent.path)))?;
        let vhd_path = parent_dir.join(filename);

        let temp = TempManager::new(paths.tmp_dir())?;
        let sys_letter = pick_free_letter().ok_or_else(|| {
            AppError::Message("no free drive letter available between S: and Z:".into())
        })?;

        let attach_script = diff_attach_list_script(&vhd_path, Path::new(&parent.path));
        let attach_path = temp.write_script("create_diff.txt", &attach_script)?;
        log_diskpart_script(&attach_path);
        let attach_res = run_diskpart_script(&attach_path)?;
        log_command("diskpart create diff", &attach_res, Some(&attach_path));
        if attach_res.exit_code.unwrap_or(-1) != 0 {
            self.rescan_after_failure("create_diff");
            return Err(command_error(
                "diskpart create diff",
                &attach_res,
                Some(&attach_path),
            ));
        }

        let guid = match (|| -> Result<String> {
            let _attach_guard = AttachedVhdGuard::new(
                temp.clone(),
                vhd_path.clone(),
                vec![sys_letter],
                "detach_diff.txt",
                "diskpart detach diff",
                true,
                true,
            );
            let layout = inspect_vhd_layout(&vhd_path)?;
            let sys_part = find_system_partition_number(&layout).ok_or_else(|| {
                AppError::Message("failed to detect system partition from storage layout".into())
            })?;

            let assign_script = assign_partitions_script(&vhd_path, &[(sys_part, sys_letter)]);
            let assign_path = temp.write_script("assign_diff.txt", &assign_script)?;
            log_diskpart_script(&assign_path);
            let assign_res = run_diskpart_script(&assign_path)?;
            log_command("diskpart assign diff", &assign_res, Some(&assign_path));
            if assign_res.exit_code.unwrap_or(-1) != 0 {
                return Err(command_error(
                    "diskpart assign diff",
                    &assign_res,
                    Some(&assign_path),
                ));
            }

            let sys_mount = PathBuf::from(format!("{sys_letter}:"));
            let bcd_res = run_bcdboot(&sys_mount)?;
            log_command("bcdboot", &bcd_res, None);
            if bcd_res.exit_code.unwrap_or(-1) != 0 {
                return Err(command_error("bcdboot", &bcd_res, None));
            }
            let bcd_enum = bcdedit_enum_all()?;
            log_command("bcdedit enum", &bcd_enum, None);
            Ok(
                extract_guid_for_vhd(&bcd_enum.stdout, vhd_path.to_str().unwrap_or_default())
                .or_else(|| extract_guid_for_partition_letter(&bcd_enum.stdout, sys_letter))
                .unwrap_or_default(),
            )
        })() {
            Ok(guid) => guid,
            Err(err) => {
                self.rescan_after_failure("create_diff");
                return Err(err);
            }
        };

        let node = Node {
            id: id.clone(),
            parent_id: Some(parent_id.to_string()),
            name: name.to_string(),
            path: vhd_path.to_string_lossy().to_string(),
            bcd_guid: if guid.is_empty() {
                None
            } else {
                Some(guid.clone())
            },
            desc,
            created_at: Utc::now(),
            status: NodeStatus::Normal,
            boot_files_ready: !guid.is_empty(),
        };
        db.insert_node(&node)?;
        db.insert_op(
            &Uuid::new_v4().to_string(),
            Some(&id),
            "create_diff",
            "ok",
            "",
        )?;
        info!("create_diff id={id} parent={parent_id}");
        Ok(node)
    }

    pub fn set_bootsequence_and_reboot(&self, node_id: &str) -> Result<CommandOutput> {
        let db = self.db()?;
        let node = db
            .fetch_node(node_id)?
            .ok_or_else(|| AppError::Message("node not found".into()))?;
        let guid = node
            .bcd_guid
            .clone()
            .ok_or_else(|| AppError::Message("node missing bcd guid".into()))?;
        let res = bcdedit_boot_sequence_and_reboot(&guid)?;
        log_command("bcdedit bootsequence", &res, None);
        db.insert_op(
            &Uuid::new_v4().to_string(),
            Some(node_id),
            "bootsequence_reboot",
            "ok",
            "",
        )?;
        info!("bootsequence node={node_id} guid={guid}");
        Ok(res)
    }

    pub fn start_vm(&self, node_id: &str) -> Result<String> {
        let db = self.db()?;
        let node = db
            .fetch_node(node_id)?
            .ok_or_else(|| AppError::Message("node not found".into()))?;

        let vhd_path = PathBuf::from(&node.path);
        if !vhd_path.exists() {
            return Err(AppError::Message(format!("vhdx not found: {}", node.path)));
        }

        let paths = self.paths()?;
        paths.ensure_layout()?;
        fs::create_dir_all(paths.vms_dir())?;

        let vm_name = format!("ls-{}", node.id);
        let vm_dir = paths.vms_dir().join(&vm_name);
        fs::create_dir_all(&vm_dir)?;

        let ps_script = format!(
            r#"$ErrorActionPreference = 'Stop'
if (-not (Get-Command -Name 'Get-VM' -ErrorAction SilentlyContinue)) {{ throw 'Hyper-V PowerShell module is not available (Get-VM not found).'; }}
if (-not (Get-Command -Name 'vmconnect.exe' -ErrorAction SilentlyContinue)) {{ throw 'vmconnect.exe not found in PATH.'; }}
$vmName = '{vm_name}'
$vmPath = '{vm_path}'
$vhdPath = '{vhd_path}'
if (-not (Test-Path -Path $vmPath)) {{ New-Item -ItemType Directory -Path $vmPath | Out-Null }}
$vm = Get-VM -Name $vmName -ErrorAction SilentlyContinue
if (-not $vm) {{
    $vm = New-VM -Name $vmName -Generation 2 -MemoryStartupBytes 2GB -VHDPath $vhdPath -Path $vmPath
}} else {{
    $drive = Get-VMHardDiskDrive -VMName $vmName -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($drive) {{
        Set-VMHardDiskDrive -VMHardDiskDrive $drive -Path $vhdPath | Out-Null
    }} else {{
        Add-VMHardDiskDrive -VMName $vmName -Path $vhdPath | Out-Null
    }}
}}
if ($vm.State -ne 'Running') {{
    Start-VM -Name $vmName | Out-Null
}}
Start-Process vmconnect.exe -ArgumentList 'localhost', $vmName | Out-Null
"#,
            vm_name = ps_escape_single(&vm_name),
            vm_path = ps_escape_single(vm_dir.to_string_lossy().as_ref()),
            vhd_path = ps_escape_single(vhd_path.to_string_lossy().as_ref()),
        );

        let res = run_elevated_command(
            "powershell.exe",
            &[
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &ps_script,
            ],
            None,
        )?;
        log_command("start_vm", &res, None);
        if res.exit_code.unwrap_or(-1) != 0 {
            return Err(command_error("start_vm", &res, None));
        }
        db.insert_op(
            &Uuid::new_v4().to_string(),
            Some(node_id),
            "start_vm",
            "ok",
            &format!("vm_name={vm_name}"),
        )?;
        info!("start_vm node={node_id} vm_name={vm_name}");
        Ok(vm_name)
    }

    pub fn delete_subtree(&self, node_id: &str) -> Result<()> {
        self.scan()?;
        let db = self.db()?;
        let nodes = db.fetch_nodes()?;
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        for n in nodes.iter() {
            if let Some(pid) = &n.parent_id {
                graph.entry(pid.clone()).or_default().push(n.id.clone());
            }
        }
        let mut order = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(node_id.to_string());
        while let Some(id) = queue.pop_front() {
            order.push(id.clone());
            if let Some(children) = graph.get(&id) {
                for c in children {
                    queue.push_back(c.clone());
                }
            }
        }
        // Delete children after parents? requirement: delete subtree; we reverse to delete leaves first.
        order.reverse();
        for id in order.iter() {
            if let Some(node) = db.fetch_node(id)?.clone() {
                if let Some(guid) = node.bcd_guid.as_ref() {
                    if let Ok(o) = bcdedit_delete(guid) {
                        log_command("bcdedit delete", &o, None);
                    }
                }
                // attempt detach
                let temp = TempManager::new(self.paths()?.tmp_dir())?;
                let detach_script = detach_vdisk_script(Path::new(&node.path), &[], true);
                let path = temp.write_script("detach_cleanup.txt", &detach_script)?;
                log_diskpart_script(&path);
                if let Ok(o) = run_diskpart_script(&path) {
                    log_command("diskpart detach cleanup", &o, Some(&path));
                }
                // delete file
                // let _ = fs::remove_file(&node.path);
            }
        }
        db.delete_ops_for_nodes(&order)?;
        db.delete_nodes(&order)?;
        db.insert_op(
            &Uuid::new_v4().to_string(),
            None,
            "delete_subtree",
            "ok",
            &format!("node_id={}", node_id),
        )?;
        info!("delete_subtree node={node_id} count={}", order.len());
        Ok(())
    }

    pub fn delete_bcd(&self, node_id: &str) -> Result<()> {
        let db = self.db()?;
        let node = db
            .fetch_node(node_id)?
            .ok_or_else(|| AppError::Message("node not found".into()))?;
        if let Some(guid) = node.bcd_guid.as_ref() {
            let res = bcdedit_delete(guid)?;
            log_command("bcdedit delete", &res, None);
            if res.exit_code.unwrap_or(-1) != 0 {
                return Err(command_error("bcdedit delete", &res, None));
            }
        }
        db.clear_node_bcd(node_id)?;
        db.insert_op(
            &Uuid::new_v4().to_string(),
            Some(node_id),
            "delete_bcd",
            "ok",
            "",
        )?;
        info!("delete_bcd node={node_id}");
        Ok(())
    }

    pub fn add_bcd_entry(
        &self,
        node_id: &str,
        description: Option<String>,
    ) -> Result<Option<String>> {
        let guid = self.repair_bcd_inner(node_id, description.as_deref())?;
        Ok(guid)
    }

    pub fn update_bcd_description(&self, node_id: &str, description: &str) -> Result<()> {
        let db = self.db()?;
        let node = db
            .fetch_node(node_id)?
            .ok_or_else(|| AppError::Message("node not found".into()))?;
        let guid = node
            .bcd_guid
            .clone()
            .ok_or_else(|| AppError::Message("node missing bcd guid".into()))?;
        let res = bcdedit_set_description(&guid, description)?;
        log_command("bcdedit set description", &res, None);
        if res.exit_code.unwrap_or(-1) != 0 {
            return Err(command_error("bcdedit set description", &res, None));
        }
        db.insert_op(
            &Uuid::new_v4().to_string(),
            Some(node_id),
            "update_bcd_description",
            "ok",
            description,
        )?;
        Ok(())
    }

    pub fn repair_bcd(&self, node_id: &str) -> Result<Option<String>> {
        self.repair_bcd_inner(node_id, None)
    }

    fn repair_bcd_inner(&self, node_id: &str, description: Option<&str>) -> Result<Option<String>> {
        self.scan()?;
        let db = self.db()?;
        let node = db
            .fetch_node(node_id)?
            .ok_or_else(|| AppError::Message("node not found".into()))?;
        self.ensure_no_attached_descendants(node_id)?;
        let paths = self.paths()?;
        let temp = TempManager::new(paths.tmp_dir())?;
        let sys_letter = pick_free_letter().ok_or_else(|| {
            AppError::Message("no free drive letter available between S: and Z:".into())
        })?;
        let was_attached = is_vhd_attached(Path::new(&node.path)).unwrap_or(false);

        if !was_attached {
            let attach_script = attach_list_vdisk_script(Path::new(&node.path));
            let attach_path = temp.write_script("attach_repair.txt", &attach_script)?;
            log_diskpart_script(&attach_path);
            let attach_res = run_diskpart_script(&attach_path)?;
            log_command("diskpart attach repair", &attach_res, Some(&attach_path));
            if attach_res.exit_code.unwrap_or(-1) != 0 {
                self.rescan_after_failure("repair_bcd");
                return Err(command_error(
                    "diskpart attach",
                    &attach_res,
                    Some(&attach_path),
                ));
            }
        }

        let guid = match (|| -> Result<Option<String>> {
            let _attach_guard = AttachedVhdGuard::new(
                temp.clone(),
                PathBuf::from(&node.path),
                vec![sys_letter],
                "detach_repair.txt",
                "diskpart detach repair",
                true,
                !was_attached,
            );
            let layout = inspect_vhd_layout(Path::new(&node.path))?;
            let sys_part = find_system_partition_number(&layout).ok_or_else(|| {
                AppError::Message("failed to detect system partition from storage layout".into())
            })?;

            let assign_script =
                assign_partitions_script(Path::new(&node.path), &[(sys_part, sys_letter)]);
            let assign_path = temp.write_script("assign_repair.txt", &assign_script)?;
            log_diskpart_script(&assign_path);
            let assign_res = run_diskpart_script(&assign_path)?;
            log_command("diskpart assign repair", &assign_res, Some(&assign_path));
            if assign_res.exit_code.unwrap_or(-1) != 0 {
                return Err(command_error(
                    "diskpart assign",
                    &assign_res,
                    Some(&assign_path),
                ));
            }

            let sys_mount = PathBuf::from(format!("{sys_letter}:"));
            let bcd_res = run_bcdboot(&sys_mount)?;
            log_command("bcdboot", &bcd_res, None);
            if bcd_res.exit_code.unwrap_or(-1) != 0 {
                return Err(command_error("bcdboot", &bcd_res, None));
            }
            let bcd_enum = bcdedit_enum_all()?;
            log_command("bcdedit enum", &bcd_enum, None);
            let guid = extract_guid_for_vhd(&bcd_enum.stdout, &node.path)
                .or_else(|| extract_guid_for_partition_letter(&bcd_enum.stdout, sys_letter));
            if let Some(guid) = &guid {
                db.update_node_bcd(&node.id, guid)?;
                if let Some(desc) = description {
                    let res = bcdedit_set_description(guid, desc)?;
                    log_command("bcdedit set description", &res, None);
                }
            }
            Ok(guid)
        })() {
            Ok(guid) => guid,
            Err(err) => {
                self.rescan_after_failure("repair_bcd");
                return Err(err);
            }
        };

        db.insert_op(
            &Uuid::new_v4().to_string(),
            Some(&node.id),
            "repair_bcd",
            "ok",
            description.unwrap_or(""),
        )?;
        info!(
            "repair_bcd node={} guid={}",
            node.id,
            guid.clone().unwrap_or_default()
        );
        Ok(guid)
    }

    pub fn attach_vhd(&self, node_id: &str) -> Result<Node> {
        self.scan()?;
        let db = self.db()?;
        let node = db
            .fetch_node(node_id)?
            .ok_or_else(|| AppError::Message("node not found".into()))?;
        if is_vhd_attached(Path::new(&node.path)).unwrap_or(false) {
            return db
                .fetch_node(node_id)?
                .ok_or_else(|| AppError::Message("node not found".into()));
        }
        self.ensure_no_attached_descendants(node_id)?;
        let paths = self.paths()?;
        let temp = TempManager::new(paths.tmp_dir())?;
        let attach_script = attach_list_vdisk_script(Path::new(&node.path));
        let attach_path = temp.write_script("attach_vhd.txt", &attach_script)?;
        log_diskpart_script(&attach_path);
        let attach_res = run_diskpart_script(&attach_path)?;
        log_command("diskpart attach vhd", &attach_res, Some(&attach_path));
        if attach_res.exit_code.unwrap_or(-1) != 0 {
            self.rescan_after_failure("attach_vhd");
            return Err(command_error(
                "diskpart attach",
                &attach_res,
                Some(&attach_path),
            ));
        }
        db.insert_op(
            &Uuid::new_v4().to_string(),
            Some(node_id),
            "attach_vhd",
            "ok",
            "",
        )?;
        info!("attach_vhd id={node_id}");
        self.scan()?;
        db.fetch_node(node_id)?
            .ok_or_else(|| AppError::Message("node not found after attach".into()))
    }

    pub fn detach_vhd(&self, node_id: &str) -> Result<Node> {
        self.scan()?;
        let db = self.db()?;
        let node = db
            .fetch_node(node_id)?
            .ok_or_else(|| AppError::Message("node not found".into()))?;
        self.ensure_no_attached_descendants(node_id)?;
        if !is_vhd_attached(Path::new(&node.path)).unwrap_or(false) {
            return db
                .fetch_node(node_id)?
                .ok_or_else(|| AppError::Message("node not found".into()));
        }
        let paths = self.paths()?;
        let temp = TempManager::new(paths.tmp_dir())?;
        let detach_script = detach_vdisk_script(Path::new(&node.path), &[], true);
        let detach_path = temp.write_script("detach_vhd.txt", &detach_script)?;
        log_diskpart_script(&detach_path);
        let detach_res = run_diskpart_script(&detach_path)?;
        log_command("diskpart detach vhd", &detach_res, Some(&detach_path));
        if detach_res.exit_code.unwrap_or(-1) != 0 {
            self.rescan_after_failure("detach_vhd");
            return Err(command_error(
                "diskpart detach",
                &detach_res,
                Some(&detach_path),
            ));
        }
        db.insert_op(
            &Uuid::new_v4().to_string(),
            Some(node_id),
            "detach_vhd",
            "ok",
            "",
        )?;
        info!("detach_vhd id={node_id}");
        self.scan()?;
        db.fetch_node(node_id)?
            .ok_or_else(|| AppError::Message("node not found after detach".into()))
    }

    fn ensure_no_attached_descendants(&self, node_id: &str) -> Result<()> {
        let nodes = self.db()?.fetch_nodes()?;
        let attached = attached_descendants(&nodes, node_id);
        if attached.is_empty() {
            return Ok(());
        }
        let labels: Vec<String> = attached
            .into_iter()
            .map(|node| format!("{} ({})", node.name, node.path))
            .collect();
        Err(AppError::Message(format!(
            "attached descendants must be detached first: {}",
            labels.join(", ")
        )))
    }

    fn rescan_after_failure(&self, context: &str) {
        if let Err(err) = self.scan() {
            info!("best-effort scan after {context} failed: {err}");
        }
    }
}

#[derive(Debug)]
struct ScannedVhd {
    path: String,
    normalized: String,
    parent_normalized: Option<String>,
    detail_ok: bool,
    created_at: DateTime<Utc>,
    bcd_guid: Option<String>,
    attached: bool,
}

fn attached_descendants(nodes: &[Node], root_id: &str) -> Vec<Node> {
    let mut by_parent: HashMap<String, Vec<Node>> = HashMap::new();
    for node in nodes {
        if let Some(parent_id) = &node.parent_id {
            by_parent
                .entry(parent_id.clone())
                .or_default()
                .push(node.clone());
        }
    }

    let mut found = Vec::new();
    let mut queue = VecDeque::new();
    queue.push_back(root_id.to_string());
    while let Some(current) = queue.pop_front() {
        if let Some(children) = by_parent.get(&current) {
            for child in children {
                queue.push_back(child.id.clone());
                if is_vhd_attached(Path::new(&child.path)).unwrap_or(false) {
                    found.push(child.clone());
                }
            }
        }
    }
    found
}

fn collect_vhdx_files(scan_root: &Path) -> Result<Vec<PathBuf>> {
    if !scan_root.exists() {
        return Ok(Vec::new());
    }

    let mut stack = vec![scan_root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) => {
                info!(
                    "skip unreadable dir during vhdx scan path={} err={err}",
                    dir.display()
                );
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    info!(
                        "skip unreadable dir entry during vhdx scan path={} err={err}",
                        dir.display()
                    );
                    continue;
                }
            };
            let path = entry.path();
            if path.is_dir() {
                if should_skip_scan_dir(&path) {
                    info!("skip system dir during vhdx scan path={}", path.display());
                    continue;
                }
                stack.push(path);
            } else if path
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("vhdx"))
                .unwrap_or(false)
            {
                files.push(path);
            }
        }
    }
    Ok(files)
}

fn should_skip_scan_dir(path: &Path) -> bool {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(name) => {
            let lower = name.to_ascii_lowercase();
            lower == "system volume information"
                || lower == "$recycle.bin"
                || lower == "recovery"
                || lower == "windows"
        }
        None => false,
    }
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim().trim_start_matches("\\\\?\\");
    let adjusted = device_path_to_drive(trimmed).unwrap_or_else(|| trimmed.to_string());
    adjusted.replace('/', "\\").to_ascii_lowercase()
}

fn derive_name_from_path(path: &str) -> String {
    let stem = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("vhdx");
    if let Some((prefix, rest)) = stem.split_once('-') {
        if prefix.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty() {
            return rest.to_string();
        }
    }
    stem.to_string()
}

fn file_time_or_now(path: &Path) -> DateTime<Utc> {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.created().or_else(|_| m.modified()).ok())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(Utc::now)
}

fn bcdedit_boot_sequence_and_reboot(guid: &str) -> Result<CommandOutput> {
    let res = bcdedit_boot_sequence(guid)?;
    // Reboot immediately
    let _ = run_elevated_command("shutdown", &["/r", "/t", "0"], None);
    Ok(res)
}

fn pick_free_letter() -> Option<char> {
    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return None;
    }
    for letter in b'S'..=b'Z' {
        let idx = (letter - b'A') as u32;
        let in_use = (mask & (1 << idx)) != 0;
        if !in_use {
            return Some(letter as char);
        }
    }
    None
}

fn pick_free_letters(count: usize) -> Option<Vec<char>> {
    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return None;
    }
    let mut letters = Vec::new();
    for letter in b'S'..=b'Z' {
        let idx = (letter - b'A') as u32;
        let in_use = (mask & (1 << idx)) != 0;
        if !in_use {
            letters.push(letter as char);
            if letters.len() == count {
                return Some(letters);
            }
        }
    }
    None
}

/// Convert a device path (e.g. `\Device\HarddiskVolume10\foo`) to a drive path if possible.
fn device_path_to_drive(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase();
    let mut cleaned: &str = &lower;
    if let Some(rest) = cleaned.strip_prefix("\\??\\") {
        cleaned = rest;
    }
    if let Some(rest) = cleaned.strip_prefix("\\globalroot\\") {
        cleaned = rest;
    }
    if !cleaned.starts_with("\\device\\") {
        return None;
    }

    for letter in b'A'..=b'Z' {
        let drive = format!("{}:", letter as char);
        if let Some(prefix) = query_dos_device(&drive) {
            let prefix_lower = prefix.to_ascii_lowercase();
            if cleaned.starts_with(&prefix_lower) && cleaned.len() >= prefix_lower.len() {
                let rest = cleaned[prefix_lower.len()..].trim_start_matches(['\\', '/']);
                return if rest.is_empty() {
                    Some(format!("{drive}\\"))
                } else {
                    Some(format!(r"{drive}\{rest}"))
                };
            }
        }
    }
    None
}

fn query_dos_device(drive: &str) -> Option<String> {
    let wide: Vec<u16> = OsStr::new(drive).encode_wide().chain(once(0)).collect();
    let mut buffer = vec![0u16; 512];
    let len = unsafe { QueryDosDeviceW(wide.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) };
    if len == 0 {
        return None;
    }
    let slice = &buffer[..len as usize];
    let end = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
    Some(String::from_utf16_lossy(&slice[..end]))
}

fn log_diskpart_script(script: &Path) {
    let mut parts = Vec::new();
    match fs::read_to_string(script) {
        Ok(content) => {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                parts.push(format!("script={trimmed}"));
            }
        }
        Err(err) => parts.push(format!("script_read_err={err}")),
    }
    info!(
        "diskpart script {}: {}",
        script.display(),
        parts.join(" | ")
    );
}

fn log_command(name: &str, output: &CommandOutput, script: Option<&Path>) {
    let mut parts = Vec::new();
    if let Some(code) = output.exit_code {
        parts.push(format!("exit={code}"));
    }
    if let Some(script) = script {
        parts.push(format!("script={}", script.display()));
    }
    let stderr = output.stderr.trim();
    let stdout = output.stdout.trim();
    if !stderr.is_empty() {
        parts.push(format!("stderr={stderr}"));
    } else if !stdout.is_empty() {
        parts.push(format!("stdout={stdout}"));
    }
    info!("{name}: {}", parts.join(" | "));
}

fn command_error(name: &str, output: &CommandOutput, script: Option<&Path>) -> AppError {
    let mut parts = Vec::new();
    if let Some(code) = output.exit_code {
        parts.push(format!("exit={code}"));
    }
    if let Some(script) = script {
        parts.push(format!("script={}", script.display()));
    }
    let stderr = output.stderr.trim();
    let stdout = output.stdout.trim();
    if !stderr.is_empty() {
        parts.push(format!("stderr={stderr}"));
    } else if !stdout.is_empty() {
        parts.push(format!("stdout={stdout}"));
    } else {
        parts.push("no output".into());
    }
    AppError::Message(format!("{name} failed: {}", parts.join(" | ")))
}

struct AttachedVhdGuard {
    temp: TempManager,
    vhd_path: PathBuf,
    letters: Vec<char>,
    script_name: &'static str,
    log_name: &'static str,
    cleanup_on_drop: bool,
    detach_vdisk: bool,
}

impl AttachedVhdGuard {
    fn new(
        temp: TempManager,
        vhd_path: PathBuf,
        letters: Vec<char>,
        script_name: &'static str,
        log_name: &'static str,
        cleanup_on_drop: bool,
        detach_vdisk: bool,
    ) -> Self {
        Self {
            temp,
            vhd_path,
            letters,
            script_name,
            log_name,
            cleanup_on_drop,
            detach_vdisk,
        }
    }
}

impl Drop for AttachedVhdGuard {
    fn drop(&mut self) {
        if !self.cleanup_on_drop {
            return;
        }
        let cleanup_script =
            detach_vdisk_script(&self.vhd_path, &self.letters, self.detach_vdisk);
        let detach_path = match self.temp.write_script(self.script_name, &cleanup_script) {
            Ok(path) => path,
            Err(err) => {
                info!("{}: failed to write cleanup script: {err}", self.log_name);
                return;
            }
        };
        log_diskpart_script(&detach_path);
        match run_diskpart_script(&detach_path) {
            Ok(output) => log_command(self.log_name, &output, Some(&detach_path)),
            Err(err) => info!("{}: cleanup failed: {err}", self.log_name),
        }
    }
}

fn ps_escape_single(input: &str) -> String {
    input.replace('\'', "''")
}

fn reconcile_scanned_paths(
    root: &Path,
    db: &std::sync::Arc<Database>,
    existing_paths: &mut HashMap<String, Node>,
    vhd_paths: &[PathBuf],
) -> Result<()> {
    for path in vhd_paths {
        let path_str = path.to_string_lossy().to_string();
        let normalized = normalize_path(&path_str);
        if existing_paths.contains_key(&normalized) {
            continue;
        }
        let Some(candidate) = find_path_relink_candidate(root, existing_paths, path) else {
            continue;
        };
        info!(
            "relink node path id={} old={} new={}",
            candidate.id, candidate.path, path_str
        );
        db.update_node_path(&candidate.id, &path_str)?;
        let old_key = normalize_path(&candidate.path);
        if let Some(mut node) = existing_paths.remove(&old_key) {
            node.path = path_str.clone();
            existing_paths.insert(normalized, node);
        }
    }
    Ok(())
}

fn find_path_relink_candidate(
    root: &Path,
    existing_paths: &HashMap<String, Node>,
    scanned_path: &Path,
) -> Option<Node> {
    let scanned_name = scanned_path.file_name()?.to_string_lossy().to_ascii_lowercase();
    let mut matches = existing_paths
        .values()
        .filter(|node| {
            let node_path = Path::new(&node.path);
            (!node_path.exists() || !path_within_root(root, node_path))
                && node_path
                    .file_name()
                    .map(|name| name.to_string_lossy().eq_ignore_ascii_case(&scanned_name))
                    .unwrap_or(false)
        })
        .cloned();
    let candidate = matches.next()?;
    matches.next().is_none().then_some(candidate)
}

fn path_within_root(root: &Path, candidate: &Path) -> bool {
    let root = normalize_path(root.to_string_lossy().as_ref());
    let candidate = normalize_path(candidate.to_string_lossy().as_ref());
    candidate == root || candidate.starts_with(&(root + "\\"))
}
