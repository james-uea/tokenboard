import { Router } from "express";
import pool from "../db.js";

const router = Router();
const MAX_USERNAME_LENGTH = 64;
const DAY_MS = 24 * 60 * 60 * 1000;
const WEEK_MS = 7 * DAY_MS;
const DIFF_FIELDS = [
	"total_tokens",
	"total_cost",
	"input_tokens",
	"output_tokens",
	"cache_read_tokens",
	"cache_write_tokens",
	"reasoning_tokens",
];

function toInt(value) {
	const parsed = Number.parseInt(value, 10);
	return Number.isFinite(parsed) ? parsed : 0;
}

function toFloat(value) {
	const parsed = Number.parseFloat(value);
	return Number.isFinite(parsed) ? parsed : 0;
}

function dateKey(value) {
	if (!value) return null;
	if (value instanceof Date) return value.toISOString().slice(0, 10);

	const text = String(value);
	const isoDate = text.match(/^\d{4}-\d{2}-\d{2}/);
	if (isoDate) return isoDate[0];

	const parsed = new Date(text);
	return Number.isNaN(parsed.getTime()) ? null : parsed.toISOString().slice(0, 10);
}

function dateKeyToIso(key) {
	return `${key}T00:00:00.000Z`;
}

function addUtcDays(key, days) {
	const [year, month, day] = key.split("-").map(Number);
	return new Date(Date.UTC(year, month - 1, day + days)).toISOString().slice(0, 10);
}

function todayDateKey() {
	return new Date().toISOString().slice(0, 10);
}

function buildDenseTimeline(rows, throughDateKey = todayDateKey()) {
	const byDate = new Map();

	for (const entry of rows) {
		const key = dateKey(entry.date);
		if (!key) continue;
		byDate.set(key, entry);
	}

	if (byDate.size === 0) return [];

	const keys = [...byDate.keys()].sort();
	const startDate = keys[0];
	const lastDataDate = keys[keys.length - 1];
	const endDate = throughDateKey && throughDateKey > lastDataDate ? throughDateKey : lastDataDate;

	let runningTotalTokens = 0;
	const timeline = [];
	for (let key = startDate; key <= endDate; key = addUtcDays(key, 1)) {
		const entry = byDate.get(key);
		const dayTotalTokens = entry ? toInt(entry.total_tokens) : 0;
		runningTotalTokens += dayTotalTokens;
		timeline.push({
			date: dateKeyToIso(key),
			total_tokens: dayTotalTokens,
			total_cost: entry ? toFloat(entry.total_cost) : 0,
			input_tokens: entry ? toInt(entry.input_tokens) : 0,
			output_tokens: entry ? toInt(entry.output_tokens) : 0,
			cache_read_tokens: entry ? toInt(entry.cache_read_tokens) : 0,
			cache_write_tokens: entry ? toInt(entry.cache_write_tokens) : 0,
			reasoning_tokens: entry ? toInt(entry.reasoning_tokens) : 0,
			running_total_tokens: runningTotalTokens,
		});
	}

	return timeline;
}

function buildDiffs(timeline) {
	const dayOverDay = [];
	for (let i = 1; i < timeline.length; i++) {
		const curr = timeline[i];
		const prev = timeline[i - 1];
		const delta = {};
		for (const field of DIFF_FIELDS) {
			const c = Number(curr[field] ?? 0);
			const p = Number(prev[field] ?? 0);
			delta[`delta_${field}`] = c - p;
		}
		const prevTotal = Number(prev.total_tokens ?? 0);
		delta.percent_change = prevTotal > 0
			? Number(((delta.delta_total_tokens / prevTotal) * 100).toFixed(1))
			: 0;
		dayOverDay.push({ date: curr.date, prev_date: prev.date, ...delta });
	}

	const byDelta = [...dayOverDay].sort((a, b) => b.delta_total_tokens - a.delta_total_tokens);
	const largestIncreases = byDelta.filter((d) => d.delta_total_tokens > 0).slice(0, 5);
	const largestDecreases = byDelta
		.filter((d) => d.delta_total_tokens < 0)
		.slice(-5)
		.reverse();

	const weekOverWeek = [];
	for (let i = 0; i < timeline.length; i++) {
		const weekEnd = timeline[i];
		const weekEndDate = new Date(weekEnd.date).getTime();
		const weekStartMs = weekEndDate - WEEK_MS;
		let prevWeekEnd = null;
		for (let j = i - 1; j >= 0; j--) {
			const t = new Date(timeline[j].date).getTime();
			if (t <= weekStartMs) {
				prevWeekEnd = timeline[j];
				break;
			}
		}
		if (prevWeekEnd) {
			const delta = weekEnd.total_tokens - prevWeekEnd.total_tokens;
			const pct = prevWeekEnd.total_tokens > 0
				? Number(((delta / prevWeekEnd.total_tokens) * 100).toFixed(1))
				: 0;
			weekOverWeek.push({
				date: weekEnd.date,
				compare_date: prevWeekEnd.date,
				delta_total_tokens: delta,
				percent_change: pct,
			});
		}
	}

	return {
		day_over_day: dayOverDay,
		largest_increases: largestIncreases,
		largest_decreases: largestDecreases,
		week_over_week: weekOverWeek,
	};
}

// GET /api/stats/:username/diffs
router.get("/:username/diffs", async (req, res) => {
	const { username } = req.params;

	if (!username || typeof username !== "string" || username.trim().length === 0) {
		return res.status(400).json({ error: "username is required" });
	}
	if (username.trim().length > MAX_USERNAME_LENGTH) {
		return res.status(400).json({ error: `username must be <= ${MAX_USERNAME_LENGTH} characters` });
	}

	const normalizedUsername = username.trim();

	try {
		const timelineResult = await pool.query(
			`SELECT
		 s.date,
		 SUM(s.total_tokens)::bigint AS total_tokens,
		 SUM(s.total_cost)::numeric(14,6) AS total_cost,
		 SUM(s.input_tokens)::bigint AS input_tokens,
		 SUM(s.output_tokens)::bigint AS output_tokens,
		 SUM(s.cache_read_tokens)::bigint AS cache_read_tokens,
		 SUM(s.cache_write_tokens)::bigint AS cache_write_tokens,
		 SUM(s.reasoning_tokens)::bigint AS reasoning_tokens
        FROM submissions s
        JOIN users u ON u.id = s.user_id
        WHERE u.username = $1
		GROUP BY s.date
        ORDER BY s.date ASC`,
			[normalizedUsername],
		);

		if (timelineResult.rows.length === 0) {
			return res.json({ username: normalizedUsername, diffs: { day_over_day: [], largest_increases: [], largest_decreases: [], week_over_week: [] } });
		}

		const diffs = buildDiffs(buildDenseTimeline(timelineResult.rows));

		res.json({
			username: normalizedUsername,
			diffs,
		});
	} catch (err) {
		console.error("Diffs error:", err);
		res.status(500).json({ error: "Internal server error" });
	}
});

// GET /api/stats/:username
router.get("/:username", async (req, res) => {
	const { username } = req.params;

	if (!username || typeof username !== "string" || username.trim().length === 0) {
		return res.status(400).json({ error: "username is required" });
	}
	if (username.trim().length > MAX_USERNAME_LENGTH) {
		return res.status(400).json({ error: `username must be <= ${MAX_USERNAME_LENGTH} characters` });
	}

	const normalizedUsername = username.trim();

	try {
		const summaryResult = await pool.query(
			`SELECT
         u.username,
         COALESCE(NULLIF(BTRIM(u.display_name), ''), u.username) AS display_name,
         COUNT(s.id)::int AS total_submissions,
         COALESCE(SUM(s.total_tokens), 0)::bigint AS total_tokens,
         COALESCE(SUM(s.total_cost), 0)::numeric(14,6) AS total_cost,
        COALESCE(SUM(s.input_tokens), 0)::bigint AS input_tokens,
        COALESCE(SUM(s.output_tokens), 0)::bigint AS output_tokens,
        COALESCE(SUM(s.cache_read_tokens), 0)::bigint AS cache_read_tokens,
        COALESCE(SUM(s.cache_write_tokens), 0)::bigint AS cache_write_tokens,
        COALESCE(SUM(s.reasoning_tokens), 0)::bigint AS reasoning_tokens,
         MAX(s.submitted_at) AS last_updated,
         MIN(s.date) AS first_date,
         MAX(s.date) AS last_date,
         COUNT(DISTINCT s.date)::int AS active_days
       FROM users u
       LEFT JOIN submissions s ON s.user_id = u.id
       WHERE u.username = $1
       GROUP BY u.id, u.username, u.display_name`,
			[normalizedUsername]
		);

		if (summaryResult.rows.length === 0) {
			return res.status(404).json({ error: "User not found" });
		}

		const row = summaryResult.rows[0];
		const timelineResult = await pool.query(
			`SELECT
		 s.date,
		 SUM(s.total_tokens)::bigint AS total_tokens,
		 SUM(s.total_cost)::numeric(14,6) AS total_cost,
		 SUM(s.input_tokens)::bigint AS input_tokens,
		 SUM(s.output_tokens)::bigint AS output_tokens,
		 SUM(s.cache_read_tokens)::bigint AS cache_read_tokens,
		 SUM(s.cache_write_tokens)::bigint AS cache_write_tokens,
		 SUM(s.reasoning_tokens)::bigint AS reasoning_tokens
        FROM submissions s
        JOIN users u ON u.id = s.user_id
        WHERE u.username = $1
		GROUP BY s.date
        ORDER BY s.date ASC`,
			[normalizedUsername]
		);

		const modelsResult = await pool.query(
			`SELECT
         model_entry.key AS model_key,
         model_entry.value->>'provider' AS provider,
         model_entry.value->>'source' AS source,
         SUM(
           CASE
             WHEN (model_entry.value->>'tokens') ~ '^[0-9]+$'
               THEN (model_entry.value->>'tokens')::bigint
             ELSE 0
           END
         )::bigint AS tokens,
         SUM(
           CASE
             WHEN (model_entry.value->>'input') ~ '^[0-9]+$'
               THEN (model_entry.value->>'input')::bigint
             ELSE 0
           END
         )::bigint AS input_tokens,
         SUM(
           CASE
             WHEN (model_entry.value->>'output') ~ '^[0-9]+$'
               THEN (model_entry.value->>'output')::bigint
             ELSE 0
           END
         )::bigint AS output_tokens,
         SUM(
           CASE
             WHEN (model_entry.value->>'cache_read') ~ '^[0-9]+$'
               THEN (model_entry.value->>'cache_read')::bigint
             ELSE 0
           END
         )::bigint AS cache_read_tokens,
         SUM(
           CASE
             WHEN (model_entry.value->>'cache_write') ~ '^[0-9]+$'
               THEN (model_entry.value->>'cache_write')::bigint
             ELSE 0
           END
         )::bigint AS cache_write_tokens,
         SUM(
           CASE
             WHEN (model_entry.value->>'cost') ~ '^[0-9]+(\\.[0-9]+)?$'
               THEN (model_entry.value->>'cost')::numeric
             ELSE 0::numeric
           END
         )::numeric(14,6) AS total_cost
       FROM submissions s
       JOIN users u ON u.id = s.user_id
       LEFT JOIN LATERAL jsonb_each(
         CASE
           WHEN jsonb_typeof(s.models) = 'object' THEN s.models
           ELSE '{}'::jsonb
         END
       ) AS model_entry(key, value) ON TRUE
       WHERE u.username = $1
       GROUP BY model_entry.key, model_entry.value->>'provider', model_entry.value->>'source'
       HAVING model_entry.key IS NOT NULL
       ORDER BY tokens DESC, model_key ASC`,
			[normalizedUsername]
		);

		const clientsResult = await pool.query(
			`SELECT
         client_entry.key AS client_name,
         SUM(
           CASE
             WHEN (client_entry.value->>'tokens') ~ '^[0-9]+$'
               THEN (client_entry.value->>'tokens')::bigint
             ELSE 0
           END
         )::bigint AS tokens,
         SUM(
           CASE
             WHEN (client_entry.value->>'cost') ~ '^[0-9]+(\\.[0-9]+)?$'
               THEN (client_entry.value->>'cost')::numeric
             ELSE 0::numeric
           END
         )::numeric(14,6) AS total_cost
       FROM submissions s
       JOIN users u ON u.id = s.user_id
       LEFT JOIN LATERAL jsonb_each(
         CASE
           WHEN jsonb_typeof(s.clients) = 'object' THEN s.clients
           ELSE '{}'::jsonb
         END
       ) AS client_entry(key, value) ON TRUE
       WHERE u.username = $1
       GROUP BY client_entry.key
       HAVING client_entry.key IS NOT NULL
       ORDER BY tokens DESC, client_name ASC`,
			[normalizedUsername]
		);

		const timeline = buildDenseTimeline(timelineResult.rows);
		const diffs = buildDiffs(timeline);

		const totalTokens = toInt(row.total_tokens);
		const totalCost = toFloat(row.total_cost);
		const inputTokens = toInt(row.input_tokens);
		const outputTokens = toInt(row.output_tokens);
		const cacheReadTokens = toInt(row.cache_read_tokens);
		const cacheWriteTokens = toInt(row.cache_write_tokens);
		const reasoningTokens = toInt(row.reasoning_tokens);

		const modelBreakdown = modelsResult.rows.map((entry) => {
			const tokens = toInt(entry.tokens);
			const inputT = toInt(entry.input_tokens);
			const outputT = toInt(entry.output_tokens);
			const cacheRead = toInt(entry.cache_read_tokens);
			const cacheWrite = toInt(entry.cache_write_tokens);
			const cacheRate = (cacheRead + cacheWrite) > 0
				? Number(((cacheRead / (cacheRead + cacheWrite)) * 100).toFixed(1))
				: 0;
			// Parse model_key: "model_name|agent" → model_name and source
			const pipeIdx = entry.model_key.lastIndexOf("|");
			const modelName = pipeIdx >= 0 ? entry.model_key.substring(0, pipeIdx) : entry.model_key;
			const modelSource = pipeIdx >= 0 ? entry.model_key.substring(pipeIdx + 1) : (entry.source || "");
			return {
				model: modelName,
				provider: entry.provider || "",
				source: modelSource,
				tokens,
				input_tokens: inputT,
				output_tokens: outputT,
				cache_read_tokens: cacheRead,
				cache_write_tokens: cacheWrite,
				cache_rate: cacheRate,
				total_tokens: tokens,
				total_cost: toFloat(entry.total_cost),
				share_of_total: totalTokens > 0 ? Number((tokens / totalTokens).toFixed(4)) : 0,
			};
		});

		const clientBreakdown = clientsResult.rows.map((entry) => {
			const tokens = toInt(entry.tokens);
			const cost = toFloat(entry.total_cost);
			return {
				client: entry.client_name,
				tokens,
				total_cost: cost,
				token_share_of_total: totalTokens > 0 ? Number((tokens / totalTokens).toFixed(4)) : 0,
				cost_share_of_total: totalCost > 0 ? Number((cost / totalCost).toFixed(4)) : 0,
			};
		});

		const tokenBreakdown = {
			input_tokens: inputTokens,
			output_tokens: outputTokens,
			cache_read_tokens: cacheReadTokens,
			cache_write_tokens: cacheWriteTokens,
			reasoning_tokens: reasoningTokens,
		};
		const tokenBreakdownSeries = [
			{ category: "input_tokens", tokens: inputTokens },
			{ category: "output_tokens", tokens: outputTokens },
			{ category: "cache_read_tokens", tokens: cacheReadTokens },
			{ category: "cache_write_tokens", tokens: cacheWriteTokens },
			{ category: "reasoning_tokens", tokens: reasoningTokens },
		];

		res.json({
			username: row.username,
			display_name: row.display_name,
			total_submissions: toInt(row.total_submissions),
			total_tokens: totalTokens,
			total_cost: totalCost,
			input_tokens: inputTokens,
			output_tokens: outputTokens,
			cache_read_tokens: cacheReadTokens,
			cache_write_tokens: cacheWriteTokens,
			reasoning_tokens: reasoningTokens,
			active_days: toInt(row.active_days),
			first_date: row.first_date,
			last_date: row.last_date,
			last_updated: row.last_updated,
			token_breakdown: tokenBreakdown,
			token_breakdown_series: tokenBreakdownSeries,
			timeline,
			diffs,
			models: modelBreakdown,
			model_breakdown: modelBreakdown,
			clients: clientBreakdown,
			client_breakdown: clientBreakdown,
		});
	} catch (err) {
		console.error("Stats error:", err);
		res.status(500).json({ error: "Internal server error" });
	}
});

export default router;
