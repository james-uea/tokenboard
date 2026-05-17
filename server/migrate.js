import pool from "./db.js";

const schema = `
CREATE TABLE IF NOT EXISTS users (
  id SERIAL PRIMARY KEY,
  username VARCHAR(64) UNIQUE NOT NULL,
  created_at TIMESTAMPTZ DEFAULT NOW()
);

ALTER TABLE users
  ADD COLUMN IF NOT EXISTS display_name VARCHAR(120);

ALTER TABLE users
  ADD COLUMN IF NOT EXISTS github_id VARCHAR(32),
  ADD COLUMN IF NOT EXISTS github_login VARCHAR(64),
  ADD COLUMN IF NOT EXISTS avatar_url TEXT,
  ADD COLUMN IF NOT EXISTS profile_url TEXT,
  ADD COLUMN IF NOT EXISTS github_verified_at TIMESTAMPTZ;

UPDATE users
SET display_name = username
WHERE display_name IS NULL OR BTRIM(display_name) = '';

UPDATE users
SET display_name = BTRIM(display_name)
WHERE display_name IS NOT NULL AND display_name <> BTRIM(display_name);

ALTER TABLE users
  ALTER COLUMN display_name SET NOT NULL;

CREATE TABLE IF NOT EXISTS submissions (
  id SERIAL PRIMARY KEY,
  user_id INT REFERENCES users(id) ON DELETE CASCADE,
  date DATE NOT NULL,
  total_tokens BIGINT NOT NULL DEFAULT 0,
  total_cost NUMERIC(14,6) NOT NULL DEFAULT 0,
  input_tokens BIGINT DEFAULT 0,
  output_tokens BIGINT DEFAULT 0,
  cache_read_tokens BIGINT DEFAULT 0,
  cache_write_tokens BIGINT DEFAULT 0,
  reasoning_tokens BIGINT DEFAULT 0,
  models JSONB DEFAULT '{}',
  clients JSONB DEFAULT '{}',
  submitted_at TIMESTAMPTZ DEFAULT NOW(),
  UNIQUE(user_id, date)
);

ALTER TABLE submissions
  ALTER COLUMN models SET DEFAULT '{}'::jsonb,
  ALTER COLUMN clients SET DEFAULT '{}'::jsonb;

UPDATE submissions
SET models = '{}'::jsonb
WHERE models IS NULL OR jsonb_typeof(models) <> 'object';

UPDATE submissions
SET clients = '{}'::jsonb
WHERE clients IS NULL OR jsonb_typeof(clients) <> 'object';

ALTER TABLE submissions
  ALTER COLUMN models SET NOT NULL,
  ALTER COLUMN clients SET NOT NULL;

CREATE INDEX IF NOT EXISTS idx_submissions_user_date ON submissions(user_id, date DESC);
CREATE INDEX IF NOT EXISTS idx_submissions_date ON submissions(date DESC);
CREATE INDEX IF NOT EXISTS idx_users_username ON users(username);
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_github_id ON users(github_id) WHERE github_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_github_login_lower ON users(LOWER(github_login)) WHERE github_login IS NOT NULL;

CREATE TABLE IF NOT EXISTS auth_sessions (
  session_hash CHAR(64) PRIMARY KEY,
  user_id INT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  expires_at TIMESTAMPTZ NOT NULL,
  last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  user_agent TEXT,
  ip_address TEXT
);

CREATE INDEX IF NOT EXISTS idx_auth_sessions_user ON auth_sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_auth_sessions_expires ON auth_sessions(expires_at);

CREATE TABLE IF NOT EXISTS oauth_states (
  state_hash CHAR(64) PRIMARY KEY,
  return_to TEXT NOT NULL DEFAULT '/',
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  expires_at TIMESTAMPTZ NOT NULL,
  consumed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_oauth_states_expires ON oauth_states(expires_at);

CREATE TABLE IF NOT EXISTS user_api_tokens (
  id SERIAL PRIMARY KEY,
  user_id INT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name VARCHAR(80) NOT NULL,
  token_hash CHAR(64) UNIQUE NOT NULL,
  token_prefix VARCHAR(16) NOT NULL,
  last_four VARCHAR(4) NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  last_used_at TIMESTAMPTZ,
  revoked_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_user_api_tokens_user ON user_api_tokens(user_id);
CREATE INDEX IF NOT EXISTS idx_user_api_tokens_hash ON user_api_tokens(token_hash) WHERE revoked_at IS NULL;

CREATE TABLE IF NOT EXISTS cli_login_requests (
  code_hash CHAR(64) PRIMARY KEY,
  token_name VARCHAR(80) NOT NULL,
  user_id INT REFERENCES users(id) ON DELETE CASCADE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  expires_at TIMESTAMPTZ NOT NULL,
  completed_at TIMESTAMPTZ,
  consumed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_cli_login_requests_expires ON cli_login_requests(expires_at);
CREATE INDEX IF NOT EXISTS idx_cli_login_requests_user ON cli_login_requests(user_id);
`;

async function migrate() {
	const client = await pool.connect();
	try {
		await client.query(schema);
		console.log("Migration completed successfully");
	} catch (err) {
		console.error("Migration failed:", err);
		process.exit(1);
	} finally {
		client.release();
		await pool.end();
	}
}

migrate();
