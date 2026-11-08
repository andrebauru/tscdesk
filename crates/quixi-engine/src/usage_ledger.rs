//! Per-session request accounting for provider usage.
//!
//! Only accounting metadata is stored: model IDs, token counts, reported or
//! estimated cost, latency and errors. Prompts, responses and credentials are
//! never written here.

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

const MAX_REQUESTS_PER_SESSION: usize = 200;
const MAX_SESSIONS: usize = 500;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestUsage {
    pub provider: String,
    pub requested_model: String,
    pub effective_model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    /// Cost reported by the provider API, when available.
    pub reported_cost_usd: Option<f64>,
    /// Local estimate from catalog prices, clearly separated from reported cost.
    pub estimated_cost_usd: Option<f64>,
    pub duration_ms: u64,
    pub attempts: u32,
    pub error: Option<String>,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    pub requests: Vec<RequestUsage>,
    pub total_requests: u64,
    pub total_errors: u64,
    pub total_attempts: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_reported_cost_usd: f64,
    pub total_estimated_cost_usd: f64,
    pub total_duration_ms: u64,
    pub last_request: Option<RequestUsage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LedgerFile {
    version: u32,
    sessions: HashMap<String, SessionUsage>,
}

pub struct UsageLedger {
    path: PathBuf,
    state: RwLock<LedgerFile>,
    write_lock: Mutex<()>,
}

impl UsageLedger {
    pub fn new(data_dir: &PathBuf) -> Self {
        let path = data_dir.join("provider-usage.json");
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<LedgerFile>(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            state: RwLock::new(state),
            write_lock: Mutex::new(()),
        }
    }

    fn persist(&self) {
        let _guard = self.write_lock.lock();
        let snapshot = self.state.read().clone();
        if let Ok(bytes) = serde_json::to_vec_pretty(&snapshot) {
            let _ = crate::storage::atomic_write(&self.path, &bytes);
        }
    }

    pub fn record(&self, session_id: &str, request: RequestUsage) {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return;
        }
        {
            let mut state = self.state.write();
            let session = state.sessions.entry(session_id.to_string()).or_default();
            session.total_requests = session.total_requests.saturating_add(1);
            if request.error.is_some() {
                session.total_errors = session.total_errors.saturating_add(1);
            }
            session.total_attempts = session
                .total_attempts
                .saturating_add(request.attempts.max(1) as u64);
            session.total_input_tokens = session
                .total_input_tokens
                .saturating_add(request.input_tokens);
            session.total_output_tokens = session
                .total_output_tokens
                .saturating_add(request.output_tokens);
            session.total_reported_cost_usd += request.reported_cost_usd.unwrap_or(0.0);
            session.total_estimated_cost_usd += request.estimated_cost_usd.unwrap_or(0.0);
            session.total_duration_ms = session.total_duration_ms.saturating_add(request.duration_ms);
            session.last_request = Some(request.clone());
            session.requests.push(request);
            if session.requests.len() > MAX_REQUESTS_PER_SESSION {
                let overflow = session.requests.len() - MAX_REQUESTS_PER_SESSION;
                session.requests.drain(0..overflow);
            }
            if state.sessions.len() > MAX_SESSIONS {
                let mut oldest: Vec<(String, u64)> = state
                    .sessions
                    .iter()
                    .map(|(id, usage)| {
                        (
                            id.clone(),
                            usage
                                .last_request
                                .as_ref()
                                .map(|request| request.timestamp_ms)
                                .unwrap_or(0),
                        )
                    })
                    .collect();
                oldest.sort_by_key(|(_, timestamp)| *timestamp);
                for (id, _) in oldest.into_iter().take(state.sessions.len() - MAX_SESSIONS) {
                    state.sessions.remove(&id);
                }
            }
        }
        self.persist();
    }

    pub fn snapshot(&self, session_id: &str) -> Value {
        let state = self.state.read();
        match state.sessions.get(session_id.trim()) {
            Some(usage) => json!({
                "ok": true,
                "sessionId": session_id.trim(),
                "totalRequests": usage.total_requests,
                "totalErrors": usage.total_errors,
                "totalAttempts": usage.total_attempts,
                "inputTokens": usage.total_input_tokens,
                "outputTokens": usage.total_output_tokens,
                "reportedCostUsd": usage.total_reported_cost_usd,
                "estimatedCostUsd": usage.total_estimated_cost_usd,
                "durationMs": usage.total_duration_ms,
                "lastRequest": usage.last_request,
                "requests": usage.requests,
            }),
            None => json!({
                "ok": true,
                "sessionId": session_id.trim(),
                "totalRequests": 0,
                "totalErrors": 0,
                "totalAttempts": 0,
                "inputTokens": 0,
                "outputTokens": 0,
                "reportedCostUsd": 0.0,
                "estimatedCostUsd": 0.0,
                "durationMs": 0,
                "lastRequest": Value::Null,
                "requests": [],
            }),
        }
    }

    pub fn clear(&self, session_id: &str) {
        {
            let mut state = self.state.write();
            state.sessions.remove(session_id.trim());
        }
        self.persist();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(model: &str, input: u64, output: u64, cost: Option<f64>, error: Option<&str>) -> RequestUsage {
        RequestUsage {
            provider: "openrouter".into(),
            requested_model: model.into(),
            effective_model: Some(model.into()),
            input_tokens: input,
            output_tokens: output,
            reported_cost_usd: cost,
            estimated_cost_usd: cost.map(|value| value / 2.0),
            duration_ms: 1200,
            attempts: 2,
            error: error.map(str::to_string),
            timestamp_ms: 42,
            ..Default::default()
        }
    }

    #[test]
    fn accumulates_reported_and_estimated_costs_separately_and_survives_restart() {
        let dir = std::env::temp_dir().join(format!("usage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let ledger = UsageLedger::new(&dir);
        ledger.record("s1", request("openrouter/auto", 100, 50, Some(0.25), None));
        ledger.record("s1", request("acme/model", 200, 100, Some(0.5), Some("HTTP 500")));
        let snapshot = ledger.snapshot("s1");
        assert_eq!(snapshot["totalRequests"], 2);
        assert_eq!(snapshot["totalErrors"], 1);
        assert_eq!(snapshot["totalAttempts"], 4);
        assert_eq!(snapshot["inputTokens"], 300);
        assert_eq!(snapshot["outputTokens"], 150);
        assert_eq!(snapshot["reportedCostUsd"], 0.75);
        assert_eq!(snapshot["estimatedCostUsd"], 0.375);
        assert_eq!(snapshot["lastRequest"]["error"], "HTTP 500");
        drop(ledger);

        let reopened = UsageLedger::new(&dir);
        assert_eq!(reopened.snapshot("s1")["totalRequests"], 2);
        assert_eq!(reopened.snapshot("missing")["totalRequests"], 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn request_history_is_bounded_without_losing_totals() {
        let dir = std::env::temp_dir().join(format!("usage-cap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let ledger = UsageLedger::new(&dir);
        for index in 0..(MAX_REQUESTS_PER_SESSION + 10) {
            ledger.record("s", request(&format!("model-{index}"), 1, 1, None, None));
        }
        let snapshot = ledger.snapshot("s");
        assert_eq!(snapshot["totalRequests"], (MAX_REQUESTS_PER_SESSION + 10) as u64);
        assert_eq!(snapshot["requests"].as_array().unwrap().len(), MAX_REQUESTS_PER_SESSION);
        assert_eq!(snapshot["inputTokens"], (MAX_REQUESTS_PER_SESSION + 10) as u64);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
