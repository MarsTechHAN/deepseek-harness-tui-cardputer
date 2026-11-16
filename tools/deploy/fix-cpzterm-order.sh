#!/bin/sh
# Repair the cpzterm early-boot ordering cycle on the device.
#
# Symptom: boot stops with
#   Requested transaction contains an unfixable cyclic ordering dependency:
#   Resource deadlock avoided
# and the panel never reaches fbterm.
#
# Cause: /etc/systemd/system/cpzterm.service declares
#   Before=... sysinit.target ...
# while cpzterm also depends on the keymap unit that is part of early boot.
# A terminal service has no business ordering itself before sysinit: it should
# start in the normal multi-user phase.
set -eu

UNIT=/etc/systemd/system/cpzterm.service
if [ ! -f "$UNIT" ]; then
    echo "no $UNIT on this host" >&2
    exit 1
fi

echo "== before"
grep -E '^(Before|After|DefaultDependencies)=' "$UNIT" || true

sudo sed -i 's/\(Before=.*\)sysinit\.target[[:space:]]*/\1/' "$UNIT"

echo "== after"
grep -E '^(Before|After|DefaultDependencies)=' "$UNIT" || true

sudo systemctl daemon-reload
sudo systemd-analyze verify "$UNIT" >/dev/null || echo "warning: systemd-analyze reported an issue" >&2
sudo systemctl restart cpzterm.service
sleep 2
systemctl is-active cpzterm.service

echo "== check for ordering cycles in this boot (expected: no matches)"
sudo journalctl -b --no-pager | grep -i "cyclic ordering" || echo "no cycle reported"
