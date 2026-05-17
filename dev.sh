#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# tokenboard dev environment — one command to start everything
#
# Usage:
#   ./dev.sh            # Start postgres, run migrations, launch server + frontend
#   ./dev.sh --api-key abc123  # Override legacy API key
#   ./dev.sh --no-frontend     # Server only (if you want separate terminal)
#   ./dev.sh stop        # Tear down the postgres container
#
# What it does:
#   1. Starts postgres:16-alpine via docker (only postgres, no app container)
#   2. Waits for postgres to be healthy
#   3. Runs DB migrations
#   4. Starts Express server on :3001 with --watch (hot reload)
#   5. Starts Vite dev server on :5173 with HMR, proxying /api → :3001
#   6. Cleans up the postgres container on Ctrl+C
# ─────────────────────────────────────────────────────────────────────────────

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# ── config ──────────────────────────────────────────────────────────────────

DB_USER="${DB_USER:-tokenboard}"
DB_PASS="${DB_PASS:-tokenboard}"
DB_NAME="${DB_NAME:-tokenboard}"
DB_PORT="${DB_PORT:-5432}"
DB_HOST="${DB_HOST:-localhost}"
API_KEY="${API_KEY:-dev-key}"
SERVER_PORT="${SERVER_PORT:-3001}"
ALLOW_LEGACY_API_KEY="${ALLOW_LEGACY_API_KEY:-false}"
APP_BASE_URL="${APP_BASE_URL:-http://localhost:${SERVER_PORT}}"
SESSION_SECRET="${SESSION_SECRET:-dev-session-secret}"
GITHUB_CLIENT_ID="${GITHUB_CLIENT_ID:-}"
GITHUB_CLIENT_SECRET="${GITHUB_CLIENT_SECRET:-}"
CONTAINER_NAME="tokenboard-postgres-dev"

DATABASE_URL="postgresql://${DB_USER}:${DB_PASS}@${DB_HOST}:${DB_PORT}/${DB_NAME}"

# ── args ────────────────────────────────────────────────────────────────────

FRONTEND=true
STOP=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        stop)
            STOP=true; shift ;;
        --api-key)
            API_KEY="$2"; shift 2 ;;
        --no-frontend)
            FRONTEND=false; shift ;;
        *)
            echo "Unknown arg: $1"; exit 1 ;;
    esac
done

# ── stop ────────────────────────────────────────────────────────────────────

if $STOP; then
    echo "🛑 Stopping postgres container..."
    docker stop "$CONTAINER_NAME" 2>/dev/null || true
    docker rm "$CONTAINER_NAME" 2>/dev/null || true
    echo "✅ Done."
    exit 0
fi

# ── cleanup on exit ─────────────────────────────────────────────────────────

cleanup() {
    echo ""
    echo "🛑 Shutting down..."

    # Kill the server + frontend background processes
    if [[ -n "${SERVER_PID:-}" ]]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    if [[ -n "${FRONTEND_PID:-}" ]]; then
        kill "$FRONTEND_PID" 2>/dev/null || true
        wait "$FRONTEND_PID" 2>/dev/null || true
    fi

    echo "✅ Dev environment stopped. Postgres container still running."
    echo "   Run './dev.sh stop' to tear it down."
}
trap cleanup EXIT INT TERM

# ── step 1: postgres container ──────────────────────────────────────────────

# Check if an existing container has the correct port mapping
need_create=true
if docker ps -a --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
    ports=$(docker port "$CONTAINER_NAME" 5432 2>/dev/null || true)
    if echo "$ports" | grep -q "0.0.0.0:${DB_PORT}\|:::${DB_PORT}\|${DB_PORT}"; then
        need_create=false
        if docker ps --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
            echo "✅ Postgres container already running"
        else
            echo "🔄 Starting existing postgres container..."
            docker start "$CONTAINER_NAME" >/dev/null
        fi
    else
        echo "🔄 Recreating postgres container (port mapping changed)..."
        docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
    fi
fi

if $need_create; then
    echo "🐘 Creating postgres:16-alpine container..."
    docker run -d \
        --name "$CONTAINER_NAME" \
        -e POSTGRES_USER="$DB_USER" \
        -e POSTGRES_PASSWORD="$DB_PASS" \
        -e POSTGRES_DB="$DB_NAME" \
        -p "${DB_PORT}:5432" \
        -v tokenboard-pgdata-dev:/var/lib/postgresql/data \
        postgres:16-alpine >/dev/null
fi

# ── step 2: wait for postgres ───────────────────────────────────────────────

echo -n "⏳ Waiting for postgres"
for i in $(seq 1 30); do
    if docker exec "$CONTAINER_NAME" pg_isready -U "$DB_USER" -q 2>/dev/null; then
        echo " ready"
        break
    fi
    echo -n "."
    sleep 1
done

# Also verify the host can reach it
if ! pg_isready -h "$DB_HOST" -p "$DB_PORT" -q 2>/dev/null; then
    echo ""
    echo "❌ Postgres is running but host can't reach ${DB_HOST}:${DB_PORT}"
    echo "   Docker port mapping may be broken. Try: docker rm -f ${CONTAINER_NAME} && ./dev.sh"
    exit 1
fi

# ── step 3: run migrations ──────────────────────────────────────────────────

echo "📦 Running DB migrations..."
DATABASE_URL="$DATABASE_URL" node server/migrate.js

# ── step 4: start express server ────────────────────────────────────────────

# Check for port conflicts
if lsof -i ":${SERVER_PORT}" -sTCP:LISTEN -t >/dev/null 2>&1; then
    echo ""
    echo "⚠️  Port ${SERVER_PORT} is already in use. Killing existing process..."
    lsof -i ":${SERVER_PORT}" -sTCP:LISTEN -t | xargs kill -9 2>/dev/null || true
    sleep 1
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  🚀 Server   → http://localhost:${SERVER_PORT}"
if $FRONTEND; then
    echo "  🎨 Frontend → http://localhost:5173 (proxies /api → :${SERVER_PORT})"
fi
echo "  🐘 Postgres → postgresql://${DB_USER}:${DB_PASS}@${DB_HOST}:${DB_PORT}/${DB_NAME}"
echo ""
echo "  Press Ctrl+C to stop the dev servers"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

DATABASE_URL="$DATABASE_URL" \
API_KEY="$API_KEY" \
ALLOW_LEGACY_API_KEY="$ALLOW_LEGACY_API_KEY" \
APP_BASE_URL="$APP_BASE_URL" \
SESSION_SECRET="$SESSION_SECRET" \
GITHUB_CLIENT_ID="$GITHUB_CLIENT_ID" \
GITHUB_CLIENT_SECRET="$GITHUB_CLIENT_SECRET" \
PORT="$SERVER_PORT" \
node --watch server/index.js &
SERVER_PID=$!

# ── step 5: start frontend ──────────────────────────────────────────────────

if $FRONTEND; then
    sleep 1.5  # let the server boot first
    cd server/frontend
    NODE_ENV=development npx vite &
    FRONTEND_PID=$!
    cd "$SCRIPT_DIR"
fi

# ── keep alive until either server or frontend dies ─────────────────────────

# Don't use wait -n (it returns on first child, and npm exits after spawning vite).
# Instead, poll both PIDs until one goes away.
while true; do
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo ""
        echo "❌ Server died unexpectedly"
        break
    fi
    if $FRONTEND; then
        if ! kill -0 "$FRONTEND_PID" 2>/dev/null; then
            echo ""
            echo "❌ Frontend died unexpectedly"
            break
        fi
    fi
    sleep 2
done
