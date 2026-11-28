use super::{
    Config, Phase, Status,
    control::PowerStop,
    persistent::{FileLock, PersistentState, StateLease},
    process::ManagedChild,
    state::start_ticks,
    status_file::publish,
    tpm::Swtpm,
};
use crate::{Error, Result};
use qapi::commands::{Cont, QueryStatus, Quit, SystemPowerdown};
use qapi_protocol::{ClientOptions, Qmp, TimeoutPhase};
use rustix::process::{Pid, Signal};
use serde::Serialize;
use std::{
    fs::OpenOptions,
    future::Future,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::{
    process::{Child, Command},
    sync::oneshot,
    time::timeout,
};

/// Requests from other managers to receive this VM by live migration, which
/// the supervision loops poll. Live migration plugs in here; this build
/// receives no requests.
struct Migrations;

impl Migrations {
    async fn next(&mut self) -> Option<std::convert::Infallible> {
        std::future::pending().await
    }
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub reason: String,
    pub exit_code: Option<i32>,
    pub shutdown_method: String,
}

/// How one QEMU process (a power-on cycle) ended.
enum Cycle {
    /// The manager exits with this outcome.
    Exit(Outcome),
    /// QEMU stopped cleanly for this reason; the VM rests powered off.
    Off(&'static str),
    /// Power cycle: start a fresh QEMU at once.
    Restart,
}

/// Why the supervision loop of a running QEMU ended.
enum Ended {
    /// QEMU exited by itself.
    Exited(ExitStatus),
    /// The manager is shutting down.
    Shutdown,
    /// A control request stops this QEMU.
    Stop(PowerStop),
}

async fn reap_after_kill(child: &mut Child) -> Result<ExitStatus> {
    child.start_kill()?;
    timeout(Duration::from_secs(2), child.wait())
        .await
        .map_err(|_| Error::Spawn("QEMU did not reap after SIGKILL".into()))?
        .map_err(Error::Io)
}

async fn stop_child(
    child: &mut Child,
    qmp: Option<&Qmp>,
    grace: Duration,
) -> Result<(ExitStatus, &'static str)> {
    if let Some(status) = child.try_wait()? {
        return Ok((status, "already-exited"));
    }
    if let Some(qmp) = qmp {
        let _ = qmp.execute(&SystemPowerdown::default()).await;
    } else if let Some(pid) = child.id().and_then(|pid| Pid::from_raw(pid as i32)) {
        let _ = rustix::process::kill_process(pid, Signal::TERM);
    }
    if let Ok(result) = timeout(grace, child.wait()).await {
        return Ok((result?, "guest-or-term"));
    }
    if let Some(qmp) = qmp {
        let _ = qmp.execute(&Quit::default()).await;
        if let Ok(result) = timeout(Duration::from_secs(2), child.wait()).await {
            return Ok((result?, "qmp-quit"));
        }
    }
    Ok((reap_after_kill(child).await?, "sigkill"))
}

/// Reap QEMU after QMP `quit` was sent, killing it if it does not exit soon.
async fn reap_after_quit(child: &mut Child) -> Result<(ExitStatus, &'static str)> {
    if let Ok(result) = timeout(Duration::from_secs(2), child.wait()).await {
        return Ok((result?, "qmp-quit"));
    }
    Ok((reap_after_kill(child).await?, "sigkill"))
}

/// Supervise a VM like [`run_controlled`], without an in-process
/// [`Control`](super::Control). kill_on_drop covers cancellation of this
/// future; explicit paths always wait for and reap each QEMU child.
pub async fn run(config: Config, shutdown: impl Future<Output = ()>) -> Result<Outcome> {
    let (_control, receiver) = super::control_channel();
    run_controlled(config, shutdown, receiver).await
}

/// Supervise a VM with a bounded in-process control channel.
///
/// The manager is the VM's init: the runtime lock, control socket and
/// persistent-state lease live as long as this call, across every QEMU it
/// starts. A requested or guest-initiated power off leaves the VM `Off` until
/// a power-on request; a power cycle starts a fresh QEMU at once. The call
/// returns how supervision ended: an [`Outcome`], such as `shutdown`
/// resolving, or the error that failed the VM.
pub async fn run_controlled(
    mut config: Config,
    shutdown: impl Future<Output = ()>,
    mut control: super::ControlReceiver,
) -> Result<Outcome> {
    config.validate()?;
    config.status_sink = control.status_sink();
    std::fs::create_dir_all(&config.runtime_dir)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(config.runtime_dir.join("owner.lock"))?;
    let _lock = FileLock::exclusive(lock)
        .map_err(|_| Error::Config("another manager owns this runtime directory".into()))?;
    control.set_config(&config);
    // Declared after the lock, so the socket server stops before it is released.
    let _control_socket = match config.control_socket {
        true => Some(super::socket::spawn(&config, control.control())?),
        false => None,
    };
    let mut persistent = PersistentState::prepare(&config)?;
    let _firmware_lease = control.firmware_lease(persistent.as_ref().map(PersistentState::lease));

    let mut state = Status {
        phase: Phase::Starting,
        ready: false,
        manager_pid: std::process::id(),
        manager_start_ticks: start_ticks(std::process::id())?,
        qemu_pid: None,
        qemu_start_ticks: None,
        qemu_version: None,
        swtpm_pid: None,
        swtpm_start_ticks: None,
        run_state: None,
        observed_unix_ms: 0,
        exit_code: None,
        detail: None,
        resize: super::Resize::new(&config),
        relaunch: false,
    };
    let mut shutdown = Box::pin(shutdown);
    let base = config;
    // Why the next QEMU starts; `None` for the first one.
    let mut relaunch: Option<&'static str> = None;
    let mut migrations = Migrations;
    loop {
        let cycle = power_on(
            &base,
            &mut shutdown,
            &mut control,
            &mut state,
            &mut persistent,
            &mut migrations,
            relaunch,
        )
        .await;
        match cycle {
            Ok(Cycle::Exit(outcome)) => {
                state.ready = false;
                publish(&base, &mut state)?;
                return Ok(outcome);
            }
            Ok(Cycle::Restart) => relaunch = Some("power-cycle"),
            Ok(Cycle::Off(reason)) => {
                if !powered_off(
                    &base,
                    &mut shutdown,
                    &mut control,
                    &mut state,
                    &mut migrations,
                    reason,
                )
                .await?
                {
                    return Ok(Outcome {
                        reason: "shutdown-requested".into(),
                        exit_code: state.exit_code,
                        shutdown_method: "powered-off".into(),
                    });
                }
                relaunch = Some("power-on");
            }
            Err(error) => {
                control.set_stopping();
                state.phase = Phase::Failed;
                state.detail = Some(error.to_string());
                state.ready = false;
                publish(&base, &mut state)?;
                return Err(error);
            }
        }
    }
}

/// One power-on cycle: a fresh session directory, staged firmware and Secure
/// Boot settings applied, swtpm, then QEMU until it ends.
async fn power_on(
    base: &Config,
    shutdown: &mut (impl Future<Output = ()> + Unpin),
    control: &mut super::ControlReceiver,
    state: &mut Status,
    persistent: &mut Option<PersistentState>,
    migrations: &mut Migrations,
    relaunch: Option<&'static str>,
) -> Result<Cycle> {
    let mut config = base.clone();
    if let Some(reason) = relaunch {
        // An in-place power cycle takes the seed from the config file as it
        // is now.
        if let Some(fresh) = config.source.as_deref().and_then(|p| Config::read(p).ok()) {
            config.seed = fresh.seed;
        }
        control.set_powering_on();
        control.record_power("PoweringOn", reason);
        eprintln!(
            "{}",
            serde_json::json!({"event":"qemu_relaunch","reason":reason})
        );
    }
    *state = Status {
        phase: Phase::Starting,
        ready: false,
        qemu_pid: None,
        qemu_start_ticks: None,
        qemu_version: None,
        swtpm_pid: None,
        swtpm_start_ticks: None,
        run_state: None,
        exit_code: None,
        detail: None,
        relaunch: relaunch.is_some(),
        ..state.clone()
    };
    let session = tempfile::Builder::new()
        .prefix("session-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(&config.runtime_dir)?;
    let socket = session.path().join("qmp.sock");
    let console = config.console.then(|| session.path().join("vnc.sock"));
    let socket_too_long = |enabled: bool, name: &str| {
        enabled
            && session
                .path()
                .join(name)
                .as_os_str()
                .as_encoded_bytes()
                .len()
                > 107
    };
    let too_long = socket_too_long(true, "qmp.sock")
        || socket_too_long(config.tpm.is_some(), "swtpm.sock")
        || socket_too_long(config.guest_agent, "qga.sock");
    if too_long {
        return Err(Error::Config(
            "runtime directory is too long for a QMP Unix socket".into(),
        ));
    }
    if let Some(seed) = &config.seed {
        seed.write(&super::seed::image_path(&socket))?;
    }
    apply_staged(control, &mut config).await?;
    config.validate()?;
    control.set_config(&config);
    state.resize = super::Resize::new(&config);
    publish(&config, state)?;
    let mut tpm = None;
    let result = async {
        if let Some(tpm_config) = &config.tpm {
            let identity = persistent.as_ref().expect("persistent TPM state");
            tpm = Some(Swtpm::spawn(
                tpm_config,
                session.path(),
                &identity.tpm_directory(),
                identity.lease(),
            )?);
            let process = tpm.as_mut().expect("TPM process");
            state.swtpm_pid = process.id();
            state.swtpm_start_ticks = process.id().and_then(|pid| start_ticks(pid).ok());
            publish(&config, state)?;
            let ready = process.ready();
            tokio::pin!(ready);
            let deadline = tokio::time::sleep(Duration::from_secs(config.startup_timeout_secs));
            tokio::pin!(deadline);
            // Keep the heartbeat fresh: a relaunch is live while it starts.
            let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
            let ready = loop {
                tokio::select! {
                    _=&mut *shutdown => break None,
                    _=&mut deadline => break Some(Err(())),
                    result=&mut ready => break Some(Ok(result)),
                    _=heartbeat.tick() => publish(&config, state)?,
                }
            };
            match ready {
                None => {
                    control.set_stopping();
                    state.phase = Phase::Exited;
                    return Ok(Cycle::Exit(Outcome {
                        reason: "shutdown-during-tpm-startup".into(),
                        exit_code: None,
                        shutdown_method: "swtpm-term".into(),
                    }));
                }
                Some(Ok(result)) => result?,
                Some(Err(())) => return Err(Error::Spawn("swtpm startup timed out".into())),
            }
            persistent
                .as_mut()
                .expect("persistent TPM state")
                .mark_tpm_initialized()?;
        }
        run_qemu(
            &config,
            &mut *shutdown,
            control,
            (&socket, console.as_deref()),
            state,
            &mut tpm,
            persistent.as_ref().map(PersistentState::lease),
            migrations,
            relaunch,
        )
        .await
    }
    .await;
    // Native iSCSI disks are released only after a clean swtpm exit.
    let require_clean = !config.iscsi_disks.is_empty();
    match (&mut tpm, result) {
        (Some(process), result) => match process.stop(require_clean).await {
            Ok(()) => result,
            Err(error) => Err(error),
        },
        (None, result) => result,
    }
}

/// Serve the control socket with QEMU powered off until a power-on request
/// (true) or shutdown (false).
async fn powered_off(
    config: &Config,
    shutdown: &mut (impl Future<Output = ()> + Unpin),
    control: &mut super::ControlReceiver,
    state: &mut Status,
    migrations: &mut Migrations,
    reason: &'static str,
) -> Result<bool> {
    control.set_off();
    *state = Status {
        phase: Phase::Off,
        ready: false,
        qemu_pid: None,
        qemu_start_ticks: None,
        qemu_version: None,
        swtpm_pid: None,
        swtpm_start_ticks: None,
        run_state: None,
        detail: None,
        resize: super::Resize::new(config),
        relaunch: false,
        ..state.clone()
    };
    publish(config, state)?;
    control.record_power("Off", reason);
    eprintln!(
        "{}",
        serde_json::json!({"event":"powered_off","reason":reason,"exit_code":state.exit_code})
    );
    let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            biased;
            _ = &mut *shutdown => {
                control.set_stopping();
                state.phase = Phase::Exited;
                publish(config, state)?;
                return Ok(false);
            }
            Some(request) = control.requests.recv() => {
                if request.powered_off() {
                    return Ok(true);
                }
            }
            Some(pending) = migrations.next() => {
                match pending {}
            }
            _ = heartbeat.tick() => publish(config, state)?,
        }
    }
}

/// A power cycle is where staged firmware and Secure Boot settings take
/// effect.
async fn apply_staged(control: &super::ControlReceiver, config: &mut Config) -> Result<()> {
    let _staging = control.firmware_guard().await;
    super::firmware::apply_staged_firmware(config)?;
    super::secure_boot_policy::apply(config)
}

#[allow(clippy::too_many_arguments)]
async fn run_qemu(
    config: &Config,
    shutdown: impl Future<Output = ()>,
    control: &mut super::ControlReceiver,
    sockets: (&Path, Option<&Path>),
    state: &mut Status,
    tpm: &mut Option<Swtpm>,
    lease: Option<StateLease>,
    migrations: &mut Migrations,
    relaunch: Option<&'static str>,
) -> Result<Cycle> {
    let (socket, console) = sockets;
    // The startup arguments that changed between releases follow the one
    // that runs.
    let qemu = match super::QemuVersion::of(&config.qemu_binary).await {
        Ok(qemu) => qemu,
        Err(error) => {
            state.phase = Phase::Failed;
            state.detail = Some(error.to_string());
            let _ = publish(config, state);
            return Err(error);
        }
    };
    let mut command = Command::new(&config.qemu_binary);
    command
        .args(config.arguments_for(socket, qemu))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    let spawned = command.spawn().map_err(Error::Io);
    drop(command);
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            state.phase = Phase::Failed;
            state.detail = Some(error.to_string());
            let _ = publish(config, state);
            return Err(error);
        }
    };
    let serial = child
        .stdout
        .take()
        .map(|stdout| control.pump_serial(stdout));
    let _serial = SerialPump(serial);
    let mut child = ManagedChild::new(child, lease);
    state.qemu_pid = Some(child.spawned_pid());
    state.qemu_start_ticks = start_ticks(child.spawned_pid()).ok();
    publish(config, state)?;
    let mut shutdown = Box::pin(shutdown);
    let options = ClientOptions::default()
        .with_connect_timeout(Duration::from_millis(200))
        .with_handshake_timeout(Duration::from_secs(1))
        .with_command_timeout(Duration::from_secs(2))
        // Headroom for bursts; events are also drained by their own task.
        .with_event_capacity(1024);
    let startup = async {
        loop {
            if let Some(exit) = child.try_wait()? {
                return Err(Error::Spawn(format!("QEMU exited during startup: {exit}")));
            }
            match Qmp::connect_with_options(socket, options.clone()).await {
                Ok(connected) => return Ok(connected),
                Err(qapi_protocol::Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                    ) => {}
                Err(qapi_protocol::Error::Timeout {
                    phase: TimeoutPhase::Connect | TimeoutPhase::Handshake,
                    ..
                }) => {}
                Err(error) => return Err(Error::Protocol(error)),
            }
            publish(config, state)?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    let qmp = tokio::select! {
        result=timeout(Duration::from_secs(config.startup_timeout_secs),startup) => match result {
            Ok(Ok(connected)) => Some(connected),
            failure => {
                let detail = match failure { Ok(Err(error))=>error.to_string(), Err(_)=>"QEMU startup timed out".into(), _=>unreachable!() };
                if let Ok((exit,_))=stop_child(&mut child,None,Duration::from_secs(1)).await {state.exit_code=exit.code();}
                state.phase=Phase::Failed;state.detail=Some(detail.clone());let _=publish(config,state);
                return Err(Error::Spawn(detail));
            }
        },
        _=&mut shutdown => None,
    };
    let Some((qmp, events)) = qmp else {
        control.set_stopping();
        let (exit, method) = stop_child(&mut child, None, Duration::from_secs(1)).await?;
        state.exit_code = exit.code();
        native_clean_shutdown(config, &exit, method)?;
        state.phase = Phase::Exited;
        publish(config, state)?;
        return Ok(Cycle::Exit(Outcome {
            reason: "shutdown-during-startup".into(),
            exit_code: exit.code(),
            shutdown_method: method.into(),
        }));
    };
    state.qemu_version = Some(qmp.version());
    eprintln!(
        "{}",
        serde_json::json!({"event":"qmp_connected","version":qmp.version(),"qemu_pid":state.qemu_pid})
    );
    let pump = control.pump_events(events);
    // QEMU created the agent chardev before QMP became available.
    let agent = config.guest_agent.then(|| {
        super::agent::GuestAgent::spawn_publishing(
            socket.with_file_name("qga.sock"),
            Default::default(),
            control.agent_status_publisher(),
        )
    });
    let execution = async {
        let oldest = super::QemuVersion::OLDEST;
        if qmp.version() < (oldest.0, oldest.1, oldest.2) { return Err(Error::Config(format!("qrab-manager requires QEMU {oldest} or newer"))); }
        let cont = Cont::default();
        tokio::select! {
            biased;
            _=&mut shutdown => return Ok(Ended::Shutdown),
            started=qmp.execute(&cont) => {started?;}
        }
        if let Some(reason) = relaunch { control.record_power("On", reason); }
        let mut heartbeat=tokio::time::interval(Duration::from_secs(1));
        let mut disk_growth=super::disks::DiskGrowth::default();
        loop {
            tokio::select! {
                _=&mut shutdown => return Ok(Ended::Shutdown),
                exit=child.wait() => return Ok(Ended::Exited(exit?)),
                exit=super::tpm::wait(tpm) => return Err(Error::Spawn(format!("swtpm exited while QEMU was active: {}", exit?))),
                Some(pending)=migrations.next() => {
                    match pending {}
                },
                Some(request)=control.requests.recv() => {
                    let stop = request.dispatch(&qmp, super::control::Devices {
                        console,
                        agent: agent.as_ref(),
                    }).await;
                    // A slow command is when the QMP event queue can overflow.
                    control.set_qmp_dropped(qmp.dropped_events());
                    if let Some(stop) = stop { return Ok(Ended::Stop(stop)); }
                },
                reason=qmp.closed() => {
                    state.ready=false;state.phase=Phase::Stopping;publish(config,state)?;
                    if let Ok(exit)=timeout(Duration::from_secs(2),child.wait()).await { return Ok(Ended::Exited(exit?)); }
                    return Err(Error::Spawn(format!("QMP disconnected: {reason}")));
                },
                _=heartbeat.tick() => {
                    let status=qmp.execute(&QueryStatus::default()).await?;
                    state.phase=Phase::Running;state.ready=status.running;state.run_state=Some(status.status.to_string());
                    if let Some(resize)=state.resize.as_mut() { super::hotplug::reconcile(config,&qmp,resize).await; }
                    super::disks::reconcile(config,&qmp,&mut disk_growth).await;
                    control.set_qmp_dropped(qmp.dropped_events());
                    publish(config,state)?;
                }
            }
        }
    }.await;
    drop(agent);
    control.set_stopping();
    state.ready = false;
    let result = match execution {
        Ok(Ended::Exited(exit)) => {
            state.exit_code = exit.code();
            if !exit.success() {
                state.phase = Phase::Failed;
                Err(Error::Spawn(format!("QEMU exited unexpectedly: {exit}")))
            } else {
                // The guest powered itself off.
                Ok(Cycle::Off("guest-poweroff"))
            }
        }
        other => {
            let stop = match &other {
                Ok(Ended::Stop(stop)) => Some(*stop),
                _ => None,
            };
            state.phase = Phase::Stopping;
            state.relaunch = stop.is_some();
            let _ = publish(config, state);
            let grace = Duration::from_secs(config.shutdown_grace_secs);
            let stopping = async {
                match stop {
                    Some(PowerStop::ForceOff | PowerStop::ColdRestart) => {
                        reap_after_quit(&mut child).await
                    }
                    Some(PowerStop::PowerOff) | None => {
                        stop_child(&mut child, Some(&qmp), grace).await
                    }
                }
            };
            let graceful = stop == Some(PowerStop::PowerOff);
            // A force off during a graceful stop cuts the ACPI grace short.
            if graceful {
                control.set_graceful_stop();
            }
            let mut forced = false;
            let stopped = match drive_stop(stopping, config, control, state, graceful).await {
                Ok(stopped) => stopped,
                Err(reply) => {
                    forced = true;
                    control.set_stopping();
                    eprintln!("{}", serde_json::json!({"event":"power_stop_escalated"}));
                    let quitting = async {
                        // SIGKILL follows a failed quit, so the force off is in effect.
                        let _ = qmp.execute(&Quit::default()).await;
                        let _ = reply.send(Ok(()));
                        reap_after_quit(&mut child).await
                    };
                    match drive_stop(quitting, config, control, state, false).await {
                        Ok(stopped) => stopped,
                        Err(_) => unreachable!("a forced stop does not escalate"),
                    }
                }
            };
            match (other, stopped) {
                (Ok(ended), Ok((exit, method))) => {
                    state.exit_code = exit.code();
                    native_clean_shutdown(config, &exit, method)?;
                    match ended {
                        Ended::Shutdown => {
                            state.phase = Phase::Exited;
                            Ok(Cycle::Exit(Outcome {
                                reason: "shutdown-requested".into(),
                                exit_code: exit.code(),
                                shutdown_method: method.into(),
                            }))
                        }
                        Ended::Stop(PowerStop::PowerOff) if forced => Ok(Cycle::Off("force-off")),
                        Ended::Stop(PowerStop::PowerOff) => Ok(Cycle::Off("graceful-shutdown")),
                        Ended::Stop(PowerStop::ForceOff) => Ok(Cycle::Off("force-off")),
                        Ended::Stop(PowerStop::ColdRestart) => Ok(Cycle::Restart),
                        Ended::Exited(_) => unreachable!(),
                    }
                }
                (Err(error), _) | (_, Err(error)) => {
                    state.phase = Phase::Failed;
                    state.detail = Some(error.to_string());
                    Err(error)
                }
            }
        }
    };
    control.set_qmp_dropped(qmp.dropped_events());
    control.end_qmp_connection();
    qmp.shutdown().await;
    // Shutdown and exit events arrive while stopping; the pump ends when the
    // closed connection drops its event queue.
    let _ = timeout(Duration::from_secs(1), pump).await;
    publish(config, state)?;
    result
}

/// Drive a stop to completion, answering the requests that raced the
/// stopping flag instead of queueing them and keeping the heartbeat fresh.
/// When `escalable`, a live force off abandons `stopping` and its reply is
/// returned for the caller's forced stop to answer.
async fn drive_stop<T>(
    stopping: impl Future<Output = T>,
    config: &Config,
    control: &mut super::ControlReceiver,
    state: &mut Status,
    escalable: bool,
) -> std::result::Result<T, oneshot::Sender<Result<()>>> {
    tokio::pin!(stopping);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            biased;
            stopped = &mut stopping => return Ok(stopped),
            Some(request) = control.requests.recv() => {
                if !escalable {
                    request.shutting_down();
                } else if let Some(reply) = request.escalates_graceful_stop() {
                    return Err(reply);
                }
            }
            _ = heartbeat.tick() => { let _ = publish(config, state); }
        }
    }
}

fn native_clean_shutdown(config: &Config, exit: &ExitStatus, method: &str) -> Result<()> {
    if !config.iscsi_disks.is_empty() && (!exit.success() || method == "sigkill") {
        return Err(Error::Spawn(
            "native iSCSI QEMU exit was forced or unsuccessful; clean disk release is not proven"
                .into(),
        ));
    }
    Ok(())
}

struct SerialPump(Option<tokio::task::JoinHandle<()>>);
impl Drop for SerialPump {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}
