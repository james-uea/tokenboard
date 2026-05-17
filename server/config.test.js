import { afterEach, describe, expect, it, vi } from "vitest";

const originalEnv = { ...process.env };

function restoreEnv() {
	for (const key of Object.keys(process.env)) {
		delete process.env[key];
	}
	Object.assign(process.env, originalEnv);
}

afterEach(() => {
	vi.resetModules();
	restoreEnv();
});

describe("production configuration validation", () => {
	it("fails startup when production auth secrets are missing", async () => {
		vi.resetModules();
		process.env.NODE_ENV = "production";
		delete process.env.SESSION_SECRET;
		delete process.env.GITHUB_CLIENT_ID;
		delete process.env.GITHUB_CLIENT_SECRET;

		await expect(import("./index.js")).rejects.toThrow(
			/Invalid production configuration: SESSION_SECRET is required; GITHUB_CLIENT_ID is required; GITHUB_CLIENT_SECRET is required/,
		);
	});

	it("fails startup when production uses the development session secret", async () => {
		vi.resetModules();
		process.env.NODE_ENV = "production";
		process.env.SESSION_SECRET = "tokenboard-dev-session-secret";
		process.env.GITHUB_CLIENT_ID = "github-client-id";
		process.env.GITHUB_CLIENT_SECRET = "github-client-secret";

		await expect(import("./index.js")).rejects.toThrow(
			/SESSION_SECRET must not use a development default in production/,
		);
	});
});
