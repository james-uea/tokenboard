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

function makeDbClient(handler) {
	return {
		query: vi.fn(handler),
		release: vi.fn(),
	};
}

function minimalContribution() {
	return {
		date: "2026-05-20",
		total_tokens: 120,
		total_cost: 1.23,
		input_tokens: 40,
		output_tokens: 80,
		cache_read_tokens: 0,
		cache_write_tokens: 0,
		reasoning_tokens: 0,
		models: {},
		clients: {},
	};
}

function buildTokenRow(tokenId) {
	return {
		token_id: tokenId,
		id: 7,
		username: "octocat",
		display_name: "The Octocat",
		github_id: "583231",
		github_login: "octocat",
	};
}

beforeEach(() => {
	mockDb.query.mockReset();
	mockDb.connect.mockReset();
	vi.restoreAllMocks();
});

describe("submit API source-aware upserts", () => {
	it("uses token id as the submission source so different devices do not overwrite each other", async () => {
		const authCallCount = { value: 0 };
		mockDb.query
			.mockImplementation((sql, params) => {
				if (String(sql).includes("t.id AS token_id")) {
					authCallCount.value += 1;
					return Promise.resolve({ rows: [buildTokenRow(authCallCount.value === 1 ? 101 : 102)] });
				}
				if (String(sql).includes("UPDATE user_api_tokens")) {
					return Promise.resolve({ rows: [] });
				}
				throw new Error(`Unexpected auth query: ${sql}`);
			});

		const tokenSources = [];
		const submitClient = () =>
			makeDbClient((sql, params) => {
				if (sql === "BEGIN" || sql === "COMMIT") return Promise.resolve({ rows: [] });
				if (String(sql).includes("UPDATE users")) return Promise.resolve({ rows: [{ id: 7 }] });
				if (String(sql).includes("INSERT INTO submissions")) {
					tokenSources.push(params[9]);
					expect(sql).toContain("(user_id, date, total_tokens, total_cost,");
					expect(sql).toContain("submission_source");
					expect(sql).toContain("ON CONFLICT (user_id, date, submission_source)");
					return Promise.resolve({ rows: [] });
				}
				throw new Error(`Unexpected submit query: ${sql}`);
			});

		mockDb.connect
			.mockResolvedValueOnce(submitClient())
			.mockResolvedValueOnce(submitClient());

		const body = {
			username: "mallory",
			display_name: "Mallory",
			contributions: [minimalContribution()],
		};

		await request(app).post("/api/submit").set("Authorization", "Bearer tbp_token_one").send(body).expect(200);
		await request(app).post("/api/submit").set("Authorization", "Bearer tbp_token_two").send(body).expect(200);

		expect(tokenSources).toEqual([101, 102]);
	});

	it("reuses the same submission source when the same token submits again", async () => {
		mockDb.query
			.mockImplementation((sql, params) => {
				if (String(sql).includes("t.id AS token_id")) {
					return Promise.resolve({ rows: [buildTokenRow(202)] });
				}
				if (String(sql).includes("UPDATE user_api_tokens")) {
					return Promise.resolve({ rows: [] });
				}
				throw new Error(`Unexpected auth query: ${sql}`);
			});

		const tokenSources = [];
		const submitClient = () =>
			makeDbClient((sql, params) => {
				if (sql === "BEGIN" || sql === "COMMIT") return Promise.resolve({ rows: [] });
				if (String(sql).includes("UPDATE users")) return Promise.resolve({ rows: [{ id: 7 }] });
				if (String(sql).includes("INSERT INTO submissions")) {
					tokenSources.push(params[9]);
					expect(sql).toContain("ON CONFLICT (user_id, date, submission_source)");
					return Promise.resolve({ rows: [] });
				}
				throw new Error(`Unexpected submit query: ${sql}`);
			});

		mockDb.connect.mockResolvedValueOnce(submitClient()).mockResolvedValueOnce(submitClient());

		const body = {
			username: "mallory",
			display_name: "Mallory",
			contributions: [minimalContribution()],
		};

		await request(app).post("/api/submit").set("Authorization", "Bearer tbp_token_replay").send(body).expect(200);
		await request(app).post("/api/submit").set("Authorization", "Bearer tbp_token_replay").send(body).expect(200);

		expect(tokenSources).toEqual([202, 202]);
	});
});
