# Copilot Instructions for Tokenboard

## Architecture

Tokenboard is a self-hosted leaderboard for AI coding agent token usage.

- `client/`: Rust CLI that scans local agent session files and submits daily
  usage.
- `server/`: Node.js/Express API, PostgreSQL access, auth, and static hosting.
- `server/frontend/`: Vue 3 SPA built by Vite into `server/public/`.

The server uses user-bound bearer tokens for submissions and GitHub OAuth for
web login and CLI setup. Keep `ALLOW_LEGACY_API_KEY=false` for public or team
deployments.

## Development Commands

```bash
npm --prefix server install
npm --prefix server/frontend install
npm --prefix server test
npm --prefix server run frontend:build
cd client && cargo test
```

Use `docker compose up -d postgres` for local PostgreSQL. Run
`npm --prefix server run migrate` after configuring local database environment
variables.

## Code Conventions

- Server code uses ES modules, Express routers under `server/routes/`, and
  parameterized PostgreSQL queries through `pg`.
- Frontend code is Vue 3. The main app lives in `server/frontend/src/App.vue`
  and styling lives in `server/frontend/src/app.css`.
- Do not edit generated `server/public/` assets directly; rebuild with
  `npm --prefix server run frontend:build`.
- Rust scanner behavior is data-sensitive. Preserve model normalization,
  per-model `model|agent` aggregation, cache-token semantics, and Claude
  deduplication.

## Pull Request Checks

Run the relevant checks for touched layers:

- Server/API: `npm --prefix server test`
- Frontend: `npm --prefix server run frontend:build`
- Client/scanner: `cd client && cargo test`
