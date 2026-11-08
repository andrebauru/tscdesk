//! Dynamic OpenRouter model catalog.
//!
//! The catalog is public metadata: it never contains credentials. It is cached
//! locally so opening the model picker does not require a network round trip on
//! every render. A manual refresh always bypasses the cache.

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
pub const AUTO_ROUTER_ID: &str = "openrouter/auto";
pub const AUTO_CODE_ROUTER_ID: &str = "openrouter/auto-code";
pub const AUTO_BETA_ROUTER_ID: &str = "openrouter/auto-beta";
pub const CACHE_TTL_MS: u64 = 12 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterModel {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub context_length: u64,
    /// Price in USD per one million input tokens. `None` means dynamic/unknown.
    pub prompt_price_per_million: Option<f64>,
    /// Price in USD per one million output tokens. `None` means dynamic/unknown.
    pub completion_price_per_million: Option<f64>,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub is_free: bool,
    pub is_router: bool,
    pub canonical_slug: String,
    pub created: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelHealth {
    pub successes: u64,
    pub failures: u64,
    pub consecutive_failures: u64,
    pub last_failure_ms: Option<u64>,
    pub last_success_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogFile {
    version: u32,
    fetched_at_ms: u64,
    source: String,
    models: Vec<OpenRouterModel>,
    health: HashMap<String, ModelHealth>,
}

#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub struct CatalogFilter {
    pub query: String,
    pub vendor: String,
    pub max_prompt_price: Option<f64>,
    pub max_completion_price: Option<f64>,
    pub min_context: u64,
    pub tools_only: bool,
    pub free_only: bool,
}

pub struct OpenRouterCatalog {
    path: PathBuf,
    state: RwLock<CatalogFile>,
    fetch_lock: Mutex<()>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn price_per_million(value: Option<&Value>) -> Option<f64> {
    let raw = match value? {
        Value::String(text) => text.trim().parse::<f64>().ok()?,
        Value::Number(number) => number.as_f64()?,
        _ => return None,
    };
    if !raw.is_finite() || raw < 0.0 {
        return None;
    }
    Some((raw * 1_000_000.0 * 1_000_000.0).round() / 1_000_000.0)
}

fn string_list(value: Option<&Value>) -> HashSet<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|item| item.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_model(item: &Value) -> Option<OpenRouterModel> {
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty() && !id.chars().any(char::is_control))?;
    let pricing = item.get("pricing");
    let prompt = price_per_million(pricing.and_then(|p| p.get("prompt")));
    let completion = price_per_million(pricing.and_then(|p| p.get("completion")));
    let supported = string_list(item.get("supported_parameters"));
    let input_modalities = string_list(
        item.pointer("/architecture/input_modalities"),
    );
    let modality = item
        .pointer("/architecture/modality")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let context_length = item
        .get("context_length")
        .and_then(Value::as_u64)
        .or_else(|| item.pointer("/top_provider/context_length").and_then(Value::as_u64))
        .unwrap_or(0);
    let vendor = id.split('/').next().unwrap_or("").to_string();
    let is_router = id.starts_with("openrouter/auto");
    let is_free = !is_router
        && prompt.is_some_and(|value| value == 0.0)
        && completion.is_some_and(|value| value == 0.0);
    Some(OpenRouterModel {
        id: id.to_string(),
        name: item
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(id)
            .to_string(),
        vendor,
        context_length,
        prompt_price_per_million: prompt,
        completion_price_per_million: completion,
        supports_tools: supported.contains("tools"),
        supports_vision: input_modalities.contains("image") || modality.contains("image"),
        is_free,
        is_router,
        canonical_slug: item
            .get("canonical_slug")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        created: item.get("created").and_then(Value::as_u64).unwrap_or(0),
    })
}

pub fn parse_catalog(value: &Value) -> Vec<OpenRouterModel> {
    let Some(items) = value.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for item in items {
        if let Some(model) = parse_model(item) {
            if seen.insert(model.id.clone()) {
                models.push(model);
            }
        }
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models
}

#[cfg(test)]
fn price_within(price: Option<f64>, limit: Option<f64>) -> bool {
    match limit {
        None => true,
        Some(max) => price.is_some_and(|value| value <= max + f64::EPSILON),
    }
}

#[cfg(test)]
pub fn filter_models(models: &[OpenRouterModel], filter: &CatalogFilter) -> Vec<OpenRouterModel> {
    let query = filter.query.trim().to_ascii_lowercase();
    let vendor = filter.vendor.trim().to_ascii_lowercase();
    models
        .iter()
        .filter(|model| {
            if !query.is_empty()
                && !model.id.to_ascii_lowercase().contains(&query)
                && !model.name.to_ascii_lowercase().contains(&query)
            {
                return false;
            }
            if !vendor.is_empty() && model.vendor.to_ascii_lowercase() != vendor {
                return false;
            }
            if filter.min_context > 0 && model.context_length < filter.min_context {
                return false;
            }
            if filter.tools_only && !model.supports_tools {
                return false;
            }
            if filter.free_only && !model.is_free {
                return false;
            }
            if !price_within(model.prompt_price_per_million, filter.max_prompt_price) {
                return false;
            }
            if !price_within(
                model.completion_price_per_million,
                filter.max_completion_price,
            ) {
                return false;
            }
            true
        })
        .cloned()
        .collect()
}

fn coding_affinity(model: &OpenRouterModel) -> f64 {
    let haystack = format!("{} {}", model.id, model.name).to_ascii_lowercase();
    let coding = [
        "coder", "code", "devstral", "swe", "programming", "software", "agent",
    ];
    if coding.iter().any(|needle| haystack.contains(needle)) {
        0.85
    } else {
        1.15
    }
}

fn health_penalty(health: Option<&ModelHealth>) -> f64 {
    let Some(health) = health else {
        return 1.0;
    };
    let consecutive = health.consecutive_failures.min(5) as f64;
    let historical = (health.failures.min(20) as f64) * 0.15;
    let reliability = (health.successes.min(20) as f64) * 0.02;
    (1.0 + consecutive * 2.0 + historical - reliability).clamp(1.0, 20.0)
}

impl OpenRouterCatalog {
    pub fn new(data_dir: &PathBuf) -> Self {
        let path = data_dir.join("openrouter-catalog.json");
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<CatalogFile>(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            state: RwLock::new(state),
            fetch_lock: Mutex::new(()),
        }
    }

    fn persist(&self) {
        let snapshot = self.state.read().clone();
        if let Ok(bytes) = serde_json::to_vec_pretty(&snapshot) {
            let _ = crate::storage::atomic_write(&self.path, &bytes);
        }
    }

    pub fn models(&self) -> Vec<OpenRouterModel> {
        self.state.read().models.clone()
    }

    pub fn model(&self, id: &str) -> Option<OpenRouterModel> {
        self.state
            .read()
            .models
            .iter()
            .find(|model| model.id == id)
            .cloned()
    }

    pub fn is_known_router(&self, id: &str) -> bool {
        matches!(
            id,
            AUTO_ROUTER_ID | AUTO_CODE_ROUTER_ID | AUTO_BETA_ROUTER_ID
        ) || self.model(id).is_some_and(|model| model.is_router)
    }

    pub fn routers(&self) -> Vec<OpenRouterModel> {
        let state = self.state.read();
        let mut routers: Vec<OpenRouterModel> = state
            .models
            .iter()
            .filter(|model| model.is_router)
            .cloned()
            .collect();
        if routers.is_empty() {
            // The catalog may not be available yet. These three slugs are the
            // official Auto Router family documented by OpenRouter.
            routers = [AUTO_ROUTER_ID, AUTO_CODE_ROUTER_ID, AUTO_BETA_ROUTER_ID]
                .iter()
                .map(|id| OpenRouterModel {
                    id: (*id).to_string(),
                    name: match *id {
                        AUTO_CODE_ROUTER_ID => "Auto Code",
                        AUTO_BETA_ROUTER_ID => "Auto Router (Beta)",
                        _ => "Auto Router",
                    }
                    .to_string(),
                    vendor: "openrouter".to_string(),
                    context_length: 2_000_000,
                    is_router: true,
                    ..Default::default()
                })
                .collect();
        }
        routers.sort_by(|a, b| a.id.cmp(&b.id));
        routers
    }

    pub fn is_stale(&self) -> bool {
        let state = self.state.read();
        state.models.is_empty()
            || state.fetched_at_ms == 0
            || now_ms().saturating_sub(state.fetched_at_ms) > CACHE_TTL_MS
    }

    pub fn snapshot(&self, refresh: bool, api_key: Option<&str>) -> Value {
        let mut fetch_error: Option<String> = None;
        let stale_before = self.is_stale();
        let mut fetched = false;
        if refresh || stale_before {
            match self.refresh(MODELS_URL, api_key) {
                Ok(()) => fetched = true,
                Err(error) => fetch_error = Some(error),
            }
        }
        let state = self.state.read();
        let (models, fetched_at_ms, source) =
            (state.models.clone(), state.fetched_at_ms, state.source.clone());
        drop(state);
        let stale = models.is_empty()
            || fetched_at_ms == 0
            || now_ms().saturating_sub(fetched_at_ms) > CACHE_TTL_MS;
        let count = models.len();
        json!({
            "ok": !models.is_empty(),
            "models": models,
            "routers": self.routers(),
            "count": count,
            "fetchedAt": fetched_at_ms,
            "source": if source.is_empty() { MODELS_URL } else { source.as_str() },
            "cached": !fetched,
            "stale": stale,
            "error": fetch_error,
        })
    }

    pub fn refresh(&self, url: &str, api_key: Option<&str>) -> Result<(), String> {
        let _guard = self.fetch_lock.lock();
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| format!("Could not create the catalog HTTP client: {error}"))?;
        let mut request = client
            .get(url)
            .header("User-Agent", concat!("quixi-desk/", env!("CARGO_PKG_VERSION")))
            .header("HTTP-Referer", "https://quixidesk.local")
            .header("X-Title", "QuixiDesk");
        if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
            request = request.header("Authorization", format!("Bearer {key}"));
        }
        let response = request
            .send()
            .map_err(|error| format!("Could not reach the OpenRouter model catalog: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "OpenRouter model catalog returned HTTP {}.",
                status.as_u16()
            ));
        }
        let body: Value = response
            .json()
            .map_err(|error| format!("OpenRouter returned an invalid model catalog: {error}"))?;
        let models = parse_catalog(&body);
        if models.is_empty() {
            return Err("OpenRouter returned an empty or invalid model catalog.".to_string());
        }
        {
            let mut state = self.state.write();
            let health = state.health.clone();
            *state = CatalogFile {
                version: 1,
                fetched_at_ms: now_ms(),
                source: url.to_string(),
                models,
                health,
            };
        }
        self.persist();
        Ok(())
    }

    pub fn record_health(&self, model_id: &str, ok: bool) {
        if model_id.trim().is_empty() {
            return;
        }
        let mut state = self.state.write();
        let health = state.health.entry(model_id.to_string()).or_default();
        if ok {
            health.successes = health.successes.saturating_add(1);
            health.consecutive_failures = 0;
            health.last_success_ms = Some(now_ms());
        } else {
            health.failures = health.failures.saturating_add(1);
            health.consecutive_failures = health.consecutive_failures.saturating_add(1);
            health.last_failure_ms = Some(now_ms());
        }
        drop(state);
        self.persist();
    }

    pub fn estimate_cost(&self, model_id: &str, input_tokens: u64, output_tokens: u64) -> Option<f64> {
        let model = self.model(model_id)?;
        let prompt = model.prompt_price_per_million?;
        let completion = model.completion_price_per_million?;
        let cost = (input_tokens as f64 * prompt + output_tokens as f64 * completion) / 1_000_000.0;
        cost.is_finite().then_some((cost * 1_000_000.0).round() / 1_000_000.0)
    }

    /// Chooses a concrete model for economic mode. Price is only one factor:
    /// tool support, context window, coding affinity and recent failures all
    /// participate. This is a cost heuristic, not a quality guarantee.
    pub fn economic_pick(
        &self,
        needs_tools: bool,
        min_context: u64,
        expected_input: u64,
        expected_output: u64,
    ) -> Option<OpenRouterModel> {
        let state = self.state.read();
        let candidates: Vec<&OpenRouterModel> = state
            .models
            .iter()
            .filter(|model| !model.is_router)
            .filter(|model| !needs_tools || model.supports_tools)
            .filter(|model| model.context_length >= min_context)
            .filter(|model| {
                model
                    .prompt_price_per_million
                    .zip(model.completion_price_per_million)
                    .is_some_and(|(prompt, completion)| prompt >= 0.0 && completion >= 0.0)
            })
            .collect();
        let mut best: Option<(f64, &OpenRouterModel)> = None;
        for model in candidates {
            let prompt = model.prompt_price_per_million.unwrap_or(0.0);
            let completion = model.completion_price_per_million.unwrap_or(0.0);
            let base = (expected_input as f64 * prompt + expected_output as f64 * completion)
                / 1_000_000.0;
            let penalty = health_penalty(state.health.get(&model.id));
            let score = base * penalty * coding_affinity(model) + 0.0005 * (penalty - 1.0);
            let replace = best.is_none_or(|(best_score, best_model)| {
                score < best_score - f64::EPSILON
                    || ((score - best_score).abs() <= f64::EPSILON
                        && (model.context_length, &model.id)
                            > (best_model.context_length, &best_model.id))
            });
            if replace {
                best = Some((score, model));
            }
        }
        best.map(|(_, model)| model.clone())
    }

    pub fn context_window(&self, model_id: &str) -> Option<u32> {
        self.model(model_id)
            .map(|model| model.context_length)
            .filter(|length| *length > 0)
            .map(|length| length.min(u32::MAX as u64) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn fixture() -> Value {
        json!({
            "data": [
                {
                    "id": "openrouter/auto",
                    "name": "Auto Router",
                    "context_length": 2_000_000,
                    "pricing": {"prompt": "-1", "completion": "-1"},
                    "supported_parameters": ["tools"],
                    "architecture": {"input_modalities": ["text"], "modality": "text->text"}
                },
                {
                    "id": "openrouter/auto-code",
                    "name": "Auto Code",
                    "context_length": 2_000_000,
                    "pricing": {"prompt": "-1", "completion": "-1"},
                    "supported_parameters": ["tools"]
                },
                {
                    "id": "acme/coder-small",
                    "name": "Acme Coder Small",
                    "context_length": 131_072,
                    "pricing": {"prompt": "0.000001", "completion": "0.000002"},
                    "supported_parameters": ["tools", "temperature"],
                    "architecture": {"input_modalities": ["text"], "modality": "text->text"}
                },
                {
                    "id": "acme/plain-cheap",
                    "name": "Acme Plain Cheap",
                    "context_length": 32_768,
                    "pricing": {"prompt": "0.0000002", "completion": "0.0000002"},
                    "supported_parameters": []
                },
                {
                    "id": "beta/free-chat:free",
                    "name": "Beta Free Chat",
                    "context_length": 65_536,
                    "pricing": {"prompt": "0", "completion": "0"},
                    "supported_parameters": ["tools"],
                    "architecture": {"input_modalities": ["text", "image"], "modality": "text+image->text"}
                },
                {
                    "id": "bad/\u{0007}model",
                    "name": "Invalid"
                }
            ]
        })
    }

    fn catalog() -> Vec<OpenRouterModel> {
        parse_catalog(&fixture())
    }

    #[test]
    fn parses_prices_capabilities_free_models_and_official_routers() {
        let models = catalog();
        assert_eq!(models.len(), 5, "control characters must be rejected");
        let coder = models.iter().find(|m| m.id == "acme/coder-small").unwrap();
        assert_eq!(coder.vendor, "acme");
        assert_eq!(coder.prompt_price_per_million, Some(1.0));
        assert_eq!(coder.completion_price_per_million, Some(2.0));
        assert!(coder.supports_tools);
        assert!(!coder.is_free);
        let free = models.iter().find(|m| m.id == "beta/free-chat:free").unwrap();
        assert!(free.is_free);
        assert!(free.supports_vision);
        let routers: Vec<&str> = models
            .iter()
            .filter(|m| m.is_router)
            .map(|m| m.id.as_str())
            .collect();
        assert!(routers.contains(&AUTO_ROUTER_ID));
        assert!(routers.contains(&AUTO_CODE_ROUTER_ID));
        let auto = models.iter().find(|m| m.id == AUTO_ROUTER_ID).unwrap();
        assert_eq!(auto.prompt_price_per_million, None, "dynamic pricing is not zero");
        assert!(!auto.is_free);
    }

    #[test]
    fn search_and_every_filter_are_composable() {
        let models = catalog();
        let only_coder = filter_models(
            &models,
            &CatalogFilter {
                query: "coder".into(),
                ..Default::default()
            },
        );
        assert_eq!(only_coder.len(), 1);
        assert_eq!(only_coder[0].id, "acme/coder-small");

        let vendor = filter_models(
            &models,
            &CatalogFilter {
                vendor: "acme".into(),
                ..Default::default()
            },
        );
        assert_eq!(vendor.len(), 2);

        let tools = filter_models(
            &models,
            &CatalogFilter {
                tools_only: true,
                ..Default::default()
            },
        );
        assert!(tools.iter().all(|m| m.supports_tools));
        assert_eq!(tools.len(), 4);

        let free = filter_models(
            &models,
            &CatalogFilter {
                free_only: true,
                ..Default::default()
            },
        );
        assert_eq!(free.len(), 1);
        assert_eq!(free[0].id, "beta/free-chat:free");

        let context = filter_models(
            &models,
            &CatalogFilter {
                min_context: 100_000,
                ..Default::default()
            },
        );
        assert!(context.iter().all(|m| m.context_length >= 100_000));

        let price = filter_models(
            &models,
            &CatalogFilter {
                max_prompt_price: Some(0.5),
                max_completion_price: Some(0.5),
                ..Default::default()
            },
        );
        assert_eq!(price.len(), 2, "only the two sub-dollar models qualify");
        assert!(price.iter().all(|m| !m.is_router));
    }

    #[test]
    fn economic_mode_requires_capabilities_not_only_the_lowest_price() {
        let dir = std::env::temp_dir().join(format!("or-cat-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = OpenRouterCatalog::new(&dir);
        {
            let mut state = store.state.write();
            state.models = catalog();
            state.fetched_at_ms = now_ms();
        }
        // The plain model is cheaper but lacks tools; it must not be selected.
        let pick = store
            .economic_pick(true, 100_000, 8_000, 4_000)
            .expect("tool-capable candidate");
        assert_eq!(pick.id, "acme/coder-small");

        // Without the tool requirement the free model is the cheapest option.
        let plain = store
            .economic_pick(false, 16_000, 8_000, 4_000)
            .expect("plain candidate");
        assert_eq!(plain.id, "beta/free-chat:free");

        // Repeated failures add a risk premium, so the healthy paid model wins.
        for _ in 0..4 {
            store.record_health("beta/free-chat:free", false);
        }
        let after_failures = store
            .economic_pick(false, 16_000, 8_000, 4_000)
            .expect("candidate after failures");
        assert_eq!(after_failures.id, "acme/plain-cheap");

        // Context requirements are hard filters, never satisfied by truncation.
        assert!(store.economic_pick(true, 500_000, 1, 1).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cache_survives_restart_and_failed_refresh_preserves_last_catalog() {
        let dir = std::env::temp_dir().join(format!("or-cache-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/api/v1/models", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let count = connection.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..count]).starts_with("GET /api/v1/models "));
            let body = fixture().to_string();
            write!(connection, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        });
        let catalog = OpenRouterCatalog::new(&dir);
        catalog.refresh(&url, None).unwrap();
        server.join().unwrap();
        drop(catalog);

        let reopened = OpenRouterCatalog::new(&dir);
        assert_eq!(reopened.models().len(), 5);
        assert!(!reopened.is_stale());
        // A failed refresh must not erase the cached catalog.
        assert!(reopened.refresh("http://127.0.0.1:1/api/v1/models", None).is_err());
        assert_eq!(reopened.models().len(), 5);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn estimate_cost_uses_per_million_prices_and_never_guesses_for_routers() {
        let dir = std::env::temp_dir().join(format!("or-cost-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = OpenRouterCatalog::new(&dir);
        {
            let mut state = store.state.write();
            state.models = catalog();
        }
        assert_eq!(
            store.estimate_cost("acme/coder-small", 1_000_000, 500_000),
            Some(2.0)
        );
        assert_eq!(store.estimate_cost(AUTO_ROUTER_ID, 1000, 1000), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
