import { Router } from "express";
import pool from "../db.js";
import { getGitHubOAuthConfig } from "../lib/config.js";
import {
	OAUTH_STATE_COOKIE,
	SESSION_COOKIE,
	clearCookie,
	createSession,
	createUserApiToken,
	destroySession,
	getBaseUrl,
	getCookie,
	getSessionUser,
	hashToken,
	publicUser,
	randomToken,
	safeReturnTo,
	serializeApiToken,
	setCookie,
} from "../lib/auth.js";

const router = Router();
const OAUTH_STATE_TTL_MS = 10 * 60 * 1000;
const CLI_LOGIN_TTL_MS = 10 * 60 * 1000;
const CLI_LOGIN_POLL_INTERVAL_SECONDS = 2;
const MAX_TOKEN_NAME_LENGTH = 80;

function getQueryString(value) {
	if (Array.isArray(value)) {
		return typeof value[0] === "string" ? value[0].trim() : "";
	}
	return typeof value === "string" ? value.trim() : "";
}

function requireGitHubConfig() {
	return getGitHubOAuthConfig();
}

function getTokenName(value) {
	const rawName = typeof value === "string" ? value.trim() : "";
	return rawName || "tokenboard CLI";
}

function cliLoginPath(code) {
	return `/api/auth/cli/complete?code=${encodeURIComponent(code)}`;
}

function cliLoginUrl(req, code) {
	const loginPath = `/api/auth/github?return_to=${encodeURIComponent(cliLoginPath(code))}`;
	return new URL(loginPath, getBaseUrl(req)).toString();
}

function renderCliLoginPage(title, message) {
	const escapeHtml = (value) => String(value || "")
		.replaceAll("&", "&amp;")
		.replaceAll("<", "&lt;")
		.replaceAll(">", "&gt;")
		.replaceAll('"', "&quot;");

	return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>${escapeHtml(title)}</title>
  <style>
    :root { color-scheme: dark; font-family: system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    body { min-height: 100vh; margin: 0; display: grid; place-items: center; background: #07080a; color: #f0f1f2; }
    main { width: min(440px, calc(100vw - 32px)); border: 1px solid rgba(255,255,255,.09); border-radius: 10px; background: #111316; padding: 28px; }
    h1 { margin: 0 0 8px; font-size: 1.2rem; }
    p { margin: 0; color: #b0b6be; line-height: 1.5; }
  </style>
</head>
<body>
  <main>
    <h1>${escapeHtml(title)}</h1>
    <p>${escapeHtml(message)}</p>
  </main>
</body>
</html>`;
}

function isExpired(row) {
	const expiresAt = new Date(row?.expires_at).getTime();
	return Number.isFinite(expiresAt) && expiresAt <= Date.now();
}

async function requireSession(req, res, next) {
	try {
		const user = await getSessionUser(req);
		if (!user) {
			return res.status(401).json({ error: "GitHub login required" });
		}
		req.user = user;
		next();
	} catch (error) {
		console.error("Session lookup failed:", error);
		res.status(500).json({ error: "Internal server error" });
	}
}

async function exchangeGitHubCode(code, redirectUri) {
	const { clientId, clientSecret } = requireGitHubConfig();
	const response = await fetch("https://github.com/login/oauth/access_token", {
		method: "POST",
		headers: {
			Accept: "application/json",
			"Content-Type": "application/x-www-form-urlencoded",
			"User-Agent": "tokenboard",
		},
		body: new URLSearchParams({
			client_id: clientId,
			client_secret: clientSecret,
			code,
			redirect_uri: redirectUri,
		}),
	});

	if (!response.ok) {
		throw new Error(`GitHub token exchange failed with ${response.status}`);
	}

	const payload = await response.json();
	if (!payload.access_token || payload.error) {
		throw new Error(payload.error_description || payload.error || "GitHub token exchange failed");
	}
	return payload.access_token;
}

async function fetchGitHubUser(accessToken) {
	const response = await fetch("https://api.github.com/user", {
		headers: {
			Accept: "application/vnd.github+json",
			Authorization: `Bearer ${accessToken}`,
			"User-Agent": "tokenboard",
			"X-GitHub-Api-Version": "2022-11-28",
		},
	});

	if (!response.ok) {
		throw new Error(`GitHub user request failed with ${response.status}`);
	}

	const profile = await response.json();
	if (!profile?.id || !profile?.login) {
		throw new Error("GitHub user response did not include id and login");
	}
	return profile;
}

async function upsertGitHubUser(profile) {
	const githubId = String(profile.id);
	const githubLogin = String(profile.login || "").trim().slice(0, 64);
	const displayName = (String(profile.name || "").trim() || githubLogin).slice(0, 120);
	const avatarUrl = typeof profile.avatar_url === "string" ? profile.avatar_url : "";
	const profileUrl = typeof profile.html_url === "string" ? profile.html_url : "";
	const client = await pool.connect();

	try {
		await client.query("BEGIN");

		let existing = await client.query(
			"SELECT id FROM users WHERE github_id = $1 FOR UPDATE",
			[githubId],
		);

		if (existing.rows.length === 0) {
			existing = await client.query(
				`SELECT id
         FROM users
         WHERE LOWER(username) = LOWER($1)
            OR LOWER(COALESCE(github_login, '')) = LOWER($1)
         ORDER BY id ASC
         LIMIT 1
         FOR UPDATE`,
				[githubLogin],
			);
		}

		let result;
		if (existing.rows.length > 0) {
			result = await client.query(
				`UPDATE users
         SET username = $2,
             display_name = $3,
             github_id = $4,
             github_login = $2,
             avatar_url = $5,
             profile_url = $6,
             github_verified_at = NOW()
         WHERE id = $1
         RETURNING id, username, display_name, github_id, github_login,
                   avatar_url, profile_url, github_verified_at`,
				[
					existing.rows[0].id,
					githubLogin,
					displayName,
					githubId,
					avatarUrl,
					profileUrl,
				],
			);
		} else {
			result = await client.query(
				`INSERT INTO users
         (username, display_name, github_id, github_login, avatar_url, profile_url, github_verified_at)
       VALUES ($1, $2, $3, $1, $4, $5, NOW())
       RETURNING id, username, display_name, github_id, github_login,
                 avatar_url, profile_url, github_verified_at`,
				[githubLogin, displayName, githubId, avatarUrl, profileUrl],
			);
		}

		await client.query("COMMIT");
		return publicUser(result.rows[0]);
	} catch (error) {
		await client.query("ROLLBACK");
		throw error;
	} finally {
		client.release();
	}
}

router.get("/github", async (req, res) => {
	let clientId;
	try {
		({ clientId } = requireGitHubConfig());
	} catch (error) {
		return res.status(500).json({ error: error.message });
	}

	const state = randomToken();
	const returnTo = safeReturnTo(req.query.return_to);
	const redirectUri = new URL("/api/auth/github/callback", getBaseUrl(req)).toString();

	try {
		await pool.query(
			`INSERT INTO oauth_states (state_hash, return_to, expires_at)
       VALUES ($1, $2, $3)`,
			[hashToken(state), returnTo, new Date(Date.now() + OAUTH_STATE_TTL_MS)],
		);

		setCookie(res, req, OAUTH_STATE_COOKIE, state, {
			maxAge: Math.floor(OAUTH_STATE_TTL_MS / 1000),
		});

		const authorizeUrl = new URL("https://github.com/login/oauth/authorize");
		authorizeUrl.searchParams.set("client_id", clientId);
		authorizeUrl.searchParams.set("redirect_uri", redirectUri);
		authorizeUrl.searchParams.set("state", state);
		authorizeUrl.searchParams.set("scope", "read:user");
		res.redirect(authorizeUrl.toString());
	} catch (error) {
		console.error("GitHub OAuth start failed:", error);
		res.status(500).json({ error: "Internal server error" });
	}
});

router.get("/github/callback", async (req, res) => {
	const code = getQueryString(req.query.code);
	const state = getQueryString(req.query.state);
	const cookieState = getCookie(req, OAUTH_STATE_COOKIE);

	if (!code || !state || !cookieState || state !== cookieState) {
		clearCookie(res, req, OAUTH_STATE_COOKIE);
		return res.status(400).json({ error: "Invalid GitHub OAuth state" });
	}

	let returnTo = "/";
	const client = await pool.connect();
	try {
		await client.query("BEGIN");
		const stateResult = await client.query(
			`SELECT return_to
       FROM oauth_states
       WHERE state_hash = $1
         AND consumed_at IS NULL
         AND expires_at > NOW()
       FOR UPDATE`,
			[hashToken(state)],
		);

		if (stateResult.rows.length === 0) {
			await client.query("ROLLBACK");
			clearCookie(res, req, OAUTH_STATE_COOKIE);
			return res.status(400).json({ error: "Invalid or expired GitHub OAuth state" });
		}

		returnTo = safeReturnTo(stateResult.rows[0].return_to);
		await client.query(
			"UPDATE oauth_states SET consumed_at = NOW() WHERE state_hash = $1",
			[hashToken(state)],
		);
		await client.query("COMMIT");
	} catch (error) {
		await client.query("ROLLBACK");
		console.error("GitHub OAuth state validation failed:", error);
		clearCookie(res, req, OAUTH_STATE_COOKIE);
		return res.status(500).json({ error: "Internal server error" });
	} finally {
		client.release();
	}

	try {
		const redirectUri = new URL("/api/auth/github/callback", getBaseUrl(req)).toString();
		const accessToken = await exchangeGitHubCode(code, redirectUri);
		const githubUser = await fetchGitHubUser(accessToken);
		const user = await upsertGitHubUser(githubUser);
		const session = await createSession(user.id, req);

		clearCookie(res, req, OAUTH_STATE_COOKIE);
		setCookie(res, req, SESSION_COOKIE, session.token, {
			maxAge: session.maxAgeSeconds,
		});
		res.redirect(returnTo);
	} catch (error) {
		console.error("GitHub OAuth callback failed:", error);
		clearCookie(res, req, OAUTH_STATE_COOKIE);
		res.status(502).json({ error: "GitHub login failed" });
	}
});

router.post("/cli/start", async (req, res) => {
	try {
		requireGitHubConfig();
	} catch (error) {
		return res.status(500).json({ error: error.message });
	}

	const code = randomToken();
	const tokenName = getTokenName(req.body?.name);
	if (tokenName.length > MAX_TOKEN_NAME_LENGTH) {
		return res.status(400).json({ error: `name must be <= ${MAX_TOKEN_NAME_LENGTH} characters` });
	}
	const expiresAt = new Date(Date.now() + CLI_LOGIN_TTL_MS);

	try {
		await pool.query(
			`INSERT INTO cli_login_requests (code_hash, token_name, expires_at)
       VALUES ($1, $2, $3)`,
			[hashToken(code), tokenName, expiresAt],
		);

		res.setHeader("Cache-Control", "no-store");
		res.status(201).json({
			code,
			login_url: cliLoginUrl(req, code),
			expires_in: Math.floor(CLI_LOGIN_TTL_MS / 1000),
			poll_interval: CLI_LOGIN_POLL_INTERVAL_SECONDS,
		});
	} catch (error) {
		console.error("CLI login start failed:", error);
		res.status(500).json({ error: "Internal server error" });
	}
});

router.get("/cli/complete", async (req, res) => {
	const code = getQueryString(req.query.code);
	if (!code) {
		return res.status(400).send(renderCliLoginPage(
			"Tokenboard login failed",
			"The CLI login request was missing its verification code.",
		));
	}

	try {
		const user = await getSessionUser(req);
		if (!user) {
			const returnTo = req.originalUrl || cliLoginPath(code);
			return res.redirect(`/api/auth/github?return_to=${encodeURIComponent(returnTo)}`);
		}

		const codeHash = hashToken(code);
		const requestResult = await pool.query(
			`SELECT token_name, completed_at, consumed_at, expires_at
       FROM cli_login_requests
       WHERE code_hash = $1`,
			[codeHash],
		);

		if (requestResult.rows.length === 0 || isExpired(requestResult.rows[0])) {
			return res.status(400).send(renderCliLoginPage(
				"Tokenboard login expired",
				"Start tokenboard setup again in your terminal.",
			));
		}

		const requestRow = requestResult.rows[0];
		if (requestRow.consumed_at) {
			return res.status(400).send(renderCliLoginPage(
				"Tokenboard login already used",
				"Start tokenboard setup again if you need another API token.",
			));
		}

		if (!requestRow.completed_at) {
			await pool.query(
				`UPDATE cli_login_requests
         SET user_id = $2,
             completed_at = NOW()
       WHERE code_hash = $1
         AND completed_at IS NULL
         AND consumed_at IS NULL`,
				[codeHash, user.id],
			);
		}

		res.setHeader("Cache-Control", "no-store");
		res.send(renderCliLoginPage(
			"Tokenboard login complete",
			"You can close this tab and return to tokenboard setup in your terminal.",
		));
	} catch (error) {
		console.error("CLI login complete failed:", error);
		res.status(500).send(renderCliLoginPage(
			"Tokenboard login failed",
			"Tokenboard could not finish the CLI login request.",
		));
	}
});

router.get("/cli/poll", async (req, res) => {
	const code = getQueryString(req.query.code);
	if (!code) {
		return res.status(400).json({ error: "code is required" });
	}

	try {
		const result = await pool.query(
			`SELECT
         r.token_name,
         r.completed_at,
         r.consumed_at,
         r.expires_at,
         u.id,
         u.username,
         u.display_name,
         u.github_id,
         u.github_login,
         u.avatar_url,
         u.profile_url,
         u.github_verified_at
       FROM cli_login_requests r
       LEFT JOIN users u ON u.id = r.user_id
       WHERE r.code_hash = $1`,
			[hashToken(code)],
		);

		res.setHeader("Cache-Control", "no-store");

		if (result.rows.length === 0) {
			return res.status(404).json({ error: "Unknown CLI login request" });
		}

		const row = result.rows[0];
		if (row.consumed_at) {
			return res.status(410).json({ error: "CLI login request was already consumed" });
		}
		if (isExpired(row)) {
			return res.status(410).json({ error: "CLI login request expired" });
		}
		if (!row.completed_at) {
			return res.status(202).json({ status: "pending" });
		}
		if (!row.id) {
			return res.status(500).json({ error: "CLI login request is incomplete" });
		}

		const claim = await pool.query(
			`UPDATE cli_login_requests
       SET consumed_at = NOW()
       WHERE code_hash = $1
         AND consumed_at IS NULL
       RETURNING user_id, token_name`,
			[hashToken(code)],
		);
		if (claim.rows.length === 0) {
			return res.status(410).json({ error: "CLI login request was already consumed" });
		}

		const created = await createUserApiToken(
			claim.rows[0].user_id,
			claim.rows[0].token_name || row.token_name,
		);

		res.json({
			status: "complete",
			token: created.token,
			user: publicUser(row),
		});
	} catch (error) {
		console.error("CLI login poll failed:", error);
		res.status(500).json({ error: "Internal server error" });
	}
});

router.get("/me", async (req, res) => {
	try {
		const user = await getSessionUser(req);
		if (!user) {
			return res.json({ authenticated: false, user: null });
		}
		res.json({ authenticated: true, user });
	} catch (error) {
		console.error("Current user lookup failed:", error);
		res.status(500).json({ error: "Internal server error" });
	}
});

router.post("/logout", async (req, res) => {
	try {
		await destroySession(req);
		clearCookie(res, req, SESSION_COOKIE);
		res.json({ success: true });
	} catch (error) {
		console.error("Logout failed:", error);
		res.status(500).json({ error: "Internal server error" });
	}
});

router.get("/tokens", requireSession, async (req, res) => {
	try {
		const result = await pool.query(
			`SELECT id, name, token_prefix, last_four, created_at, last_used_at, revoked_at
       FROM user_api_tokens
       WHERE user_id = $1 AND revoked_at IS NULL
       ORDER BY created_at DESC`,
			[req.user.id],
		);
		res.json({ tokens: result.rows.map(serializeApiToken) });
	} catch (error) {
		console.error("Token list failed:", error);
		res.status(500).json({ error: "Internal server error" });
	}
});

router.post("/tokens", requireSession, async (req, res) => {
	const name = getTokenName(req.body?.name);
	if (name.length > MAX_TOKEN_NAME_LENGTH) {
		return res.status(400).json({ error: `name must be <= ${MAX_TOKEN_NAME_LENGTH} characters` });
	}

	try {
		const created = await createUserApiToken(req.user.id, name);
		res.status(201).json({
			token: created.token,
			api_token: created.record,
		});
	} catch (error) {
		console.error("Token create failed:", error);
		res.status(500).json({ error: "Internal server error" });
	}
});

router.delete("/tokens/:id", requireSession, async (req, res) => {
	const tokenId = Number.parseInt(req.params.id, 10);
	if (!Number.isFinite(tokenId)) {
		return res.status(400).json({ error: "token id must be numeric" });
	}

	try {
		const result = await pool.query(
			`UPDATE user_api_tokens
       SET revoked_at = NOW()
       WHERE id = $1
         AND user_id = $2
         AND revoked_at IS NULL
       RETURNING id`,
			[tokenId, req.user.id],
		);
		if (result.rows.length === 0) {
			return res.status(404).json({ error: "Token not found" });
		}
		res.json({ success: true });
	} catch (error) {
		console.error("Token revoke failed:", error);
		res.status(500).json({ error: "Internal server error" });
	}
});

export default router;
