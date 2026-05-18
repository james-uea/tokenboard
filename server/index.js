import express from "express";
import helmet from "helmet";
import path from "path";
import { ipKeyGenerator, rateLimit } from "express-rate-limit";
import { fileURLToPath } from "url";
import leaderboardRouter from "./routes/leaderboard.js";
import submitRouter from "./routes/submit.js";
import badgesRouter from "./routes/badges.js";
import statsRouter from "./routes/stats.js";
import authRouter from "./routes/auth.js";
import pool from "./db.js";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const app = express();
const PORT = parseInt(process.env.PORT || "3001", 10);
const DEFAULT_RATE_LIMIT_WINDOW_MS = 15 * 60 * 1000;
const INSTALLER_SCRIPT_URL =
	"https://raw.githubusercontent.com/james-uea/tokenboard/main/scripts/install.sh";
const WINDOWS_INSTALLER_SCRIPT_URL =
	"https://raw.githubusercontent.com/james-uea/tokenboard/main/scripts/install.ps1";

function getIntegerEnv(name, defaultValue) {
	const parsed = Number.parseInt(process.env[name] || "", 10);
	return Number.isFinite(parsed) && parsed > 0 ? parsed : defaultValue;
}

function getBooleanEnv(name, defaultValue = false) {
	const value = process.env[name];
	if (typeof value === "undefined" || value === "") {
		return defaultValue;
	}
	return ["1", "true", "yes", "on"].includes(value.toLowerCase());
}

function configureTrustProxy() {
	const trustProxy = process.env.TRUST_PROXY;
	if (!trustProxy) {
		return;
	}
	if (trustProxy === "true") {
		app.set("trust proxy", true);
		return;
	}
	const numericValue = Number.parseInt(trustProxy, 10);
	app.set("trust proxy", Number.isFinite(numericValue) ? numericValue : trustProxy);
}

function getClientKey(req) {
	if (getBooleanEnv("TRUST_CLOUDFLARE_HEADERS", false)) {
		const headerValue = req.headers["cf-connecting-ip"];
		const cfConnectingIp = Array.isArray(headerValue) ? headerValue[0] : headerValue;
		if (cfConnectingIp) {
			return ipKeyGenerator(cfConnectingIp);
		}
	}
	return ipKeyGenerator(req.ip);
}

function createLimiter({ limitEnv, defaultLimit, message }) {
	return rateLimit({
		windowMs: getIntegerEnv("RATE_LIMIT_WINDOW_MS", DEFAULT_RATE_LIMIT_WINDOW_MS),
		limit: getIntegerEnv(limitEnv, defaultLimit),
		standardHeaders: "draft-8",
		legacyHeaders: false,
		keyGenerator: getClientKey,
		skip: () =>
			process.env.NODE_ENV === "test" ||
			process.env.RATE_LIMIT_ENABLED === "false",
		message: { error: message },
	});
}

function shouldRedirectToHttps(req) {
	if (!getBooleanEnv("FORCE_HTTPS", false)) {
		return false;
	}
	if (req.secure) {
		return false;
	}
	const forwardedProto = String(req.headers["x-forwarded-proto"] || "")
		.split(",")[0]
		.trim()
		.toLowerCase();
	return forwardedProto === "http";
}

function isSensitiveProbePath(req) {
	let pathname = "/";
	try {
		pathname = new URL(req.originalUrl || req.url || "/", "http://tokenboard.local").pathname;
	} catch {
		pathname = req.path || "/";
	}

	if (pathname.startsWith("/.")) {
		return true;
	}
	if (pathname.startsWith("/server/") || pathname.startsWith("/scripts/")) {
		return true;
	}
	return new Set([
		"/package.json",
		"/package-lock.json",
		"/docker-compose.yml",
		"/docker-compose.prod.yml",
		"/DEPLOYMENT.md",
		"/README.md",
		"/AGENTS.md",
	]).has(pathname);
}

function hashString(value) {
	let hash = 0;
	for (const character of String(value || "")) {
		hash = (hash * 31 + character.charCodeAt(0)) >>> 0;
	}
	return hash;
}

function escapeSvgText(value) {
	return String(value || "")
		.replaceAll("&", "&amp;")
		.replaceAll("<", "&lt;")
		.replaceAll(">", "&gt;")
		.replaceAll('"', "&quot;")
		.replaceAll("'", "&apos;");
}

function isLikelyGitHubUsername(value) {
	return /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$/.test(String(value || ""));
}

function initialsFromUsername(username) {
	const parts = String(username || "")
		.split(/[._-]+/g)
		.filter(Boolean);

	if (parts.length) {
		return parts
			.slice(0, 2)
			.map((part) => part.charAt(0))
			.join("")
			.toUpperCase();
	}

	return (String(username || "").slice(0, 2) || "TB").toUpperCase();
}

function buildFallbackAvatarSvg(username) {
	const hash = hashString(username);
	const hueStart = hash % 360;
	const hueEnd = (hueStart + 42) % 360;
	const label = escapeSvgText(initialsFromUsername(username));
	const ariaLabel = escapeSvgText(username);

	return `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 160 160" role="img" aria-label="${ariaLabel}">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="hsl(${hueStart} 90% 66%)" />
      <stop offset="100%" stop-color="hsl(${hueEnd} 90% 60%)" />
    </linearGradient>
  </defs>
  <rect width="160" height="160" rx="28" fill="url(#bg)" />
  <circle cx="80" cy="62" r="28" fill="rgba(255,255,255,0.14)" />
  <path d="M42 126c6-19 22-30 38-30s32 11 38 30" fill="rgba(255,255,255,0.16)" />
  <text x="80" y="95" text-anchor="middle" fill="#ffffff" font-family="Space Grotesk, Arial, sans-serif" font-size="54" font-weight="700" letter-spacing="-2">${label}</text>
	</svg>`;
}

configureTrustProxy();
app.disable("x-powered-by");

app.use((req, res, next) => {
	if (!shouldRedirectToHttps(req)) {
		next();
		return;
	}
	const host = req.headers.host;
	if (!host) {
		next();
		return;
	}
	res.redirect(308, `https://${host}${req.originalUrl || req.url || "/"}`);
});

app.use(helmet({
	contentSecurityPolicy: {
		directives: {
			defaultSrc: ["'self'"],
			baseUri: ["'self'"],
			connectSrc: ["'self'"],
			fontSrc: ["'self'", "data:"],
			formAction: ["'self'"],
			frameAncestors: ["'none'"],
			imgSrc: ["'self'", "data:"],
			objectSrc: ["'none'"],
			scriptSrc: ["'self'"],
			styleSrc: ["'self'", "'unsafe-inline'"],
			upgradeInsecureRequests: process.env.NODE_ENV === "production" ? [] : null,
		},
	},
	crossOriginResourcePolicy: { policy: "same-origin" },
}));

const apiLimiter = createLimiter({
	limitEnv: "API_RATE_LIMIT",
	defaultLimit: 600,
	message: "Too many API requests",
});
const authLimiter = createLimiter({
	limitEnv: "AUTH_RATE_LIMIT",
	defaultLimit: 60,
	message: "Too many authentication requests",
});
const cliPollLimiter = createLimiter({
	limitEnv: "CLI_POLL_RATE_LIMIT",
	defaultLimit: 180,
	message: "Too many CLI polling requests",
});
const submitLimiter = createLimiter({
	limitEnv: "SUBMIT_RATE_LIMIT",
	defaultLimit: 60,
	message: "Too many submission requests",
});
const githubProxyLimiter = createLimiter({
	limitEnv: "GITHUB_PROXY_RATE_LIMIT",
	defaultLimit: 120,
	message: "Too many GitHub proxy requests",
});

app.use("/api/auth/cli/poll", cliPollLimiter);
app.use("/api/auth", authLimiter);
app.use("/api/submit", submitLimiter);
app.use("/api/github-contributions", githubProxyLimiter);
app.use("/api/github-daily-detail", githubProxyLimiter);
app.use("/api", apiLimiter);

app.use(express.json({ limit: process.env.JSON_BODY_LIMIT || "256kb" }));

app.get("/healthz", (_req, res) => {
	res.setHeader("Cache-Control", "no-store");
	res.json({ status: "ok" });
});

app.get("/readyz", async (_req, res) => {
	res.setHeader("Cache-Control", "no-store");
	try {
		await pool.query("SELECT 1");
		res.json({ status: "ok", database: "ok" });
	} catch (error) {
		console.error("Readiness check failed:", error);
		res.status(503).json({ status: "error", database: "unavailable" });
	}
});

app.get("/install.sh", (_req, res) => {
	res.setHeader("Cache-Control", "public, max-age=300");
	res.redirect(302, INSTALLER_SCRIPT_URL);
});

app.get("/install.ps1", (_req, res) => {
	res.setHeader("Cache-Control", "public, max-age=300");
	res.redirect(302, WINDOWS_INSTALLER_SCRIPT_URL);
});

// API routes
app.use("/api/auth", authRouter);
app.use("/api/leaderboard", leaderboardRouter);
app.use("/api/submit", submitRouter);
app.use("/api/badges", badgesRouter);
app.use("/api/stats", statsRouter);

app.get("/api/avatar/:username", async (req, res) => {
	const username = String(req.params.username || "").trim();
	if (!username) {
		res.status(400).end();
		return;
	}

	try {
		if (!isLikelyGitHubUsername(username)) {
			throw new Error("Invalid GitHub username format");
		}
		const response = await fetch(`https://github.com/${encodeURIComponent(username)}.png?size=160`);
		if (!response.ok) {
			throw new Error(`Avatar request failed with ${response.status}`);
		}

		const buffer = Buffer.from(await response.arrayBuffer());
		res.setHeader("Content-Type", response.headers.get("content-type") || "image/png");
		res.setHeader("Cache-Control", "public, max-age=86400, stale-while-revalidate=604800");
		res.send(buffer);
	} catch (error) {
		console.error(`Avatar proxy failed for ${username}:`, error);
		res.setHeader("Content-Type", "image/svg+xml; charset=utf-8");
		res.setHeader("Cache-Control", "public, max-age=86400, stale-while-revalidate=604800");
		res.send(buildFallbackAvatarSvg(username));
	}
});

// Serve static frontend files

// GitHub contributions proxy
app.get("/api/github-contributions/:username", async (req, res) => {
  const username = String(req.params.username || "").trim();
  if (!username) {
    res.status(400).json({ error: "username required" });
    return;
  }

  try {
    const response = await fetch(
      `https://github-contributions-api.deno.dev/${encodeURIComponent(username)}.json`
    );
    if (!response.ok) {
      throw new Error(`Contributions API returned ${response.status}`);
    }
    const data = await response.json();
    // contributions is an array of weeks, each week is an array of days
    const all = Array.isArray(data.contributions)
      ? data.contributions.flat().filter((d) => d && d.date)
      : [];

    const levelMap = {
      NONE: 0,
      FIRST_QUARTILE: 1,
      SECOND_QUARTILE: 2,
      THIRD_QUARTILE: 3,
      FOURTH_QUARTILE: 4,
    };

    // Return only the last 63 days (~9 weeks, roughly 2 months)
    const last63 = all.slice(-63).map((d) => ({
      date: d.date,
      count: d.contributionCount ?? 0,
      level: levelMap[d.contributionLevel] ?? 0,
    }));

    res.setHeader("Cache-Control", "public, max-age=3600, stale-while-revalidate=86400");
    res.json({ contributions: last63 });
  } catch (error) {
    console.error(`GitHub contributions proxy failed for ${username}:`, error);
    // Return empty fallback — don't break the profile page
    res.setHeader("Cache-Control", "public, max-age=300");
    res.json({ contributions: [] });
  }
});

// GitHub daily repo detail — public events API, no token needed
// Groups PushEvents by repo for a given date, returns top 3 with commit counts.
const dailyDetailCache = new Map();

app.get("/api/github-daily-detail/:username/:date", async (req, res) => {
  const username = String(req.params.username || "").trim();
  const date = String(req.params.date || "").trim();
  if (!username || !date) {
    res.status(400).json({ error: "username and date required" });
    return;
  }

  const cacheKey = `${username}:${date}`;
  const cached = dailyDetailCache.get(cacheKey);
  if (cached && cached.expires > Date.now()) {
    res.setHeader("Cache-Control", "public, max-age=3600");
    return res.json(cached.data);
  }

  try {
    const evRes = await fetch(
      `https://api.github.com/users/${encodeURIComponent(username)}/events/public?per_page=100`,
      { headers: { "User-Agent": "tokenboard" } }
    );
    if (!evRes.ok) throw new Error(`GitHub Events API returned ${evRes.status}`);

    const events = await evRes.json();
    if (!Array.isArray(events)) throw new Error("Unexpected events response");

    const pushEvents = events.filter(
      (e) => e.type === "PushEvent" && (e.created_at || "").startsWith(date)
    );

    // Group by repo: collect push count and the before/head SHAs for stats
    const repoMap = {};
    for (const ev of pushEvents) {
      const name = ev.repo?.name || "";
      if (!name) continue;
      if (!repoMap[name]) repoMap[name] = { pushes: 0, additions: 0, deletions: 0, commits: 0 };
      repoMap[name].pushes += 1;
    }

    const ranked = Object.entries(repoMap)
      .sort((a, b) => b[1].pushes - a[1].pushes)
      .slice(0, 3);

    // Enrich with repo descriptions, additions/deletions via compare API
    const repos = await Promise.all(
      ranked.map(async ([name, info]) => {
        let description = "";
        let additions = 0;
        let deletions = 0;
        let commits = 0;

        // Fetch repo info
        try {
          const repoRes = await fetch(`https://api.github.com/repos/${name}`, {
            headers: { "User-Agent": "tokenboard" },
          });
          if (repoRes.ok) {
            const repo = await repoRes.json();
            description = repo.description || "";
          }
        } catch {}

        // Aggregate additions/deletions from all pushes to this repo on this day
        const dayPushes = pushEvents.filter((e) => e.repo?.name === name);
        for (const ev of dayPushes) {
          const before = ev.payload?.before;
          const head = ev.payload?.head;
          if (before && head && before !== head) {
            try {
              const cmpRes = await fetch(
                `https://api.github.com/repos/${name}/compare/${before}...${head}`,
                { headers: { "User-Agent": "tokenboard" } }
              );
              if (cmpRes.ok) {
                const cmp = await cmpRes.json();
                commits += cmp.total_commits || 0;
                additions += (cmp.files || []).reduce((s, f) => s + (f.additions || 0), 0);
                deletions += (cmp.files || []).reduce((s, f) => s + (f.deletions || 0), 0);
              }
            } catch {}
          }
        }

        return {
          repo: name,
          description,
          pushes: info.pushes,
          commits,
          additions,
          deletions,
        };
      })
    );

    const result = { date, repos };
    dailyDetailCache.set(cacheKey, { data: result, expires: Date.now() + 3600_000 });
    res.setHeader("Cache-Control", "public, max-age=3600");
    res.json(result);
	  } catch (error) {
	    console.error(`GitHub daily detail failed for ${username} ${date}:`, error);
	    res.setHeader("Cache-Control", "public, max-age=300");
	    res.json({ date, repos: [] });
	  }
	});

app.use("/api", (_req, res) => {
	res.status(404).json({ error: "Not found" });
});

app.use((req, res, next) => {
	if (!isSensitiveProbePath(req)) {
		next();
		return;
	}
	res.status(404).type("text/plain").send("Not found");
});

// Serve static frontend files
app.use(express.static(path.join(__dirname, "public")));

// Fall through to index.html for SPA-like behavior
app.get(/.*/, (_req, res) => {
	res.sendFile(path.join(__dirname, "public", "index.html"));
});

if (process.env.NODE_ENV !== "test") {
	app.listen(PORT, () => {
		console.log(`tokenboard server running on http://localhost:${PORT}`);
	});
}

export default app;
