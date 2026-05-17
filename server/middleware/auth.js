import { authenticateApiToken, getBearerToken } from "../lib/auth.js";

export default async function authMiddleware(req, res, next) {
	const apiKey = process.env.API_KEY;
	const token = getBearerToken(req);

	if (!token) {
		return res
			.status(401)
			.json({ error: "Missing or invalid Authorization header" });
	}

	try {
		const apiTokenAuth = await authenticateApiToken(token);
		if (apiTokenAuth) {
			req.auth = {
				type: "user_api_token",
				token_id: apiTokenAuth.token_id,
				user: apiTokenAuth.user,
			};
			return next();
		}
	} catch (error) {
		console.error("API token authentication failed:", error);
		return res.status(500).json({ error: "Internal server error" });
	}

	const legacyAllowed = process.env.ALLOW_LEGACY_API_KEY === "true";
	if (legacyAllowed) {
		if (!apiKey || apiKey === "change-me") {
			return res
				.status(500)
				.json({ error: "Server not configured with API_KEY" });
		}
		if (token === apiKey) {
			req.auth = { type: "legacy_api_key", user: null };
			return next();
		}
	}

	if (apiKey && token === apiKey && !legacyAllowed) {
		return res.status(403).json({
			error: "Legacy API key submissions are disabled. Sign in with GitHub and create a Tokenboard API token.",
		});
	}

	return res.status(403).json({ error: "Invalid API token" });
}
