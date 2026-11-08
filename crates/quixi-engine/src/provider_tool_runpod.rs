//! RunPod queue adapter for SvenBrnn/runpod-worker-ollama (master, checked 2026-11-06).
//! POST /v2/{endpoint}/run, GET /status/{job}, POST /cancel/{job}; no SSE or tools.
//! utils.py forwards openai_route/openai_input; engine.py yields a completion object
//! when stream=false; handler.py uses return_aggregate_stream=true (an output array).
//! JSON-string completions are also accepted, but streaming deltas are not.
//! The selected model is forwarded unchanged; provisioning/pulling it is the worker's
//! responsibility. The upstream handler logs job input remotely; never log it here.
//!
//! Cancellation is irreversible for each AiRequest. A known job is cancelled once
//! on pause, timeout or interrupted polling. If /run is accepted but its response is
//! lost/invalid before a usable job ID is known, remote cancellation is impossible:
//! report that exception, do not resubmit, and let the operator inspect RunPod.

use super::{ProviderManager, ProviderTokenUsage, ProviderToolTurn, StreamChunk};
use crate::agent_control::{AgentControl, AiRequest};
use reqwest::blocking::{Client, Response};
use serde_json::{json, Value};
use std::io::Read;
use std::time::{Duration, Instant};

pub(super) const API_BASE: &str = "https://api.runpod.ai/v2";
const CANCELLED: &str = "RunPod request cancelled locally; no response or tools were applied.";
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

fn valid_identifier(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128
        && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

pub(super) fn validate_config(endpoint: &str, key: &str, model: &str) -> Result<(), String> {
    validate_auth(endpoint, key)?;
    if model.trim().is_empty() || model.chars().any(char::is_control) {
        return Err("Enter a nonempty worker model ID without control characters.".into());
    }
    Ok(())
}

fn validate_auth(endpoint: &str, key: &str) -> Result<(), String> {
    if !valid_identifier(endpoint) {
        return Err("Enter a RunPod endpoint ID (1–128 ASCII letters, digits, hyphens or underscores).".into());
    }
    if key.is_empty() || key.len() > 4096 || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("Enter a RunPod API key without spaces or control characters.".into());
    }
    Ok(())
}

// There is no public URL override. Even saved baseUrl is not used by this transport.
pub(super) struct Transport {
    base: String,
    timeout: Duration,
    poll_delays: [Duration; 4],
}

impl Transport {
    pub(super) fn production() -> Self {
        Self {
            base: API_BASE.into(),
            timeout: Duration::from_secs(600),
            poll_delays: [250, 500, 1000, 2000].map(Duration::from_millis),
        }
    }

    #[cfg(test)]
    pub(super) fn loopback(address: std::net::SocketAddr, timeout: Duration) -> Self {
        assert!(address.ip().is_loopback(), "Fixture transport is loopback only");
        Self {
            base: format!("http://{address}/v2"),
            timeout,
            poll_delays: [Duration::from_millis(5); 4],
        }
    }

    fn client(&self) -> Result<Client, String> {
        Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| "RunPod HTTP client could not be initialized.".into())
    }

    fn url(&self, endpoint: &str, route: &str) -> String {
        format!("{}/{endpoint}/{route}", self.base)
    }
}

// Never format reqwest/serde errors or untrusted response bodies: they may echo credentials.
fn response_json(response: Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err(format!("RunPod returned HTTP {}.", response.status().as_u16()));
    }
    let mut bytes = Vec::new();
    response.take(MAX_RESPONSE_BYTES + 1).read_to_end(&mut bytes)
        .map_err(|_| "RunPod response could not be read.".to_string())?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err("RunPod response exceeded the local size limit.".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "RunPod returned invalid JSON.".into())
}

fn payload(messages: &[Value], model: &str) -> Result<Value, String> {
    let mut wire = Vec::new();
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if !matches!(role, "system" | "user" | "assistant")
            || message.get("tool_calls").is_some_and(|v| !v.is_null() && v != &json!([]))
            || message.get("function_call").is_some_and(|v| !v.is_null())
            || message.get("tool_call_id").is_some()
            || message.get("_responses_output").is_some_and(|v| !v.is_null() && v != &json!([]))
        {
            return Err("RunPod text-only adapter cannot replay tool calls or tool results.".into());
        }
        let content = message.get("content").and_then(Value::as_str)
            .ok_or_else(|| "RunPod text-only adapter cannot send images or non-text message content.".to_string())?;
        wire.push(json!({"role":role,"content":content}));
    }
    if wire.is_empty() { return Err("RunPod requires text messages.".into()); }
    Ok(json!({"input":{
        "openai_route":"/v1/chat/completions",
        "openai_input":{"model":model,"messages":wire,"stream":false}
    }}))
}

fn parse_output(output: &Value, model: &str) -> Result<ProviderToolTurn, String> {
    let items = output.as_array().filter(|a| a.len() == 1)
        .ok_or_else(|| "RunPod worker returned an invalid aggregated completion.".to_string())?;
    let completion = match &items[0] {
        Value::String(text) => serde_json::from_str::<Value>(text)
            .map_err(|_| "RunPod worker returned invalid completion JSON.".to_string())?,
        Value::Object(_) => items[0].clone(),
        _ => return Err("RunPod worker returned an invalid completion format.".into()),
    };
    if completion.get("error").is_some() {
        return Err("RunPod worker reported an inference error.".into());
    }
    let choices = completion.get("choices").and_then(Value::as_array).filter(|a| a.len() == 1)
        .ok_or_else(|| "RunPod worker returned an invalid completion format.".to_string())?;
    let choice = &choices[0];
    let message = choice.get("message").filter(|m| m.get("role").and_then(Value::as_str) == Some("assistant"))
        .ok_or_else(|| "RunPod worker did not return a complete assistant message.".to_string())?;
    if message.get("tool_calls").is_some_and(|v| !v.is_null() && v != &json!([]))
        || message.get("function_call").is_some_and(|v| !v.is_null())
        || choice.get("finish_reason").and_then(Value::as_str) == Some("tool_calls")
    {
        return Err("RunPod worker requested unsupported actions; no tools were executed.".into());
    }
    if choice.get("finish_reason").and_then(Value::as_str) != Some("stop") {
        return Err("RunPod worker did not finish its complete response successfully.".into());
    }
    let text = message.get("content").and_then(Value::as_str).filter(|t| !t.trim().is_empty())
        .ok_or_else(|| "RunPod worker returned no complete text answer.".to_string())?;
    let usage = completion.get("usage").and_then(|u| Some(ProviderTokenUsage {
        input_tokens: u.get("prompt_tokens")?.as_u64()?,
        output_tokens: u.get("completion_tokens")?.as_u64()?,
        ..Default::default()
    }));
    Ok(ProviderToolTurn {
        response_items: Vec::new(), text: text.into(), thinking: String::new(),
        reasoning_details: None, tool_calls: Vec::new(), provider_name: "RunPod Serverless".into(),
        model_id: model.into(), requested_model: model.into(), effective_model: None,
        cost_usd: None, attempts: 1, stream_error: None, usage,
    })
}

// An armed job is cleaned up on every early return, once, with a bounded HTTP call.
struct RemoteJob<'a> {
    http: &'a Client,
    transport: &'a Transport,
    control: &'a AgentControl,
    endpoint: &'a str,
    key: &'a str,
    id: String,
    armed: bool,
}

impl RemoteJob<'_> {
    fn cancel(&mut self) {
        if !self.armed { return; }
        self.armed = false;
        let result = self.http.post(self.transport.url(self.endpoint, &format!("cancel/{}", self.id)))
            .bearer_auth(self.key).timeout(Duration::from_secs(15)).send();
        let confirmed = result.ok().and_then(|response| response_json(response).ok())
            .is_some_and(|value| value["id"].as_str() == Some(self.id.as_str())
                && value["status"].as_str() == Some("CANCELLED"));
        if !confirmed {
            self.control.report_cancel_error("RunPod remote cancellation could not be confirmed; inspect the endpoint's jobs in RunPod.");
        }
    }
}
impl Drop for RemoteJob<'_> {
    fn drop(&mut self) { self.cancel(); }
}

fn stopped(request: &AiRequest) -> Result<(), String> {
    if request.cancelled() { Err(CANCELLED.into()) } else { Ok(()) }
}

fn request_timeout(deadline: Instant) -> Result<Duration, String> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("RunPod job exceeded the local time limit.".into());
    }
    Ok(Duration::from_secs(15).min(remaining))
}

fn wait(request: &AiRequest, deadline: Instant, delay: Duration) -> Result<(), String> {
    let until = (Instant::now() + delay).min(deadline);
    while Instant::now() < until {
        stopped(request)?;
        std::thread::sleep(Duration::from_millis(25).min(until.saturating_duration_since(Instant::now())));
    }
    stopped(request)?;
    if Instant::now() >= deadline { return Err("RunPod job exceeded the local time limit.".into()); }
    Ok(())
}

fn infer<F: FnMut(StreamChunk)>(
    control: &AgentControl, transport: &Transport, endpoint: &str, key: &str,
    messages: &[Value], model: &str, on_chunk: &mut F,
) -> Result<ProviderToolTurn, String> {
    // Register before /run. start_request returns a local rejection when paused.
    let request = control.start_request().map_err(|_| CANCELLED.to_string())?;
    validate_config(endpoint, key, model)?;
    let body = payload(messages, model)?;
    let http = transport.client()?;
    let deadline = Instant::now() + transport.timeout;
    stopped(&request)?;
    let submission = http.post(transport.url(endpoint, "run")).bearer_auth(key)
        .timeout(request_timeout(deadline)?)
        .json(&body).send();
    let submitted = match submission {
        Ok(response) => {
            let success = response.status().is_success();
            match response_json(response) {
                Ok(value) => value,
                Err(error) => {
                    if success { control.report_cancel_error("RunPod submission returned no usable job ID; remote cancellation is unavailable. Inspect RunPod before submitting again."); }
                    stopped(&request)?;
                    return Err(error);
                }
            }
        }
        Err(_) => {
            control.report_cancel_error("RunPod submission outcome is unknown (no job ID); remote cancellation is unavailable. Inspect RunPod before submitting again.");
            stopped(&request)?;
            return Err("RunPod submission outcome is unknown; it was not submitted again.".into());
        }
    };
    let id = submitted.get("id").and_then(Value::as_str).filter(|id| valid_identifier(id));
    let Some(id) = id else {
        control.report_cancel_error("RunPod returned no usable job ID; remote cancellation is unavailable. Inspect RunPod before submitting again.");
        stopped(&request)?;
        return Err("RunPod submission returned an invalid job identifier.".into());
    };
    let mut job = RemoteJob { http: &http, transport, control, endpoint, key, id: id.into(), armed: true };
    let mut status = submitted;
    let mut poll = 0usize;
    loop {
        stopped(&request)?;
        if Instant::now() >= deadline { return Err("RunPod job exceeded the local time limit.".into()); }
        match status.get("status").and_then(Value::as_str) {
            Some("COMPLETED") => {
                job.armed = false;
                let mut turn = parse_output(&status["output"], model)?;
                // Even a successful, malicious upstream answer must not echo the HTTP credential.
                turn.text = turn.text.replace(key, "[credential redacted]");
                stopped(&request)?;
                // Exactly one complete answer, never incremental SSE or partial tool calls.
                on_chunk(StreamChunk::Text(turn.text.clone()));
                stopped(&request)?;
                return Ok(turn);
            }
            Some("FAILED" | "CANCELLED" | "TIMED_OUT") => {
                job.armed = false;
                return Err(match status["status"].as_str().unwrap() {
                    "FAILED" => "RunPod job failed; worker diagnostics are available in RunPod.",
                    "CANCELLED" => "RunPod job was cancelled; no response or tools were applied.",
                    _ => "RunPod job timed out remotely; no response or tools were applied.",
                }.into());
            }
            Some("IN_QUEUE" | "IN_PROGRESS") => {}
            _ => return Err("RunPod returned an invalid job status.".into()),
        }
        wait(&request, deadline, transport.poll_delays[poll.min(3)])?;
        poll = poll.saturating_add(1);
        stopped(&request)?;
        let response = http.get(transport.url(endpoint, &format!("status/{}", job.id)))
            .bearer_auth(key)
            .timeout(request_timeout(deadline)?)
            .send().map_err(|_| "RunPod job status could not be read.".to_string());
        // Cancellation wins over network/protocol errors, including a late final response.
        stopped(&request)?;
        let parsed = response_json(response?);
        stopped(&request)?;
        status = parsed?;
        if status.get("id").is_some_and(|value| value.as_str() != Some(job.id.as_str())) {
            return Err("RunPod returned a mismatched job identifier.".into());
        }
    }
}

pub(super) fn complete_runpod_tool_turn<F: FnMut(StreamChunk)>(
    control: &AgentControl, messages: &[Value], model: &str, key: &str,
    endpoint: &str, on_chunk: &mut F,
) -> Result<ProviderToolTurn, String> {
    infer(control, &Transport::production(), endpoint, key, messages, model, on_chunk)
}

fn health(control: &AgentControl, transport: &Transport, endpoint: &str, key: &str) -> Result<(), String> {
    let request = control.start_request().map_err(|_| CANCELLED.to_string())?;
    validate_auth(endpoint, key)?;
    let http = transport.client()?;
    stopped(&request)?;
    let response = http.get(transport.url(endpoint, "health")).bearer_auth(key).send()
        .map_err(|_| "RunPod health check could not connect.".to_string());
    stopped(&request)?;
    let value = response_json(response?)?;
    stopped(&request)?;
    if !value.get("jobs").is_some_and(Value::is_object) || !value.get("workers").is_some_and(Value::is_object) {
        return Err("RunPod returned an invalid health response.".into());
    }
    Ok(())
}

impl ProviderManager {
    pub(super) fn test_runpod_provider(&self, body: &Value, transport: &Transport) -> Value {
        let start = Instant::now();
        let (endpoint, key) = {
            let state = self.state.read();
            let entry = state.providers.iter().find(|p| p.id == "runpod-ollama").unwrap();
            let endpoint = body.get("endpointId").map(|v| v.as_str().unwrap_or(""))
                .unwrap_or(&entry.endpoint_id).to_string();
            let key = match body.get("apiKey") {
                Some(value) if value.as_str() == Some("") && !entry.api_key_raw.is_empty() => entry.api_key_raw.clone(),
                Some(value) if value.as_str() == Some(entry.api_key_masked.as_str()) => entry.api_key_raw.clone(),
                Some(value) => value.as_str().unwrap_or("").to_string(),
                None => entry.api_key_raw.clone(),
            };
            (endpoint, key)
        };
        let result = health(&self.ai_control, transport, &endpoint, &key);
        let ok = result.is_ok();
        let message = result.err().unwrap_or_else(|| "RunPod endpoint configuration and authentication verified. Health does not prove model readiness or inference; no GPU job was submitted.".into());
        json!({"ok":ok,"providerId":"runpod-ollama","latencyMs":start.elapsed().as_millis() as u64,
            "message":message,"discoveredModels":[],"inferenceVerified":false})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::sync::{Arc, Mutex};

    const KEY: &str = "synthetic-runpod-secret-DO-NOT-ECHO";
    const ENDPOINT: &str = "fixture_123";
    const MODEL: &str = "qwen2.5-coder:7b";

    fn messages() -> Vec<Value> { vec![json!({"role":"user","content":"fixture prompt"})] }
    fn completion() -> Value {
        json!({"choices":[{"message":{"role":"assistant","content":"complete answer"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":12,"completion_tokens":3}})
    }
    fn completed() -> Value { json!({"id":"job-1","status":"COMPLETED","output":[completion()]}) }

    struct Reply {
        route: &'static str,
        code: u16,
        body: String,
        before_reply: Option<Box<dyn FnOnce() + Send>>,
    }
    fn reply(route: &'static str, value: Value) -> Reply {
        Reply { route, code: 200, body: value.to_string(), before_reply: None }
    }
    type Requests = Arc<Mutex<Vec<(String, Value)>>>;
    fn fixture(replies: Vec<Reply>) -> (SocketAddr, Requests, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let server = std::thread::spawn(move || {
            for mut reply in replies {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                            std::thread::sleep(Duration::from_millis(5)),
                        Err(error) => panic!("Fixture request missing: {error}"),
                    }
                };
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request_line = line.trim().to_string();
                assert_eq!(request_line, reply.route);
                let mut content_length = 0usize;
                let mut authenticated = false;
                loop {
                    line.clear(); reader.read_line(&mut line).unwrap();
                    if line == "\r\n" { break; }
                    let header = line.trim();
                    if header.to_ascii_lowercase().starts_with("authorization:") {
                        authenticated = header.split_once(':').unwrap().1.trim() == format!("Bearer {KEY}");
                    }
                    if header.to_ascii_lowercase().starts_with("content-length:") {
                        content_length = header.split_once(':').unwrap().1.trim().parse().unwrap();
                    }
                }
                assert!(authenticated, "Fixture expected Bearer authentication");
                let mut body = vec![0; content_length];
                reader.read_exact(&mut body).unwrap();
                let value = if body.is_empty() { Value::Null } else { serde_json::from_slice(&body).unwrap() };
                captured.lock().unwrap().push((request_line, value));
                if let Some(hook) = reply.before_reply.take() { hook(); }
                let reason = if reply.code == 200 { "OK" } else { "Failure" };
                write!(stream, "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    reply.code, reason, reply.body.len(), reply.body).unwrap();
            }
        });
        (address, requests, server)
    }
    fn control() -> (Arc<AgentControl>, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        (Arc::new(AgentControl::new(root.path())), root)
    }
    fn run(control: &AgentControl, transport: &Transport, chunks: &mut Vec<StreamChunk>) -> Result<ProviderToolTurn, String> {
        infer(control, transport, ENDPOINT, KEY, &messages(), MODEL, &mut |c| chunks.push(c))
    }

    #[test]
    fn queue_run_status_completed_object_and_exact_payload() {
        let (address, requests, server) = fixture(vec![
            reply("POST /v2/fixture_123/run HTTP/1.1", json!({"id":"job-1","status":"IN_QUEUE"})),
            reply("GET /v2/fixture_123/status/job-1 HTTP/1.1", json!({"id":"job-1","status":"IN_PROGRESS"})),
            reply("GET /v2/fixture_123/status/job-1 HTTP/1.1", completed()),
        ]);
        let (control, _profile) = control();
        let mut chunks = Vec::new();
        let result = run(&control, &Transport::loopback(address, Duration::from_secs(2)), &mut chunks).unwrap();
        server.join().unwrap();
        assert_eq!(result.text, "complete answer");
        assert!(result.tool_calls.is_empty());
        assert!(result.stream_error.is_none());
        assert_eq!(result.usage.unwrap().input_tokens, 12);
        assert_eq!(chunks.len(), 1);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].1, json!({"input":{"openai_route":"/v1/chat/completions",
            "openai_input":{"model":MODEL,"messages":messages(),"stream":false}}}));
        assert!(!requests[0].1.to_string().contains("tools"));
    }

    #[test]
    fn successful_answer_cannot_echo_the_http_credential() {
        let mut value = completed();
        value["output"][0]["choices"][0]["message"]["content"] = json!(format!("answer {KEY}"));
        let (address, _, server) = fixture(vec![reply("POST /v2/fixture_123/run HTTP/1.1", value)]);
        let (control, _profile) = control();
        let mut chunks = Vec::new();
        let turn = run(&control, &Transport::loopback(address, Duration::from_secs(2)), &mut chunks).unwrap();
        assert!(!turn.text.contains(KEY));
        assert!(!format!("{chunks:?}").contains(KEY));
        server.join().unwrap();
    }

    #[test]
    fn aggregated_json_string_works_but_tools_streams_and_malformed_outputs_do_not() {
        assert_eq!(parse_output(&json!([completion().to_string()]), MODEL).unwrap().text, "complete answer");
        let mut tool = completion();
        tool["choices"][0]["message"]["tool_calls"] = json!([{"function":{"name":"Bash","arguments":"{}"}}]);
        let mut length = completion(); length["choices"][0]["finish_reason"] = json!("length");
        for output in [json!([tool]), json!([length]), completion(), json!([]), json!([completion(), completion()]),
            json!([{"error":KEY}]), json!(["bad json"]), json!(["data: [DONE]"]),
            json!([{"choices":[{"delta":{"content":"partial"},"finish_reason":"stop"}]}])]
        {
            let error = parse_output(&output, MODEL).unwrap_err();
            assert!(!error.contains(KEY));
        }
    }

    #[test]
    fn terminal_failed_cancelled_and_timed_out_never_resubmit_or_echo_secrets() {
        for status in ["FAILED", "CANCELLED", "TIMED_OUT"] {
            let (address, requests, server) = fixture(vec![
                reply("POST /v2/fixture_123/run HTTP/1.1", json!({"id":"job-1","status":"IN_QUEUE"})),
                reply("GET /v2/fixture_123/status/job-1 HTTP/1.1", json!({"id":"job-1","status":status,"error":KEY})),
            ]);
            let (control, _profile) = control();
            let mut chunks = Vec::new();
            let error = run(&control, &Transport::loopback(address, Duration::from_secs(2)), &mut chunks).unwrap_err();
            assert!(!error.contains(KEY)); assert!(chunks.is_empty());
            server.join().unwrap(); assert_eq!(requests.lock().unwrap().len(), 2);
        }
    }

    #[test]
    fn http_401_invalid_json_and_invalid_ids_are_safe_without_submission_retry() {
        for (code, body) in [(401, json!({"error":KEY}).to_string()), (200, format!("not JSON {KEY}")),
            (200, json!({"id":"../arbitrary-destination","status":"IN_QUEUE","error":KEY}).to_string())]
        {
            let (address, requests, server) = fixture(vec![Reply {
                route:"POST /v2/fixture_123/run HTTP/1.1", code, body, before_reply:None,
            }]);
            let (control, _profile) = control();
            let mut chunks = Vec::new();
            let error = run(&control, &Transport::loopback(address, Duration::from_secs(2)), &mut chunks).unwrap_err();
            assert!(!error.contains(KEY)); assert!(chunks.is_empty());
            server.join().unwrap(); assert_eq!(requests.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn completed_tool_call_is_refused_without_partial_text_or_execution() {
        let mut value=completed();
        value["output"][0]["choices"][0]["message"]["tool_calls"]=json!([{"function":{"name":"Bash","arguments":"{}"}}]);
        let (address, requests, server)=fixture(vec![reply("POST /v2/fixture_123/run HTTP/1.1",value)]);
        let (control, _profile)=control(); let mut chunks=Vec::new();
        let error=run(&control,&Transport::loopback(address,Duration::from_secs(2)),&mut chunks).unwrap_err();
        assert!(error.contains("no tools were executed")); assert!(chunks.is_empty());
        server.join().unwrap(); assert_eq!(requests.lock().unwrap().len(),1);
    }

    #[test]
    fn redirects_are_not_followed_even_to_another_loopback_destination() {
        let target=TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();
        let address=listener.local_addr().unwrap();
        let location=format!("http://{}/stolen",target.local_addr().unwrap());
        let server=std::thread::spawn(move || {
            let (mut stream,_)=listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader=BufReader::new(stream.try_clone().unwrap()); let mut line=String::new();
            let mut length=0usize;
            loop {
                line.clear(); reader.read_line(&mut line).unwrap(); if line=="\r\n" { break; }
                if line.to_ascii_lowercase().starts_with("content-length:") {
                    length=line.split_once(':').unwrap().1.trim().parse().unwrap();
                }
            }
            let mut body=vec![0;length]; reader.read_exact(&mut body).unwrap();
            write!(stream,"HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let (control,_profile)=control(); let mut chunks=Vec::new();
        assert!(run(&control,&Transport::loopback(address,Duration::from_secs(2)),&mut chunks).unwrap_err().contains("HTTP 302"));
        server.join().unwrap();
        assert_eq!(target.accept().unwrap_err().kind(),std::io::ErrorKind::WouldBlock);
        assert!(chunks.is_empty());
    }

    #[test]
    fn polling_errors_cancel_known_job_once_and_do_not_echo_untrusted_bodies() {
        for (code, body) in [(401, KEY.to_string()), (200, "{broken".to_string()),
            (200, json!({"id":"job-1","status":KEY}).to_string())]
        {
            let (address, requests, server) = fixture(vec![
                reply("POST /v2/fixture_123/run HTTP/1.1", json!({"id":"job-1","status":"IN_QUEUE"})),
                Reply { route:"GET /v2/fixture_123/status/job-1 HTTP/1.1", code, body, before_reply:None },
                reply("POST /v2/fixture_123/cancel/job-1 HTTP/1.1", json!({"id":"job-1","status":"CANCELLED"})),
            ]);
            let (control, _profile) = control(); let mut chunks = Vec::new();
            let error = run(&control, &Transport::loopback(address, Duration::from_secs(2)), &mut chunks).unwrap_err();
            assert!(!error.contains(KEY)); assert!(chunks.is_empty());
            server.join().unwrap(); assert_eq!(requests.lock().unwrap().len(), 3);
        }
    }

    #[test]
    fn pause_during_submission_then_resume_does_not_revive_old_request() {
        let (control, _profile) = control();
        let hook = control.clone();
        let mut submission = reply("POST /v2/fixture_123/run HTTP/1.1", json!({"id":"job-1","status":"IN_QUEUE"}));
        submission.before_reply = Some(Box::new(move || { hook.pause(); hook.resume(); }));
        let (address, requests, server) = fixture(vec![submission,
            reply("POST /v2/fixture_123/cancel/job-1 HTTP/1.1", json!({"id":"job-1","status":"CANCELLED"})),
        ]);
        let mut chunks = Vec::new();
        assert_eq!(run(&control, &Transport::loopback(address, Duration::from_secs(2)), &mut chunks).unwrap_err(), CANCELLED);
        assert!(!control.is_paused()); assert!(chunks.is_empty());
        server.join().unwrap(); assert_eq!(requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn pause_and_resume_before_late_final_response_never_emits_answer() {
        let (control, _profile) = control(); let hook = control.clone();
        let mut final_reply = reply("GET /v2/fixture_123/status/job-1 HTTP/1.1", completed());
        final_reply.before_reply = Some(Box::new(move || { hook.pause(); hook.resume(); }));
        let (address, requests, server) = fixture(vec![
            reply("POST /v2/fixture_123/run HTTP/1.1", json!({"id":"job-1","status":"IN_PROGRESS"})),
            final_reply,
            reply("POST /v2/fixture_123/cancel/job-1 HTTP/1.1", json!({"id":"job-1","status":"CANCELLED"})),
        ]);
        let mut chunks = Vec::new();
        assert_eq!(run(&control, &Transport::loopback(address, Duration::from_secs(2)), &mut chunks).unwrap_err(), CANCELLED);
        assert!(chunks.is_empty()); server.join().unwrap(); assert_eq!(requests.lock().unwrap().len(), 3);
    }

    #[test]
    fn local_timeout_cancels_known_job_once() {
        let (address, requests, server) = fixture(vec![
            reply("POST /v2/fixture_123/run HTTP/1.1", json!({"id":"job-1","status":"IN_QUEUE"})),
            reply("POST /v2/fixture_123/cancel/job-1 HTTP/1.1", json!({"id":"job-1","status":"CANCELLED"})),
        ]);
        let (control, _profile) = control(); let mut chunks = Vec::new();
        let mut transport = Transport::loopback(address, Duration::from_millis(500));
        transport.poll_delays = [Duration::from_secs(1); 4];
        assert!(run(&control, &transport, &mut chunks).unwrap_err().contains("local time limit"));
        assert!(chunks.is_empty()); server.join().unwrap(); assert_eq!(requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn cancellation_failure_is_reported_safely() {
        let (control, _profile) = control(); let hook = control.clone();
        let mut submission = reply("POST /v2/fixture_123/run HTTP/1.1", json!({"id":"job-1","status":"IN_QUEUE"}));
        submission.before_reply = Some(Box::new(move || { hook.pause(); }));
        let (address, _, server) = fixture(vec![submission, Reply {
            route:"POST /v2/fixture_123/cancel/job-1 HTTP/1.1", code:401, body:KEY.into(), before_reply:None,
        }]);
        let mut chunks = Vec::new();
        assert_eq!(run(&control, &Transport::loopback(address, Duration::from_secs(2)), &mut chunks).unwrap_err(), CANCELLED);
        server.join().unwrap();
        let state = control.state().to_string();
        assert!(!state.contains(KEY));
        assert!(state.contains("remote cancellation could not be confirmed"));
    }

    #[test]
    fn paused_inference_and_health_do_not_touch_network() {
        let (control, _profile) = control(); control.pause();
        let transport = Transport::loopback("127.0.0.1:9".parse().unwrap(), Duration::from_secs(1));
        let mut chunks = Vec::new();
        assert_eq!(run(&control, &transport, &mut chunks).unwrap_err(), CANCELLED);
        assert_eq!(health(&control, &transport, ENDPOINT, KEY).unwrap_err(), CANCELLED);
        assert!(chunks.is_empty());
    }

    #[test]
    fn text_only_rejects_tool_results_replay_and_images_instead_of_discarding_them() {
        for message in [json!({"role":"tool","tool_call_id":"call","content":"result"}),
            json!({"role":"assistant","content":"","tool_calls":[{}]}),
            json!({"role":"function","content":"result"}),
            json!({"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AA"}}]}),
            json!({"role":"assistant","content":"reply","_responses_output":[{"type":"function_call_output"}]})]
        { assert!(payload(&[message], MODEL).is_err()); }
    }

    #[test]
    fn health_draft_works_without_model_or_save_and_is_not_inference() {
        let (address, requests, server) = fixture(vec![
            reply("GET /v2/fixture_123/health HTTP/1.1", json!({"jobs":{"inQueue":0},"workers":{"idle":0}})),
        ]);
        let root = tempfile::tempdir().unwrap();
        let manager = ProviderManager::new(&root.path().to_path_buf());
        let before = std::fs::read(root.path().join("providers.json")).unwrap();
        let result = manager.test_runpod_provider(&json!({"endpointId":ENDPOINT,"apiKey":KEY,"baseUrl":"http://evil.invalid"}),
            &Transport::loopback(address, Duration::from_secs(2)));
        assert_eq!(result["ok"], true); assert_eq!(result["inferenceVerified"], false);
        assert_eq!(result["discoveredModels"], json!([]));
        assert!(!result.to_string().contains(KEY));
        assert_eq!(std::fs::read(root.path().join("providers.json")).unwrap(), before);
        assert!(!manager.state.read().providers.iter().find(|p| p.id == "runpod-ollama").unwrap().configured);
        server.join().unwrap(); assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn health_blank_password_reuses_saved_key_without_writing_it() {
        let (address, _, server) = fixture(vec![
            reply("GET /v2/fixture_123/health HTTP/1.1", json!({"jobs":{},"workers":{}})),
        ]);
        let root = tempfile::tempdir().unwrap();
        let manager = ProviderManager::new(&root.path().to_path_buf());
        assert!(manager.save_provider(&json!({"providerId":"runpod-ollama", "endpointId":ENDPOINT,
            "apiKey":KEY,"selectedModel":MODEL})).get("error").is_none());
        let before = std::fs::read(&manager.config_path).unwrap();
        let result = manager.test_runpod_provider(&json!({"apiKey":"","endpointId":ENDPOINT}),
            &Transport::loopback(address, Duration::from_secs(2)));
        assert_eq!(result["ok"], true);
        assert_eq!(std::fs::read(&manager.config_path).unwrap(), before);
        server.join().unwrap();
    }

    #[test]
    fn health_http_errors_and_invalid_format_are_not_success_and_never_echo_secrets() {
        for (code, body) in [(401, json!({"error":KEY}).to_string()), (200, "{broken".into()),
            (200, json!({"error":KEY}).to_string())]
        {
            let (address, _, server) = fixture(vec![Reply {
                route:"GET /v2/fixture_123/health HTTP/1.1", code, body, before_reply:None,
            }]);
            let (control, _profile) = control();
            let error = health(&control, &Transport::loopback(address, Duration::from_secs(2)), ENDPOINT, KEY).unwrap_err();
            assert!(!error.contains(KEY)); server.join().unwrap();
        }
    }
}
