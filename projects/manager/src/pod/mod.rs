//! QEMU supervision inside an existing container sandbox. It needs no host
//! namespaces and no Kubernetes API.

mod agent;
mod block;
mod boot;
mod command;
mod config;
mod control;
mod disks;
mod events;
mod firmware;
mod hotplug;
mod inventory;
mod iscsi;
mod media;
mod metrics;
mod network;
mod persistent;
mod process;
mod remote;
mod resources;
mod secure_boot_policy;
mod secure_boot_store;
mod seed;
mod serial;
pub mod socket;
mod state;
mod status_file;
mod supervisor;
mod tpm;
mod view;

pub use agent::{AgentStatus, FreezeState, UpdateState, UpdateStatus};
pub use block::BlockDisk;
pub use boot::{BootOption, BootSnapshot, BootState};
pub use command::QemuVersion;
pub use config::{
    Accelerator, Config, Disk, DiskFormat, HOTPLUG_BLOCK_MIB, Hotplug, KVM_DEVICE, Tpm, Uefi,
};
pub use control::{Action, Control, ControlReceiver, control_channel, control_channel_for};
pub use events::{
    EVENT_CAPACITY, EVENT_DATA_LIMIT, EVENT_PAGE_LIMIT, EventPage, POWER_EVENT, QemuTime, VmEvent,
};
pub use firmware::{FirmwareArtifact, FirmwareCatalogView, FirmwareStageStatus};
pub use firmware::{
    FirmwareSnapshot, FirmwareState, SecureBootDatabaseInventory, SecureBootDatabaseSnapshot,
    SecureBootSignatureInventory, SecureBootUpdateRequest, validate_secure_boot_update,
};
pub use inventory::{DriveCounters, DriveInventory, HardwareInventory, ProcessorInventory};
pub use iscsi::{IscsiBus, IscsiDisk};
pub use media::{MEDIA_DEVICE, MediaOperation, MediaPolicy, MediaStatus};
pub use metrics::VmMetrics;
pub use network::{GuestNetworkSnapshot, NetworkAddress, NetworkInterface, NetworkStatistics};
pub use remote::{Backend, POOL_SIZE as REMOTE_POOL_SIZE, Remote};
pub use resources::{ContainerResources, ProcessResources, ResourceUsage};
pub use secure_boot_policy::SecureBootSettings;
pub use seed::{SEED_IMAGE, SEED_IMAGE_BYTES, SEED_INPUT_LIMIT, Seed};
pub use serial::{SERIAL_CAPACITY, SerialSnapshot};
pub use state::{GuestSize, Resize, ResizePhase};
pub use state::{Phase, Status};
pub use status_file::{probe, status};
pub use supervisor::{Outcome, run, run_controlled};
pub use view::HotplugView;
pub use view::{BootMode, ConfigView, Deployment, DiskView, NicView, PathView};
pub use view::{IscsiTargetView, SeedView};
