#!/usr/bin/env bash

set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/copy-to-home-server.sh <ssh-target> [remote-dir]

Examples:
  scripts/copy-to-home-server.sh deploy@home-server
  scripts/copy-to-home-server.sh deploy@192.168.1.50 /opt/tokenboard

Copies the current working tree to the target server using one shared SSH
connection, so an encrypted SSH key should only prompt once.
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ $# -lt 1 || $# -gt 2 ]]; then
  usage
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

TARGET="$1"
REMOTE_DIR="${2:-/opt/tokenboard}"

if [[ "$REMOTE_DIR" != /* ]]; then
  echo "remote-dir must be an absolute path, got: $REMOTE_DIR" >&2
  exit 1
fi

command -v ssh >/dev/null 2>&1 || { echo "ssh is required" >&2; exit 1; }
command -v rsync >/dev/null 2>&1 || { echo "rsync is required locally" >&2; exit 1; }

quote_for_sh() {
  printf "'"
  printf "%s" "$1" | sed "s/'/'\\\\''/g"
  printf "'"
}

CONTROL_DIR="/tmp/tokenboard-ssh-${UID:-$(id -u)}"
mkdir -p "$CONTROL_DIR"
chmod 700 "$CONTROL_DIR"

CONTROL_PATH="$CONTROL_DIR/%C"
SSH_OPTS=(
  -o ControlMaster=auto
  -o ControlPersist=10m
  -o "ControlPath=$CONTROL_PATH"
)

cleanup() {
  ssh "${SSH_OPTS[@]}" -O exit "$TARGET" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "Opening shared SSH connection to $TARGET..."
ssh "${SSH_OPTS[@]}" -MNf "$TARGET"

echo "Preparing $REMOTE_DIR on $TARGET..."
REMOTE_DIR_Q="$(quote_for_sh "$REMOTE_DIR")"
ssh -tt "${SSH_OPTS[@]}" "$TARGET" "set -eu
remote_dir=$REMOTE_DIR_Q
if [ \"\${remote_dir#/}\" = \"\$remote_dir\" ]; then
  echo \"remote-dir must be absolute: \$remote_dir\" >&2
  exit 1
fi

command -v rsync >/dev/null 2>&1 || {
  echo \"rsync is required on the target server\" >&2
  exit 1
}

remote_user=\"\$(id -un)\"
remote_group=\"\$(id -gn)\"

if [ \"\$(id -u)\" -eq 0 ]; then
  mkdir -p \"\$remote_dir\"
else
  sudo mkdir -p \"\$remote_dir\"
  sudo chown -R \"\$remote_user:\$remote_group\" \"\$remote_dir\"
fi
"

echo "Copying repository to $TARGET:$REMOTE_DIR..."
rsync -az --delete \
  -e "ssh -o ControlMaster=auto -o ControlPersist=10m -o ControlPath=$CONTROL_PATH" \
  --exclude='.git/' \
  --exclude='.DS_Store' \
  --exclude='.env' \
  --exclude='.env.production' \
  --exclude='.env.backup' \
  --exclude='.env.local' \
  --exclude='.env.*.local' \
  --exclude='client/.env' \
  --exclude='node_modules/' \
  --exclude='server/node_modules/' \
  --exclude='client/target/' \
  --exclude='**/target/' \
  --exclude='backups/' \
  --exclude='logs/' \
  --exclude='*.log' \
  "$REPO_ROOT/" "$TARGET:$REMOTE_DIR/"

echo "Finalizing $REMOTE_DIR on $TARGET..."
ssh "${SSH_OPTS[@]}" "$TARGET" 'sh -s' -- "$REMOTE_DIR" <<'REMOTE'
set -eu

remote_dir="$1"
if [ "${remote_dir#/}" = "$remote_dir" ]; then
  echo "remote-dir must be absolute: $remote_dir" >&2
  exit 1
fi

cd "$remote_dir"

chmod +x scripts/*.sh

if [ ! -f .env.production ]; then
  cp .env.production.example .env.production
  chmod 600 .env.production
  echo "Created .env.production from .env.production.example"
else
  chmod 600 .env.production
  echo "Preserved existing .env.production"
fi

echo "Copied Tokenboard to $remote_dir"
REMOTE

cat <<EOF

Done.

Next on the target:
  ssh $TARGET
  cd $REMOTE_DIR
  nano .env.production
  docker compose -f docker-compose.prod.yml --env-file .env.production up -d --build
EOF
