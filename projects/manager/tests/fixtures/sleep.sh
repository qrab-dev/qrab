#!/bin/sh
# Stable executable fixture: no write/exec race under parallel process tests.
# It answers the manager's version probe as QEMU 11.1.3 does.
[ "${1-}" = --version ] && exec echo 'QEMU emulator version 11.1.3'
exec /bin/sleep 1000
