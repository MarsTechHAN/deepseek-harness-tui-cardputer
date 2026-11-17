#!/bin/sh
# Build and install dst on a Cardputer Zero over SSH.
#
#   tools/deploy/deploy.sh hanxiao@192.168.43.49
#   tools/deploy/deploy.sh hanxiao@192.168.43.49 /home/hanxiao/dsh-lite
#
# The device has 512 MB of RAM: a parallel LTO build gets OOM-killed, so the
# build runs with CARGO_BUILD_JOBS=1. The source tree on the device is
# /home/hanxiao/dsh-lite (the systemd unit runs /usr/local/bin/dst). SSH needs
# a key the device accepts (the box does not allow password login for scripts);
# set DST_SSH_KEY=/path/to/key to pick one explicitly.
#
# The panel terminal (cpzterm.service) is stopped for the build so the 512 MB
# device does not compile next to a running TUI, then restarted.
set -eu

TARGET=${1:?usage: deploy.sh [user@host] [remote dir]}
DIR=${2:-/home/hanxiao/dsh-lite}
HERE=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
# Password auth (the box only offers publickey+password and has no key for us):
#   DST_SSH_PASSWORD=... tools/deploy/deploy.sh ...
# rsync needs the same password, so a small wrapper script is generated.
SSH="ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10"
if [ -n "${DST_SSH_KEY:-}" ]; then
    SSH="ssh -i $DST_SSH_KEY -o IdentitiesOnly=yes"
fi
if [ -n "${DST_SSH_PASSWORD:-}" ]; then
    command -v sshpass >/dev/null || { echo "DST_SSH_PASSWORD needs sshpass (brew install sshpass)" >&2; exit 1; }
    WRAP=$(mktemp)
    printf '#!/bin/sh\nexec sshpass -e ssh %s "$@"\n' "$SSH" > "$WRAP"
    chmod +x "$WRAP"
    export SSHPASS=$DST_SSH_PASSWORD
    SSH="$WRAP"
    trap 'rm -f "$WRAP"' EXIT
fi

echo "== check device"
$SSH "$TARGET" 'uname -srm; cat /etc/debian_version 2>/dev/null || true; free -m | head -2 || true'

echo "== copy sources to $TARGET:$DIR"
$SSH "$TARGET" "mkdir -p '$DIR'"
rsync -az --delete-after -e "$SSH" \
    --exclude target --exclude .git \
    "$HERE/Cargo.toml" "$HERE/Cargo.lock" "$HERE/src" \
    "$TARGET:$DIR/"

echo "== stop the panel terminal during the build"
$SSH "$TARGET" "echo '$DST_SSH_PASSWORD' | sudo -S true 2>/dev/null; sudo systemctl stop cpzterm.service || true"

echo "== build on the device (single job)"
$SSH "$TARGET" "cd '$DIR' && touch src/*.rs && CARGO_BUILD_JOBS=1 cargo build --release"

echo "== install and restart"
$SSH "$TARGET" "sudo install -m755 '$DIR/target/release/dst' /usr/local/bin/dst && \
    sudo systemctl restart cpzterm.service && sleep 3 && systemctl is-active cpzterm.service"

echo "== runtime check (tools the model can see in creator mode)"
$SSH "$TARGET" "/usr/local/bin/dst --tools | sed -e 's/^tools=.*/tools=(list shown only in the TUI)/' || true"

echo "== done: the panel should be back on dst (or check journalctl -u cpzterm -b)"
