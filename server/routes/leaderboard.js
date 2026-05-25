import { Router } from "express";
import pool from "../db.js";

const router = Router();
const PERIOD_ALIASES = {
	all: "all",
	custom: "custom",
	day: "day",
	daily: "day",
	week: "week",
	weekly: "week",
	month: "month",
	monthly: "month",
	year: "year",
	yearly: "year",
};
const MAX_SEARCH_QUERY_LENGTH = 64;

function getQueryString(value) {
	if (Array.isArray(value)) {
		return typeof value[0] === "string" ? value[0].trim() : "";
	}
	return typeof value === "string" ? value.trim() : "";
}

function isValidIsoDate(value) {
	if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) {
		return false;
	}
	const parsed = new Date(`${value}T00:00:00.000Z`);
	return !Number.isNaN(parsed.getTime()) && parsed.toISOString().slice(0, 10) === value;
}

function escapeLikePattern(value) {
	return value.replace(/[\\%_]/g, "\\$&");
}

function toInt(value) {
	const parsed = Number.parseInt(value, 10);
	return Number.isFinite(parsed) ? parsed : 0;
}

function toFloat(value) {
	const parsed = Number.parseFloat(value);
	return Number.isFinite(parsed) ? parsed : 0;
}

// GET /api/leaderboard?limit=50&period=all&periodStart=2024-01-01&periodEnd=2024-12-31
router.get("/", async (req, res) => {
	const requestedLimit = Number.parseInt(getQueryString(req.query.limit), 10);
	const limit = Number.isFinite(requestedLimit) ? Math.min(Math.max(requestedLimit, 1), 200) : 50;
	const requestedPeriod = getQueryString(req.query.period).toLowerCase() || "all";
	const period = PERIOD_ALIASES[requestedPeriod];
	const periodStart = getQueryString(req.query.periodStart);
	const periodEnd = getQueryString(req.query.periodEnd);
	const searchQuery = getQueryString(req.query.q).slice(0, MAX_SEARCH_QUERY_LENGTH);

	if (!period) {
		return res.status(400).json({
			error: "Invalid period. Supported values: all, day|daily, week|weekly, month|monthly, year|yearly, custom",
		});
	}

	const filters = [];
	const params = [];
	let paramIndex = 1;

	if (period === "custom") {
		if (!periodStart || !periodEnd) {
			return res.status(400).json({ error: "periodStart and periodEnd are required for custom period" });
		}
		if (!isValidIsoDate(periodStart) || !isValidIsoDate(periodEnd)) {
			return res.status(400).json({ error: "periodStart and periodEnd must be YYYY-MM-DD" });
		}
		if (periodStart > periodEnd) {
			return res.status(400).json({ error: "periodStart must be less than or equal to periodEnd" });
		}

		filters.push(`s.date >= $${paramIndex} AND s.date <= $${paramIndex + 1}`);
		params.push(periodStart, periodEnd);
		paramIndex += 2;
	} else if (period === "year") {
		filters.push(`EXTRACT(YEAR FROM s.date) = EXTRACT(YEAR FROM CURRENT_DATE)`);
	} else if (period === "month") {
		filters.push(`s.date >= date_trunc('month', CURRENT_DATE)::date`);
	} else if (period === "week") {
		filters.push(`s.date >= date_trunc('week', CURRENT_DATE)::date`);
	} else if (period === "day") {
		filters.push(`s.date = CURRENT_DATE`);
	}
	// period === "all" → no filter

	if (searchQuery) {
		filters.push(`LOWER(u.username) LIKE $${paramIndex} ESCAPE '\\'`);
		params.push(`%${escapeLikePattern(searchQuery.toLowerCase())}%`);
		paramIndex += 1;
	}

	const whereClause = filters.length > 0 ? `WHERE ${filters.join(" AND ")}` : "";

	const query = `
    WITH effective_submissions AS (
      SELECT s.*
      FROM submissions s
      WHERE s.submission_source <> 0
         OR NOT EXISTS (
           SELECT 1
           FROM submissions replacement
           WHERE replacement.user_id = s.user_id
             AND replacement.date = s.date
             AND replacement.submission_source <> 0
         )
    ),
    filtered AS (
      SELECT
        u.id AS user_id,
        u.username,
        COALESCE(NULLIF(BTRIM(u.display_name), ''), u.username) AS display_name,
        s.date,
        s.total_tokens,
        s.total_cost,
        s.input_tokens,
        s.output_tokens,
        s.cache_read_tokens,
        s.cache_write_tokens,
        s.reasoning_tokens,
        s.models,
        s.submitted_at
      FROM effective_submissions s
      JOIN users u ON u.id = s.user_id
      ${whereClause}
    ),
    daily AS (
      SELECT
        user_id,
        username,
        display_name,
        date,
        SUM(total_tokens) AS total_tokens,
        SUM(total_cost) AS total_cost,
        SUM(input_tokens) AS input_tokens,
        SUM(output_tokens) AS output_tokens,
        SUM(cache_read_tokens) AS cache_read_tokens,
        SUM(cache_write_tokens) AS cache_write_tokens,
        SUM(reasoning_tokens) AS reasoning_tokens,
        MAX(submitted_at) AS submitted_at
      FROM filtered
      GROUP BY user_id, username, display_name, date
    ),
    aggregated AS (
      SELECT
        user_id,
        username,
        display_name,
        SUM(total_tokens)::bigint AS total_tokens,
        SUM(total_cost)::numeric(14,6) AS total_cost,
        SUM(input_tokens)::bigint AS input_tokens,
        SUM(output_tokens)::bigint AS output_tokens,
        SUM(cache_read_tokens)::bigint AS cache_read_tokens,
        SUM(cache_write_tokens)::bigint AS cache_write_tokens,
        SUM(reasoning_tokens)::bigint AS reasoning_tokens,
        MAX(submitted_at) AS last_updated,
        COUNT(DISTINCT date) AS active_days,
        COUNT(*)::int AS total_submissions
      FROM daily
      GROUP BY user_id, username, display_name
    ),
    model_totals AS (
      SELECT
        f.user_id,
        model_entry.key AS model_name,
        SUM(
          CASE
            WHEN (model_entry.value->>'tokens') ~ '^[0-9]+$'
              THEN (model_entry.value->>'tokens')::bigint
            ELSE 0
          END
        )::bigint AS model_tokens,
        SUM(
          CASE
            WHEN (model_entry.value->>'input') ~ '^[0-9]+$'
              THEN (model_entry.value->>'input')::bigint
            ELSE 0
          END
        )::bigint AS model_input_tokens,
        SUM(
          CASE
            WHEN (model_entry.value->>'output') ~ '^[0-9]+$'
              THEN (model_entry.value->>'output')::bigint
            ELSE 0
          END
        )::bigint AS model_output_tokens
      FROM filtered f
      LEFT JOIN LATERAL jsonb_each(
        CASE
          WHEN jsonb_typeof(f.models) = 'object' THEN f.models
          ELSE '{}'::jsonb
        END
      ) AS model_entry(key, value) ON TRUE
      GROUP BY f.user_id, model_entry.key
    ),
    ranked_models AS (
      SELECT
        user_id,
        model_name,
        model_tokens,
        model_input_tokens,
        model_output_tokens,
        ROW_NUMBER() OVER (
          PARTITION BY user_id
          ORDER BY model_tokens DESC NULLS LAST, model_name ASC
        ) AS rank_position
      FROM model_totals
      WHERE model_name IS NOT NULL
    )
    SELECT
      a.username,
      a.display_name,
      a.total_tokens,
      a.total_cost,
      a.input_tokens,
      a.output_tokens,
      a.cache_read_tokens,
      a.cache_write_tokens,
      a.reasoning_tokens,
      a.last_updated,
      a.active_days,
      a.total_submissions,
      COALESCE(m.model_name, '—') AS top_model,
      COALESCE(m.model_tokens, 0)::bigint AS top_model_tokens,
      COALESCE(m.model_input_tokens, 0)::bigint AS top_model_input_tokens,
      COALESCE(m.model_output_tokens, 0)::bigint AS top_model_output_tokens
    FROM aggregated a
    LEFT JOIN ranked_models m
      ON m.user_id = a.user_id AND m.rank_position = 1
    ORDER BY a.total_tokens DESC
    LIMIT $${paramIndex}
  `;

	params.push(limit);

	try {
		const result = await pool.query(query, params);

		const serializedLeaderboard = result.rows.map((row, index) => {
			const topModel = row.top_model;
			const topModelTokens = toInt(row.top_model_tokens);
			const topModelInputTokens = toInt(row.top_model_input_tokens);
			const topModelOutputTokens = toInt(row.top_model_output_tokens);
			return {
				rank: index + 1,
				username: row.username,
				display_name: row.display_name,
				top_model: topModel,
				top_model_tokens: topModelTokens,
				top_model_input_tokens: topModelInputTokens,
				top_model_output_tokens: topModelOutputTokens,
				model: {
					name: topModel === "—" ? null : topModel,
					tokens: topModelTokens,
					input_tokens: topModelInputTokens,
					output_tokens: topModelOutputTokens,
				},
				total_tokens: toInt(row.total_tokens),
				total_cost: toFloat(row.total_cost),
				input_tokens: toInt(row.input_tokens),
				output_tokens: toInt(row.output_tokens),
				cache_read_tokens: toInt(row.cache_read_tokens),
				cache_write_tokens: toInt(row.cache_write_tokens),
				reasoning_tokens: toInt(row.reasoning_tokens),
				active_days: toInt(row.active_days),
				total_submissions: toInt(row.total_submissions),
				last_updated: row.last_updated,
			};
		});

		res.json({
			period,
			query: searchQuery,
			entries: serializedLeaderboard.length,
			leaderboard: serializedLeaderboard,
		});
	} catch (err) {
		console.error("Leaderboard error:", err);
		res.status(500).json({ error: "Internal server error" });
	}
});

export default router;
