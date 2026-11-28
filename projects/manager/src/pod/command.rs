//! The QEMU command line of a [`Config`]. Each device module contributes its
//! own arguments, so the configuration itself depends on none of them.
use super::MediaPolicy;
use super::{Accelerator, Config};
use super::{HOTPLUG_BLOCK_MIB, hotplug::MEMORY_DEVICE};
use super::{block, iscsi, seed};
use crate::{Error, Result};
use std::{fmt, path::Path, process::Stdio, time::Duration};

/// A QEMU release, as `qemu-system-x86_64 --version` reports it. The manager
/// starts QEMU [`OLDEST`](Self::OLDEST) or newer and chooses each startup
/// argument that changed in that range by the release it starts.
///
/// ```
/// use manager::pod::QemuVersion;
/// let debian = "QEMU emulator version 10.0.13 (Debian 1:10.0.13+ds-0+deb13u1)\n\
///     Copyright (c) 2003-2025 Fabrice Bellard and the QEMU Project developers\n";
/// assert_eq!(QemuVersion::parse(debian), Some(QemuVersion(10, 0, 13)));
/// assert_eq!(QemuVersion::parse("QEMU emulator version 11.1.3\n"), Some(QemuVersion(11, 1, 3)));
/// assert_eq!(QemuVersion::parse("QEMU emulator version 11.1\n"), Some(QemuVersion(11, 1, 0)));
/// assert_eq!(QemuVersion::parse("sleep (GNU coreutils) 9.7\n"), None);
/// assert!(QemuVersion(8, 2, 2) >= QemuVersion::OLDEST);
/// assert!(QemuVersion(7, 2, 15) < QemuVersion::OLDEST);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct QemuVersion(pub u32, pub u32, pub u32);

impl QemuVersion {
    /// The oldest QEMU the manager starts: Ubuntu 24.04's QEMU 8.2.
    pub const OLDEST: Self = Self(8, 2, 0);
    /// The newest QEMU the manager is tested with; [`Config::arguments`]
    /// renders its command line.
    pub const NEWEST: Self = Self(11, 1, 3);
    /// QEMU 11.1 creates monitors as `monitor-qmp` objects; older releases
    /// take `-mon`.
    const MONITOR_QMP: Self = Self(11, 1, 0);

    /// Parses the first line of `--version`'s output.
    pub fn parse(output: &str) -> Option<Self> {
        let version = output
            .lines()
            .next()?
            .strip_prefix("QEMU emulator version ")?
            .split_whitespace()
            .next()?;
        let mut parts = version.split('.').map(str::parse::<u32>);
        let major = parts.next()?.ok()?;
        let minor = parts.next()?.ok()?;
        let micro = parts.next().transpose().ok()?.unwrap_or(0);
        Some(Self(major, minor, micro))
    }

    /// Asks `binary --version`, for at most five seconds, and refuses a QEMU
    /// older than [`OLDEST`](Self::OLDEST).
    pub async fn of(binary: &Path) -> Result<Self> {
        let probe = tokio::process::Command::new(binary)
            .arg("--version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output();
        let output = tokio::time::timeout(Duration::from_secs(5), probe)
            .await
            .map_err(|_| Error::Spawn(format!("{} --version did not answer", binary.display())))?
            .map_err(Error::Io)?;
        let version = Self::parse(&String::from_utf8_lossy(&output.stdout))
            .filter(|_| output.status.success())
            .ok_or_else(|| {
                Error::Spawn(format!(
                    "{} --version names no QEMU release",
                    binary.display()
                ))
            })?;
        if version < Self::OLDEST {
            return Err(Error::Config(format!(
                "qrab-manager requires QEMU {} or newer; {} is QEMU {version}",
                Self::OLDEST,
                binary.display()
            )));
        }
        Ok(version)
    }
}

impl fmt::Display for QemuVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

impl Config {
    /// QEMU's command line for [`QemuVersion::NEWEST`].
    pub fn arguments(&self, socket: &Path) -> Vec<String> {
        self.arguments_for(socket, QemuVersion::NEWEST)
    }

    /// QEMU's command line for the QEMU release that runs it. JSON blockdev
    /// options keep filenames with commas or spaces unambiguous.
    pub fn arguments_for(&self, socket: &Path, qemu: QemuVersion) -> Vec<String> {
        let hotplug_memory = self.hotplug_memory_mib();
        let memory = if hotplug_memory == 0 {
            self.memory_mib.to_string()
        } else {
            format!(
                "size={}M,maxmem={}M",
                self.memory_mib,
                self.memory_mib + hotplug_memory
            )
        };
        let smp = if self.max_cpus() > self.cpus {
            format!("cpus={},maxcpus={}", self.cpus, self.max_cpus())
        } else {
            self.cpus.to_string()
        };
        let mut machine = String::from("q35");
        if self.uefi.as_ref().is_some_and(|u| u.secure_boot_capable) {
            machine.push_str(",smm=on");
        }
        if self.uefi.is_some() {
            machine.push_str(",pflash0=pflash-code,pflash1=pflash-vars");
        }
        let accel = match self.accelerator {
            Accelerator::Tcg => "tcg,thread=multi".to_owned(),
            Accelerator::Kvm => "kvm".to_owned(),
        };
        let mut args: Vec<String> = [
            "-name", &self.name, "-machine", &machine, "-m", &memory, "-smp", &smp,
        ]
        .map(str::to_owned)
        .into();
        args.extend(
            [
                "-accel",
                &accel,
                "-cpu",
                match self.accelerator {
                    Accelerator::Tcg => "max",
                    Accelerator::Kvm => "host",
                },
                "-nodefaults",
                "-display",
                "none",
                "-S",
            ]
            .map(str::to_owned),
        );
        args.extend(["-nic".into(), "none".into()]);
        args.extend([
            "-chardev".into(),
            "stdio,id=serial,signal=off".into(),
            "-serial".into(),
            "chardev:serial".into(),
            "-chardev".into(),
            format!(
                "socket,id=qmp,path={},server=on,wait=off",
                socket.to_string_lossy().replace(',', ",,")
            ),
        ]);
        if qemu >= QemuVersion::MONITOR_QMP {
            args.extend([
                "-object".into(),
                serde_json::json!({"qom-type":"monitor-qmp", "id":"monitor", "chardev":"qmp"})
                    .to_string(),
            ]);
        } else {
            args.extend(["-mon".into(), "chardev=qmp,mode=control".into()]);
        }
        if self.guest_agent {
            args.extend([
                "-device".into(),
                "virtio-serial-pci,id=qga-serial".into(),
                "-chardev".into(),
                format!(
                    "socket,id=qga0,path={},server=on,wait=off",
                    socket
                        .with_file_name("qga.sock")
                        .to_string_lossy()
                        .replace(',', ",,")
                ),
                "-device".into(),
                "virtserialport,bus=qga-serial.0,chardev=qga0,name=org.qemu.guest_agent.0".into(),
            ]);
        }
        if let Some(uefi) = &self.uefi {
            let vars = self.state_dir.join("identity/OVMF_VARS.fd");
            for (name, path, read_only) in [
                ("pflash-code", &uefi.code, true),
                ("pflash-vars", &vars, false),
            ] {
                args.extend([
                    "-blockdev".into(), serde_json::json!({"driver":"file", "filename":path, "node-name":format!("{name}-file"), "read-only":read_only}).to_string(),
                    "-blockdev".into(), serde_json::json!({"driver":"raw", "file":format!("{name}-file"), "node-name":name, "read-only":read_only}).to_string(),
                ]);
            }
            args.extend([
                "-blockdev".into(), serde_json::json!({"driver":"file", "filename":uefi.boot_disk, "node-name":"boot-file", "read-only":true}).to_string(),
                "-blockdev".into(), serde_json::json!({"driver":"raw", "file":"boot-file", "node-name":"boot", "read-only":true}).to_string(),
                "-device".into(), "virtio-blk-pci,drive=boot,bootindex=1".into(),
            ]);
        } else if let Some(kernel) = &self.kernel {
            args.extend(["-kernel".into(), kernel.to_string_lossy().into_owned()]);
        }
        if self.uefi.as_ref().is_some_and(|u| u.secure_boot_capable) {
            args.extend([
                "-global".into(),
                "driver=cfi.pflash01,property=secure,value=on".into(),
            ]);
        }
        if self.tpm.is_some() {
            args.extend([
                "-chardev".into(),
                format!(
                    "socket,id=tpm,path={}",
                    socket
                        .with_file_name("swtpm.sock")
                        .to_string_lossy()
                        .replace(',', ",,")
                ),
                "-tpmdev".into(),
                "emulator,id=tpm0,chardev=tpm".into(),
                "-device".into(),
                "tpm-tis,tpmdev=tpm0".into(),
            ]);
        }
        if let Some(initrd) = &self.initrd {
            args.extend(["-initrd".into(), initrd.to_string_lossy().into_owned()]);
        }
        if !self.kernel_args.is_empty() {
            args.extend(["-append".into(), self.kernel_args.clone()]);
        }
        if self.console {
            args.extend([
                "-device".into(),
                "VGA,vgamem_mb=16".into(),
                "-device".into(),
                "qemu-xhci,id=usb".into(),
                "-device".into(),
                "usb-tablet,bus=usb.0".into(),
            ]);
            let vnc = format!(
                "unix:{},share=force-shared",
                socket
                    .with_file_name("vnc.sock")
                    .to_string_lossy()
                    .replace(',', ",,")
            );
            args.extend(["-vnc".into(), vnc]);
        }
        if let Some(path) = &self.firmware_dir {
            args.extend(["-L".into(), path.to_string_lossy().into_owned()]);
        }
        if let Some(disk) = &self.disk {
            args.extend(["-blockdev".into(), serde_json::json!({"driver":"file", "filename":disk.path, "node-name":"disk-file", "read-only":disk.read_only}).to_string(),
                "-blockdev".into(), serde_json::json!({"driver":disk.format, "file":"disk-file", "node-name":"disk", "read-only":disk.read_only}).to_string(),
                "-device".into(), "virtio-blk-pci,drive=disk".into()]);
        }
        if hotplug_memory > 0 {
            // Unplugged blocks are reserved address space only; the guest
            // touches memory after the supervisor raises requested-size.
            args.extend([
                "-object".into(),
                serde_json::json!({"qom-type":"memory-backend-ram", "id":"hotplug-memory", "size":hotplug_memory << 20}).to_string(),
                "-device".into(),
                format!("virtio-mem-pci,id={},memdev=hotplug-memory,requested-size=0,block-size={HOTPLUG_BLOCK_MIB}M", MEMORY_DEVICE),
            ]);
        }
        if let Ok(Some(policy)) = MediaPolicy::from_env() {
            policy.arguments(&mut args);
        }
        iscsi::arguments(&self.iscsi_disks, &mut args);
        block::arguments(&self.block_disks, &mut args);
        if self.seed.is_some() {
            seed::arguments(socket, &mut args);
        }
        args
    }
}
