# Tokenboard

Tokenboard is a self-hosted leaderboard for AI coding agent token usage.

It gives a team one shared view of how much agent work is happening across
tools, models, people, and days, without uploading raw prompts or transcripts.
The desktop CLI scans local agent history, aggregates token counts and cost
estimates, and submits only those aggregates to a private Tokenboard server.

## Quick Start

Use this path to install the CLI and sign in to the hosted Tokenboard server.
During setup, press Enter to keep `https://tokenboard.net`, or choose a custom
self-hosted server URL when prompted.

macOS and Linux:

```bash
curl -fsSL https://tokenboard.net/install.sh | bash
```

Windows PowerShell:

```powershell
irm https://tokenboard.net/install.ps1 | iex
```

Then finish setup:

```bash
tokenboard --version
tokenboard setup
tokenboard scan
tokenboard sync
tokenboard autosync status
```

`tokenboard setup` opens a GitHub login flow in your browser, saves your
configuration in `~/.tokenboard/config.toml`, and asks whether to enable
automatic background sync.

Automatic sync runs every 3 hours. `tokenboard autosync status` verifies that it
is registered, and `tokenboard autosync install` enables it later if you skip it
during setup.

Useful commands:

```bash
tokenboard scan --today
tokenboard scan --week
tokenboard scan -c claude,codex
tokenboard sync --dry-run
tokenboard sync --week
tokenboard update check
tokenboard update install
```

## What It Does

- Scans local usage from 21 AI coding agents.
- Aggregates daily input, output, cache, reasoning, model, and client totals.
- Syncs usage to a PostgreSQL-backed Express server.
- Shows leaderboards, user profiles, badges, model breakdowns, charts, and
  GitHub-style usage heatmaps in a Vue 3 web UI.
- Uses GitHub OAuth to issue user-bound CLI API tokens.
- Supports automatic background sync every 3 hours.
- Ships Docker Compose paths for local development and production deployment.

## Privacy Model

Tokenboard is designed to track usage metadata, not conversation content.

The CLI submits:

- date-level token totals
- token-type totals
- cost estimates
- model breakdowns
- client breakdowns
- the authenticated Tokenboard user

The CLI does not submit:

- prompts
- responses
- transcript text
- file contents from agent sessions
- full local session records

The scanner reads local history files only to extract aggregate token usage.

The web UI also shows optional GitHub activity context on profile pages. The
server fetches public contribution heatmap data from
`github-contributions-api.deno.dev` and public event/repository data from
GitHub for the requested GitHub username; it does not send Tokenboard usage
records, API tokens, prompts, transcripts, or local file data to those services.

## Install The CLI

The installer downloads the matching binary from GitHub Releases, verifies the
`.sha256` checksum when available, and installs `tokenboard` to `/usr/local/bin`
or `~/.local/bin` on macOS/Linux, or `tokenboard.exe` to the user's
`Microsoft\WindowsApps` directory on Windows.

Release assets are built for:

| Platform | Asset |
| --- | --- |
| macOS Apple Silicon | `tokenboard-aarch64-apple-darwin` |
| macOS Intel | `tokenboard-x86_64-apple-darwin` |
| Linux x86_64 | `tokenboard-x86_64-unknown-linux-musl` |
| Windows x86_64 | `tokenboard-x86_64-pc-windows-msvc.exe` |

Installer options:

```bash
TOKENBOARD_VERSION=<release-tag> scripts/install.sh
TOKENBOARD_INSTALL_DIR="$HOME/bin" scripts/install.sh
TOKENBOARD_REPO=james-uea/tokenboard scripts/install.sh
```

Source builds are mainly for contributors:

```bash
cd client
cargo install --path . --locked
```

## Supported Agents

Default scanners run unless you filter with `-c` or `--client`.

| Agent ID | Agent | Local store |
| --- | --- | --- |
| `claude` | Claude Code | `~/.claude/projects` |
| `codex` | OpenAI Codex | `$CODEX_HOME/sessions` or `~/.codex/sessions` |
| `gemini` | Gemini CLI | `~/.gemini/tmp` |
| `openclaw` | OpenClaw | `~/.openclaw/agents` |
| `pi` | Pi AI | `~/.pi/agent/sessions` |
| `kimi` | Kimi | `~/.kimi/sessions` |
| `qwen` | Qwen Code | `~/.qwen/projects` |
| `copilot` | GitHub Copilot | `~/.copilot/otel` |
| `opencode` | OpenCode | `$XDG_DATA_HOME/opencode/opencode.db` |
| `amp` | Amp | `$XDG_DATA_HOME/amp/threads` |
| `droid` | Factory Droid | `~/.factory/sessions` |
| `roocode` | RooCode | `~/.config/Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks` |
| `kilocode` | KiloCode | `~/.config/Code/User/globalStorage/kilocode.kilo-code/tasks` |
| `mux` | Mux | `~/.mux/sessions` |
| `codebuff` | Codebuff | `$CODEBUFF_DATA_DIR/projects` or `~/.config/manicode/projects` |
| `cursor` | Cursor IDE | `~/.config/tokscale/cursor-cache` |
| `hermes` | Hermes Agent | `$HERMES_HOME/state.db` or `~/.hermes/state.db` |
| `kilo` | Kilo Code | `$XDG_DATA_HOME/kilo/kilo.db` |
| `goose` | Goose | `$XDG_DATA_HOME/goose/sessions/sessions.db` |

Opt-in scanners:

| Agent ID | Agent | Local store |
| --- | --- | --- |
| `antigravity` | Antigravity | `$XDG_CONFIG_HOME/tokscale/antigravity-cache/sessions` |
| `crush` | Crush | `$XDG_DATA_HOME/crush` |

Run an opt-in scanner explicitly:

```bash
tokenboard scan -c antigravity
tokenboard sync -c crush
```

## Self-Host Locally

For local development or a private LAN test, run PostgreSQL and the server with
Docker Compose:

```bash
cp .env.example .env
docker compose up --build
```

The local server listens on `http://localhost:3001` by default.

For direct server development:

```bash
cp .env.example .env
docker compose up -d postgres
set -a
source .env
set +a
npm --prefix server install
npm --prefix server run frontend:build
npm --prefix server run migrate
npm --prefix server run dev
```

Set `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`, and `SESSION_SECRET` in `.env`
before running the OAuth flow locally.

For frontend development:

```bash
npm --prefix server install
npm --prefix server/frontend install
npm --prefix server run frontend:dev
```

For production deployment, see [DEPLOYMENT.md](DEPLOYMENT.md). The production
Compose path is designed for a private Docker network exposed through
Cloudflare Tunnel.

## Server Configuration

Copy `.env.example` to `.env` for local development or `.env.production` for a
production Compose deployment.

Important settings:

| Variable | Purpose |
| --- | --- |
| `APP_BASE_URL` | Public server origin, for example `https://tokenboard.example.com` |
| `DATABASE_URL` | PostgreSQL connection string |
| `SESSION_SECRET` | Secret used for server sessions |
| `GITHUB_CLIENT_ID` | GitHub OAuth app client ID |
| `GITHUB_CLIENT_SECRET` | GitHub OAuth app client secret |
| `ALLOW_LEGACY_API_KEY` | Enables the old shared API key path when set to `true` |
| `FORCE_HTTPS` | Redirects HTTP to HTTPS in production-style deployments |
| `CLOUDFLARE_TUNNEL_TOKEN` | Required by `docker-compose.prod.yml` |

For public or team deployments, keep `ALLOW_LEGACY_API_KEY=false` so
submissions are tied to verified GitHub users.

## CLI Configuration

The CLI reads configuration from environment variables, `.env` files in the
current directory, `~/.tokenboard/.env`, and `~/.tokenboard/config.toml`.

| Variable | Default | Description |
| --- | --- | --- |
| `TOKENBOARD_API_URL` | `https://tokenboard.net` | Tokenboard server URL |
| `TOKENBOARD_API_TOKEN` | unset | User-bound token saved by GitHub setup |
| `TOKENBOARD_API_KEY` | unset | Legacy shared-key fallback |
| `TOKENBOARD_GITHUB_USERNAME` | unset | GitHub username shown on the leaderboard |
| `TOKENBOARD_DISPLAY_NAME` | GitHub username | Display name shown in the UI |
| `TOKENBOARD_AUTO_UPDATE` | `true` | Enables release auto-update before `sync` |
| `TOKENBOARD_UPDATE_REPO` | `james-uea/tokenboard` | GitHub `owner/repo` used for CLI releases |
| `TOKENBOARD_UPDATE_GITHUB_TOKEN` | unset | Optional token for authenticated release access |

## Development

Repository layout:

```text
client/           Rust CLI scanner and sync client
server/           Express API, migrations, tests, and built public assets
server/frontend/  Vue 3 + Vite frontend source
scripts/          Install, deployment, backup, and restore helpers
DEPLOYMENT.md     Docker Compose production deployment guide
```

Run the main checks:

```bash
npm --prefix server test
npm --prefix server run frontend:build
cd client && cargo test
```

Scanner changes should include representative fixture coverage where possible
and preserve existing aggregation, model normalization, and cache-token
semantics.

See [CONTRIBUTING.md](CONTRIBUTING.md) for pull request expectations.

## Security

Do not publish production `.env` files, API tokens, OAuth secrets, PostgreSQL
credentials, backups, or local agent history.

Report suspected vulnerabilities privately through the repository owner's
GitHub vulnerability reporting flow when available. See
[SECURITY.md](SECURITY.md).

## Acknowledgments

Tokenboard was heavily inspired by [Tokscale](https://github.com/junhoyeo/tokscale),
created by [Junho Yeo](https://github.com/junhoyeo). Tokenboard's scanner
coverage decisions, local agent data-source research, token aggregation
techniques, and usage-visualization ideas owe a clear debt to Tokscale.

Tokenboard is an independent project with a different architecture focused on
self-hosted team usage tracking.

## License

Tokenboard is released under the [MIT License](LICENSE).
