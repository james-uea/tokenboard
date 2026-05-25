import { Router } from "express";
import pool from "../db.js";
import authMiddleware from "../middleware/auth.js";
import { normalizeProviderName } from "../lib/providers.js";

const router = Router();
const MAX_USERNAME_LENGTH = 64;
const MAX_DISPLAY_NAME_LENGTH = 120;
const MAX_TOTAL_COST = 99_999_999.999999;

function isRecord(value) {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}

function isValidIsoDate(value) {
	if (typeof value !== "string") {
		return false;
	}
	const trimmed = value.trim();
	if (!/^\d{4}-\d{2}-\d{2}$/.test(trimmed)) {
		return false;
	}
	const parsed = new Date(`${trimmed}T00:00:00.000Z`);
	return !Number.isNaN(parsed.getTime()) && parsed.toISOString().slice(0, 10) === trimmed;
}

function parseNonNegativeInteger(value, fieldName, defaultValue = 0) {
	if (typeof value === "undefined" || value === null) {
		return defaultValue;
	}
	if (typeof value === "string" && value.trim().length === 0) {
		throw new Error(`${fieldName} must be a non-negative integer`);
	}

	const parsed = typeof value === "number" ? value : Number(value);
	if (!Number.isFinite(parsed) || !Number.isSafeInteger(parsed) || parsed < 0) {
		throw new Error(`${fieldName} must be a non-negative integer`);
	}
	return parsed;
}

function parseNonNegativeNumber(value, fieldName, defaultValue = 0, max = Number.POSITIVE_INFINITY) {
	if (typeof value === "undefined" || value === null) {
		return defaultValue;
	}
	if (typeof value === "string" && value.trim().length === 0) {
		throw new Error(`${fieldName} must be a non-negative number`);
	}

	const parsed = typeof value === "number" ? value : Number(value);
	if (!Number.isFinite(parsed) || parsed < 0 || parsed > max) {
		throw new Error(`${fieldName} must be a non-negative number`);
	}
	return parsed;
}

function roundCurrency(value) {
	return Math.round(value * 1_000_000) / 1_000_000;
}

function normalizeModelBreakdown(value, fieldName) {
	if (typeof value === "undefined" || value === null) {
		return {};
	}
	if (!isRecord(value)) {
		throw new Error(`${fieldName} must be an object`);
	}

	const normalized = {};
	for (const [rawModel, rawEntry] of Object.entries(value)) {
		const model = String(rawModel || "").trim();
		if (!model) {
			continue;
		}
		if (!isRecord(rawEntry)) {
			throw new Error(`${fieldName}.${model} must be an object`);
		}

		normalized[model] = {
			tokens: parseNonNegativeInteger(rawEntry.tokens, `${fieldName}.${model}.tokens`),
			input: parseNonNegativeInteger(rawEntry.input, `${fieldName}.${model}.input`),
			output: parseNonNegativeInteger(rawEntry.output, `${fieldName}.${model}.output`),
			cache_read: parseNonNegativeInteger(rawEntry.cache_read, `${fieldName}.${model}.cache_read`, 0),
			cache_write: parseNonNegativeInteger(rawEntry.cache_write, `${fieldName}.${model}.cache_write`, 0),
			cost: roundCurrency(
				parseNonNegativeNumber(
					rawEntry.cost,
					`${fieldName}.${model}.cost`,
					0,
					MAX_TOTAL_COST
				)
			),
			provider: normalizeProviderName(rawEntry.provider),
			source: typeof rawEntry.source === "string" ? rawEntry.source.trim() : "",
		};
	}

	return normalized;
}

function normalizeClientBreakdown(value, fieldName) {
	if (typeof value === "undefined" || value === null) {
		return {};
	}
	if (!isRecord(value)) {
		throw new Error(`${fieldName} must be an object`);
	}

	const normalized = {};
	for (const [rawClient, rawEntry] of Object.entries(value)) {
		const clientName = String(rawClient || "").trim();
		if (!clientName) {
			continue;
		}
		if (!isRecord(rawEntry)) {
			throw new Error(`${fieldName}.${clientName} must be an object`);
		}

		normalized[clientName] = {
			tokens: parseNonNegativeInteger(rawEntry.tokens, `${fieldName}.${clientName}.tokens`),
			cost: roundCurrency(
				parseNonNegativeNumber(
					rawEntry.cost,
					`${fieldName}.${clientName}.cost`,
					0,
					MAX_TOTAL_COST
				)
			),
		};
	}

	return normalized;
}

router.post("/", authMiddleware, async (req, res) => {
	if (!isRecord(req.body)) {
		return res.status(400).json({ error: "Request body must be a JSON object" });
	}

	const { username, display_name, contributions } = req.body;
	const authenticatedUser =
		req.auth?.type === "user_api_token" && req.auth.user ? req.auth.user : null;

	if (!authenticatedUser && (!username || typeof username !== "string" || username.trim().length === 0)) {
		return res.status(400).json({ error: "username is required" });
	}
	if (!authenticatedUser && username.trim().length > MAX_USERNAME_LENGTH) {
		return res.status(400).json({ error: `username must be <= ${MAX_USERNAME_LENGTH} characters` });
	}
	if (typeof display_name !== "undefined" && typeof display_name !== "string") {
		return res.status(400).json({ error: "display_name must be a string when provided" });
	}

	if (!Array.isArray(contributions) || contributions.length === 0) {
		return res.status(400).json({ error: "contributions array is required" });
	}

	const normalizedUsername = authenticatedUser
		? String(authenticatedUser.github_login || authenticatedUser.username || "").trim()
		: username.trim();
	const normalizedDisplayNameRaw = authenticatedUser
		? String(authenticatedUser.display_name || normalizedUsername).trim()
		: typeof display_name === "string" && display_name.trim().length > 0
			? display_name.trim()
			: normalizedUsername;
	if (!normalizedUsername) {
		return res.status(400).json({ error: "authenticated user is missing a GitHub login" });
	}
	if (normalizedUsername.length > MAX_USERNAME_LENGTH) {
		return res.status(400).json({ error: `username must be <= ${MAX_USERNAME_LENGTH} characters` });
	}
	if (normalizedDisplayNameRaw.length > MAX_DISPLAY_NAME_LENGTH) {
		return res
			.status(400)
			.json({ error: `display_name must be <= ${MAX_DISPLAY_NAME_LENGTH} characters` });
	}
	const normalizedDisplayName = normalizedDisplayNameRaw;
	const submissionSourceId = req.auth?.type === "user_api_token" ? req.auth.token_id : 0;

	if (!normalizedDisplayName) {
		return res.status(400).json({ error: "display_name cannot be empty" });
	}

	const normalizedContributions = [];
	for (const [index, contrib] of contributions.entries()) {
		if (!isRecord(contrib)) {
			return res.status(400).json({ error: `contributions[${index}] must be an object` });
		}

		const date = typeof contrib.date === "string" ? contrib.date.trim() : "";
		if (!isValidIsoDate(date)) {
			return res.status(400).json({
				error: `contributions[${index}].date must be a valid YYYY-MM-DD string`,
			});
		}

		try {
			normalizedContributions.push({
				date,
				total_tokens: parseNonNegativeInteger(
					contrib.total_tokens,
					`contributions[${index}].total_tokens`
				),
				total_cost: roundCurrency(
					parseNonNegativeNumber(
						contrib.total_cost,
						`contributions[${index}].total_cost`,
						0,
						MAX_TOTAL_COST
					)
				),
				input_tokens: parseNonNegativeInteger(
					contrib.input_tokens,
					`contributions[${index}].input_tokens`
				),
				output_tokens: parseNonNegativeInteger(
					contrib.output_tokens,
					`contributions[${index}].output_tokens`
				),
				cache_read_tokens: parseNonNegativeInteger(
					contrib.cache_read_tokens,
					`contributions[${index}].cache_read_tokens`
				),
				cache_write_tokens: parseNonNegativeInteger(
					contrib.cache_write_tokens,
					`contributions[${index}].cache_write_tokens`
				),
				reasoning_tokens: parseNonNegativeInteger(
					contrib.reasoning_tokens,
					`contributions[${index}].reasoning_tokens`
				),
				models: normalizeModelBreakdown(contrib.models, `contributions[${index}].models`),
				clients: normalizeClientBreakdown(contrib.clients, `contributions[${index}].clients`),
			});
		} catch (validationError) {
			return res.status(400).json({ error: validationError.message });
		}
	}

	const client = await pool.connect();
	try {
		await client.query("BEGIN");

		let userId;
		if (authenticatedUser) {
			const userResult = await client.query(
				`UPDATE users
         SET username = $2,
             display_name = $3,
             github_login = COALESCE(github_login, $2)
         WHERE id = $1
         RETURNING id`,
				[authenticatedUser.id, normalizedUsername, normalizedDisplayName]
			);
			if (userResult.rows.length === 0) {
				throw new Error("Authenticated user no longer exists");
			}
			userId = userResult.rows[0].id;
		} else {
			// Legacy API-key submissions keep the historical username payload behavior.
			const userResult = await client.query(
				`INSERT INTO users (username, display_name) VALUES ($1, $2)
         ON CONFLICT (username) DO UPDATE SET
           username = EXCLUDED.username,
           display_name = EXCLUDED.display_name
         RETURNING id`,
				[normalizedUsername, normalizedDisplayName]
			);
			userId = userResult.rows[0].id;
		}

		// Upsert each daily contribution
		for (const contrib of normalizedContributions) {
			const {
				date,
				total_tokens,
				total_cost,
				input_tokens,
				output_tokens,
				cache_read_tokens,
				cache_write_tokens,
				reasoning_tokens,
				models,
				clients,
			} = contrib;

			await client.query(
				`INSERT INTO submissions
           (user_id, date, total_tokens, total_cost,
            input_tokens, output_tokens,
            cache_read_tokens, cache_write_tokens, reasoning_tokens,
            submission_source, models, clients)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
         ON CONFLICT (user_id, date, submission_source) DO UPDATE SET
           total_tokens = EXCLUDED.total_tokens,
           total_cost = EXCLUDED.total_cost,
           input_tokens = EXCLUDED.input_tokens,
           output_tokens = EXCLUDED.output_tokens,
           cache_read_tokens = EXCLUDED.cache_read_tokens,
           cache_write_tokens = EXCLUDED.cache_write_tokens,
           reasoning_tokens = EXCLUDED.reasoning_tokens,
           models = EXCLUDED.models,
           clients = EXCLUDED.clients,
           submitted_at = NOW()`,
					[
						userId,
						date,
						total_tokens,
						total_cost,
						input_tokens,
						output_tokens,
						cache_read_tokens,
						cache_write_tokens,
						reasoning_tokens,
						submissionSourceId,
						JSON.stringify(models),
						JSON.stringify(clients),
					]
				);
		}

		await client.query("COMMIT");

		res.json({
			success: true,
			username: normalizedUsername,
			display_name: normalizedDisplayName,
			contributions_updated: normalizedContributions.length,
		});
	} catch (err) {
		await client.query("ROLLBACK");
		console.error("Submit error:", err);
		res.status(500).json({ error: "Internal server error" });
	} finally {
		client.release();
	}
});

export default router;
