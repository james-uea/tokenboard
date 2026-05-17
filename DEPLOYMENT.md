# Tokenboard Docker Compose Deployment

This path publishes Tokenboard from a Linux host with Docker Compose. The
included production Compose file keeps the app and PostgreSQL private on Docker
networking and exposes the app through Cloudflare Tunnel.

## Public Flow

```text
visitor
  -> Cloudflare DNS/WAF
  -> Cloudflare Tunnel
  -> cloudflared container
  -> app:3001
  -> postgres:5432
```

Anyone can view the leaderboard. Users run `tokenboard setup`, sign in with
GitHub in the browser, and submit with the user-bound token created by the CLI
login. Keep
`ALLOW_LEGACY_API_KEY=false` for public deployments so submissions are tied to
verified GitHub accounts.

## Cloudflare Setup

1. In Cloudflare Zero Trust, create a remotely managed tunnel for the home server.
2. Add a public hostname:
   ```text
   Hostname: tokenboard.<domain>
   Service:  http://app:3001
   ```
3. Copy the tunnel token into `.env.production`.
4. Do not create an `A` or `AAAA` record pointing at the home IP for this app.

Recommended Cloudflare rules:

- Redirect HTTP to HTTPS. The app also enforces this when `FORCE_HTTPS=true`.
- WAF custom rule or rate limiting rule for `/api/auth/*`.
- WAF custom rule or rate limiting rule for `/api/submit`.
- WAF custom rule or rate limiting rule for `/api/github-daily-detail/*`.
- Cache static assets only; do not cache `/api/auth/*` or `/api/submit`.

## GitHub OAuth

Create a GitHub OAuth App:

```text
Homepage URL: https://tokenboard.<domain>
Callback URL: https://tokenboard.<domain>/api/auth/github/callback
```

Set the same public origin in `.env.production`:

```bash
APP_BASE_URL=https://tokenboard.<domain>
```

## Copy To Remote Host

From this workstation, copy the current repository state to a remote Docker
host:

```bash
scripts/deploy-remote.sh <user>@<host>
```

The script prepares `/opt/tokenboard`, rsyncs the repo, preserves any existing
`.env.production`, excludes local secrets/build outputs, and uses one shared SSH
connection so an encrypted SSH key should only prompt once.
It requires `ssh` and `rsync` locally, and `rsync` plus `sudo` privileges on the
target when creating `/opt/tokenboard`.

Then SSH to the server and edit the generated production env:

```bash
ssh <user>@<host>
cd /opt/tokenboard
nano .env.production
```

Set at least:

```bash
DOMAIN=tokenboard.<domain>
APP_BASE_URL=https://tokenboard.<domain>
CLOUDFLARE_TUNNEL_TOKEN=<cloudflare-tunnel-token>
SESSION_SECRET=<openssl rand -hex 32>
ALLOW_LEGACY_API_KEY=false
GITHUB_CLIENT_ID=<github-oauth-client-id>
GITHUB_CLIENT_SECRET=<github-oauth-client-secret>
POSTGRES_PASSWORD=<openssl rand -hex 32>
```

Start the stack:

```bash
scripts/deploy-compose.sh --no-pull
```

No service in this Compose file publishes host ports. The only public path is
the outbound Cloudflare Tunnel connection.

## Firewall

On the router:

- Do not forward ports `80`, `443`, `3001`, or `5432`.
- Do not expose Docker directly to the internet.

On the server:

```bash
sudo ufw default deny incoming
sudo ufw default allow outgoing
sudo ufw allow from 192.168.0.0/16 to any port 22 proto tcp
sudo ufw enable
```

Adjust the LAN CIDR if your home network is not `192.168.0.0/16`.

## Verification

```bash
docker compose -f docker-compose.prod.yml --env-file .env.production ps
docker compose -f docker-compose.prod.yml --env-file .env.production logs --tail=100 app
docker compose -f docker-compose.prod.yml --env-file .env.production logs --tail=100 cloudflared
curl -fsS https://tokenboard.<domain>/healthz
curl -fsS https://tokenboard.<domain>/readyz
curl -fsS https://tokenboard.<domain>/api/leaderboard
```

Then run `tokenboard setup`, complete the GitHub login in the browser, and
submit one `tokenboard sync`.

## Optional Offsite Backups

The app can deploy without offsite backups. To enable encrypted PostgreSQL
backups to an S3-compatible bucket, add backup settings to `.env.production`:

```bash
cd /opt/tokenboard
nano .env.production
```

Set a strong `BACKUP_PASSPHRASE` and the `S3_*` variables from `.env.example`.
For Cloudflare R2, `S3_ENDPOINT_URL` usually looks like
`https://<account-id>.r2.cloudflarestorage.com`. Then run one manual backup:

```bash
cd /opt/tokenboard
scripts/backup.sh
```

Nightly cron example:

```bash
sudo tee /etc/cron.d/tokenboard-backup >/dev/null <<'EOF'
SHELL=/bin/bash
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
17 3 * * * root cd /opt/tokenboard && /opt/tokenboard/scripts/backup.sh >> /var/log/tokenboard-backup.log 2>&1
EOF
```

Restore:

```bash
cd /opt/tokenboard
CONFIRM_RESTORE=tokenboard \
scripts/restore.sh latest
```

## Updates

Server updates can be deployed from git:

```bash
cd /opt/tokenboard
scripts/deploy-compose.sh
```

The Rust `tokenboard` CLI updates itself from GitHub Releases during `sync`.
The public `james-uea/tokenboard` release repository does not require a GitHub
token for release checks or downloads.
