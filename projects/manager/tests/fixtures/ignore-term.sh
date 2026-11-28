#!/bin/sh
# It answers the manager's version probe as QEMU 11.1.3 does.
[ "${1-}" = --version ] && exec echo 'QEMU emulator version 11.1.3'
# SIG_IGN survives exec, so the supervisor must exercise its SIGKILL path.
trap '' TERM
exec /bin/sleep 1000
