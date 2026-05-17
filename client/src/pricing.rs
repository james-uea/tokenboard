//! Live model pricing lookup.
//!
//! Pricing is loaded from LiteLLM first and OpenRouter as a fallback. The
//! scanner remains synchronous, so this module uses reqwest's blocking client
//! and a small on-disk cache to keep normal scans fast and resilient offline.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

const LITELLM_PRICING_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelPricing {
    pub input_cost_per_token: Option<f64>,
    pub input_cost_per_token_above_128k_tokens: Option<f64>,
    pub input_cost_per_token_above_200k_tokens: Option<f64>,
    pub input_cost_per_token_above_256k_tokens: Option<f64>,
    pub input_cost_per_token_above_272k_tokens: Option<f64>,
    pub output_cost_per_token: Option<f64>,
    pub output_cost_per_token_above_128k_tokens: Option<f64>,
    pub output_cost_per_token_above_200k_tokens: Option<f64>,
    pub output_cost_per_token_above_256k_tokens: Option<f64>,
    pub output_cost_per_token_above_272k_tokens: Option<f64>,
    pub cache_creation_input_token_cost: Option<f64>,
    pub cache_creation_input_token_cost_above_200k_tokens: Option<f64>,
    pub cache_read_input_token_cost: Option<f64>,
    pub cache_read_input_token_cost_above_200k_tokens: Option<f64>,
    pub cache_read_input_token_cost_above_272k_tokens: Option<f64>,
}

type PricingDataset = HashMap<String, ModelPricing>;

#[derive(Debug, Clone)]
struct LookupResult {
    key: String,
    source: PricingSource,
    pricing: ModelPricing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PricingSource {
    LiteLlm,
    OpenRouter,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CostTokens {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub reasoning: i64,
}

pub struct PricingService {
    litellm: PricingDataset,
    openrouter: PricingDataset,
    client: reqwest::blocking::Client,
    openrouter_endpoint_cache: Mutex<HashMap<String, Option<ModelPricing>>>,
}

impl PricingService {
    pub fn load() -> Self {
        #[cfg(test)]
        {
            return Self::new(PricingDataset::new(), PricingDataset::new());
        }

        #[cfg(not(test))]
        {
            let client = build_client();
            let litellm = load_litellm_pricing(&client);
            let openrouter = load_openrouter_pricing(&client);
            Self::with_client(litellm, openrouter, client)
        }
    }

    #[cfg(test)]
    fn new(litellm: PricingDataset, openrouter: PricingDataset) -> Self {
        Self::with_client(litellm, openrouter, build_client())
    }

    fn with_client(
        litellm: PricingDataset,
        openrouter: PricingDataset,
        client: reqwest::blocking::Client,
    ) -> Self {
        Self {
            litellm,
            openrouter,
            client,
            openrouter_endpoint_cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn calculate_cost(&self, model: &str, provider: &str, tokens: CostTokens) -> f64 {
        if let Some(result) = self.lookup(model, provider) {
            let pricing = if result.source == PricingSource::OpenRouter {
                self.enrich_openrouter_pricing(&result.key, result.pricing)
            } else {
                result.pricing
            };
            let cost = compute_cost(&pricing, tokens);
            if cost > 0.0 || !tokens.has_billable_tokens() {
                return cost;
            }
        }

        fallback_cost(model, tokens)
    }

    fn lookup(&self, model: &str, provider: &str) -> Option<LookupResult> {
        let candidates = lookup_candidates(model, provider);

        for candidate in &candidates {
            if let Some(result) = exact_lookup(candidate, &self.litellm, PricingSource::LiteLlm) {
                return Some(result);
            }
        }
        for candidate in &candidates {
            if let Some(result) =
                exact_lookup(candidate, &self.openrouter, PricingSource::OpenRouter)
            {
                return Some(result);
            }
        }
        for candidate in &candidates {
            if let Some(result) = fuzzy_lookup(candidate, &self.litellm, PricingSource::LiteLlm) {
                return Some(result);
            }
        }
        for candidate in &candidates {
            if let Some(result) =
                fuzzy_lookup(candidate, &self.openrouter, PricingSource::OpenRouter)
            {
                return Some(result);
            }
        }

        None
    }

    fn enrich_openrouter_pricing(&self, key: &str, fallback: ModelPricing) -> ModelPricing {
        if fallback.cache_read_input_token_cost.is_some()
            && fallback.cache_creation_input_token_cost.is_some()
        {
            return fallback;
        }

        if let Ok(cache) = self.openrouter_endpoint_cache.lock() {
            if let Some(cached) = cache.get(key) {
                return cached.clone().unwrap_or(fallback);
            }
        }

        let fetched = fetch_openrouter_endpoint_pricing(&self.client, key);

        if let Ok(mut cache) = self.openrouter_endpoint_cache.lock() {
            cache.insert(key.to_string(), fetched.clone());
        }

        fetched.unwrap_or(fallback)
    }
}

fn build_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(8))
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

fn load_litellm_pricing(client: &reqwest::blocking::Client) -> PricingDataset {
    load_or_fetch_cache("pricing-litellm.json", || {
        client
            .get(LITELLM_PRICING_URL)
            .send()
            .ok()?
            .error_for_status()
            .ok()?
            .json::<PricingDataset>()
            .ok()
            .map(filter_litellm_pricing)
    })
}

fn load_openrouter_pricing(client: &reqwest::blocking::Client) -> PricingDataset {
    load_or_fetch_cache("pricing-openrouter.json", || {
        let response = client
            .get(OPENROUTER_MODELS_URL)
            .send()
            .ok()?
            .error_for_status()
            .ok()?
            .json::<OpenRouterModelsResponse>()
            .ok()?;
        Some(map_openrouter_models(response))
    })
}

fn load_or_fetch_cache<F>(name: &str, fetch: F) -> PricingDataset
where
    F: FnOnce() -> Option<PricingDataset>,
{
    if let Some(cached) = load_cache(name, Some(CACHE_TTL)) {
        return cached;
    }

    if let Some(fresh) = fetch() {
        save_cache(name, &fresh);
        return fresh;
    }

    load_cache(name, None).unwrap_or_default()
}

fn filter_litellm_pricing(mut data: PricingDataset) -> PricingDataset {
    data.retain(|key, pricing| {
        let lower = key.to_lowercase();
        !lower.starts_with("github_copilot/") && has_any_price(pricing)
    });
    data
}

fn cache_path(name: &str) -> PathBuf {
    let base = dirs::cache_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("tokenboard").join(name)
}

fn load_cache(name: &str, max_age: Option<Duration>) -> Option<PricingDataset> {
    let path = cache_path(name);
    if let Some(max_age) = max_age {
        let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
        let age = SystemTime::now().duration_since(modified).ok()?;
        if age > max_age {
            return None;
        }
    }
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_cache(name: &str, data: &PricingDataset) {
    let path = cache_path(name);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(content) = serde_json::to_string(data) {
        let _ = std::fs::write(path, content);
    }
}

#[derive(Debug, Deserialize)]
struct OpenRouterModelsResponse {
    data: Vec<OpenRouterModel>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterModel {
    id: String,
    pricing: Option<OpenRouterListPricing>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterListPricing {
    prompt: String,
    completion: String,
}

fn map_openrouter_models(response: OpenRouterModelsResponse) -> PricingDataset {
    response
        .data
        .into_iter()
        .filter_map(|model| {
            let pricing = model.pricing?;
            Some((
                model.id,
                ModelPricing {
                    input_cost_per_token: parse_price(&pricing.prompt),
                    output_cost_per_token: parse_price(&pricing.completion),
                    ..Default::default()
                },
            ))
        })
        .filter(|(_, pricing)| has_any_price(pricing))
        .collect()
}

#[derive(Debug, Deserialize)]
struct OpenRouterEndpointsResponse {
    data: OpenRouterEndpointData,
}

#[derive(Debug, Deserialize)]
struct OpenRouterEndpointData {
    endpoints: Vec<OpenRouterEndpoint>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterEndpoint {
    provider_name: String,
    pricing: OpenRouterEndpointPricing,
}

#[derive(Debug, Deserialize)]
struct OpenRouterEndpointPricing {
    prompt: String,
    completion: String,
    #[serde(default)]
    input_cache_read: Option<String>,
    #[serde(default)]
    input_cache_write: Option<String>,
}

fn fetch_openrouter_endpoint_pricing(
    client: &reqwest::blocking::Client,
    model_id: &str,
) -> Option<ModelPricing> {
    let url = format!("{}/{}/endpoints", OPENROUTER_MODELS_URL, model_id);
    let response = client
        .get(url)
        .header("Content-Type", "application/json")
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .json::<OpenRouterEndpointsResponse>()
        .ok()?;

    let author = openrouter_author_provider(model_id);
    let endpoint = response
        .data
        .endpoints
        .iter()
        .find(|endpoint| {
            author
                .map(|name| endpoint.provider_name.eq_ignore_ascii_case(name))
                .unwrap_or(false)
        })
        .or_else(|| response.data.endpoints.first())?;

    Some(ModelPricing {
        input_cost_per_token: parse_price(&endpoint.pricing.prompt),
        output_cost_per_token: parse_price(&endpoint.pricing.completion),
        cache_read_input_token_cost: endpoint
            .pricing
            .input_cache_read
            .as_deref()
            .and_then(parse_price),
        cache_creation_input_token_cost: endpoint
            .pricing
            .input_cache_write
            .as_deref()
            .and_then(parse_price),
        ..Default::default()
    })
    .filter(has_any_price)
}

fn openrouter_author_provider(model_id: &str) -> Option<&'static str> {
    let prefix = model_id.split('/').next()?.to_lowercase();
    match prefix.as_str() {
        "anthropic" => Some("Anthropic"),
        "deepseek" => Some("DeepSeek"),
        "google" => Some("Google"),
        "meta-llama" => Some("Meta"),
        "mistralai" => Some("Mistral"),
        "moonshotai" => Some("Moonshot AI"),
        "openai" => Some("OpenAI"),
        "qwen" => Some("Alibaba"),
        "x-ai" => Some("xAI"),
        "z-ai" => Some("Z.AI"),
        _ => None,
    }
}

fn lookup_candidates(model: &str, provider: &str) -> Vec<String> {
    let mut bases = Vec::new();
    push_unique(&mut bases, canonical_model(model));

    if let Some(base) = bases.first().cloned() {
        if let Some(stripped) = strip_colon_variant(&base) {
            push_unique(&mut bases, stripped);
        }
        if let Some(normalized) = normalize_version_separator(&base) {
            push_unique(&mut bases, normalized);
        }
    }

    let mut candidates = Vec::new();
    let prefixes = provider_prefixes(provider, model);
    for base in bases {
        for prefix in &prefixes {
            if !base.starts_with(&format!("{}/", prefix)) {
                push_unique(&mut candidates, format!("{}/{}", prefix, base));
            }
        }
        push_unique(&mut candidates, base);
    }
    candidates
}

fn canonical_model(model: &str) -> String {
    let mut name = model.trim().to_lowercase();

    if let Some(alias) = model_alias(&name) {
        name = alias.to_string();
    }
    if let Some(base) = strip_parenthesized_reasoning_tier(&name) {
        name = base.to_string();
    }
    if name.len() > 9 {
        let suffix = &name[name.len() - 9..];
        if suffix.as_bytes()[0] == b'-' && suffix[1..].chars().all(|c| c.is_ascii_digit()) {
            name.truncate(name.len() - 9);
        }
    }

    name
}

fn model_alias(model: &str) -> Option<&'static str> {
    match model {
        "model_placeholder_m26" => Some("claude-opus-4-6"),
        "model_placeholder_m35" => Some("claude-sonnet-4-6"),
        "model_placeholder_m36" | "model_placeholder_m37" => Some("gemini-3.1-pro"),
        "model_placeholder_m47" => Some("gemini-3-flash-preview"),
        "model_openai_gpt_oss_120b_medium" => Some("gpt-oss-120b-medium"),
        "claude-opus-4.6" | "claude-opus-4.6-thinking" | "claude-opus-4-6-thinking" => {
            Some("claude-opus-4-6")
        }
        "claude-sonnet-4.6" | "claude-sonnet-4.6-thinking" | "claude-sonnet-4-6-thinking" => {
            Some("claude-sonnet-4-6")
        }
        "gemini-3-flash" | "gemini-3-flash-c" => Some("gemini-3-flash-preview"),
        "gemini-3.1-pro-high" | "gemini-3.1-pro-low" => Some("gemini-3.1-pro"),
        "gemini-3-pro-high" | "gemini-3-pro-low" => Some("gemini-3-pro"),
        "zai-org/glm-5:thinking" => Some("novita/zai-org/glm-4.5-air"),
        "k2p5" | "k2-p5" | "kimi-k2.5-thinking" => Some("kimi-k2-thinking"),
        "moonshotai/kimi-k2.5:thinking" => Some("deepinfra/moonshotai/kimi-k2-instruct-0905"),
        "kimi-for-coding" => Some("kimi-k2.5"),
        "kimi-k2.5-nvfp4" | "kimi-k2-instruct-0905" => Some("kimi-k2.5"),
        _ => None,
    }
}

fn strip_parenthesized_reasoning_tier(model: &str) -> Option<&str> {
    let without_close = model.strip_suffix(')')?;
    let (base, tier) = without_close.rsplit_once('(')?;
    if matches!(
        tier,
        "minimal" | "low" | "medium" | "high" | "xhigh" | "auto" | "none"
    ) {
        Some(base)
    } else {
        None
    }
}

fn strip_colon_variant(model: &str) -> Option<String> {
    model
        .rsplit_once(':')
        .map(|(base, _)| base.to_string())
        .filter(|base| !base.is_empty())
}

fn normalize_version_separator(model: &str) -> Option<String> {
    let mut result = String::with_capacity(model.len());
    let chars: Vec<char> = model.chars().collect();
    let mut changed = false;

    for i in 0..chars.len() {
        if chars[i] == '-'
            && i > 0
            && i < chars.len() - 1
            && chars[i - 1].is_ascii_digit()
            && chars[i + 1].is_ascii_digit()
        {
            let multi_digit_before = i >= 2 && chars[i - 2].is_ascii_digit();
            let multi_digit_after = i + 2 < chars.len() && chars[i + 2].is_ascii_digit();
            if multi_digit_before || multi_digit_after {
                result.push(chars[i]);
            } else {
                result.push('.');
                changed = true;
            }
        } else {
            result.push(chars[i]);
        }
    }

    changed.then_some(result)
}

fn provider_prefixes(provider: &str, model: &str) -> Vec<String> {
    let lower_provider = provider.trim().to_lowercase();
    let lower_model = model.trim().to_lowercase();
    let mut prefixes = Vec::new();

    match lower_provider.as_str() {
        "anthropic" => push_unique(&mut prefixes, "anthropic".into()),
        "deepseek" => push_unique(&mut prefixes, "deepseek".into()),
        "google" | "vertex_ai" | "vertex ai" => push_unique(&mut prefixes, "google".into()),
        "meta" => push_unique(&mut prefixes, "meta-llama".into()),
        "mistral" | "mistralai" => push_unique(&mut prefixes, "mistralai".into()),
        "moonshot" | "moonshot ai" => push_unique(&mut prefixes, "moonshotai".into()),
        "openai" => push_unique(&mut prefixes, "openai".into()),
        "qwen" | "alibaba" => push_unique(&mut prefixes, "qwen".into()),
        _ => {}
    }

    if lower_model.contains("deepseek") {
        push_unique(&mut prefixes, "deepseek".into());
    }
    if lower_model.contains("kimi") || lower_model.contains("moonshot") {
        push_unique(&mut prefixes, "moonshotai".into());
    }
    if lower_model.contains("qwen") {
        push_unique(&mut prefixes, "qwen".into());
    }
    if lower_model.contains("claude") {
        push_unique(&mut prefixes, "anthropic".into());
    }
    if lower_model.contains("gemini") {
        push_unique(&mut prefixes, "google".into());
    }
    if lower_model.starts_with("gpt-")
        || lower_model.starts_with("o1")
        || lower_model.starts_with("o3")
        || lower_model.starts_with("o4")
    {
        push_unique(&mut prefixes, "openai".into());
    }

    prefixes
}

fn exact_lookup(
    candidate: &str,
    dataset: &PricingDataset,
    source: PricingSource,
) -> Option<LookupResult> {
    let key = dataset
        .keys()
        .find(|key| key.eq_ignore_ascii_case(candidate))?
        .clone();
    let pricing = dataset.get(&key)?.clone();
    has_any_price(&pricing).then_some(LookupResult {
        key,
        source,
        pricing,
    })
}

fn fuzzy_lookup(
    candidate: &str,
    dataset: &PricingDataset,
    source: PricingSource,
) -> Option<LookupResult> {
    let candidate = candidate.to_lowercase();
    if candidate.len() < 5 {
        return None;
    }

    let mut matches: Vec<&String> = dataset
        .keys()
        .filter(|key| {
            let lower = key.to_lowercase();
            lower.contains(&candidate)
                || lower
                    .split('/')
                    .next_back()
                    .map(|part| part == candidate || part.contains(&candidate))
                    .unwrap_or(false)
        })
        .collect();

    matches.sort_by_key(|key| key.len());
    let key = matches.into_iter().next()?.clone();
    let pricing = dataset.get(&key)?.clone();
    has_any_price(&pricing).then_some(LookupResult {
        key,
        source,
        pricing,
    })
}

fn compute_cost(pricing: &ModelPricing, tokens: CostTokens) -> f64 {
    let input = tokens.input.max(0) as f64;
    let output = tokens.output.max(0).saturating_add(tokens.reasoning.max(0)) as f64;
    let cache_read = tokens.cache_read.max(0) as f64;
    let cache_write = tokens.cache_write.max(0) as f64;

    tiered_cost(
        input,
        pricing.input_cost_per_token,
        &[
            (128_000.0, pricing.input_cost_per_token_above_128k_tokens),
            (200_000.0, pricing.input_cost_per_token_above_200k_tokens),
            (256_000.0, pricing.input_cost_per_token_above_256k_tokens),
            (272_000.0, pricing.input_cost_per_token_above_272k_tokens),
        ],
    ) + tiered_cost(
        output,
        pricing.output_cost_per_token,
        &[
            (128_000.0, pricing.output_cost_per_token_above_128k_tokens),
            (200_000.0, pricing.output_cost_per_token_above_200k_tokens),
            (256_000.0, pricing.output_cost_per_token_above_256k_tokens),
            (272_000.0, pricing.output_cost_per_token_above_272k_tokens),
        ],
    ) + tiered_cost(
        cache_read,
        pricing.cache_read_input_token_cost,
        &[
            (
                200_000.0,
                pricing.cache_read_input_token_cost_above_200k_tokens,
            ),
            (
                272_000.0,
                pricing.cache_read_input_token_cost_above_272k_tokens,
            ),
        ],
    ) + tiered_cost(
        cache_write,
        pricing.cache_creation_input_token_cost,
        &[(
            200_000.0,
            pricing.cache_creation_input_token_cost_above_200k_tokens,
        )],
    )
}

fn tiered_cost(tokens: f64, base: Option<f64>, tiers: &[(f64, Option<f64>)]) -> f64 {
    let mut cost = 0.0;
    let mut lower_bound = 0.0;
    let mut active_price = safe_price(base);

    for (threshold, tier_price) in tiers {
        let Some(tier_price) = tier_price.filter(|v| is_valid_price(*v)) else {
            continue;
        };
        if !threshold.is_finite() || *threshold <= lower_bound {
            continue;
        }
        if tokens <= *threshold {
            return cost + (tokens - lower_bound).max(0.0) * active_price;
        }
        cost += (*threshold - lower_bound) * active_price;
        lower_bound = *threshold;
        active_price = tier_price;
    }

    cost + (tokens - lower_bound).max(0.0) * active_price
}

fn fallback_cost(model: &str, tokens: CostTokens) -> f64 {
    let m = canonical_model(model);
    let (input_price, output_price, cache_read_price, cache_write_price) = match m.as_str() {
        m if m.starts_with("claude-opus-4-") => (5.0, 25.0, 0.5, 6.25),
        m if m.starts_with("claude-sonnet-4-") => (3.0, 15.0, 0.3, 3.75),
        m if m.starts_with("claude-haiku-4-5") => (1.0, 5.0, 0.1, 1.25),
        m if m.starts_with("claude-") => (3.0, 15.0, 0.3, 3.75),
        m if m.starts_with("gpt-5.5") => (5.0, 30.0, 0.5, 0.0),
        m if m.starts_with("gpt-5.4") => (2.5, 15.0, 0.25, 0.0),
        m if m.starts_with("gpt-5.3") => (1.75, 14.0, 0.175, 0.0),
        m if m.starts_with("gpt-5.2") => (1.75, 14.0, 0.175, 0.0),
        m if m.starts_with("gpt-5") => (1.25, 10.0, 0.125, 0.0),
        m if m.starts_with("gpt-4o") => (2.5, 10.0, 1.25, 0.0),
        m if m.starts_with("gpt-4") => (2.5, 10.0, 1.25, 0.0),
        m if m.starts_with("o1") => (15.0, 60.0, 7.5, 0.0),
        m if m.starts_with("o3") || m.starts_with("o4") => (10.0, 40.0, 2.5, 0.0),
        m if m.starts_with("gemini-3-flash") => (0.5, 3.0, 0.05, 0.0),
        m if m.starts_with("gemini-3") => (2.0, 12.0, 0.2, 0.0),
        m if m.starts_with("gemini-2.5-pro") => (1.25, 5.0, 0.0, 0.0),
        m if m.starts_with("gemini-2.5-flash") => (0.15, 0.6, 0.15, 0.0),
        m if m.starts_with("deepseek-v4-pro") => (0.435, 0.87, 0.003625, 0.0),
        m if m.starts_with("deepseek-chat") => (0.27, 1.10, 0.027, 0.0),
        m if m.starts_with("deepseek-reasoner") => (0.55, 2.19, 0.055, 0.0),
        m if m.starts_with("deepseek-") => (0.27, 1.10, 0.027, 0.0),
        "zai-org/glm-5" => (0.95, 3.15, 0.0, 0.0),
        "zai-org/glm-5:thinking" => (0.13, 0.85, 0.0, 0.0),
        m if m.contains("kimi") || m.contains("moonshot") => (0.5, 2.0, 0.4, 0.0),
        m if m.contains("qwen") => (0.25, 0.5, 0.0, 0.0),
        m if m.contains("mistral") => (0.2, 0.6, 0.0, 0.0),
        m if m.contains("llama") => (0.2, 0.6, 0.0, 0.0),
        m if m.contains("composer-2-fast") => (1.5, 7.5, 0.35, 0.0),
        m if m.contains("composer-2") => (0.5, 2.5, 0.2, 0.0),
        m if m.contains("composer-1.5") || m.contains("composer 1.5") => (3.5, 17.5, 0.35, 0.0),
        m if m.contains("composer-1") || m.contains("composer 1") => (1.25, 10.0, 0.125, 0.0),
        _ => (1.0, 5.0, 0.5, 0.0),
    };

    (tokens.input.max(0) as f64 / 1_000_000.0) * input_price
        + (tokens.output.max(0).saturating_add(tokens.reasoning.max(0)) as f64 / 1_000_000.0)
            * output_price
        + (tokens.cache_read.max(0) as f64 / 1_000_000.0) * cache_read_price
        + (tokens.cache_write.max(0) as f64 / 1_000_000.0) * cache_write_price
}

impl CostTokens {
    fn has_billable_tokens(self) -> bool {
        self.input > 0
            || self.output > 0
            || self.cache_read > 0
            || self.cache_write > 0
            || self.reasoning > 0
    }
}

fn has_any_price(pricing: &ModelPricing) -> bool {
    [
        pricing.input_cost_per_token,
        pricing.input_cost_per_token_above_128k_tokens,
        pricing.input_cost_per_token_above_200k_tokens,
        pricing.input_cost_per_token_above_256k_tokens,
        pricing.input_cost_per_token_above_272k_tokens,
        pricing.output_cost_per_token,
        pricing.output_cost_per_token_above_128k_tokens,
        pricing.output_cost_per_token_above_200k_tokens,
        pricing.output_cost_per_token_above_256k_tokens,
        pricing.output_cost_per_token_above_272k_tokens,
        pricing.cache_creation_input_token_cost,
        pricing.cache_creation_input_token_cost_above_200k_tokens,
        pricing.cache_read_input_token_cost,
        pricing.cache_read_input_token_cost_above_200k_tokens,
        pricing.cache_read_input_token_cost_above_272k_tokens,
    ]
    .into_iter()
    .flatten()
    .any(is_valid_price)
}

fn safe_price(value: Option<f64>) -> f64 {
    value.filter(|v| is_valid_price(*v)).unwrap_or(0.0)
}

fn is_valid_price(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

fn parse_price(raw: &str) -> Option<f64> {
    raw.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| is_valid_price(*v))
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !value.is_empty() && !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_dataset_price_overrides_fallback() {
        let mut litellm = PricingDataset::new();
        litellm.insert(
            "gpt-5.4".into(),
            ModelPricing {
                input_cost_per_token: Some(0.0000025),
                output_cost_per_token: Some(0.000015),
                cache_read_input_token_cost: Some(0.00000025),
                ..Default::default()
            },
        );
        let service = PricingService::new(litellm, PricingDataset::new());

        let cost = service.calculate_cost(
            "gpt-5.4",
            "OpenAI",
            CostTokens {
                input: 1_000_000,
                output: 100_000,
                cache_read: 1_000_000,
                ..Default::default()
            },
        );

        assert!((cost - 4.25).abs() < 1e-9);
    }

    #[test]
    fn openrouter_provider_prefix_matches_unprefixed_model() {
        let mut openrouter = PricingDataset::new();
        openrouter.insert(
            "deepseek/deepseek-v4-pro".into(),
            ModelPricing {
                input_cost_per_token: Some(0.000000435),
                output_cost_per_token: Some(0.00000087),
                cache_read_input_token_cost: Some(0.000000003625),
                ..Default::default()
            },
        );
        let service = PricingService::new(PricingDataset::new(), openrouter);

        let cost = service.calculate_cost(
            "deepseek-v4-pro",
            "DeepSeek",
            CostTokens {
                input: 1_000_000,
                output: 1_000_000,
                cache_read: 1_000_000,
                ..Default::default()
            },
        );

        assert!((cost - 1.308625).abs() < 1e-9);
    }

    #[test]
    fn colon_variant_alias_is_not_priced_as_base_model() {
        let mut litellm = PricingDataset::new();
        litellm.insert(
            "baseten/zai-org/GLM-5".into(),
            ModelPricing {
                input_cost_per_token: Some(0.00000095),
                output_cost_per_token: Some(0.00000315),
                ..Default::default()
            },
        );
        litellm.insert(
            "novita/zai-org/glm-4.5-air".into(),
            ModelPricing {
                input_cost_per_token: Some(0.00000013),
                output_cost_per_token: Some(0.00000085),
                ..Default::default()
            },
        );
        let service = PricingService::new(litellm, PricingDataset::new());

        let cost = service.calculate_cost(
            "zai-org/glm-5:thinking",
            "nano-gpt",
            CostTokens {
                input: 1_000_000,
                ..Default::default()
            },
        );

        assert!((cost - 0.13).abs() < 1e-9);
    }

    #[test]
    fn reasoning_is_priced_as_output() {
        let pricing = ModelPricing {
            output_cost_per_token: Some(0.00001),
            ..Default::default()
        };

        let cost = compute_cost(
            &pricing,
            CostTokens {
                output: 100,
                reasoning: 50,
                ..Default::default()
            },
        );

        assert!((cost - 0.0015).abs() < 1e-12);
    }

    #[test]
    fn offline_fallback_uses_updated_opus_rates() {
        let cost = fallback_cost(
            "claude-opus-4-6",
            CostTokens {
                input: 1_000_000,
                output: 1_000_000,
                cache_read: 1_000_000,
                cache_write: 1_000_000,
                ..Default::default()
            },
        );

        assert!((cost - 36.75).abs() < 1e-9);
    }
}
