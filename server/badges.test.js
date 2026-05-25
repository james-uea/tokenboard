import request from "supertest";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mockDb = vi.hoisted(() => ({
	query: vi.fn(),
	connect: vi.fn(),
}));

vi.mock("./db.js", () => ({
	default: mockDb,
}));

process.env.NODE_ENV = "test";
process.env.API_KEY = "legacy-key";
process.env.GITHUB_CLIENT_ID = "github-client-id";
process.env.GITHUB_CLIENT_SECRET = "github-client-secret";
process.env.APP_BASE_URL = "http://localhost:3001";
process.env.SESSION_SECRET = "test-session-secret";

const { default: app } = await import("./index.js");

beforeEach(() => {
	mockDb.query.mockReset();
	mockDb.connect.mockReset().mockResolvedValue({
		query: vi.fn().mockResolvedValue({ rows: [] }),
		release: vi.fn(),
	});
	global.fetch = vi.fn();
});

describe("badges API", () => {
	it("awards Speed Demon based on daily aggregated totals, not per-source rows", async () => {
		const speedDate = new Date("2026-05-20T00:00:00.000Z").toLocaleDateString("en-US", {
			month: "short",
			day: "numeric",
		});
		mockDb.query.mockImplementation((sql) => {
			if (sql.includes("d.total_tokens::bigint AS value, d.date")) {
				return Promise.resolve({
					rows: [
						{
							username: "octocat",
							display_name: "The Octocat",
							value: "200",
							date: "2026-05-20",
						},
					],
				});
			}
			return Promise.resolve({ rows: [] });
		});

		const response = await request(app).get("/api/badges").expect(200);
		const badge = response.body.badges.find((entry) => entry.key === "speed-demon");
		const speedQuery = mockDb.query.mock.calls.find(([query]) =>
			query.includes("d.total_tokens::bigint AS value, d.date")
		)?.[0];

		expect(speedQuery).toContain("WITH daily AS (");
		expect(speedQuery).toContain("GROUP BY s.user_id, s.date");
		expect(speedQuery).toContain("ORDER BY d.total_tokens DESC LIMIT 1");
		expect(badge).toMatchObject({
			key: "speed-demon",
			holder: { username: "octocat", display_name: "The Octocat" },
			raw_value: 200,
			value: "200",
			detail: speedDate,
		});
	});

	it("awards Rising Tide based on daily aggregates for week-over-week change", async () => {
		const riseDate = new Date("2026-05-14T00:00:00.000Z").toLocaleDateString("en-US", {
			month: "short",
			day: "numeric",
		});
		mockDb.query.mockImplementation((sql) => {
			if (sql.includes("JOIN ranked b ON b.user_id = a.user_id AND b.rn = a.rn - 7")) {
				return Promise.resolve({
					rows: [
						{
							username: "octocat",
							display_name: "The Octocat",
							value: "120",
							date: "2026-05-14",
						},
					],
				});
			}
			return Promise.resolve({ rows: [] });
		});

		const response = await request(app).get("/api/badges").expect(200);
		const badge = response.body.badges.find((entry) => entry.key === "rising-tide");
		const risingQuery = mockDb.query.mock.calls.find(([query]) =>
			query.includes("JOIN ranked b ON b.user_id = a.user_id AND b.rn = a.rn - 7")
		)?.[0];

		expect(risingQuery).toContain("WITH daily AS (");
		expect(risingQuery).toContain("SUM(s.total_tokens) AS total_tokens");
		expect(risingQuery).toContain("GROUP BY s.user_id, s.date");
		expect(risingQuery).toContain("ROW_NUMBER() OVER (PARTITION BY user_id ORDER BY date)");
		expect(badge).toMatchObject({
			key: "rising-tide",
			holder: { username: "octocat", display_name: "The Octocat" },
			raw_value: 120,
			value: "120",
			detail: `Week of ${riseDate}`,
		});
	});

	it("awards Cache Purist by highest cache-read share", async () => {
		mockDb.query.mockImplementation((sql) => {
			if (sql.includes("SUM(s.cache_read_tokens)::bigint AS cache_read")) {
				return Promise.resolve({
					rows: [
						{
							username: "reeceus",
							display_name: "reeceus",
							cache_read: "415295232",
							total: "436715850",
							value: "95.102",
						},
					],
				});
			}
			return Promise.resolve({ rows: [] });
		});
		mockDb.connect.mockResolvedValue({
			query: vi.fn().mockResolvedValue({ rows: [] }),
			release: vi.fn(),
		});

		const response = await request(app).get("/api/badges").expect(200);
		const badge = response.body.badges.find((entry) => entry.key === "cache-purist");
		const puristQuery = mockDb.query.mock.calls.find(([sql]) => sql.includes("AS cache_read"))?.[0];

		expect(puristQuery).toContain("SUM(s.cache_read_tokens)::numeric / SUM(s.total_tokens)::numeric * 100");
		expect(puristQuery).toContain("ORDER BY value DESC, cache_read DESC LIMIT 1");
		expect(badge).toMatchObject({
			key: "cache-purist",
			emoji: "💎",
			label: "Cache Purist",
			description: "Highest cache-read share",
			holder: {
				username: "reeceus",
				display_name: "reeceus",
			},
			value: "95.1%",
			raw_value: 95.102,
			detail: "415,295,232 cache read / 436,715,850 total",
		});
	});
});
