# Standalone VM without Kubernetes

`contrib/standalone` runs one qrab VM with podman or Docker and no
Kubernetes cluster. The manager supervises QEMU and swtpm, and the VM boots
through UEFI with a persistent TPM 2.0, the demo guest by default.
It is a small subset of qrab;
[What standalone supports](#what-standalone-supports) lists what is left out.

| File | Purpose |
| --- | --- |
| `compose.yaml` | The services, for `podman-compose` or `docker compose` |
| `vm.json` | The VM ([The VM](#the-vm)): the RAM-only demo guest, TCG, UEFI and TPM, with the control socket on |
| `env.example` | Every setting, to copy to `.env` |
| `compose.kvm.yaml` | Optional: KVM acceleration |

## Layout

| Service | Runs | Network | Mounts |
| --- | --- | --- | --- |
| `init` | `chown` of the two volumes to UID/GID 10001, then exits | none | `state`, `control` |
| `manager` | `qrab-manager run --config /etc/qrab/vm.json`: QEMU, swtpm, status and the control socket | none | `vm.json` (read-only), `state` at `/var/lib/qrab`, `control` at `/run/qrab-control`, tmpfs `/run/qrab` and `/tmp` |

The manager has no network: the VM has no NIC and the manager opens no TCP
listener. QMP, VNC and the swtpm sockets stay in the manager's private
`/run/qrab`. The manager's control socket, on the `control` volume, is the
only way in ([Managing the VM](#managing-the-vm)).
The containers run as UID/GID 10001 with a read-only root filesystem, no
capabilities and `no-new-privileges`, except `init`: it runs as root with
the default capabilities, gives the volumes to that user and exits.

## Prerequisites

- Linux with podman and `podman-compose`, or Docker with Compose v2.
- For KVM, an openable `/dev/kvm`. Without it the VM runs under TCG.

## The image

The services run the image that `QRAB_MANAGER_IMAGE` names; there is no
default. There are no releases yet, so build it with
`contrib/images/build.sh` and podman, from a checkout of this repository.
The image runs Debian 13's QEMU 10.0, swtpm and OVMF packages, and no
registry is needed:

```sh
export QRAB_REGISTRY=localhost
contrib/images/build.sh manager dev
```

That builds `localhost/qrab/manager:dev`, the image `env.example` names.
Docker Compose needs the image in Docker: `podman save
localhost/qrab/manager:dev | docker load`. The setup is tested with
podman-compose only.

Any other reference your container engine can pull or already holds works
too, such as a release pinned by digest once there are releases:

```sh
QRAB_MANAGER_IMAGE=registry.example.org/qrab/manager@sha256:<digest>
```

## Configuring

Copy the directory, so that your settings and VM configuration live outside
the source tree:

```sh
cp -r contrib/standalone ~/qrab-vm
cd ~/qrab-vm
cp env.example .env
```

Both Compose implementations read `.env` from the project directory, the
one holding `compose.yaml`; run the commands below there. Edit it:

| Variable | Meaning |
| --- | --- |
| `QRAB_MANAGER_IMAGE` | The manager image; required ([The image](#the-image)) |
| `QRAB_VM_CONFIG` | Another VM config file instead of `./vm.json` |
| `QRAB_KVM_GROUP` | With `compose.kvm.yaml`, the group that may open `/dev/kvm` |

### The VM

`vm.json` is the manager's demo VM with `"control_socket": true` and
`"control_dir": "/run/qrab-control/manager"`. Keep `state_dir`,
`runtime_dir` and `control_dir` as they are; they are the mount points. The
manager reads strict JSON and refuses unknown fields:

| Field | Meaning |
| --- | --- |
| `name` | The VM's name, 1 to 63 letters, digits, dots, hyphens or underscores |
| `memory_mib`, `cpus` | Guest memory in MiB and vCPUs |
| `accelerator` | `tcg` (default) or `kvm` |
| `qemu_binary` | The QEMU binary, default Debian's `/usr/bin/qemu-system-x86_64`: QEMU 8.2 or newer, tested with 8.2.2, 10.0.13 and 11.1.3. The manager renders its startup arguments for the release it finds |
| `uefi` | Boot through OVMF: `{}` boots the image's demo disk with the default firmware; `"boot_disk"` names another read-only image, and `"code"` and `"vars_template"` other firmware files |
| `uefi.secure_boot_capable` | `true` declares that `"code"` is a Secure Boot build, such as the image's `/opt/qrab/firmware/OVMF_CODE_4M.secboot.fd`, and turns on SMM and protected flash. Secure Boot itself also needs keys enrolled in the VM's variable store; the default template has none |
| `tpm` | A persistent swtpm TPM 2.0 (`{}`) |
| `disk` | An optional disk: `{"path": "/absolute/file-or-device", "format": "raw" or "qcow2", "read_only": false}`, added to the manager's volumes |
| `startup_timeout_secs`, `shutdown_grace_secs` | How long swtpm and QEMU may take to start (1 to 300 seconds), and how long the guest has to shut down before QEMU is stopped |

For KVM, set `"accelerator": "kvm"` and add `-f compose.kvm.yaml`, which
passes `/dev/kvm` to the manager. `/dev/kvm` is usually `0660 root:kvm`:
rootless podman keeps the caller's groups with the default
`QRAB_KVM_GROUP=keep-groups` (the caller must be in `kvm`); with Docker, set
it to the numeric GID, `getent group kvm | cut -d: -f3`.

## Starting

```sh
podman-compose up -d          # or: docker compose up -d
podman-compose ps
podman-compose logs manager   # the guest's serial console and manager events
```

With KVM:

```sh
podman-compose -f compose.yaml -f compose.kvm.yaml up -d
```

`podman-compose config` (or `docker compose config`) prints the resolved
file, and fails, naming the variable, when `.env` does not set
`QRAB_MANAGER_IMAGE`.

The manager is healthy while its heartbeat is fresh, including when the VM is
paused or powered off. `ready` additionally requires a running guest:

```sh
podman-compose exec manager qrab-manager ready --config /etc/qrab/vm.json
```

The demo guest prints `QRAB_GUEST_BOOTED`, `QRAB_TPM2_READY` and
`QRAB_GUEST_NO_NIC_OK` on its serial console, in the manager's log.

## Managing the VM

`qrab-manager` in the manager container reads the running manager's state.
`status`, `ready` and `live` read the status file that the manager writes
every second. `inspect`, `control` and `agent` talk to the manager over its
control socket: `inspect` prints one read-only view (`inventory`,
`metrics`, `network`, `boot`, `firmware`, `secure-boot`, the Secure Boot
`settings`, `resources`, `media` or the redacted `config`), and `agent`
asks the guest agent:

```sh
podman-compose exec manager qrab-manager status --config /etc/qrab/vm.json
podman-compose exec manager qrab-manager inspect inventory --socket /run/qrab-control/manager/control.sock
podman-compose exec manager qrab-manager agent ping --config /etc/qrab/vm.json
```

`control` sends one operation, read from stdin as JSON. Power actions are
`{"op": "action", "action": "<action>"}` with `pause`, `resume`, `reset`,
`powerdown`, `nmi`, `reboot` (through the guest agent), `coldrestart`, `on`
or `forceoff`:

```sh
echo '{"op": "action", "action": "powerdown"}' |
    podman-compose exec -T manager qrab-manager control --socket /run/qrab-control/manager/control.sock
```

A powered-off VM stays off inside the running manager until `on`, with the
same TPM and UEFI variables.

## Stopping and state

```sh
podman-compose stop     # shut the VM down; keep the containers
podman-compose down     # also remove the containers; keep the volumes
podman-compose down -v  # also remove the volumes: the VM's TPM and UEFI state
```

Stopping the manager sends SIGTERM: it asks the guest to shut down, waits
`shutdown_grace_secs`, then stops QEMU and swtpm. Compose kills the
container 20 seconds after SIGTERM (`stop_grace_period` in `compose.yaml`),
so keep `shutdown_grace_secs` (at most 120) well below that, or raise both. The `state` volume holds
the persistent UEFI variables, the swtpm TPM and the state lease, so the VM
keeps its TPM identity across `down` and `up`; `down -v` discards it. The
`control` volume holds only the socket. Changing `vm.json` takes effect when
the manager container is recreated (`podman-compose up -d --force-recreate
manager`). Each VM needs its own copy of the directory and a project name of
its own (`podman-compose -p <name>`).

## What standalone supports

Supported:

- one VM per Compose project, booted through UEFI from the image's demo boot
  disk (or another read-only `uefi.boot_disk` added to the manager's
  volumes), with a persistent swtpm TPM 2.0 and persistent UEFI variables;
- TCG, or KVM with `compose.kvm.yaml`;
- the manager's power actions, status and guest agent through its control
  socket;
- the manager's probes as health checks.

Not supported:

- guest NICs: the VM has `-nic none`;
- live migration;
- growing CPUs and memory in place: nothing in this setup writes the size
  files that the manager's `hotplug` settings read.

The manager also reads these settings from its environment, which
`compose.yaml` can add to the manager service:

| Variable | Meaning |
| --- | --- |
| `QRAB_MEDIA_ROOT`, `QRAB_MEDIA_MAX_BYTES` | Virtual media: the read-only directory of ISO images the VM may insert, and the largest image (`media` operations) |
| `QRAB_FIRMWARE_ROOT` | The catalog of firmware builds the VM may stage for its next cold boot |
| `QRAB_RESOURCE_SCOPE` | `container` (as `compose.yaml` sets it) reports the whole container's resource usage, not only the manager process |
| `QRAB_IMAGE` | The image reference that the configuration view reports |

## Troubleshooting

- The manager cannot open `/dev/kvm`: see [The VM](#the-vm) for the group,
  or set `"accelerator": "tcg"`.
