#!/usr/bin/env bash

set -Eeuo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/redeploy-prod.sh [options]

Options:
  --no-pull             Do not run git fetch/pull before redeploying.
  --allow-dirty         Allow redeploying from a git worktree with local changes.
  --pull-images         Pull postgres/cloudflared images before starting containers.
  --no-build            Recreate containers without rebuilding the app image.
  --compose-file PATH   Compose file to use. Default: docker-compose.prod.yml
  --env-file PATH       Environment file to use. Default: .env.production
  --timeout SECONDS     Readiness timeout. Default: 120
  --logs                Tail app logs after a successful redeploy.
  -h, --help            Show this help.

Environment overrides:
  COMPOSE_FILE, ENV_FILE, APP_SERVICE, READY_URL, WAIT_TIMEOUT
USAGE
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

COMPOSE_FILE="${COMPOSE_FILE:-docker-compose.prod.yml}"
ENV_FILE="${ENV_FILE:-.env.production}"
APP_SERVICE="${APP_SERVICE:-app}"
READY_URL="${READY_URL:-http://127.0.0.1:3001/readyz}"
WAIT_TIMEOUT="${WAIT_TIMEOUT:-120}"

RUN_GIT_PULL=1
ALLOW_DIRTY=0
PULL_IMAGES=0
BUILD_APP=1
TAIL_LOGS=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-pull)
      RUN_GIT_PULL=0
      shift
      ;;
    --allow-dirty)
      ALLOW_DIRTY=1
      shift
      ;;
    --pull-images)
      PULL_IMAGES=1
      shift
      ;;
    --no-build)
      BUILD_APP=0
      shift
      ;;
    --compose-file)
      COMPOSE_FILE="${2:-}"
      shift 2
      ;;
    --env-file)
      ENV_FILE="${2:-}"
      shift 2
      ;;
    --timeout)
      WAIT_TIMEOUT="${2:-}"
      shift 2
      ;;
    --logs)
      TAIL_LOGS=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

cd "$REPO_ROOT"

if [[ -z "$COMPOSE_FILE" || ! -f "$COMPOSE_FILE" ]]; then
  echo "Compose file not found: ${COMPOSE_FILE:-<empty>}" >&2
  exit 1
fi

if [[ -z "$ENV_FILE" || ! -f "$ENV_FILE" ]]; then
  echo "Environment file not found: ${ENV_FILE:-<empty>}" >&2
  echo "Create it from .env.production.example and fill in production secrets." >&2
  exit 1
fi

if ! [[ "$WAIT_TIMEOUT" =~ ^[0-9]+$ ]] || [[ "$WAIT_TIMEOUT" -lt 1 ]]; then
  echo "--timeout must be a positive integer, got: $WAIT_TIMEOUT" >&2
  exit 1
fi

if docker compose version >/dev/null 2>&1; then
  COMPOSE=(docker compose)
elif command -v docker-compose >/dev/null 2>&1; then
  COMPOSE=(docker-compose)
else
  echo "Docker Compose is required: install the docker compose plugin or docker-compose." >&2
  exit 1
fi

compose() {
  "${COMPOSE[@]}" -f "$COMPOSE_FILE" --env-file "$ENV_FILE" "$@"
}

LOCK_DIR="${TMPDIR:-/tmp}/tokenboard-redeploy.lock"
if ! mkdir "$LOCK_DIR" 2>/dev/null; then
  echo "Another tokenboard redeploy appears to be running: $LOCK_DIR" >&2
  exit 1
fi

cleanup() {
  rm -rf "$LOCK_DIR"
}
trap cleanup EXIT

echo "Redeploying Tokenboard from $REPO_ROOT"

if [[ "$RUN_GIT_PULL" -eq 1 ]]; then
  if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git update-index -q --refresh
    if [[ "$ALLOW_DIRTY" -ne 1 ]] && ! git diff-index --quiet HEAD --; then
      echo "Git worktree has local tracked changes. Commit/stash them or rerun with --allow-dirty." >&2
      exit 1
    fi

    current_branch="$(git branch --show-current || true)"
    echo "Updating source${current_branch:+ on branch $current_branch}..."
    git fetch --prune
    git pull --ff-only
  else
    echo "No git checkout found; redeploying the current files without pulling source."
  fi
else
  echo "Skipping git pull."
fi

if [[ "$PULL_IMAGES" -eq 1 ]]; then
  echo "Pulling production service images..."
  compose pull postgres cloudflared
fi

up_args=(up -d --remove-orphans)
if [[ "$BUILD_APP" -eq 1 ]]; then
  up_args+=(--build)
fi

echo "Starting production containers..."
compose "${up_args[@]}"

echo "Waiting for $APP_SERVICE readiness at $READY_URL..."
deadline=$((SECONDS + WAIT_TIMEOUT))
until compose exec -T "$APP_SERVICE" wget -qO- "$READY_URL" >/dev/null 2>&1; do
  if (( SECONDS >= deadline )); then
    echo "Timed out waiting for $APP_SERVICE readiness after ${WAIT_TIMEOUT}s." >&2
    echo "Recent $APP_SERVICE logs:" >&2
    compose logs --tail=100 "$APP_SERVICE" >&2 || true
    exit 1
  fi
  sleep 2
done

compose ps
echo "Redeploy complete."

if [[ "$TAIL_LOGS" -eq 1 ]]; then
  compose logs -f --tail=100 "$APP_SERVICE"
fi
