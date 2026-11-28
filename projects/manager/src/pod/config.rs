use super::{BlockDisk, IscsiDisk, Seed};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
};

/// Debian's QEMU, which the manager image installs.
fn binary() -> PathBuf {
    "/usr/bin/qemu-system-x86_64".into()
}
fn name() -> String {
    "vm".into()
}
fn memory() -> u64 {
    256
}

fn cpus() -> u32 {
    1
}
fn runtime() -> PathBuf {
    "/run/qrab".into()
}
fn startup() -> u64 {
    30
}
fn grace() -> u64 {
    5
}
fn persistent_state() -> PathBuf {
    "/var/lib/qrab".into()
}
fn uefi_code() -> PathBuf {
    "/opt/qrab/firmware/OVMF_CODE_4M.fd".into()
}
fn uefi_vars() -> PathBuf {
    "/opt/qrab/firmware/OVMF_VARS_4M.fd".into()
}
fn uefi_boot_disk() -> PathBuf {
    "/opt/qrab/demo/boot.img".into()
}
fn enabled() -> bool {
    true
}
fn swtpm_binary() -> PathBuf {
    "/usr/bin/swtpm".into()
}
fn limits_dir() -> PathBuf {
    "/run/qrab-size".into()
}
fn cgroup_dir() -> PathBuf {
    "/sys/fs/cgroup".into()
}

/// virtio-mem plugs whole blocks; QEMU's x86 default and minimum is 2 MiB.
pub const HOTPLUG_BLOCK_MIB: u64 = 2;

/// Boot-time headroom for in-place growth. QEMU starts with `cpus` and
/// `memory_mib`; the supervisor grows the guest toward the container limits
/// that files in `limits_dir` name, never past what its cgroup already
/// permits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Hotplug {
    pub max_cpus: u32,
    pub max_memory_mib: u64,
    /// The directory of this container's limit files, `cpu_limit_millis` and
    /// `memory_limit_mib`, each holding one number.
    #[serde(default = "limits_dir")]
    pub limits_dir: PathBuf,
    #[serde(default = "cgroup_dir")]
    pub cgroup_dir: PathBuf,
    /// The container limit is the guest size plus these fixed overheads.
    pub cpu_overhead_millis: u64,
    pub memory_overhead_mib: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Accelerator {
    #[default]
    Tcg,
    Kvm,
}

/// The KVM device node that [`Accelerator::Kvm`] uses.
pub const KVM_DEVICE: &str = "/dev/kvm";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DiskFormat {
    #[default]
    Raw,
    Qcow2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Disk {
    pub path: PathBuf,
    #[serde(default)]
    pub format: DiskFormat,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Uefi {
    /// Explicit secure-capable firmware contract; enables SMM-protected pflash.
    /// This is not the requested SecureBootEnable setting or proof of enrollment.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secure_boot_capable: bool,
    #[serde(default = "uefi_code")]
    pub code: PathBuf,
    #[serde(default = "uefi_vars")]
    pub vars_template: PathBuf,
    #[serde(default = "uefi_boot_disk")]
    pub boot_disk: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tpm {
    #[serde(default = "swtpm_binary")]
    pub swtpm_binary: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Declares that whoever runs the manager starts it again after a clean
    /// exit, so a fresh VM instance follows. The manager only reports it, in
    /// [`ConfigView`](super::ConfigView).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restart_on_exit: bool,
    #[serde(default = "name")]
    pub name: String,
    #[serde(default = "binary")]
    pub qemu_binary: PathBuf,
    #[serde(default = "memory")]
    pub memory_mib: u64,
    #[serde(default = "cpus")]
    pub cpus: u32,
    #[serde(default)]
    pub accelerator: Accelerator,
    #[serde(default)]
    pub kernel: Option<PathBuf>,
    #[serde(default)]
    pub initrd: Option<PathBuf>,
    #[serde(default)]
    pub kernel_args: String,
    /// Enable a graphical console on a private Unix socket in the session directory.
    #[serde(default)]
    pub console: bool,
    /// Serve the typed control API on `{runtime_dir}/control/control.sock`
    /// for clients that share the runtime volume. See `pod::socket`.
    #[serde(default)]
    pub control_socket: bool,
    /// Attach the `org.qemu.guest_agent.0` virtio-serial port to a private
    /// Unix socket in the session directory. Harmless if no agent runs.
    #[serde(default = "enabled")]
    pub guest_agent: bool,
    /// Directory for the control socket, created by the manager. Defaults to
    /// `{runtime_dir}/control`; a separate volume shares only the socket.
    #[serde(default)]
    pub control_dir: Option<PathBuf>,
    /// In-memory status for the supervisor's control channel; never serialized.
    #[serde(skip)]
    pub(crate) status_sink: super::state::StatusSink,
    #[serde(default)]
    pub disk: Option<Disk>,
    #[serde(default)]
    pub iscsi_disks: Vec<IscsiDisk>,
    /// Raw block devices, attached after the iSCSI disks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub block_disks: Vec<BlockDisk>,
    #[serde(default)]
    pub firmware_dir: Option<PathBuf>,
    #[serde(default)]
    pub uefi: Option<Uefi>,
    #[serde(default)]
    pub tpm: Option<Tpm>,
    #[serde(default = "persistent_state")]
    pub state_dir: PathBuf,
    /// Immutable VM identity, which the persistent state is bound to. Unset,
    /// it is the VM name.
    #[serde(default)]
    pub state_id: Option<String>,
    #[serde(default = "runtime")]
    pub runtime_dir: PathBuf,
    #[serde(default = "startup")]
    pub startup_timeout_secs: u64,
    #[serde(default = "grace")]
    pub shutdown_grace_secs: u64,
    #[serde(default)]
    pub hotplug: Option<Hotplug>,
    /// The file this config was read from, which the manager reads again
    /// while it runs.
    #[serde(skip)]
    pub source: Option<PathBuf>,
    /// cloud-init NoCloud files, attached as a read-only `cidata` disk.
    #[serde(default)]
    pub seed: Option<Seed>,
}

impl Config {
    /// Upper vCPU bound fixed at boot; equal to `cpus` without hot-plug.
    pub fn max_cpus(&self) -> u32 {
        self.hotplug
            .as_ref()
            .map_or(self.cpus, |h| h.max_cpus.max(self.cpus))
    }

    /// Size of the virtio-mem region above boot memory, in whole blocks.
    pub fn hotplug_memory_mib(&self) -> u64 {
        self.hotplug.as_ref().map_or(0, |h| {
            h.max_memory_mib
                .saturating_sub(self.memory_mib)
                .div_ceil(HOTPLUG_BLOCK_MIB)
                * HOTPLUG_BLOCK_MIB
        })
    }

    pub fn read(path: &Path) -> Result<Self> {
        let mut config: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        config.source = Some(path.to_owned());
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        super::MediaPolicy::from_env()?;
        if self.name.is_empty()
            || self.name.len() > 63
            || !self
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err(Error::Config(
                "name must be 1..63 ASCII letters, digits, dots, hyphens or underscores".into(),
            ));
        }
        if !(64..=1_048_576).contains(&self.memory_mib)
            || !(1..=256).contains(&self.cpus)
            || !(1..=300).contains(&self.startup_timeout_secs)
            || self.shutdown_grace_secs > 120
        {
            return Err(Error::Config(
                "invalid memory, CPU count or lifecycle timeout".into(),
            ));
        }
        if let Some(hotplug) = &self.hotplug
            && (!(self.cpus..=256).contains(&hotplug.max_cpus)
                || !(self.memory_mib..=1_048_576).contains(&hotplug.max_memory_mib)
                || hotplug.cpu_overhead_millis > 64_000
                || hotplug.memory_overhead_mib > 65_536
                || !hotplug.limits_dir.is_absolute()
                || !hotplug.cgroup_dir.is_absolute())
        {
            return Err(Error::Config(
                    "hotplug maxima must cover the boot size within 256 vCPUs/1 TiB, with bounded overheads and absolute source directories".into(),
                ));
        }
        if let Some(uefi) = &self.uefi {
            if self.kernel.is_some() || self.initrd.is_some() || !self.kernel_args.is_empty() {
                return Err(Error::Config(
                    "UEFI boot uses its EFI boot disk, not a direct kernel/initrd/command line"
                        .into(),
                ));
            }
            if uefi.boot_disk.starts_with("/var/lib/qrab-media") {
                super::MediaPolicy::from_env()?
                    .ok_or_else(|| {
                        Error::Config(
                            "boot media requires an explicit approved media policy".into(),
                        )
                    })?
                    .validate_boot_disk(&uefi.boot_disk)?;
            }
            for path in [&uefi.code, &uefi.vars_template, &uefi.boot_disk] {
                required_file(path)?;
            }
            if std::fs::metadata(&uefi.vars_template)?.len() < 64 {
                return Err(Error::Config("UEFI variable template is empty".into()));
            }
        } else if self.kernel.is_none() {
            return Err(Error::Config(
                "select UEFI boot or supply a direct kernel".into(),
            ));
        }
        for path in std::iter::once(&self.qemu_binary)
            .chain(self.kernel.iter())
            .chain(self.initrd.iter())
        {
            required_file(path)?;
        }
        if !self.runtime_dir.is_absolute()
            || self
                .control_dir
                .as_ref()
                .is_some_and(|dir| !dir.is_absolute())
        {
            return Err(Error::Config(
                "runtime_dir and control_dir must be absolute".into(),
            ));
        }
        if self.uefi.is_some() || self.tpm.is_some() || !self.iscsi_disks.is_empty() {
            if !self.state_dir.is_absolute() || self.state_dir == self.runtime_dir {
                return Err(Error::Config(
                    "state_dir must be an absolute persistent directory separate from runtime_dir"
                        .into(),
                ));
            }
            let id = self.state_id.as_deref().unwrap_or(&self.name);
            if id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            {
                return Err(Error::Config(
                    "state_id must be 1..128 ASCII letters, digits, dots, hyphens or underscores"
                        .into(),
                ));
            }
        }
        if let Some(tpm) = &self.tpm {
            required_file(&tpm.swtpm_binary)?;
            for path in [&self.state_dir, &self.runtime_dir] {
                if path.to_str().is_none_or(|path| path.contains(',')) {
                    return Err(Error::Config(
                        "swtpm state/runtime paths must be UTF-8 without commas".into(),
                    ));
                }
            }
        }
        if let Some(path) = &self.firmware_dir
            && (!path.is_absolute() || !path.is_dir())
        {
            return Err(Error::Config(
                "firmware_dir must be an existing absolute directory".into(),
            ));
        }
        if let Some(disk) = &self.disk {
            let kind = std::fs::metadata(&disk.path)?.file_type();
            if !disk.path.is_absolute() || !(kind.is_file() || kind.is_block_device()) {
                return Err(Error::Config(
                    "disk must name an absolute regular file or block device".into(),
                ));
            }
        }
        super::iscsi::validate(&self.iscsi_disks)?;
        super::block::validate(&self.block_disks, &self.iscsi_disks)?;
        if let Some(seed) = &self.seed {
            seed.validate()?;
        }
        self.check_kvm()
    }

    /// A KVM VM must be able to open [`KVM_DEVICE`].
    pub(crate) fn check_kvm(&self) -> Result<()> {
        if self.accelerator == Accelerator::Kvm {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(KVM_DEVICE)
                .map_err(|e| Error::Config(format!("KVM requires access to {KVM_DEVICE}: {e}")))?;
        }
        Ok(())
    }
}

fn required_file(path: &Path) -> Result<()> {
    if !path.is_absolute() || !path.is_file() {
        return Err(Error::Config(format!(
            "required absolute file is missing: {}",
            path.display()
        )));
    }
    Ok(())
}
