#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ENV_FILE="${ENV_FILE:-$REPO_ROOT/.env.production}"
DEFAULT_BACKUP_ENV_FILE="$REPO_ROOT/.env.backup"
BACKUP_ENV_FILE="${BACKUP_ENV_FILE:-}"
COMPOSE_FILE="${COMPOSE_FILE:-$REPO_ROOT/docker-compose.prod.yml}"

if [[ -z "$BACKUP_ENV_FILE" && -f "$DEFAULT_BACKUP_ENV_FILE" ]]; then
  BACKUP_ENV_FILE="$DEFAULT_BACKUP_ENV_FILE"
fi

if [[ ! -f "$ENV_FILE" ]]; then
  echo "Missing env file: $ENV_FILE" >&2
  exit 1
fi

set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
if [[ -n "$BACKUP_ENV_FILE" ]]; then
  if [[ ! -f "$BACKUP_ENV_FILE" ]]; then
    echo "Missing backup env file: $BACKUP_ENV_FILE" >&2
    echo "Set backup variables in $ENV_FILE or point BACKUP_ENV_FILE at another env file." >&2
    exit 1
  fi
  # shellcheck disable=SC1090
  source "$BACKUP_ENV_FILE"
fi
set +a

S3_BUCKET="${S3_BUCKET:-${R2_BUCKET:-}}"
S3_PREFIX="${S3_PREFIX:-${R2_PREFIX:-}}"
S3_ACCESS_KEY_ID="${S3_ACCESS_KEY_ID:-${R2_ACCESS_KEY_ID:-}}"
S3_SECRET_ACCESS_KEY="${S3_SECRET_ACCESS_KEY:-${R2_SECRET_ACCESS_KEY:-}}"
S3_REGION="${S3_REGION:-auto}"
if [[ -z "${S3_ENDPOINT_URL:-}" && -n "${R2_ACCOUNT_ID:-}" ]]; then
  S3_ENDPOINT_URL="https://${R2_ACCOUNT_ID}.r2.cloudflarestorage.com"
fi

: "${POSTGRES_USER:?POSTGRES_USER is required}"
: "${POSTGRES_DB:?POSTGRES_DB is required}"
: "${BACKUP_PASSPHRASE:?BACKUP_PASSPHRASE is required}"
: "${S3_BUCKET:?S3_BUCKET is required}"
: "${S3_PREFIX:?S3_PREFIX is required}"
: "${S3_ACCESS_KEY_ID:?S3_ACCESS_KEY_ID is required}"
: "${S3_SECRET_ACCESS_KEY:?S3_SECRET_ACCESS_KEY is required}"

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
retention_days="${BACKUP_RETENTION_DAYS:-30}"
aws_endpoint_args=()
if [[ -n "${S3_ENDPOINT_URL:-}" ]]; then
  aws_endpoint_args=(--endpoint-url "$S3_ENDPOINT_URL")
fi

if ! [[ "$retention_days" =~ ^[0-9]+$ ]]; then
  echo "BACKUP_RETENTION_DAYS must be a positive integer, got: $retention_days" >&2
  exit 1
fi

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

AWS_ACCESS_KEY_ID="$S3_ACCESS_KEY_ID" \
AWS_SECRET_ACCESS_KEY="$S3_SECRET_ACCESS_KEY" \
AWS_DEFAULT_REGION="${S3_REGION:-auto}" \
aws s3 cp "$encrypted_path" "s3://${S3_BUCKET}/${S3_PREFIX}/${backup_name}.gpg" \
  "${aws_endpoint_args[@]}"

if cutoff="$(date -u -d "${retention_days} days ago" +'%Y-%m-%dT%H:%M:%SZ' 2>/dev/null)"; then
  :
elif cutoff="$(date -u -v-"${retention_days}"d +'%Y-%m-%dT%H:%M:%SZ' 2>/dev/null)"; then
  :
else
  echo "Could not calculate backup retention cutoff; skipping remote pruning." >&2
  cutoff=""
fi

old_keys=""
if [[ -n "$cutoff" ]]; then
  old_keys="$(AWS_ACCESS_KEY_ID="$S3_ACCESS_KEY_ID" \
  AWS_SECRET_ACCESS_KEY="$S3_SECRET_ACCESS_KEY" \
  AWS_DEFAULT_REGION="${S3_REGION:-auto}" \
  aws s3api list-objects-v2 \
    --bucket "$S3_BUCKET" \
    --prefix "${S3_PREFIX}/" \
    --query "Contents[?LastModified<='${cutoff}'].Key" \
    --output text \
    "${aws_endpoint_args[@]}" || true)"
fi

if [[ -n "$old_keys" && "$old_keys" != "None" ]]; then
  for key in $old_keys; do
    AWS_ACCESS_KEY_ID="$S3_ACCESS_KEY_ID" \
    AWS_SECRET_ACCESS_KEY="$S3_SECRET_ACCESS_KEY" \
    AWS_DEFAULT_REGION="${S3_REGION:-auto}" \
    aws s3 rm "s3://${S3_BUCKET}/${key}" "${aws_endpoint_args[@]}"
  done
fi

echo "Uploaded encrypted backup: s3://${S3_BUCKET}/${S3_PREFIX}/${backup_name}.gpg"
