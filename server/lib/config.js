const DEV_SESSION_SECRET = "tokenboard-dev-session-secret";
const WEAK_SESSION_SECRETS = new Set([
	DEV_SESSION_SECRET,
	"change-me",
	"changeme",
	"development",
	"password",
	"secret",
	"test",
]);

function envValue(env, name) {
	const value = env[name];
	return typeof value === "string" ? value.trim() : "";
}

function isProduction(env = process.env) {
	return env.NODE_ENV === "production";
}

export function getSessionSecret(env = process.env) {
	const secret = envValue(env, "SESSION_SECRET");
	if (!secret) {
		if (isProduction(env)) {
			throw new Error("SESSION_SECRET is required when NODE_ENV=production");
		}
		return DEV_SESSION_SECRET;
	}
	if (isProduction(env) && WEAK_SESSION_SECRETS.has(secret.toLowerCase())) {
		throw new Error("SESSION_SECRET must not use a development default in production");
	}
	return secret;
}

export function getGitHubOAuthConfig(env = process.env) {
	const clientId = envValue(env, "GITHUB_CLIENT_ID");
	const clientSecret = envValue(env, "GITHUB_CLIENT_SECRET");
	if (!clientId || !clientSecret) {
		throw new Error("GitHub OAuth is not configured");
	}
	return { clientId, clientSecret };
}

export function validateProductionConfig(env = process.env) {
	if (!isProduction(env)) {
		return;
	}

	const errors = [];
	if (!envValue(env, "SESSION_SECRET")) {
		errors.push("SESSION_SECRET is required");
	} else {
		try {
			getSessionSecret(env);
		} catch (error) {
			errors.push(error.message);
		}
	}
	if (!envValue(env, "GITHUB_CLIENT_ID")) {
		errors.push("GITHUB_CLIENT_ID is required");
	}
	if (!envValue(env, "GITHUB_CLIENT_SECRET")) {
		errors.push("GITHUB_CLIENT_SECRET is required");
	}

	if (errors.length > 0) {
		throw new Error(`Invalid production configuration: ${errors.join("; ")}`);
	}
}

validateProductionConfig();
