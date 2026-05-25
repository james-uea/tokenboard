import { Router } from "express";
import pool from "../db.js";

const router = Router();
const EFFECTIVE_SUBMISSIONS_WITH = `
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
        )`;

function toInt(value) {
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) ? parsed : 0;
}

function toFloat(value) {
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) ? parsed : 0;
}

// GET /api/badges
// Computes all competitive badges from live submission data.
// Each badge is held by exactly one user at any time.
router.get("/", async (req, res) => {
  try {
    const badges = [];

    // ── 👑 Token King: highest total_tokens ──
    {
      const result = await pool.query(`
        ${EFFECTIVE_SUBMISSIONS_WITH}
        SELECT u.username, u.display_name, SUM(s.total_tokens)::bigint AS value
        FROM effective_submissions s
        JOIN users u ON u.id = s.user_id
        GROUP BY u.id, u.username, u.display_name
        ORDER BY value DESC LIMIT 1
      `);
      badges.push(buildBadge("token-king", "👑", "Token King", "Highest total token usage", result.rows[0]));
    }

    // ── 💰 Big Spender: highest total_cost ──
    {
      const result = await pool.query(`
        ${EFFECTIVE_SUBMISSIONS_WITH}
        SELECT u.username, u.display_name, SUM(s.total_cost)::numeric(14,6) AS value
        FROM effective_submissions s
        JOIN users u ON u.id = s.user_id
        GROUP BY u.id, u.username, u.display_name
        ORDER BY value DESC LIMIT 1
      `);
      const row = result.rows[0];
      badges.push(buildBadge("big-spender", "💰", "Big Spender", "Highest cumulative cost", row, (v) => `$${Number(v).toLocaleString(undefined, { minimumFractionDigits: 2 })}`));
    }

	// ── ⚡ Speed Demon: biggest single-day tokens ──
	{
		const result = await pool.query(`
		  ${EFFECTIVE_SUBMISSIONS_WITH},
		  daily AS (
		    SELECT s.user_id, s.date, SUM(s.total_tokens) AS total_tokens
		    FROM effective_submissions s
		    GROUP BY s.user_id, s.date
		  )
		  SELECT u.username, u.display_name, d.total_tokens::bigint AS value, d.date
		  FROM daily d
		  JOIN users u ON u.id = d.user_id
		  ORDER BY d.total_tokens DESC LIMIT 1
		`);
		const row = result.rows[0];
		badges.push(buildBadge("speed-demon", "⚡", "Speed Demon", "Biggest single-day token spike", row));
		if (row) badges[badges.length - 1].detail = formatDate(row.date);
	}

    // ── 🦉 Wise Owl: highest reasoning ratio (min 100k reasoning) ──
    {
      const result = await pool.query(`
        ${EFFECTIVE_SUBMISSIONS_WITH}
        SELECT u.username, u.display_name,
               SUM(s.reasoning_tokens)::bigint AS reasoning,
               SUM(s.total_tokens)::bigint AS total,
               CASE WHEN SUM(s.total_tokens) > 0
                 THEN (SUM(s.reasoning_tokens)::numeric / SUM(s.total_tokens)::numeric * 100)
                 ELSE 0 END AS value
        FROM effective_submissions s
        JOIN users u ON u.id = s.user_id
        GROUP BY u.id, u.username, u.display_name
        HAVING SUM(s.reasoning_tokens) >= 100000
        ORDER BY value DESC LIMIT 1
      `);
      const row = result.rows[0];
      badges.push(buildBadge("wise-owl", "🦉", "Wise Owl", "Highest reasoning token ratio", row, (v) => `${Number(v).toFixed(1)}%`));
      if (row) badges[badges.length - 1].detail = `${Number(row.reasoning).toLocaleString()} reasoning / ${Number(row.total).toLocaleString()} total`;
    }

    // ── 🎯 Sniper: highest output:input ratio (min 100k output) ──
    {
      const result = await pool.query(`
        ${EFFECTIVE_SUBMISSIONS_WITH}
        SELECT u.username, u.display_name,
               SUM(s.output_tokens)::bigint AS output,
               SUM(s.input_tokens)::bigint AS input,
               CASE WHEN SUM(s.input_tokens) > 0
                 THEN (SUM(s.output_tokens)::numeric / SUM(s.input_tokens)::numeric)
                 ELSE 0 END AS value
        FROM effective_submissions s
        JOIN users u ON u.id = s.user_id
        GROUP BY u.id, u.username, u.display_name
        HAVING SUM(s.output_tokens) >= 100000
        ORDER BY value DESC LIMIT 1
      `);
      const row = result.rows[0];
      badges.push(buildBadge("sniper", "🎯", "Sniper", "Best output-to-input token ratio", row, (v) => `${Number(v).toFixed(1)}x`));
      if (row) badges[badges.length - 1].detail = `${Number(row.output).toLocaleString()} out / ${Number(row.input).toLocaleString()} in`;
    }

    // ── 📚 Polyglot: most distinct models ──
    {
      const result = await pool.query(`
        ${EFFECTIVE_SUBMISSIONS_WITH},
        model_counts AS (
          SELECT s.user_id,
                 COUNT(DISTINCT model_entry.key)::int AS value
          FROM effective_submissions s
          LEFT JOIN LATERAL jsonb_each(
            CASE WHEN jsonb_typeof(s.models) = 'object' THEN s.models ELSE '{}'::jsonb END
          ) model_entry(key, value) ON TRUE
          WHERE model_entry.key IS NOT NULL
          GROUP BY s.user_id
        )
        SELECT u.username, u.display_name, mc.value
        FROM model_counts mc
        JOIN users u ON u.id = mc.user_id
        ORDER BY mc.value DESC LIMIT 1
      `);
      badges.push(buildBadge("polyglot", "📚", "Polyglot", "Most distinct models used", result.rows[0]));
    }

    // ── 🔥 Hot Streak: longest consecutive active days ──
    {
      const client = await pool.connect();
      try {
        const result = await client.query(`
          ${EFFECTIVE_SUBMISSIONS_WITH},
          user_dates AS (
            SELECT user_id, date,
                   date - (ROW_NUMBER() OVER (PARTITION BY user_id ORDER BY date))::int AS streak_grp
            FROM (SELECT DISTINCT user_id, date FROM effective_submissions) sub_dates
          ),
          streaks AS (
            SELECT user_id, COUNT(*)::int AS streak_len
            FROM user_dates
            GROUP BY user_id, streak_grp
          ),
          best AS (
            SELECT user_id, MAX(streak_len) AS value
            FROM streaks
            GROUP BY user_id
          )
          SELECT u.username, u.display_name, b.value
          FROM best b
          JOIN users u ON u.id = b.user_id
          ORDER BY b.value DESC LIMIT 1
        `);
        badges.push(buildBadge("hot-streak", "🔥", "Hot Streak", "Longest consecutive active days", result.rows[0]));
      } finally {
        client.release();
      }
    }

    // ── 💾 Cache Cow: most cache read tokens ──
    {
      const result = await pool.query(`
        ${EFFECTIVE_SUBMISSIONS_WITH}
        SELECT u.username, u.display_name, SUM(s.cache_read_tokens)::bigint AS value
        FROM effective_submissions s
        JOIN users u ON u.id = s.user_id
        GROUP BY u.id, u.username, u.display_name
        HAVING SUM(s.cache_read_tokens) > 0
        ORDER BY value DESC LIMIT 1
      `);
      badges.push(buildBadge("cache-cow", "💾", "Cache Cow", "Most cache read tokens saved", result.rows[0]));
    }

    // ── 💎 Cache Purist: highest cache read share ──
    {
      const result = await pool.query(`
        ${EFFECTIVE_SUBMISSIONS_WITH}
        SELECT u.username, u.display_name,
               SUM(s.cache_read_tokens)::bigint AS cache_read,
               SUM(s.total_tokens)::bigint AS total,
               (SUM(s.cache_read_tokens)::numeric / SUM(s.total_tokens)::numeric * 100) AS value
        FROM effective_submissions s
        JOIN users u ON u.id = s.user_id
        GROUP BY u.id, u.username, u.display_name
        HAVING SUM(s.total_tokens) > 0 AND SUM(s.cache_read_tokens) > 0
        ORDER BY value DESC, cache_read DESC LIMIT 1
      `);
      const row = result.rows[0];
      badges.push(buildBadge("cache-purist", "💎", "Cache Purist", "Highest cache-read share", row, (v) => `${Number(v).toFixed(1)}%`));
      if (row) badges[badges.length - 1].detail = `${Number(row.cache_read).toLocaleString()} cache read / ${Number(row.total).toLocaleString()} total`;
    }

    // ── 🏃 Marathoner: most active days ──
    {
      const result = await pool.query(`
        ${EFFECTIVE_SUBMISSIONS_WITH}
        SELECT u.username, u.display_name, COUNT(DISTINCT s.date)::int AS value
        FROM effective_submissions s
        JOIN users u ON u.id = s.user_id
        GROUP BY u.id, u.username, u.display_name
        ORDER BY value DESC LIMIT 1
      `);
      badges.push(buildBadge("marathoner", "🏃", "Marathoner", "Most active days on record", result.rows[0]));
    }

	// ── 🌊 Rising Tide: biggest WoW token surge ──
	{
		const result = await pool.query(`
		  ${EFFECTIVE_SUBMISSIONS_WITH},
		  daily AS (
		    SELECT s.user_id, s.date, SUM(s.total_tokens) AS total_tokens
		    FROM effective_submissions s
		    GROUP BY s.user_id, s.date
		  ),
		  ranked AS (
		    SELECT *, ROW_NUMBER() OVER (PARTITION BY user_id ORDER BY date) AS rn
          FROM daily
        ),
        wow AS (
          SELECT a.user_id, a.date,
                 a.total_tokens - b.total_tokens AS delta
          FROM ranked a
          JOIN ranked b ON b.user_id = a.user_id AND b.rn = a.rn - 7
          WHERE a.total_tokens - b.total_tokens > 0
        ),
        best AS (
          SELECT DISTINCT ON (user_id) user_id, date, delta
          FROM wow
          ORDER BY user_id, delta DESC
        )
        SELECT u.username, u.display_name, b.delta::bigint AS value, b.date
        FROM best b
        JOIN users u ON u.id = b.user_id
        ORDER BY b.delta::bigint DESC LIMIT 1
      `);
      const row = result.rows[0];
      badges.push(buildBadge("rising-tide", "🌊", "Rising Tide", "Biggest week-over-week token surge", row));
      if (row) badges[badges.length - 1].detail = `Week of ${formatDate(row.date)}`;
    }

    res.json({ badges });
  } catch (err) {
    console.error("Badges error:", err);
    res.status(500).json({ error: "Internal server error" });
  }
});

function buildBadge(key, emoji, label, description, row, formatValue) {
  if (!row) {
    return { key, emoji, label, description, holder: null, value: null };
  }
  const raw = Number(row.value ?? 0);
  return {
    key,
    emoji,
    label,
    description,
    holder: {
      username: row.username,
      display_name: row.display_name || row.username,
    },
    value: formatValue ? formatValue(raw) : raw.toLocaleString(),
    raw_value: raw,
  };
}

function formatDate(value) {
  if (!value) return "";
  const d = value instanceof Date ? value : new Date(String(value));
  if (Number.isNaN(d.getTime())) return String(value);
  return d.toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

export default router;
