import crypto from "crypto";
import pool from "../db.js";
import { getSessionSecret } from "./config.js";

export const SESSION_COOKIE = "tokenboard_session";
export const OAUTH_STATE_COOKIE = "tokenboard_oauth_state";

const SESSION_TTL_MS = 30 * 24 * 60 * 60 * 1000;
const API_TOKEN_PREFIX = "tbp_";

export function randomToken(bytes = 32) {
	return crypto.randomBytes(bytes).toString("base64url");
}

export function hashToken(value) {
	const secret = getSessionSecret();
	return crypto.createHmac("sha256", secret).update(String(value || "")).digest("hex");
}

export function getBearerToken(req) {
	const authHeader = req.headers.authorization;
	if (!authHeader || !authHeader.startsWith("Bearer ")) {
		return "";
	}
	return authHeader.slice(7).trim();
}

export function parseCookies(header = "") {
	const cookies = {};
	for (const part of String(header || "").split(";")) {
		const separator = part.indexOf("=");
		if (separator === -1) {
			continue;
		}
		const key = part.slice(0, separator).trim();
		const value = part.slice(separator + 1).trim();
		if (!key) {
			continue;
		}
		try {
			cookies[key] = decodeURIComponent(value);
		} catch {
			cookies[key] = value;
		}
	}
	return cookies;
}

export function getCookie(req, name) {
	return parseCookies(req.headers.cookie || "")[name] || "";
}

export function safeReturnTo(value) {
	const raw = Array.isArray(value) ? value[0] : value;
	const candidate = typeof raw === "string" && raw.trim() ? raw.trim() : "/";
	if (!candidate.startsWith("/") || candidate.startsWith("//")) {
		return "/";
	}
	if (candidate.includes("\r") || candidate.includes("\n")) {
		return "/";
	}
	return candidate;
}

export function getBaseUrl(req) {
	if (process.env.APP_BASE_URL) {
		return process.env.APP_BASE_URL.replace(/\/+$/, "");
	}
	const protocol = req.headers["x-forwarded-proto"] || req.protocol || "http";
	return `${protocol}://${req.get("host")}`;
}

function shouldUseSecureCookie(req) {
	return process.env.NODE_ENV === "production" || getBaseUrl(req).startsWith("https://");
}

function serializeCookie(name, value, req, options = {}) {
	const parts = [
		`${name}=${encodeURIComponent(value)}`,
		`Path=${options.path || "/"}`,
		`SameSite=${options.sameSite || "Lax"}`,
	];

	if (options.httpOnly !== false) {
		parts.push("HttpOnly");
	}
	if (typeof options.maxAge === "number") {
		parts.push(`Max-Age=${Math.max(0, Math.floor(options.maxAge))}`);
	}
	if (options.expires) {
		parts.push(`Expires=${options.expires.toUTCString()}`);
	}
	if (options.secure ?? shouldUseSecureCookie(req)) {
		parts.push("Secure");
	}

	return parts.join("; ");
}

export function setCookie(res, req, name, value, options = {}) {
	res.append("Set-Cookie", serializeCookie(name, value, req, options));
}

export function clearCookie(res, req, name) {
	setCookie(res, req, name, "", {
		maxAge: 0,
		expires: new Date(0),
	});
}

export function publicUser(row) {
	if (!row) {
		return null;
	}
	return {
		id: row.id,
		username: row.username,
		display_name: row.display_name || row.username,
		github_id: row.github_id || null,
		github_login: row.github_login || row.username,
		avatar_url: row.avatar_url || null,
		profile_url: row.profile_url || null,
		github_verified_at: row.github_verified_at || null,
	};
}

export async function getSessionUser(req) {
	const sessionToken = getCookie(req, SESSION_COOKIE);
	if (!sessionToken) {
		return null;
	}

	const sessionHash = hashToken(sessionToken);
	const result = await pool.query(
		`SELECT
       u.id,
       u.username,
       u.display_name,
       u.github_id,
       u.github_login,
       u.avatar_url,
       u.profile_url,
       u.github_verified_at
     FROM auth_sessions s
     JOIN users u ON u.id = s.user_id
     WHERE s.session_hash = $1
       AND s.expires_at > NOW()`,
		[sessionHash],
	);

	if (result.rows.length === 0) {
		return null;
	}

	await pool.query(
		"UPDATE auth_sessions SET last_seen_at = NOW() WHERE session_hash = $1",
		[sessionHash],
	);
	return publicUser(result.rows[0]);
}

export async function createSession(userId, req) {
	const token = randomToken();
	const sessionHash = hashToken(token);
	const expiresAt = new Date(Date.now() + SESSION_TTL_MS);
	await pool.query(
		`INSERT INTO auth_sessions
       (session_hash, user_id, expires_at, user_agent, ip_address)
     VALUES ($1, $2, $3, $4, $5)`,
		[
			sessionHash,
			userId,
			expiresAt,
			req.headers["user-agent"] || "",
			req.ip || req.socket?.remoteAddress || "",
		],
	);
	return { token, maxAgeSeconds: Math.floor(SESSION_TTL_MS / 1000) };
}

export async function destroySession(req) {
	const sessionToken = getCookie(req, SESSION_COOKIE);
	if (!sessionToken) {
		return;
	}
	await pool.query("DELETE FROM auth_sessions WHERE session_hash = $1", [
		hashToken(sessionToken),
	]);
}

export async function authenticateApiToken(rawToken) {
	if (!rawToken) {
		return null;
	}

	const tokenHash = hashToken(rawToken);
	const result = await pool.query(
		`SELECT
       t.id AS token_id,
       u.id,
       u.username,
       u.display_name,
       u.github_id,
       u.github_login,
       u.avatar_url,
       u.profile_url,
       u.github_verified_at
     FROM user_api_tokens t
     JOIN users u ON u.id = t.user_id
     WHERE t.token_hash = $1
       AND t.revoked_at IS NULL`,
		[tokenHash],
	);

	if (result.rows.length === 0) {
		return null;
	}

	const row = result.rows[0];
	await pool.query("UPDATE user_api_tokens SET last_used_at = NOW() WHERE id = $1", [
		row.token_id,
	]);

	return {
		token_id: row.token_id,
		user: publicUser(row),
	};
}

export async function createUserApiToken(userId, name) {
	const rawToken = `${API_TOKEN_PREFIX}${randomToken(32)}`;
	const tokenHash = hashToken(rawToken);
	const tokenPrefix = rawToken.slice(0, 12);
	const lastFour = rawToken.slice(-4);
	const result = await pool.query(
		`INSERT INTO user_api_tokens (user_id, name, token_hash, token_prefix, last_four)
     VALUES ($1, $2, $3, $4, $5)
     RETURNING id, name, token_prefix, last_four, created_at, last_used_at, revoked_at`,
		[userId, name, tokenHash, tokenPrefix, lastFour],
	);

	return {
		token: rawToken,
		record: serializeApiToken(result.rows[0]),
	};
}

export function serializeApiToken(row) {
	return {
		id: row.id,
		name: row.name,
		token_prefix: row.token_prefix,
		last_four: row.last_four,
		created_at: row.created_at,
		last_used_at: row.last_used_at,
		revoked_at: row.revoked_at,
	};
}
