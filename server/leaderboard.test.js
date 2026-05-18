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

function leaderboardRow(overrides = {}) {
	return {
		username: "octocat",
		display_name: "The Octocat",
		total_tokens: "1000",
		total_cost: "1.250000",
		input_tokens: "600",
		output_tokens: "300",
		cache_read_tokens: "80",
		cache_write_tokens: "20",
		reasoning_tokens: "0",
		last_updated: new Date("2026-05-15T12:00:00.000Z"),
		active_days: "3",
		total_submissions: "4",
		top_model: "gpt-5.5",
		top_model_tokens: "700",
		top_model_input_tokens: "450",
		top_model_output_tokens: "250",
		...overrides,
	};
}

beforeEach(() => {
	mockDb.query.mockReset();
	mockDb.connect.mockReset();
	global.fetch = vi.fn();
});

describe("leaderboard API search", () => {
	it("matches username fragments case-insensitively", async () => {
		mockDb.query.mockResolvedValueOnce({ rows: [leaderboardRow()] });

		const response = await request(app).get("/api/leaderboard?q=OcTo&limit=25").expect(200);
		const [sql, params] = mockDb.query.mock.calls[0];

		expect(sql).toContain("LOWER(u.username) LIKE $1 ESCAPE '\\'");
		expect(params).toEqual(["%octo%", 25]);
		expect(response.body).toMatchObject({
			period: "all",
			query: "OcTo",
			entries: 1,
		});
		expect(response.body.leaderboard[0]).toMatchObject({
			username: "octocat",
			display_name: "The Octocat",
			total_tokens: 1000,
		});
	});

	it("preserves period filtering and limit behavior while searching", async () => {
		mockDb.query.mockResolvedValueOnce({ rows: [] });

		await request(app).get("/api/leaderboard?period=week&q=cat&limit=300").expect(200);
		const [sql, params] = mockDb.query.mock.calls[0];

		expect(sql).toContain("s.date >= date_trunc('week', CURRENT_DATE)::date");
		expect(sql).toContain("LOWER(u.username) LIKE $1 ESCAPE '\\'");
		expect(params).toEqual(["%cat%", 200]);
	});

	it("keeps custom date parameters before the username search parameter", async () => {
		mockDb.query.mockResolvedValueOnce({ rows: [] });

		await request(app)
			.get("/api/leaderboard?period=custom&periodStart=2026-01-01&periodEnd=2026-05-18&q=octo&limit=10")
			.expect(200);
		const [sql, params] = mockDb.query.mock.calls[0];

		expect(sql).toContain("s.date >= $1 AND s.date <= $2");
		expect(sql).toContain("LOWER(u.username) LIKE $3 ESCAPE '\\'");
		expect(params).toEqual(["2026-01-01", "2026-05-18", "%octo%", 10]);
	});

	it("trims empty search input so it behaves like the existing endpoint", async () => {
		mockDb.query.mockResolvedValueOnce({ rows: [] });

		await request(app).get("/api/leaderboard?q=%20%20&limit=7").expect(200);
		const [sql, params] = mockDb.query.mock.calls[0];

		expect(sql).not.toContain("LOWER(u.username)");
		expect(params).toEqual([7]);
	});

	it("clamps long search input before building the LIKE pattern", async () => {
		mockDb.query.mockResolvedValueOnce({ rows: [] });
		const query = "A".repeat(80);

		const response = await request(app).get(`/api/leaderboard?q=${query}`).expect(200);
		const [, params] = mockDb.query.mock.calls[0];

		expect(response.body.query).toBe("A".repeat(64));
		expect(params[0]).toBe(`%${"a".repeat(64)}%`);
	});
});
