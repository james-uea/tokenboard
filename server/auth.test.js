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

function jsonResponse(body, ok = true, status = 200) {
	return {
		ok,
		status,
		json: vi.fn().mockResolvedValue(body),
	};
}

function makeDbClient(handler) {
	return {
		query: vi.fn(handler),
		release: vi.fn(),
	};
}

function minimalContribution() {
	return {
		date: "2026-05-08",
		total_tokens: 100,
		total_cost: 0.001,
		input_tokens: 40,
		output_tokens: 60,
		cache_read_tokens: 0,
		cache_write_tokens: 0,
		reasoning_tokens: 0,
		models: {},
		clients: {},
	};
}

beforeEach(() => {
	mockDb.query.mockReset();
	mockDb.connect.mockReset();
	vi.restoreAllMocks();
	delete process.env.ALLOW_LEGACY_API_KEY;
	global.fetch = vi.fn();
});

describe("health checks", () => {
	it("returns liveness without touching the database", async () => {
		const response = await request(app).get("/healthz").expect(200);

		expect(response.body).toEqual({ status: "ok" });
		expect(response.headers["cache-control"]).toBe("no-store");
		expect(mockDb.query).not.toHaveBeenCalled();
	});

	it("returns readiness when the database responds", async () => {
		mockDb.query.mockResolvedValueOnce({ rows: [{ "?column?": 1 }] });

		const response = await request(app).get("/readyz").expect(200);

		expect(response.body).toEqual({ status: "ok", database: "ok" });
		expect(mockDb.query).toHaveBeenCalledWith("SELECT 1");
	});

	it("returns 503 readiness when the database query fails", async () => {
		mockDb.query.mockRejectedValueOnce(new Error("database unavailable"));
		vi.spyOn(console, "error").mockImplementation(() => {});

		const response = await request(app).get("/readyz").expect(503);

		expect(response.body).toEqual({ status: "error", database: "unavailable" });
	});
});

describe("installer script", () => {
	it("redirects the public Bash install path to the GitHub-hosted script", async () => {
		const response = await request(app).get("/install.sh").expect(302);

		expect(response.headers.location).toBe(
			"https://raw.githubusercontent.com/james-uea/tokenboard/main/scripts/install.sh",
		);
		expect(response.headers["cache-control"]).toBe("public, max-age=300");
		expect(mockDb.query).not.toHaveBeenCalled();
	});

	it("redirects the public PowerShell install path to the GitHub-hosted script", async () => {
		const response = await request(app).get("/install.ps1").expect(302);

		expect(response.headers.location).toBe(
			"https://raw.githubusercontent.com/james-uea/tokenboard/main/scripts/install.ps1",
		);
		expect(response.headers["cache-control"]).toBe("public, max-age=300");
		expect(mockDb.query).not.toHaveBeenCalled();
	});
});

describe("avatar proxy", () => {
	it("escapes fallback SVG content for invalid usernames", async () => {
		const response = await request(app)
			.get("/api/avatar/%22%3E%3Cscript%3Ealert(1)%3C%2Fscript%3E")
			.expect(200);

		const body = Buffer.isBuffer(response.body)
			? response.body.toString("utf8")
			: response.text;

		expect(response.headers["content-type"]).toContain("image/svg+xml");
		expect(body).toContain("aria-label=\"&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;\"");
		expect(body).not.toContain("<script>");
		expect(global.fetch).not.toHaveBeenCalled();
	});
});

describe("GitHub OAuth", () => {
	it("returns anonymous auth state without a session cookie", async () => {
		const response = await request(app).get("/api/auth/me").expect(200);

		expect(response.body).toEqual({ authenticated: false, user: null });
		expect(mockDb.query).not.toHaveBeenCalled();
	});

	it("starts GitHub OAuth with a stored state and HTTP-only state cookie", async () => {
		mockDb.query.mockResolvedValueOnce({ rows: [] });

		const response = await request(app)
			.get("/api/auth/github?return_to=%2Fusers%2Foctocat")
			.expect(302);

		expect(response.headers.location).toContain("https://github.com/login/oauth/authorize");
		expect(response.headers.location).toContain("client_id=github-client-id");
		expect(response.headers.location).toContain("state=");
		expect(response.headers["set-cookie"].join("\n")).toContain("tokenboard_oauth_state=");
		expect(response.headers["set-cookie"].join("\n")).toContain("HttpOnly");
		expect(mockDb.query.mock.calls[0][0]).toContain("INSERT INTO oauth_states");
		expect(mockDb.query.mock.calls[0][1][1]).toBe("/users/octocat");
	});

	it("rejects a callback with tampered state before contacting GitHub", async () => {
		mockDb.query.mockResolvedValueOnce({ rows: [] });
		const start = await request(app).get("/api/auth/github").expect(302);
		const cookie = start.headers["set-cookie"][0];

		const response = await request(app)
			.get("/api/auth/github/callback?code=abc&state=tampered")
			.set("Cookie", cookie)
			.expect(400);

		expect(response.body.error).toContain("Invalid GitHub OAuth state");
		expect(mockDb.connect).not.toHaveBeenCalled();
		expect(global.fetch).not.toHaveBeenCalled();
	});

	it("exchanges a valid callback, upserts the GitHub user, and creates a session", async () => {
		mockDb.query.mockResolvedValueOnce({ rows: [] });
		const start = await request(app)
			.get("/api/auth/github?return_to=%2Fusers%2Foctocat")
			.expect(302);
		const callbackState = new URL(start.headers.location).searchParams.get("state");
		const stateCookie = start.headers["set-cookie"][0];

		const stateClient = makeDbClient((sql) => {
			if (sql === "BEGIN" || sql === "COMMIT") return Promise.resolve({ rows: [] });
			if (String(sql).includes("SELECT return_to")) {
				return Promise.resolve({ rows: [{ return_to: "/users/octocat" }] });
			}
			if (String(sql).includes("UPDATE oauth_states")) return Promise.resolve({ rows: [] });
			throw new Error(`Unexpected state query: ${sql}`);
		});
		const userClient = makeDbClient((sql) => {
			if (sql === "BEGIN" || sql === "COMMIT") return Promise.resolve({ rows: [] });
			if (String(sql).includes("WHERE github_id")) return Promise.resolve({ rows: [] });
			if (String(sql).includes("LOWER(username)")) return Promise.resolve({ rows: [] });
			if (String(sql).includes("INSERT INTO users")) {
				return Promise.resolve({
					rows: [{
						id: 42,
						username: "octocat",
						display_name: "The Octocat",
						github_id: "583231",
						github_login: "octocat",
						avatar_url: "https://avatars.githubusercontent.com/u/583231?v=4",
						profile_url: "https://github.com/octocat",
						github_verified_at: new Date("2026-05-08T00:00:00Z"),
					}],
				});
			}
			throw new Error(`Unexpected user query: ${sql}`);
		});
		mockDb.connect
			.mockResolvedValueOnce(stateClient)
			.mockResolvedValueOnce(userClient);
		mockDb.query.mockResolvedValueOnce({ rows: [] });
		global.fetch
			.mockResolvedValueOnce(jsonResponse({ access_token: "gho_token" }))
			.mockResolvedValueOnce(jsonResponse({
				id: 583231,
				login: "octocat",
				name: "The Octocat",
				avatar_url: "https://avatars.githubusercontent.com/u/583231?v=4",
				html_url: "https://github.com/octocat",
			}));

		const response = await request(app)
			.get(`/api/auth/github/callback?code=abc&state=${callbackState}`)
			.set("Cookie", stateCookie)
			.expect(302);

		expect(response.headers.location).toBe("/users/octocat");
		expect(response.headers["set-cookie"].join("\n")).toContain("tokenboard_session=");
		expect(global.fetch).toHaveBeenCalledTimes(2);
		expect(userClient.query.mock.calls.some(([sql]) => String(sql).includes("INSERT INTO users"))).toBe(true);
		expect(mockDb.query.mock.calls.some(([sql]) => String(sql).includes("INSERT INTO auth_sessions"))).toBe(true);
	});
});

describe("CLI GitHub login", () => {
	it("starts a one-time CLI login request", async () => {
		mockDb.query.mockResolvedValueOnce({ rows: [] });

		const response = await request(app)
			.post("/api/auth/cli/start")
			.send({ name: "Laptop" })
			.expect(201);

		expect(response.body.code).toBeTruthy();
		expect(response.body.login_url).toContain("/api/auth/github?return_to=");
		expect(response.body.expires_in).toBe(600);
		expect(response.body.poll_interval).toBe(2);
		expect(mockDb.query.mock.calls[0][0]).toContain("INSERT INTO cli_login_requests");
		expect(mockDb.query.mock.calls[0][1][1]).toBe("Laptop");
	});

	it("completes browser login and lets the CLI poll the created token once", async () => {
		mockDb.query.mockResolvedValueOnce({ rows: [] });
		const start = await request(app)
			.post("/api/auth/cli/start")
			.send({ name: "Laptop" })
			.expect(201);

		mockDb.query.mockReset();
		mockDb.query
			.mockResolvedValueOnce({
				rows: [{
					id: 7,
					username: "octocat",
					display_name: "The Octocat",
					github_id: "583231",
					github_login: "octocat",
				}],
			})
			.mockResolvedValueOnce({ rows: [] })
			.mockResolvedValueOnce({
				rows: [{
					token_name: "Laptop",
					completed_at: null,
					consumed_at: null,
					expires_at: new Date(Date.now() + 600_000),
				}],
			})
			.mockResolvedValueOnce({ rows: [] });

		const complete = await request(app)
			.get(`/api/auth/cli/complete?code=${encodeURIComponent(start.body.code)}`)
			.set("Cookie", "tokenboard_session=session-token")
			.expect(200);

		expect(complete.text).toContain("Tokenboard login complete");
		expect(mockDb.query.mock.calls.some(([sql]) => String(sql).includes("UPDATE cli_login_requests"))).toBe(true);

		mockDb.query.mockReset();
		mockDb.query
			.mockResolvedValueOnce({
				rows: [{
					completed_at: new Date("2026-05-08T00:00:00Z"),
					consumed_at: null,
					expires_at: new Date(Date.now() + 600_000),
					token_name: "Laptop",
					id: 7,
					username: "octocat",
					display_name: "The Octocat",
					github_id: "583231",
					github_login: "octocat",
				}],
			})
			.mockResolvedValueOnce({ rows: [{ user_id: 7, token_name: "Laptop" }] })
			.mockResolvedValueOnce({
				rows: [{
					id: 11,
					name: "Laptop",
					token_prefix: "tbp_abcdefg",
					last_four: "wxyz",
					created_at: new Date("2026-05-08T00:00:00Z"),
					last_used_at: null,
					revoked_at: null,
				}],
			});

		const poll = await request(app)
			.get(`/api/auth/cli/poll?code=${encodeURIComponent(start.body.code)}`)
			.expect(200);

		expect(poll.body.status).toBe("complete");
		expect(poll.body.token).toMatch(/^tbp_/);
		expect(poll.body.user).toMatchObject({
			username: "octocat",
			github_login: "octocat",
		});
		expect(mockDb.query.mock.calls[1][0]).toContain("RETURNING user_id, token_name");
		expect(mockDb.query.mock.calls.some(([sql]) => String(sql).includes("INSERT INTO user_api_tokens"))).toBe(true);
	});
});

describe("user API tokens", () => {
	it("creates a CLI token for the signed-in user without returning its hash", async () => {
		mockDb.query
			.mockResolvedValueOnce({
				rows: [{
					id: 7,
					username: "octocat",
					display_name: "The Octocat",
					github_id: "583231",
					github_login: "octocat",
				}],
			})
			.mockResolvedValueOnce({ rows: [] })
			.mockResolvedValueOnce({
				rows: [{
					id: 11,
					name: "Laptop",
					token_prefix: "tbp_abcdefg",
					last_four: "wxyz",
					created_at: new Date("2026-05-08T00:00:00Z"),
					last_used_at: null,
					revoked_at: null,
				}],
			});

		const response = await request(app)
			.post("/api/auth/tokens")
			.set("Cookie", "tokenboard_session=session-token")
			.send({ name: "Laptop" })
			.expect(201);

		expect(response.body.token).toMatch(/^tbp_/);
		expect(response.body.api_token).not.toHaveProperty("token_hash");
		const insertCall = mockDb.query.mock.calls.find(([sql]) => String(sql).includes("INSERT INTO user_api_tokens"));
		expect(insertCall[1][2]).toHaveLength(64);
		expect(insertCall[1][2]).not.toBe(response.body.token);
	});
});

describe("submission authentication", () => {
	it("uses the user token owner and ignores a spoofed payload username", async () => {
		mockDb.query
			.mockResolvedValueOnce({
				rows: [{
					token_id: 99,
					id: 7,
					username: "octocat",
					display_name: "The Octocat",
					github_id: "583231",
					github_login: "octocat",
				}],
			})
			.mockResolvedValueOnce({ rows: [] });

		const submitClient = makeDbClient((sql, params) => {
			if (sql === "BEGIN" || sql === "COMMIT") return Promise.resolve({ rows: [] });
			if (String(sql).includes("UPDATE users")) {
				expect(params).toEqual([7, "octocat", "The Octocat"]);
				return Promise.resolve({ rows: [{ id: 7 }] });
			}
			if (String(sql).includes("INSERT INTO submissions")) {
				expect(params[0]).toBe(7);
				return Promise.resolve({ rows: [] });
			}
			throw new Error(`Unexpected submit query: ${sql}`);
		});
		mockDb.connect.mockResolvedValueOnce(submitClient);

		const response = await request(app)
			.post("/api/submit")
			.set("Authorization", "Bearer tbp_valid")
			.send({
				username: "mallory",
				display_name: "Mallory",
				contributions: [minimalContribution()],
			})
			.expect(200);

		expect(response.body.username).toBe("octocat");
		expect(response.body.display_name).toBe("The Octocat");
	});

	it("stores custom model providers without custom hostnames", async () => {
		mockDb.query
			.mockResolvedValueOnce({
				rows: [{
					token_id: 99,
					id: 7,
					username: "octocat",
					display_name: "The Octocat",
					github_id: "583231",
					github_login: "octocat",
				}],
			})
			.mockResolvedValueOnce({ rows: [] });

		const contribution = minimalContribution();
		contribution.models = {
			"gpt-5.5|Hermes": {
				tokens: 100,
				input: 40,
				output: 10,
				cache_read: 50,
				cache_write: 0,
				cost: 0.123456,
				provider: "Custom:api.example.com",
				source: "Hermes",
			},
		};

		const submitClient = makeDbClient((sql, params) => {
			if (sql === "BEGIN" || sql === "COMMIT") return Promise.resolve({ rows: [] });
			if (String(sql).includes("UPDATE users")) {
				return Promise.resolve({ rows: [{ id: 7 }] });
			}
			if (String(sql).includes("INSERT INTO submissions")) {
				const models = JSON.parse(params[9]);
				expect(models["gpt-5.5|Hermes"].provider).toBe("Custom");
				expect(JSON.stringify(models)).not.toContain("api.example.com");
				return Promise.resolve({ rows: [] });
			}
			throw new Error(`Unexpected submit query: ${sql}`);
		});
		mockDb.connect.mockResolvedValueOnce(submitClient);

		await request(app)
			.post("/api/submit")
			.set("Authorization", "Bearer tbp_valid")
			.send({
				username: "mallory",
				display_name: "Mallory",
				contributions: [contribution],
			})
			.expect(200);
	});

	it("rejects the legacy shared API key unless explicitly enabled", async () => {
		mockDb.query.mockResolvedValueOnce({ rows: [] });

		const response = await request(app)
			.post("/api/submit")
			.set("Authorization", "Bearer legacy-key")
			.send({
				username: "octocat",
				display_name: "The Octocat",
				contributions: [minimalContribution()],
			})
			.expect(403);

		expect(response.body.error).toContain("Legacy API key submissions are disabled");
		expect(mockDb.connect).not.toHaveBeenCalled();
	});
});
