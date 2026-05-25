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
	mockDb.connect.mockReset();
	global.fetch = vi.fn();
});

describe("badges API", () => {
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
