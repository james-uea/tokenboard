import request from "supertest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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

function timelineRow(date, totalTokens) {
	return {
		date: new Date(`${date}T00:00:00.000Z`),
		total_tokens: String(totalTokens),
		total_cost: "1.500000",
		input_tokens: "10",
		output_tokens: "20",
		cache_read_tokens: "30",
		cache_write_tokens: "40",
		reasoning_tokens: "0",
	};
}

function summaryRow() {
	return {
		username: "octocat",
		display_name: "The Octocat",
		total_submissions: 1,
		total_tokens: "100",
		total_cost: "1.500000",
		input_tokens: "10",
		output_tokens: "20",
		cache_read_tokens: "30",
		cache_write_tokens: "40",
		reasoning_tokens: "0",
		last_updated: new Date("2026-05-15T12:00:00.000Z"),
		first_date: new Date("2026-05-15T00:00:00.000Z"),
		last_date: new Date("2026-05-15T00:00:00.000Z"),
		active_days: 1,
	};
}

beforeEach(() => {
	mockDb.query.mockReset();
	mockDb.connect.mockReset();
	vi.useFakeTimers();
	vi.setSystemTime(new Date("2026-05-17T12:00:00.000Z"));
	global.fetch = vi.fn();
});

afterEach(() => {
	vi.useRealTimers();
	vi.restoreAllMocks();
});

describe("stats API timeline", () => {
	it("fills missing usage days through today in the user stats timeline", async () => {
		mockDb.query
			.mockResolvedValueOnce({ rows: [summaryRow()] })
			.mockResolvedValueOnce({ rows: [timelineRow("2026-05-15", 100)] })
			.mockResolvedValueOnce({ rows: [] })
			.mockResolvedValueOnce({ rows: [] });

		const response = await request(app).get("/api/stats/octocat").expect(200);

		expect(response.body.timeline.map((entry) => entry.date.slice(0, 10))).toEqual([
			"2026-05-15",
			"2026-05-16",
			"2026-05-17",
		]);
		expect(response.body.timeline.map((entry) => entry.total_tokens)).toEqual([100, 0, 0]);
		expect(response.body.timeline.map((entry) => entry.running_total_tokens)).toEqual([100, 100, 100]);
		expect(response.body.timeline.map((entry) => entry.has_data)).toEqual([true, false, false]);
		expect(response.body.diffs.day_over_day.map((entry) => entry.date.slice(0, 10))).toEqual([
			"2026-05-16",
			"2026-05-17",
		]);
		expect(response.body.diffs.day_over_day[0]).toMatchObject({
			delta_total_tokens: -100,
			percent_change: -100,
		});
		expect(response.body.diffs.day_over_day[1]).toMatchObject({
			delta_total_tokens: 0,
			percent_change: 0,
		});
	});

	it("distinguishes real future data from padded future gaps", async () => {
		mockDb.query
			.mockResolvedValueOnce({ rows: [summaryRow()] })
			.mockResolvedValueOnce({ rows: [timelineRow("2026-05-15", 100), timelineRow("2026-05-19", 25)] })
			.mockResolvedValueOnce({ rows: [] })
			.mockResolvedValueOnce({ rows: [] });

		const response = await request(app).get("/api/stats/octocat").expect(200);

		expect(response.body.timeline.map((entry) => entry.date.slice(0, 10))).toEqual([
			"2026-05-15",
			"2026-05-16",
			"2026-05-17",
			"2026-05-18",
			"2026-05-19",
		]);
		expect(response.body.timeline.map((entry) => entry.total_tokens)).toEqual([100, 0, 0, 0, 25]);
		expect(response.body.timeline.map((entry) => entry.has_data)).toEqual([true, false, false, false, true]);
	});

	it("fills missing usage days through today in the standalone diffs endpoint", async () => {
		mockDb.query.mockResolvedValueOnce({ rows: [timelineRow("2026-05-15", 100)] });

		const response = await request(app).get("/api/stats/octocat/diffs").expect(200);

		expect(response.body.diffs.day_over_day.map((entry) => entry.date.slice(0, 10))).toEqual([
			"2026-05-16",
			"2026-05-17",
		]);
		expect(response.body.diffs.day_over_day.map((entry) => entry.delta_total_tokens)).toEqual([-100, 0]);
		expect(response.body.diffs.largest_decreases).toHaveLength(1);
		expect(response.body.diffs.largest_decreases[0].date.slice(0, 10)).toBe("2026-05-16");
	});
});
