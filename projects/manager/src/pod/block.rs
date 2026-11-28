//! Raw block devices given to the manager's container, under `/dev/`. QEMU
//! opens each as a `host_device` with O_DIRECT and native AIO, so every write
//! it completed is on the device itself, none in this host's page cache.
use super::IscsiDisk;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, os::unix::fs::FileTypeExt, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockDisk {
    pub id: String,
    /// The device node, under `/dev/`.
    pub path: PathBuf,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub boot_index: Option<u32>,
}

/// The QEMU raw node a block disk is attached through.
pub fn node(id: &str) -> String {
    format!("block-{id}")
}

/// IDs, device paths and boot indexes are unique, also against the iSCSI
/// disks: a guest serial or boot slot never names two disks.
pub(super) fn validate_shape(disks: &[BlockDisk], iscsi: &[IscsiDisk]) -> Result<()> {
    if disks.len() + iscsi.len() > 16 {
        return Err(Error::Config(
            "at most 16 native disks are supported".into(),
        ));
    }
    let mut ids: HashSet<&str> = iscsi.iter().map(|d| d.id.as_str()).collect();
    let mut boot_indexes: HashSet<u32> = iscsi.iter().filter_map(|d| d.boot_index).collect();
    let mut paths = HashSet::new();
    for disk in disks {
        let id = &disk.id;
        if id.is_empty()
            || id.len() > 20
            || !id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || !id.as_bytes()[0].is_ascii_alphanumeric()
            || !id.as_bytes()[id.len() - 1].is_ascii_alphanumeric()
            || !ids.insert(id)
        {
            return Err(Error::Config("block disk IDs must be unique among all disks, 1..20 lowercase letters/digits/hyphens with alphanumeric ends".into()));
        }
        let path = disk.path.to_str().unwrap_or_default();
        if !path.starts_with("/dev/")
            || path.contains(',')
            || disk
                .path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            || !paths.insert(&disk.path)
        {
            return Err(Error::Config(format!(
                "block disk {id}: path must be a distinct device node under /dev/ without commas"
            )));
        }
        if let Some(index) = disk.boot_index
            && (index == 1 || index > i32::MAX as u32 || !boot_indexes.insert(index))
        {
            return Err(Error::Config("disk boot indexes must be unique nonnegative int32 values other than reserved ESP index1".into()));
        }
    }
    Ok(())
}

pub(super) fn validate(disks: &[BlockDisk], iscsi: &[IscsiDisk]) -> Result<()> {
    validate_shape(disks, iscsi)?;
    for disk in disks {
        let kind = std::fs::metadata(&disk.path)
            .map_err(|e| {
                Error::Config(format!(
                    "block disk {}: {} is not available: {e}",
                    disk.id,
                    disk.path.display()
                ))
            })?
            .file_type();
        if !kind.is_block_device() {
            return Err(Error::Config(format!(
                "block disk {}: {} is not a block device",
                disk.id,
                disk.path.display()
            )));
        }
    }
    Ok(())
}

pub(super) fn arguments(disks: &[BlockDisk], args: &mut Vec<String>) {
    for disk in disks {
        let device = format!("block-{}-dev", disk.id);
        let node = node(&disk.id);
        let cache = serde_json::json!({"direct": true, "no-flush": false});
        args.extend([
            "-blockdev".into(),
            serde_json::json!({"driver":"host_device","filename":disk.path,"node-name":device,"read-only":disk.read_only,"aio":"native","cache":cache,"discard":"unmap"}).to_string(),
            "-blockdev".into(),
            serde_json::json!({"driver":"raw","file":device,"node-name":node,"read-only":disk.read_only,"cache":cache,"discard":"unmap"}).to_string(),
        ]);
        let device = format!(
            "virtio-blk-pci,drive={node},id=disk-{},serial={}",
            disk.id, disk.id
        );
        args.extend([
            "-device".into(),
            match disk.boot_index {
                Some(index) => format!("{device},bootindex={index}"),
                None => device,
            },
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(id: &str, path: &str, boot_index: Option<u32>) -> BlockDisk {
        BlockDisk {
            id: id.into(),
            path: path.into(),
            read_only: false,
            boot_index,
        }
    }

    #[test]
    fn host_device_blockdevs_use_direct_native_io_in_boot_order() {
        let mut readonly = disk("data", "/dev/qrab/disk-data", None);
        readonly.read_only = true;
        let mut args = Vec::new();
        arguments(
            &[disk("root", "/dev/qrab/disk-root", Some(0)), readonly],
            &mut args,
        );
        assert_eq!(args.len(), 12);
        let backend: serde_json::Value = serde_json::from_str(&args[1]).unwrap();
        assert_eq!(
            backend,
            serde_json::json!({"driver":"host_device","filename":"/dev/qrab/disk-root","node-name":"block-root-dev",
                "read-only":false,"aio":"native","cache":{"direct":true,"no-flush":false},"discard":"unmap"})
        );
        let raw: serde_json::Value = serde_json::from_str(&args[3]).unwrap();
        assert_eq!(raw["driver"], "raw");
        assert_eq!(raw["file"], "block-root-dev");
        assert_eq!(raw["node-name"], node("root"));
        assert_eq!(
            args[5],
            "virtio-blk-pci,drive=block-root,id=disk-root,serial=root,bootindex=0"
        );
        let readonly: serde_json::Value = serde_json::from_str(&args[7]).unwrap();
        assert_eq!(readonly["read-only"], true);
        assert_eq!(
            args[11],
            "virtio-blk-pci,drive=block-data,id=disk-data,serial=data"
        );
    }

    #[test]
    fn ambiguous_block_disk_configs_are_rejected() {
        let iscsi: IscsiDisk = serde_json::from_value(serde_json::json!({
            "id":"data","portal":"192.0.2.1:3260","target":"iqn.2026-11.dev.qrab:qrab.disk.x",
            "initiator_name":"iqn.2026-11.dev.qrab:qrab.vm.x","username":"u","password_file":"/run/x","boot_index":2}))
        .unwrap();
        let ok = disk("root", "/dev/qrab/disk-root", Some(0));
        validate_shape(std::slice::from_ref(&ok), std::slice::from_ref(&iscsi)).unwrap();
        for bad in [
            vec![disk("data", "/dev/qrab/disk-data", None)],
            vec![ok.clone(), disk("root", "/dev/qrab/disk-other", None)],
            vec![ok.clone(), disk("other", "/dev/qrab/disk-root", None)],
            vec![disk("root", "/run/qrab/disk", None)],
            vec![disk("root", "/dev/../etc/shadow", None)],
            vec![disk("root", "/dev/qrab/a,b", None)],
            vec![disk("root", "/dev/qrab/disk-root", Some(1))],
            vec![disk("root", "/dev/qrab/disk-root", Some(2))],
            vec![disk("Root", "/dev/qrab/disk-root", None)],
        ] {
            assert!(
                validate_shape(&bad, std::slice::from_ref(&iscsi)).is_err(),
                "{bad:?}"
            );
        }
        // Unknown fields are refused, like every manager config.
        assert!(
            serde_json::from_value::<BlockDisk>(
                serde_json::json!({"id":"root","path":"/dev/qrab/disk-root","cache":"unsafe"})
            )
            .is_err()
        );
    }

    #[test]
    fn a_path_that_is_not_a_block_device_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("disk");
        std::fs::write(&file, b"").unwrap();
        // A character device, a missing node and a regular file.
        assert!(validate(&[disk("root", "/dev/null", None)], &[]).is_err());
        assert!(validate(&[disk("root", "/dev/qrab-missing/disk-root", None)], &[]).is_err());
        assert!(validate(&[disk("root", file.to_str().unwrap(), None)], &[]).is_err());
    }
}
