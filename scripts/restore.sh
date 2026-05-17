#!/usr/bin/env bash

set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  CONFIRM_RESTORE=tokenboard scripts/restore.sh <r2-key|latest>

Examples:
  CONFIRM_RESTORE=tokenboard scripts/restore.sh latest
  CONFIRM_RESTORE=tokenboard scripts/restore.sh postgres/tokenboard-20260509T030000Z.dump.gpg
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ $# -ne 1 ]]; then
  usage
  exit 1
fi

if [[ "${CONFIRM_RESTORE:-}" != "tokenboard" ]]; then
  echo "Refusing to restore without CONFIRM_RESTORE=tokenboard." >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ENV_FILE="${ENV_FILE:-$REPO_ROOT/.env.production}"
BACKUP_ENV_FILE="${BACKUP_ENV_FILE:-$REPO_ROOT/.env.backup}"
COMPOSE_FILE="${COMPOSE_FILE:-$REPO_ROOT/docker-compose.prod.yml}"

if [[ ! -f "$ENV_FILE" ]]; then
  echo "Missing env file: $ENV_FILE" >&2
  exit 1
fi
if [[ ! -f "$BACKUP_ENV_FILE" ]]; then
  echo "Missing backup env file: $BACKUP_ENV_FILE" >&2
  echo "Create it from .env.backup.example or set BACKUP_ENV_FILE." >&2
  exit 1
fi

set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
# shellcheck disable=SC1090
source "$BACKUP_ENV_FILE"
set +a

: "${POSTGRES_USER:?POSTGRES_USER is required}"
: "${POSTGRES_DB:?POSTGRES_DB is required}"
: "${BACKUP_PASSPHRASE:?BACKUP_PASSPHRASE is required}"
: "${R2_BUCKET:?R2_BUCKET is required}"
: "${R2_PREFIX:?R2_PREFIX is required}"
: "${R2_ACCOUNT_ID:?R2_ACCOUNT_ID is required}"
: "${R2_ACCESS_KEY_ID:?R2_ACCESS_KEY_ID is required}"
: "${R2_SECRET_ACCESS_KEY:?R2_SECRET_ACCESS_KEY is required}"

command -v docker >/dev/null 2>&1 || { echo "docker is required" >&2; exit 1; }
command -v gpg >/dev/null 2>&1 || { echo "gpg is required" >&2; exit 1; }
command -v aws >/dev/null 2>&1 || { echo "aws CLI is required" >&2; exit 1; }

BACKUP_DIR="${BACKUP_DIR:-$REPO_ROOT/backups}"
mkdir -p "$BACKUP_DIR"
chmod 700 "$BACKUP_DIR"

endpoint="https://${R2_ACCOUNT_ID}.r2.cloudflarestorage.com"
backup_key="$1"

if [[ "$backup_key" == "latest" ]]; then
  backup_key="$(AWS_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID" \
    AWS_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY" \
    AWS_DEFAULT_REGION=auto \
    aws s3api list-objects-v2 \
      --bucket "$R2_BUCKET" \
      --prefix "${R2_PREFIX}/" \
      --query 'sort_by(Contents, &LastModified)[-1].Key' \
      --output text \
      --endpoint-url "$endpoint")"

  if [[ -z "$backup_key" || "$backup_key" == "None" ]]; then
    echo "No backups found in s3://${R2_BUCKET}/${R2_PREFIX}/" >&2
    exit 1
  fi
fi

encrypted_path="$BACKUP_DIR/$(basename "$backup_key")"
dump_path="${encrypted_path%.gpg}"

cleanup() {
  rm -f "$dump_path"
}
trap cleanup EXIT

AWS_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID" \
AWS_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY" \
AWS_DEFAULT_REGION=auto \
aws s3 cp "s3://${R2_BUCKET}/${backup_key}" "$encrypted_path" \
  --endpoint-url "$endpoint"

gpg --batch --yes --pinentry-mode loopback \
  --passphrase "$BACKUP_PASSPHRASE" \
  --decrypt --output "$dump_path" "$encrypted_path"

cd "$REPO_ROOT"

docker compose --env-file "$ENV_FILE" -f "$COMPOSE_FILE" exec -T postgres \
  pg_restore --clean --if-exists --no-owner --no-acl \
  -U "$POSTGRES_USER" -d "$POSTGRES_DB" < "$dump_path"

echo "Restored backup: s3://${R2_BUCKET}/${backup_key}"
