use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Starting,
    Running,
    Stopping,
    /// QEMU is not running and the manager keeps serving its control socket
    /// until a power-on request or shutdown.
    Off,
    Exited,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub phase: Phase,
    pub ready: bool,
    pub manager_pid: u32,
    pub manager_start_ticks: u64,
    pub qemu_pid: Option<u32>,
    pub qemu_start_ticks: Option<u64>,
    pub qemu_version: Option<(u32, u32, u32)>,
    #[serde(default)]
    pub swtpm_pid: Option<u32>,
    #[serde(default)]
    pub swtpm_start_ticks: Option<u64>,
    pub run_state: Option<String>,
    pub observed_unix_ms: u64,
    pub exit_code: Option<i32>,
    pub detail: Option<String>,
    /// In-place vCPU/memory growth progress when hot-plug headroom is configured.
    #[serde(default)]
    pub resize: Option<Resize>,
    /// True while `Starting` or `Stopping` QEMU for a power transition inside
    /// this manager (power on, off or cycle), which keeps serving throughout.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub relaunch: bool,
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// A zombie still has a /proc record and start time, but cannot be healthy.
/// The final ')' belongs to comm, which may itself contain spaces/parentheses.
fn live_start_ticks(stat: &str) -> Result<u64> {
    let (_, tail) = stat
        .rsplit_once(") ")
        .ok_or_else(|| Error::Spawn("invalid process identity".into()))?;
    let mut fields = tail.split_whitespace();
    let state = fields.next().unwrap_or("");
    if state.len() != 1
        || !state.as_bytes()[0].is_ascii_alphabetic()
        || matches!(state, "Z" | "X" | "x")
    {
        return Err(Error::Spawn(
            "process is dead or its identity is invalid".into(),
        ));
    }
    // Field 3 (state) was consumed; starttime is field 22.
    fields
        .nth(18)
        .and_then(|ticks| ticks.parse().ok())
        .ok_or_else(|| Error::Spawn("invalid process start time".into()))
}

/// PID reuse must not make an old readiness record look current.
pub(super) fn start_ticks(pid: u32) -> Result<u64> {
    live_start_ticks(&std::fs::read_to_string(format!("/proc/{pid}/stat"))?)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestSize {
    pub cpus: u32,
    pub memory_mib: u64,
}

/// Where in-place growth stands. Not sensitive: viewers see it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizePhase {
    /// The guest has the desired size, or no resize has been requested.
    #[default]
    Idle,
    /// The limit files ask for more than the applied cgroup limits carry.
    WaitingForLimits,
    /// A hot-plug step failed; the next attempt waits `retry_after_unix_ms`.
    Retrying,
    /// The desired size exceeds the boot-time maximum.
    RestartRequired,
    /// Container limit or cgroup files cannot be read.
    LimitsUnavailable,
}

/// Resize progress published with the runtime status.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resize {
    pub boot: GuestSize,
    pub maximum: GuestSize,
    /// Size requested by the container limits (limit minus fixed overhead).
    pub desired: Option<GuestSize>,
    /// Size the applied cgroup limits can carry.
    pub permitted: Option<GuestSize>,
    /// Present vCPUs and boot plus requested virtio-mem memory.
    pub current: GuestSize,
    /// Memory the guest's virtio-mem driver has actually plugged.
    pub plugged_memory_mib: Option<u64>,
    pub detail: Option<String>,
    #[serde(default)]
    pub phase: ResizePhase,
    /// Consecutive failed hot-plug steps; reset by a successful step.
    #[serde(default)]
    pub failures: u32,
    /// The most recent hot-plug failure, kept until a step succeeds.
    #[serde(default)]
    pub last_error: Option<String>,
    /// No QMP hot-plug attempt is made before this time unless the
    /// container limits change.
    #[serde(default)]
    pub retry_after_unix_ms: Option<u64>,
}

/// Where a supervisor's published status records are mirrored in memory.
#[derive(Clone, Default)]
pub(crate) struct StatusSink(Option<std::sync::Arc<tokio::sync::watch::Sender<Option<Status>>>>);

impl StatusSink {
    pub(super) fn new(sender: std::sync::Arc<tokio::sync::watch::Sender<Option<Status>>>) -> Self {
        Self(Some(sender))
    }

    pub(super) fn send(&self, status: &Status) {
        if let Some(sink) = &self.0 {
            sink.send_replace(Some(status.clone()));
        }
    }
}

impl std::fmt::Debug for StatusSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StatusSink")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(state: &str) -> String {
        let mut fields = vec!["0"; 20];
        fields[0] = state;
        fields[19] = "12345";
        format!("42 (name with ) parentheses) {}", fields.join(" "))
    }

    #[test]
    fn process_identity_handles_comm_and_rejects_dead_states() {
        for state in ["R", "S", "T", "I"] {
            assert_eq!(live_start_ticks(&record(state)).unwrap(), 12345);
        }
        for state in ["Z", "X", "x", "", "invalid"] {
            assert!(
                live_start_ticks(&record(state)).is_err(),
                "accepted state {state:?}"
            );
        }
        assert!(live_start_ticks("truncated").is_err());
        assert!(live_start_ticks("42 (process) R 0").is_err());
    }
}
