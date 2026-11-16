#!/bin/sh
# Build and install dst on a Cardputer Zero over SSH.
#
#   tools/deploy/deploy.sh hanxiao@192.168.88.234
#
# The device has 512 MB of RAM: a parallel LTO build gets OOM-killed, so the
# build runs with CARGO_BUILD_JOBS=1. The source tree on the device is
# /home/hanxiao/dsh-lite (the systemd unit runs /usr/local/bin/dst).
set -eu

TARGET=${1:?usage: deploy.sh [user@host]}
DIR=${2:-/home/hanxiao/dsh-lite}
HERE=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)

echo "== copy sources to $TARGET:$DIR"
ssh "$TARGET" "mkdir -p '$DIR'"
rsync -az --delete-after \
    --exclude target --exclude .git \
    "$HERE/Cargo.toml" "$HERE/Cargo.lock" "$HERE/src" \
    "$TARGET:$DIR/"

echo "== build on the device (single job)"
ssh "$TARGET" "cd '$DIR' && touch src/*.rs && CARGO_BUILD_JOBS=1 cargo build --release"

echo "== install and restart"
ssh "$TARGET" "sudo install -m755 '$DIR/target/release/dst' /usr/local/bin/dst && \
    sudo systemctl restart cpzterm.service && sleep 2 && systemctl is-active cpzterm.service"

echo "== done: terminal should be back on the panel (or check journalctl -u cpzterm -b)"
