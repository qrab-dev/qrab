#!/usr/bin/env bash
# Runs the manager's ignored real-guest tests (docs/testing.md) inside a
# manager image, against that image's own QEMU, OVMF, swtpm and demo guest
# (TCG, no /dev/kvm needed):
#   pod_runtime, pod_uefi, pod_guest_agent
# The tests are built from this checkout with the image's pinned Rust
# toolchain (Containerfile.manager's RUST_IMAGE), so the binaries match the
# image's glibc.
#
#   bash contrib/tests/real-guest.sh MANAGER_IMAGE [TEST...]
#
# MANAGER_IMAGE is a digest-pinned manager image (name@sha256:...); the
# digest of a local podman image works. TEST defaults to all of them.
# QEMU and QEMU_DATA name another QEMU binary in the image and its data
# directory; the defaults are Debian's, which the image installs.
#
# The script needs podman and the cargo registry in ~/.cargo/registry
# (CARGO_NET_OFFLINE builds). pod_guest_agent also installs e2fsprogs into
# its throwaway container from the image's Debian archive, which needs
# network access.
set -euo pipefail
cd "$(dirname "$0")/../.."
image=${1:?usage: real-guest.sh MANAGER_IMAGE [TEST...]}
shift
[[ $image =~ @sha256:[0-9a-f]{64}$ ]] || { echo 'MANAGER_IMAGE must be digest-pinned' >&2; exit 2; }
tests=("$@")
default=(pod_runtime pod_uefi pod_guest_agent)
(( ${#tests[@]} )) || tests=("${default[@]}")
qemu=${QEMU:-/usr/bin/qemu-system-x86_64}
qemu_data=${QEMU_DATA:-/usr/share/qemu}
rust=$(sed -n 's/^ARG RUST_IMAGE=//p' contrib/images/Containerfile.manager)
[[ -n $rust ]] || { echo 'Containerfile.manager names no RUST_IMAGE' >&2; exit 2; }
out=target/real-guest
mkdir -p "$out/guest"

# The demo guest: the image's ESP, initramfs and UKI, plus the kernel the
# direct-boot tests need, taken from the UKI's .linux section.
podman run --rm --user 0 -v "$PWD/$out/guest":/out --entrypoint sh "$image" \
  -c 'cp -rL /opt/qrab/demo/. /out/'
objcopy -O binary --only-section=.linux "$out/guest/BOOTX64.EFI" "$out/guest/vmlinuz"

args=()
for test in "${tests[@]}"; do args+=(--test "$test"); done
built=$(podman run --rm -v "$PWD":/src:ro -v "$PWD/$out":/target \
  -v "$HOME/.cargo/registry":/usr/local/cargo/registry:ro \
  -e CARGO_TARGET_DIR=/target -e CARGO_NET_OFFLINE=true -w /src "$rust" \
  cargo test --release --locked -p qrab-manager "${args[@]}" --no-run 2>&1) || {
  printf '%s\n' "$built" >&2
  exit 1
}

failed=0
for test in "${tests[@]}"; do
  binary=$(printf '%s\n' "$built" | sed -n "s|.*(/target/\(release/deps/${test}-[0-9a-f]*\)).*|\1|p" | tail -1)
  [[ -n $binary ]] || { echo "no test binary for $test" >&2; exit 1; }
  run=(podman run --rm -v "$PWD/$out":/target:ro
    -e "QRAB_TEST_QEMU=$qemu"
    -e "QRAB_TEST_QEMU_DATA=$qemu_data"
    -e QRAB_TEST_FIRMWARE=/usr/share/OVMF
    -e "QRAB_TEST_FIRMWARE_DIR=$qemu_data"
    -e QRAB_TEST_GUEST=/target/guest
    -e QRAB_TEST_KERNEL=/target/guest/vmlinuz
    -e QRAB_TEST_INITRD=/target/guest/initramfs.gz)
  echo "== $test"
  if [[ $test == pod_guest_agent ]]; then
    # The test formats a scratch filesystem with mke2fs, which the runtime
    # image does not ship.
    command=("${run[@]}" --user 0 --entrypoint sh "$image" -c
      "apt-get update -qq >/dev/null && apt-get install -y -qq e2fsprogs >/dev/null && exec /target/$binary --ignored")
  else
    command=("${run[@]}" --entrypoint "/target/$binary" "$image" --ignored)
  fi
  if ! "${command[@]}"; then
    failed=$((failed + 1))
  fi
done
(( failed == 0 )) || { echo "FAIL: $failed of ${#tests[@]} real-guest tests" >&2; exit 1; }
echo "PASS: ${tests[*]} in $image with $qemu"
