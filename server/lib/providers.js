const KNOWN_PROVIDERS = new Map([
	["alibaba", "Alibaba"],
	["anthropic", "Anthropic"],
	["deepseek", "DeepSeek"],
	["github", "GitHub"],
	["google", "Google"],
	["meta", "Meta"],
	["moonshot", "Moonshot"],
	["nous", "Nous"],
	["openai", "OpenAI"],
	["unknown", "Unknown"],
]);

export function normalizeProviderName(value) {
	const provider = typeof value === "string" ? value.trim() : "";
	if (!provider) {
		return "";
	}

	const lower = provider.toLowerCase();
	if (lower === "custom" || /^custom\s*:/.test(lower)) {
		return "Custom";
	}

	return KNOWN_PROVIDERS.get(lower) || provider;
}
