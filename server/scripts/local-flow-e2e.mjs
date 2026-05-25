import assert from "node:assert/strict";
import process from "node:process";

import pool from "../db.js";
import { createUserApiToken } from "../lib/auth.js";

const BASE_URL = "http://localhost:3001";
const API_KEY = process.env.API_KEY || "local-test-key";

function responseSummary(response) {
	return `${response.status} ${response.statusText}`;
}

async function requestJson(path, options = {}) {
	const response = await fetch(`${BASE_URL}${path}`, {
		headers: {
			"content-type": "application/json",
			...options.headers,
		},
		...options,
	});
	if (!response.ok) {
		const text = await response.text();
		throw new Error(`Unexpected HTTP response for ${options.method || "GET"} ${path}: ${responseSummary(response)} -> ${text}`);
	}
	const data = await response.json();
	return data;
}

function contribution(date, tokens, source) {
	return {
		date,
		total_tokens: tokens,
		total_cost: 0.5,
		input_tokens: Math.floor(tokens * 0.4),
		output_tokens: Math.floor(tokens * 0.6),
		cache_read_tokens: 0,
		cache_write_tokens: 0,
		reasoning_tokens: 0,
		models: {
			"gpt-5|tokenboard": {
				tokens,
				input: Math.floor(tokens * 0.4),
				output: Math.floor(tokens * 0.6),
				cost: 0.5,
				provider: "OpenAI",
				source,
			},
		},
		clients: {
			cli: {
				tokens,
				cost: 0.5,
			},
		},
	};
}

async function submitWithToken(token, payload) {
	return requestJson("/api/submit", {
		method: "POST",
		headers: {
			Authorization: `Bearer ${token}`,
			"content-type": "application/json",
		},
		body: JSON.stringify(payload),
	});
}

function timelineDateKey(rawDate) {
	return new Date(rawDate).toISOString().slice(0, 10);
}

async function resetDatabase() {
	await pool.query(`TRUNCATE users, user_api_tokens, submissions, auth_sessions, oauth_states, cli_login_requests RESTART IDENTITY CASCADE`);
}

const allDates = [
	"2026-05-16",
	"2026-05-17",
	"2026-05-18",
	"2026-05-19",
	"2026-05-20",
	"2026-05-21",
	"2026-05-22",
	"2026-05-23",
];

const expectedTotalsByDay = {
	[allDates[0]]: 35,
	[allDates[1]]: 20,
	[allDates[2]]: 30,
	[allDates[3]]: 25,
	[allDates[4]]: 40,
	[allDates[5]]: 15,
	[allDates[6]]: 12,
	[allDates[7]]: 190,
};

const expectedTotalTokens = Object.values(expectedTotalsByDay).reduce((sum, value) => sum + value, 0);
const riseDelta = expectedTotalsByDay[allDates[7]] - expectedTotalsByDay[allDates[0]];
const expectedSubmissionRows = 10;
const expectedTotalCost = expectedSubmissionRows * 0.5;

async function main() {
	console.log("Starting local full-flow verification");

	const health = await requestJson("/healthz");
	assert.deepEqual(health, { status: "ok" });
	console.log("✓ /healthz");

	const readiness = await requestJson("/readyz");
	assert.equal(readiness.status, "ok");
	assert.equal(readiness.database, "ok");
	console.log("✓ /readyz");

	await resetDatabase();
	console.log("✓ database reset");

	const userResult = await pool.query(
		`INSERT INTO users (username, display_name, github_id, github_login)
   VALUES ($1, $2, $3, $4)
   ON CONFLICT (username) DO UPDATE SET display_name = EXCLUDED.display_name
   RETURNING id`,
		["integration-multi-device", "Integration Multi-Device", "999000", "integration-multi-device"],
	);
	const userId = userResult.rows[0].id;

	const tokenA = await createUserApiToken(userId, "device-a");
	const tokenB = await createUserApiToken(userId, "device-b");
	console.log("✓ users and API tokens seeded");

	const contributionsByTokenA = [
		contribution(allDates[0], 50, "cli"),
		contribution(allDates[1], 20, "cli"),
		contribution(allDates[2], 30, "cli"),
		contribution(allDates[3], 25, "cli"),
		contribution(allDates[4], 40, "cli"),
		contribution(allDates[5], 15, "cli"),
		contribution(allDates[6], 12, "cli"),
		contribution(allDates[7], 140, "cli"),
	];
	const contributionsByTokenB = [
		contribution(allDates[0], 10, "cli"),
		contribution(allDates[7], 50, "cli"),
	];

	await submitWithToken(tokenA.token, {
		username: "integration-multi-device",
		display_name: "Integration Multi-Device",
		contributions: [contributionsByTokenA[0]],
	});

	await submitWithToken(tokenB.token, {
		username: "integration-multi-device",
		display_name: "Integration Multi-Device",
		contributions: [contributionsByTokenB[0]],
	});

	// Verify same token overwrites previous value rather than appending rows.
	await submitWithToken(tokenA.token, {
		username: "integration-multi-device",
		display_name: "Integration Multi-Device",
		contributions: [contribution(allDates[0], 25, "cli")],
	});

	for (const entry of contributionsByTokenA.slice(1)) {
		await submitWithToken(tokenA.token, {
			username: "integration-multi-device",
			display_name: "Integration Multi-Device",
			contributions: [entry],
		});
	}

	for (const entry of contributionsByTokenB.slice(1)) {
		await submitWithToken(tokenB.token, {
			username: "integration-multi-device",
			display_name: "Integration Multi-Device",
			contributions: [entry],
		});
	}

	console.log("✓ multi-source submit flow");

	await submitWithToken(API_KEY, {
		username: "integration-legacy",
		display_name: "Integration Legacy",
		contributions: [contribution(allDates[6], 77, "legacy")],
	});
	console.log("✓ legacy submit");

	const migratedUser = await pool.query(
		`INSERT INTO users (username, display_name, github_id, github_login)
   VALUES ($1, $2, $3, $4)
   RETURNING id`,
		["integration-migrated-token", "Integration Migrated Token", "999001", "integration-migrated-token"],
	);
	const migratedToken = await createUserApiToken(migratedUser.rows[0].id, "migrated-device");
	await pool.query(
		`INSERT INTO submissions
     (user_id, date, total_tokens, total_cost, input_tokens, output_tokens,
      cache_read_tokens, cache_write_tokens, reasoning_tokens, models, clients, submission_source)
   VALUES ($1, $2, 90, 0.5, 30, 30, 30, 0, 0, $3, $4, 0)`,
		[
			migratedUser.rows[0].id,
			allDates[5],
			JSON.stringify(contribution(allDates[5], 90, "cli").models),
			JSON.stringify(contribution(allDates[5], 90, "cli").clients),
		],
	);
	await submitWithToken(migratedToken.token, {
		username: "integration-migrated-token",
		display_name: "Integration Migrated Token",
		contributions: [contribution(allDates[5], 90, "cli")],
	});
	const migratedRows = await pool.query(
		`SELECT submission_source, total_tokens
     FROM submissions
     WHERE user_id = $1 AND date = $2
     ORDER BY submission_source`,
		[migratedUser.rows[0].id, allDates[5]],
	);
	assert.deepEqual(
		migratedRows.rows.map((row) => [row.submission_source, Number(row.total_tokens)]),
		[[migratedToken.record.id, 90]],
	);
	const migratedStats = await requestJson("/api/stats/integration-migrated-token");
	assert.equal(migratedStats.total_tokens, 90);
	assert.equal(migratedStats.models[0].tokens, 90);
	console.log("✓ migrated source-zero token resync does not double-count");

	const unauthorized = await fetch(`${BASE_URL}/api/submit`, {
		method: "POST",
		headers: {
			"content-type": "application/json",
			Authorization: "Bearer invalid-token-value",
		},
		body: JSON.stringify({
			username: "bad-user",
			display_name: "Bad User",
			contributions: [contribution(allDates[0], 5, "cli")],
		}),
	});
	assert.equal(unauthorized.status, 403);
	console.log("✓ invalid token rejected");

	const invalidDate = await fetch(`${BASE_URL}/api/submit`, {
		method: "POST",
		headers: {
			"content-type": "application/json",
			Authorization: `Bearer ${tokenA.token}`,
		},
		body: JSON.stringify({
			username: "integration-multi-device",
			display_name: "Integration Multi-Device",
			contributions: [
				{
					date: "2026-99-99",
					total_tokens: 1,
					total_cost: 0.1,
					input_tokens: 0,
					output_tokens: 1,
					cache_read_tokens: 0,
					cache_write_tokens: 0,
					reasoning_tokens: 0,
				},
			],
		}),
	});
	assert.equal(invalidDate.status, 400);
	console.log("✓ invalid date rejected");

	const stats = await requestJson(`/api/stats/integration-multi-device`);
	assert.equal(stats.total_tokens, expectedTotalTokens);
	assert.equal(stats.total_cost, expectedTotalCost);
	assert.equal(stats.active_days, 8);
	assert.equal(stats.total_submissions, 8);
	const dataRows = stats.timeline.filter((entry) => entry.has_data);
	assert.deepEqual(
		dataRows.map((entry) => entry.total_tokens),
		[35, 20, 30, 25, 40, 15, 12, 190],
	);
	const lastDataDate = dataRows.at(-1)?.date;
	assert.ok(lastDataDate, "Expected at least one data row");
	const lastDataDelta = stats.diffs.day_over_day.find((entry) => entry.date === lastDataDate);
	assert.equal(lastDataDelta?.delta_total_tokens, 178);
	console.log("✓ /api/stats combines multi-source rows");

	const diffs = await requestJson(`/api/stats/integration-multi-device/diffs`);
	const diffLastData = diffs.diffs.day_over_day.find((entry) => entry.date === lastDataDate);
	assert.equal(diffLastData?.delta_total_tokens, 178);
	console.log("✓ /api/stats/diffs combines multi-source rows");

	const leaderboard = await requestJson("/api/leaderboard?q=integration-multi");
	const row = leaderboard.leaderboard.find((entry) => entry.username === "integration-multi-device");
	assert.ok(row);
	assert.equal(row.total_tokens, expectedTotalTokens);
	assert.equal(row.active_days, 8);
	assert.equal(row.total_submissions, 8);
	console.log("✓ /api/leaderboard totals are daily-aggregated");

	const submissions = await pool.query(
		"SELECT date, submission_source, total_tokens FROM submissions WHERE user_id = $1 ORDER BY date, submission_source",
		[userId],
	);
	assert.equal(submissions.rows.length, expectedSubmissionRows);
	const firstDateKey = timelineDateKey(submissions.rows[0].date);
	const lastDateKey = timelineDateKey(submissions.rows[submissions.rows.length - 1].date);
	const day1Rows = submissions.rows.filter((row) => timelineDateKey(row.date) === firstDateKey);
	const day8Rows = submissions.rows.filter((row) => timelineDateKey(row.date) === lastDateKey);
	assert.equal(day1Rows.length, 2);
	assert.equal(day8Rows.length, 2);
	assert.equal(Number(day1Rows.find((row) => row.submission_source === tokenA.record.id)?.total_tokens), 25);
	assert.equal(Number(day1Rows.find((row) => row.submission_source === tokenB.record.id)?.total_tokens), 10);
	assert.equal(day8Rows.reduce((sum, row) => sum + Number(row.total_tokens), 0), 190);

	const legacyStats = await requestJson("/api/stats/integration-legacy");
	assert.equal(legacyStats.total_tokens, 77);
	assert.equal(legacyStats.total_submissions, 1);

	const badges = await requestJson("/api/badges");
	const speedDemon = badges.badges.find((entry) => entry.key === "speed-demon");
	assert.ok(speedDemon);
	assert.equal(speedDemon.raw_value, 190);
	const risingTide = badges.badges.find((entry) => entry.key === "rising-tide");
	assert.ok(risingTide);
	assert.equal(risingTide.raw_value, riseDelta);
	const expectedRiseDate = `Week of ${speedDemon.detail}`;
	assert.equal(risingTide.detail, expectedRiseDate);
	console.log("✓ /api/badges honors daily aggregation");

	console.log("✓ Full local flow verification complete");
}

main()
	.then(() => process.exit(0))
	.catch((error) => {
		console.error("Full local flow verification failed:", error.message);
		process.exit(1);
	})
	.finally(() => pool.end());
