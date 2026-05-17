# Tokenboard

Tokenboard is a self-hosted leaderboard for AI coding agent token usage. The
desktop CLI scans local agent session files and submits aggregate daily usage to
a private server, giving a team one shared view of token volume, cost, models,
clients, badges, and trends.

The CLI submits token aggregates only: daily totals, token-type counts, cost
estimates, model breakdowns, and client breakdowns. It does not submit raw
prompts, responses, or transcript text.

Tokenboard builds on the token-scanning approach pioneered by
[tokscale](https://github.com/anthropics/tokscale), but separates the local
scanner from the hosted leaderboard so teams can run their own instance.

## Highlights

- Rust desktop CLI named `tokenboard` for scan, sync, setup, autosync, and
  self-update workflows.
- Node.js/Express server with PostgreSQL, GitHub OAuth, user-bound API tokens,
  and production rate limits.
- Vue 3 web UI with period-filtered leaderboards, user profiles, Chart.js
  visualizations, competitive badges, model tables, and GitHub-style heatmaps.
- Support for 21 AI coding agents, with default scanners for common local
  session stores and opt-in scanners for less common stores.
- Docker Compose paths for local development and Cloudflare Tunnel production
  deployment.

## Project Status

Tokenboard is an actively developed self-hosted project intended for real team
usage tracking. The main roadmap themes are broader scanner coverage, more
pricing accuracy, stronger release automation, and richer product screenshots or
demo assets for the repository landing page.

Contributions are welcome, especially for new scanner fixtures, deployment
hardening, UI polish, and documentation improvements.

## Repository Layout

```text
client/           Rust CLI scanner and sync client
server/           Express API, migrations, tests, and built public assets
server/frontend/  Vue 3 + Vite frontend source
scripts/          Installer, backup, restore, and deployment helpers
DEPLOYMENT.md     Home-server deployment guide
```

## Choose Your Path

| Goal | Start here |
| --- | --- |
| Send your own usage to an existing team server | [Use Tokenboard](#use-tokenboard) |
| Run the shared server for a team | [Self-Host Tokenboard](#self-host-tokenboard) |
| Work on the project locally | [Local Development](#local-development) |

## How It Works

```text
tokenboard CLI
  scans local AI coding agent session files
  aggregates usage by day, model, and client
  submits with a user-bound API token
        |
        v
Tokenboard server
  Express API + PostgreSQL
  serves the Vue leaderboard UI
```

Default scanner IDs:

```text
claude, codex, gemini, openclaw, pi, kimi, qwen, copilot, opencode,
amp, droid, roocode, kilocode, mux, codebuff, cursor, hermes, kilo, goose
```

Opt-in scanner IDs, available with `-c`:

```text
antigravity, crush
```

Use `tokenboard scan -c claude,codex` or
`tokenboard sync -c claude,codex` to limit a run to specific agents.

## Use Tokenboard

Use this path when your team already runs a Tokenboard server and you only need
to submit local usage data.

### Requirements

- A running Tokenboard server URL.
- A GitHub account, used to create your user-bound CLI API token.
- At least one supported local coding agent with session history.
- The `tokenboard` CLI binary for your operating system.

### Install the CLI

| Platform | Release support | Recommended install |
| --- | --- | --- |
| macOS Apple Silicon | `tokenboard-aarch64-apple-darwin` | Use the installer script. |
| macOS Intel | `tokenboard-x86_64-apple-darwin` | Use the installer script. |
| Linux x86_64 | `tokenboard-x86_64-unknown-linux-gnu` | Use the installer script. |
| Windows x86_64 | `tokenboard-x86_64-pc-windows-msvc.exe` | Use Git Bash, or put the `.exe` on `PATH`. |

```bash
curl -fsSL https://raw.githubusercontent.com/james-uea/tokenboard/main/scripts/install.sh | bash
tokenboard --version
```

The installer detects your OS and CPU, downloads the matching release binary,
verifies the `.sha256` checksum when available, and installs `tokenboard` to
`/usr/local/bin` or `~/.local/bin`. On Windows it installs the binary as
`tokenboard.exe` in the chosen install directory. Set `TOKENBOARD_INSTALL_DIR`
to choose a different destination.

Source builds are mainly useful for contributors:

```bash
cargo install --path client --locked
```

### Configure and Sync

1. Run setup and enter your server URL when prompted:

   ```bash
   tokenboard setup
   ```

   Setup opens GitHub in your browser, creates a user-bound API token, and saves
   config to `~/.tokenboard/config.toml`.

2. Test a local scan without submitting:

   ```bash
   tokenboard scan
   ```

3. Submit usage:

   ```bash
   tokenboard sync
   ```

4. Install automatic sync:

   ```bash
   tokenboard autosync install
   tokenboard autosync status
   ```

Autosync runs every 3 hours using the native scheduler for your OS: LaunchAgent
on macOS, systemd user timer on Linux with cron fallback, and Task Scheduler on
Windows.

Useful client commands:

```bash
tokenboard scan --today
tokenboard scan --week
tokenboard sync --week
tokenboard sync -c claude,codex
tokenboard sync --dry-run
tokenboard autosync uninstall
tokenboard update check
tokenboard update install
```

### Client Configuration

The CLI reads configuration from environment variables, `.env` files in the
current directory, `~/.tokenboard/.env`, and `~/.tokenboard/config.toml`.

| Variable | Default | Description |
| --- | --- | --- |
| `TOKENBOARD_API_URL` | `http://localhost:3001` | Tokenboard server URL |
| `TOKENBOARD_API_TOKEN` | unset | User-bound API token created by `tokenboard setup` |
| `TOKENBOARD_API_KEY` | unset | Legacy shared-key fallback, only for servers that explicitly allow it |
| `TOKENBOARD_GITHUB_USERNAME` | unset | GitHub username shown on the leaderboard |
| `TOKENBOARD_DISPLAY_NAME` | GitHub username | Display name shown in the UI |
| `TOKENBOARD_AUTO_UPDATE` | `true` | Install newer CLI releases before `sync` |
| `TOKENBOARD_UPDATE_REPO` | `james-uea/tokenboard` | GitHub `owner/repo` used for CLI releases |
| `TOKENBOARD_UPDATE_GITHUB_TOKEN` | unset | Optional token for authenticated GitHub release access |

## Privacy and Security

Tokenboard is designed to aggregate usage metadata, not copy agent transcripts
to the server.

### What the CLI scans

The scanner reads local agent history files from the registered scanner paths.
Common defaults are:

| Agent ID | Local store |
| --- | --- |
| `claude` | `~/.claude/projects` |
| `codex` | `$CODEX_HOME/sessions`, or `~/.codex/sessions` |
| `gemini` | `~/.gemini/tmp` |
| `openclaw` | `~/.openclaw/agents` |
| `pi` | `~/.pi/agent/sessions` |
| `kimi` | `~/.kimi/sessions` |
| `qwen` | `~/.qwen/projects` |
| `copilot` | `~/.copilot/otel` |
| `opencode` | `$XDG_DATA_HOME/opencode/opencode.db` |
| `amp` | `$XDG_DATA_HOME/amp/threads` |
| `droid` | `~/.factory/sessions` |
| `roocode` | `~/.config/Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks` |
| `kilocode` | `~/.config/Code/User/globalStorage/kilocode.kilo-code/tasks` |
| `mux` | `~/.mux/sessions` |
| `codebuff` | `$CODEBUFF_DATA_DIR/projects`, or `~/.config/manicode/projects` |
| `cursor` | `~/.config/tokscale/cursor-cache` |
| `hermes` | `$HERMES_HOME/state.db`, or `~/.hermes/state.db` |
| `kilo` | `$XDG_DATA_HOME/kilo/kilo.db` |
| `goose` | `$XDG_DATA_HOME/goose/sessions/sessions.db` |
| `antigravity` | `$XDG_CONFIG_HOME/tokscale/antigravity-cache/sessions`, opt-in only |
| `crush` | `$XDG_DATA_HOME/crush`, opt-in only |

When those environment variables are unset, Tokenboard currently resolves
`$XDG_DATA_HOME` as `~/.local/share` and `$XDG_CONFIG_HOME` as `~/.config`.

### What is uploaded and stored

`tokenboard scan` reads local files and prints aggregates without submitting.
`tokenboard sync` sends daily aggregate records to `/api/submit`: date, total
tokens, token-type counts, estimated cost, model breakdowns, client breakdowns,
and your GitHub-derived Tokenboard identity.

The CLI does not upload raw prompts, responses, tool calls, transcript text, file
paths from your workspaces, or source code. The server stores the submitted
daily aggregates in PostgreSQL. User-bound CLI API tokens are created through
GitHub OAuth and stored hashed on the server; the CLI stores its token locally in
`~/.tokenboard/config.toml` unless you provide it through environment variables.

Report vulnerabilities privately; see [SECURITY.md](./SECURITY.md). Do not post
secrets, API tokens, private logs, or raw agent transcripts in public issues.

## Self-Host Tokenboard

Use this path when you are running the shared server for a team.

### Requirements

- Docker and Docker Compose.
- A public HTTPS URL for the app.
- A GitHub OAuth App.
- PostgreSQL, usually via the included Compose stack.
- Strong `SESSION_SECRET` and PostgreSQL credentials.

### Deploy

1. Create a GitHub OAuth App:

   ```text
   Homepage URL: https://tokenboard.example.com
   Callback URL: https://tokenboard.example.com/api/auth/github/callback
   ```

2. Create a production environment file:

   ```bash
   cp .env.production.example .env.production
   ```

3. Fill in at least these values:

   ```bash
   DOMAIN=tokenboard.example.com
   APP_BASE_URL=https://tokenboard.example.com
   CLOUDFLARE_TUNNEL_TOKEN=<cloudflare-tunnel-token>
   SESSION_SECRET=<long-random-secret>
   ALLOW_LEGACY_API_KEY=false
   GITHUB_CLIENT_ID=<github-oauth-client-id>
   GITHUB_CLIENT_SECRET=<github-oauth-client-secret>
   POSTGRES_PASSWORD=<long-random-postgres-password>
   ```

   Generate strong secrets with a password manager or:

   ```bash
   openssl rand -hex 32
   ```

4. Start the production stack:

   ```bash
   docker compose -f docker-compose.prod.yml --env-file .env.production up -d --build
   ```

5. Verify the deployment:

   ```bash
   docker compose -f docker-compose.prod.yml --env-file .env.production ps
   curl -fsS https://tokenboard.example.com/healthz
   curl -fsS https://tokenboard.example.com/readyz
   curl -fsS https://tokenboard.example.com/api/leaderboard
   ```

6. Ask each user to run:

   ```bash
   tokenboard setup
   tokenboard sync
   tokenboard autosync install
   ```

The production Compose file keeps PostgreSQL and the app private on Docker
networking and exposes the app through Cloudflare Tunnel. See
[DEPLOYMENT.md](./DEPLOYMENT.md) for the full home-server deployment, firewall,
backup, restore, and update workflow.

### Security Notes

- `POST /api/submit` requires `Authorization: Bearer <token>`.
- User-bound API tokens are created through GitHub OAuth and stored hashed on
  the server.
- Keep `ALLOW_LEGACY_API_KEY=false` for public or team deployments.
- Public viewers can see leaderboard and profile data, but submission requires a
  valid user API token.
- `FORCE_HTTPS=true`, `TRUST_PROXY=1`, and `TRUST_CLOUDFLARE_HEADERS=true` are
  recommended behind Cloudflare Tunnel or another trusted reverse proxy.

## Local Development

Use this path when you are working on Tokenboard itself.

### Requirements

- Node.js 22 or newer.
- npm.
- Rust stable and Cargo.
- Docker and Docker Compose for local PostgreSQL.

### Setup

1. Start PostgreSQL:

   ```bash
   cp .env.example .env
   docker compose up -d postgres
   ```

2. Install server dependencies:

   ```bash
   npm --prefix server install
   ```

   If your shell has `NODE_ENV=production`, install with:

   ```bash
   NODE_ENV=development npm --prefix server install
   ```

3. Install frontend dependencies:

   ```bash
   npm --prefix server/frontend install
   ```

4. Configure the local server environment in `.env`:

   ```bash
   PORT=3001
   DATABASE_URL=postgresql://tokenboard:tokenboard@localhost:5432/tokenboard
   APP_BASE_URL=http://localhost:3001
   SESSION_SECRET=<local-random-secret>
   GITHUB_CLIENT_ID=<github-oauth-client-id>
   GITHUB_CLIENT_SECRET=<github-oauth-client-secret>
   ALLOW_LEGACY_API_KEY=false
   ```

   For local OAuth, the GitHub OAuth callback URL should be:

   ```text
   http://localhost:3001/api/auth/github/callback
   ```

5. Export the environment for server commands. The Node server reads
   environment variables directly, so source the root `.env` in each terminal
   that runs migrations, the dev server, or tests:

   ```bash
   set -a
   source .env
   set +a
   ```

6. Run migrations:

   ```bash
   npm --prefix server run migrate
   ```

7. Start the API and frontend dev server in separate terminals:

   ```bash
   npm --prefix server run dev
   ```

   ```bash
   npm --prefix server run frontend:dev
   ```

   The API runs on `http://localhost:3001`. Vite serves the Vue frontend on its
   own dev port and proxies API calls according to
   `server/frontend/vite.config.js`.

8. Work on the Rust CLI:

   ```bash
   cd client
   cargo build
   cargo run -- scan
   cargo run -- setup
   cargo run -- sync
   cargo test
   ```

### Verification Commands

```bash
npm --prefix server test
npm --prefix server run frontend:build
cargo test --manifest-path client/Cargo.toml
```

## Server Configuration

| Variable | Default | Description |
| --- | --- | --- |
| `PORT` | `3001` | Server listen port |
| `DATABASE_URL` | `postgresql://tokenboard:tokenboard@postgres:5432/tokenboard` | PostgreSQL connection string |
| `APP_BASE_URL` | `http://localhost:3001` | Public origin used for OAuth callbacks and CLI login links |
| `SESSION_SECRET` | required | Secret for sessions, OAuth states, and API token hashing |
| `GITHUB_CLIENT_ID` | required | GitHub OAuth App client ID |
| `GITHUB_CLIENT_SECRET` | required | GitHub OAuth App client secret |
| `ALLOW_LEGACY_API_KEY` | `false` | Temporarily allow shared-key submissions |
| `API_KEY` | unset | Legacy shared key, only accepted when legacy auth is enabled |
| `TRUST_PROXY` | unset | Express trust proxy setting for reverse proxies |
| `TRUST_CLOUDFLARE_HEADERS` | `false` | Use `CF-Connecting-IP` for rate limit identity |
| `FORCE_HTTPS` | `false` | Redirect forwarded HTTP requests to HTTPS |
| `JSON_BODY_LIMIT` | `256kb` | Express JSON body limit |
| `RATE_LIMIT_ENABLED` | `true` | Enable API rate limits outside tests |
| `RATE_LIMIT_WINDOW_MS` | `900000` | Rate limit window in milliseconds |
| `API_RATE_LIMIT` | `600` | General API requests per window |
| `AUTH_RATE_LIMIT` | `60` | Auth requests per window |
| `CLI_POLL_RATE_LIMIT` | `180` | CLI login poll requests per window |
| `SUBMIT_RATE_LIMIT` | `60` | Submissions per window |
| `GITHUB_PROXY_RATE_LIMIT` | `120` | GitHub proxy requests per window |

When running the server outside Docker, use `localhost` in `DATABASE_URL`.
Inside Docker Compose, use the service hostname `postgres`.

## API

Submission uses Bearer-token auth. Most read endpoints are public so the web UI
can render the leaderboard without a login.

| Method | Path | Auth | Description |
| --- | --- | --- | --- |
| `POST` | `/api/submit` | Bearer user API token | Submit token usage data |
| `GET` | `/api/leaderboard` | Public | All-time leaderboard |
| `GET` | `/api/leaderboard?period=daily` | Public | Daily leaderboard |
| `GET` | `/api/leaderboard?period=weekly` | Public | Weekly leaderboard |
| `GET` | `/api/leaderboard?period=monthly` | Public | Monthly leaderboard |
| `GET` | `/api/stats/:username` | Public | User totals, timelines, model breakdowns, and client breakdowns |
| `GET` | `/api/stats/:username/diffs` | Public | Day-over-day and week-over-week deltas |
| `GET` | `/api/badges` | Public | Competitive badge winners |
| `GET` | `/api/avatar/:username` | Public | GitHub avatar proxy with deterministic fallback |
| `GET` | `/api/github-contributions/:username` | Public | Recent GitHub contribution heatmap data |
| `GET` | `/api/github-daily-detail/:username/:date` | Public | GitHub activity detail for one date |
| `GET` | `/api/auth/github` | Public | Start GitHub OAuth login |
| `GET` | `/api/auth/github/callback` | Public | Complete GitHub OAuth login |
| `GET` | `/api/auth/me` | Session cookie | Current login state |
| `POST` | `/api/auth/logout` | Session cookie | Clear the web session |
| `GET` | `/api/auth/tokens` | Session cookie | List user API tokens |
| `POST` | `/api/auth/tokens` | Session cookie | Create a user-bound CLI API token |
| `DELETE` | `/api/auth/tokens/:id` | Session cookie | Delete a user API token |
| `POST` | `/api/auth/cli/start` | Public | Start CLI GitHub login for `tokenboard setup` |
| `GET` | `/api/auth/cli/complete` | Session cookie | Complete CLI login after GitHub auth |
| `GET` | `/api/auth/cli/poll` | One-time CLI code | Poll for the setup API token after browser login |
| `GET` | `/healthz` | Public | Liveness check |
| `GET` | `/readyz` | Public | Database readiness check |
| `GET` | `/install.sh` | Public | Redirect to the release installer script |

Submissions with user-bound API tokens are stored under the verified GitHub
account that owns the token.

### Submit Example

```bash
curl -fsS https://tokenboard.example.com/api/submit \
  -H "Authorization: Bearer $TOKENBOARD_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "username": "octocat",
    "display_name": "Octocat",
    "contributions": [
      {
        "date": "2026-05-15",
        "total_tokens": 42000,
        "total_cost": 0.84,
        "input_tokens": 18000,
        "output_tokens": 12000,
        "cache_read_tokens": 9000,
        "cache_write_tokens": 3000,
        "reasoning_tokens": 0,
        "models": {
          "claude-sonnet-4-5|claude": {
            "tokens": 42000,
            "input": 18000,
            "output": 12000,
            "cache_read": 9000,
            "cache_write": 3000,
            "cost": 0.84,
            "provider": "Anthropic",
            "source": "claude"
          }
        },
        "clients": {
          "claude": {
            "tokens": 42000,
            "cost": 0.84
          }
        }
      }
    ]
  }'
```

Successful response:

```json
{
  "success": true,
  "username": "octocat",
  "display_name": "Octocat",
  "contributions_updated": 1
}
```

### Read Example

```bash
curl -fsS "https://tokenboard.example.com/api/leaderboard?period=weekly"
```

Response shape:

```json
{
  "period": "weekly",
  "entries": 1,
  "leaderboard": [
    {
      "rank": 1,
      "username": "octocat",
      "display_name": "Octocat",
      "total_tokens": 42000,
      "total_cost": 0.84,
      "active_days": 1,
      "top_model": "claude-sonnet-4-5"
    }
  ]
}
```

## Support

- Questions, bug reports, scanner requests, and deployment help:
  [open a GitHub issue](https://github.com/james-uea/tokenboard/issues).
- Security reports: follow [SECURITY.md](./SECURITY.md) instead of opening a
  public issue.
- Deployment operations: start with [DEPLOYMENT.md](./DEPLOYMENT.md) and include
  Compose status, health-check output, server logs, and sanitized environment
  details when asking for help.

## Contributing

Read [CONTRIBUTING.md](./CONTRIBUTING.md) before opening a pull request. In
short: keep changes focused, describe user-visible behavior, mention deployment
or configuration impact, include screenshots for visible frontend changes, and
run the relevant checks:

```bash
npm --prefix server test
npm --prefix server run frontend:build
cargo test --manifest-path client/Cargo.toml
```

Scanner contributions should include representative fixture coverage where
possible and must preserve aggregation, model normalization, cache-token
semantics, and API payload compatibility.

## License

MIT. See [LICENSE](./LICENSE).
