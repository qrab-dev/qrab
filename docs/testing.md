# Core-library validation

The normal developer path is the default workspace (`default-members` in the
root `Cargo.toml`).
No QEMU checkout, Python, firmware, privileged hardware or cluster is required for these tests:

```sh
cargo test --locked
bash contrib/check-runtime.sh
cargo deny --locked check advisories
```

The advisories check reads the RustSec database and the `[advisories]`
section of `deny.toml`, which ignores only advisories reviewed as unreachable
from qrab's code, each with its reason. Remove an entry once the upstream
dependency moves to a patched release.

The script also checks formatting and runs strict Clippy on the
hand-written crates.

The manager's real-guest tests are ignored by default and need the built guest
assets (`contrib/images`):

| Test | Checks | Inputs |
| --- | --- | --- |
| `pod_runtime` | CLI boot, probes and SIGTERM shutdown | `QRAB_TEST_QEMU`, `QRAB_TEST_QEMU_DATA`, `QRAB_TEST_GUEST` |
| `pod_uefi` | UEFI/TPM boot and retained identity | the above plus `QRAB_TEST_FIRMWARE`, optional `QRAB_TEST_SWTPM` |
| `pod_guest_agent` | ping, freeze/thaw and an update | `QRAB_TEST_QEMU`, `QRAB_TEST_FIRMWARE_DIR`, `QRAB_TEST_KERNEL`, `QRAB_TEST_INITRD` |

Run one with `cargo test --locked -p qrab-manager --test <name> -- --ignored --nocapture`,
or all of them inside a manager image, against its own QEMU, OVMF, swtpm and
demo guest (TCG, no `/dev/kvm` needed):

```sh
bash contrib/tests/real-guest.sh registry.example.org/qrab/manager@sha256:<digest> [TEST...]
```

It builds the tests with the image's pinned Rust toolchain, takes the direct-boot
kernel from the demo UKI and installs `e2fsprogs` for `pod_guest_agent` in a
throwaway container, which needs the image's Debian archive.
The tests run the image's QEMU, Debian 13's QEMU 10.0; `QEMU` and
`QEMU_DATA` name another QEMU binary in the image and its data directory.

One more ignored test edits a real Secure Boot variable store. Give it a
disposable copy of an OVMF variable store with keys enrolled, such as
Debian's `/usr/share/OVMF/OVMF_VARS_4M.ms.fd`, and, optionally, where to
write the edited copy:

```sh
QRAB_TEST_OVMF_VARS=/path/to/OVMF_VARS.fd QRAB_TEST_OVMF_CANDIDATE=/tmp/candidate.fd \
    cargo test --locked -p qrab-manager --lib real_firmware_vars -- --ignored --nocapture
```

## QAPI crates

The QMP/QGA libraries and the QAPI generator are tested in their own
repository; see the
[qrab-qapi testing guide](https://github.com/qrab-dev/qrab-qapi/blob/main/docs/testing.md),
which also covers the real-QEMU and real guest-agent smoke tests.
