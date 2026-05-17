#!/usr/bin/env bash

set -euo pipefail

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

timestamp="$(date -u +'%Y%m%dT%H%M%SZ')"
backup_name="tokenboard-${timestamp}.dump"
dump_path="$BACKUP_DIR/$backup_name"
encrypted_path="${dump_path}.gpg"
endpoint="https://${R2_ACCOUNT_ID}.r2.cloudflarestorage.com"

cleanup() {
  rm -f "$dump_path"
}
trap cleanup EXIT

cd "$REPO_ROOT"

docker compose --env-file "$ENV_FILE" -f "$COMPOSE_FILE" exec -T postgres \
  pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" \
  --format=custom --no-owner --no-acl > "$dump_path"

gpg --batch --yes --pinentry-mode loopback \
  --passphrase "$BACKUP_PASSPHRASE" \
  --symmetric --cipher-algo AES256 \
  --output "$encrypted_path" "$dump_path"

AWS_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID" \
AWS_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY" \
AWS_DEFAULT_REGION=auto \
aws s3 cp "$encrypted_path" "s3://${R2_BUCKET}/${R2_PREFIX}/${backup_name}.gpg" \
  --endpoint-url "$endpoint"

cutoff="$(date -u -d '30 days ago' +'%Y-%m-%dT%H:%M:%SZ')"
old_keys="$(AWS_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID" \
  AWS_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY" \
  AWS_DEFAULT_REGION=auto \
  aws s3api list-objects-v2 \
    --bucket "$R2_BUCKET" \
    --prefix "${R2_PREFIX}/" \
    --query "Contents[?LastModified<='${cutoff}'].Key" \
    --output text \
    --endpoint-url "$endpoint" || true)"

if [[ -n "$old_keys" && "$old_keys" != "None" ]]; then
  for key in $old_keys; do
    AWS_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID" \
    AWS_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY" \
    AWS_DEFAULT_REGION=auto \
    aws s3 rm "s3://${R2_BUCKET}/${key}" --endpoint-url "$endpoint"
  done
fi

echo "Uploaded encrypted backup: s3://${R2_BUCKET}/${R2_PREFIX}/${backup_name}.gpg"
