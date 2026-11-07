//! Instance-wide AI admission and cancellation. No provider credentials belong here.
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak, atomic::{AtomicBool, Ordering}};

pub(crate) const PAUSED_MESSAGE: &str = "IA pausada. Envie INICIAR para permitir novas tarefas.";
const STORAGE_ERROR: &str = "Não foi possível salvar o controle da IA. Novas tarefas estão bloqueadas; tente INICIAR novamente.";

#[derive(Clone)]
pub(crate) struct AgentControl {
    shared: Arc<Shared>,
}
struct Shared {
    path: PathBuf,
    inner: Mutex<Inner>,
}
struct Inner {
    paused: bool,
    storage_error: bool,
    cancel_error: Option<String>,
    generation: u64,
    next_id: u64,
    requests: HashMap<u64, Weak<AtomicBool>>,
}

/// Registration lasts until Drop. A resumed instance never clears old cancellation tokens.
pub(crate) struct AiRequest {
    shared: Arc<Shared>,
    id: u64,
    signal: Arc<AtomicBool>,
}
thread_local! {
    static REQUEST_SCOPES: std::cell::RefCell<Vec<(usize, u64, Weak<AtomicBool>)>> = const { std::cell::RefCell::new(Vec::new()) };
}
/// Nested provider/compaction admission inherits the worker token, including after INICIAR.
/// This closes the check-to-call race without holding a mutex across remote I/O.
pub(crate) struct RequestScope {
    owner: usize,
    id: u64,
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl Drop for RequestScope {
    fn drop(&mut self) {
        REQUEST_SCOPES.with(|scopes| {
            let mut scopes = scopes.borrow_mut();
            if let Some(index) = scopes.iter().rposition(|(owner, id, _)| *owner == self.owner && *id == self.id) {
                scopes.remove(index);
            }
        });
    }
}
impl AiRequest {
    pub(crate) fn cancelled(&self) -> bool { self.signal.load(Ordering::SeqCst) }
    pub(crate) fn signal(&self) -> Arc<AtomicBool> { self.signal.clone() }
    pub(crate) fn enter(&self) -> RequestScope {
        let owner = Arc::as_ptr(&self.shared) as usize;
        REQUEST_SCOPES.with(|scopes| scopes.borrow_mut().push((owner, self.id, Arc::downgrade(&self.signal))));
        RequestScope { owner, id: self.id, _thread_bound: std::marker::PhantomData }
    }
}
impl Drop for AiRequest {
    fn drop(&mut self) { self.shared.inner.lock().requests.remove(&self.id); }
}

impl AgentControl {
    pub(crate) fn new(data_dir: &Path) -> Self {
        let path = data_dir.join("ai-control.json");
        let saved = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes).ok()
                .filter(|v| v["paused"].is_boolean()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(json!({"paused":false})),
            Err(_) => None,
        };
        let inner = Inner {
            paused: saved.as_ref().and_then(|v| v["paused"].as_bool()).unwrap_or(true),
            storage_error: saved.is_none(),
            cancel_error: None,
            generation: saved.as_ref().and_then(|v| v["generation"].as_u64()).unwrap_or(0),
            next_id: 0,
            requests: HashMap::new(),
        };
        Self { shared: Arc::new(Shared { path, inner: Mutex::new(inner) }) }
    }

    fn view(inner: &Inner) -> Value {
        let mut running = 0;
        let mut cancelling = 0;
        for signal in inner.requests.values().filter_map(Weak::upgrade) {
            if signal.load(Ordering::SeqCst) { cancelling += 1; } else { running += 1; }
        }
        let paused = inner.paused || inner.storage_error;
        let (status, message) = if inner.storage_error {
            ("cancel_error", Some(STORAGE_ERROR.to_string()))
        } else if let Some(error) = &inner.cancel_error {
            ("cancel_error", Some(error.clone()))
        } else if paused && cancelling > 0 {
            ("cancelling", Some("IA pausada; cancelamento solicitado. Aguardando operações em andamento terminarem.".into()))
        } else if paused {
            ("paused", Some(PAUSED_MESSAGE.into()))
        } else if running > 0 {
            ("processing", None)
        } else if cancelling > 0 {
            ("cancelling", Some("Aguardando operações canceladas terminarem; novas tarefas estão permitidas.".into()))
        } else { ("available", None) };
        let mut value = json!({"paused":paused,"status":status,"generation":inner.generation});
        if let Some(message) = message { value["message"] = json!(message); }
        value
    }
    pub(crate) fn state(&self) -> Value { Self::view(&self.shared.inner.lock()) }
    pub(crate) fn is_paused(&self) -> bool {
        let inner = self.shared.inner.lock();
        inner.paused || inner.storage_error
    }
    fn cancel_registered(inner: &Inner) {
        for signal in inner.requests.values().filter_map(Weak::upgrade) {
            signal.store(true, Ordering::SeqCst);
        }
    }
    fn persist(&self, inner: &mut Inner, paused: bool) {
        let generation = inner.generation.saturating_add(1);
        // Only permission and a revision are durable; remote errors are transient and not secrets.
        let bytes = serde_json::to_vec_pretty(&json!({"version":1,"paused":paused,"generation":generation})).expect("JSON serializes");
        match crate::storage::atomic_write(&self.shared.path, &bytes) {
            Ok(()) => {
                inner.paused = paused;
                inner.generation = generation;
                inner.storage_error = false;
                inner.cancel_error = None;
            }
            Err(_) => {
                // Never report a successful resume/persistence on write failure.
                inner.paused = true;
                inner.storage_error = true;
                Self::cancel_registered(inner);
            }
        }
    }
    pub(crate) fn pause(&self) -> Value {
        let mut inner = self.shared.inner.lock();
        inner.paused = true;
        Self::cancel_registered(&inner);
        self.persist(&mut inner, true);
        Self::view(&inner)
    }
    pub(crate) fn resume(&self) -> Value {
        let mut inner = self.shared.inner.lock();
        self.persist(&mut inner, false);
        Self::view(&inner)
    }
    pub(crate) fn start_request(&self) -> Result<AiRequest, String> {
        let mut inner = self.shared.inner.lock();
        if inner.paused || inner.storage_error {
            return Err(if inner.storage_error { STORAGE_ERROR } else { PAUSED_MESSAGE }.into());
        }
        let owner = Arc::as_ptr(&self.shared) as usize;
        if REQUEST_SCOPES.with(|scopes| scopes.borrow().iter().any(|(key, _, signal)| *key == owner && signal.upgrade().is_some_and(|s| s.load(Ordering::SeqCst)))) {
            return Err("Task stopped. A cancelled task cannot start another provider request after INICIAR.".into());
        }
        let id = inner.next_id;
        inner.next_id += 1;
        let signal = Arc::new(AtomicBool::new(false));
        inner.requests.insert(id, Arc::downgrade(&signal));
        Ok(AiRequest { shared: self.shared.clone(), id, signal })
    }
    /// Adapters supply sanitized text. Do not persist it or let a late old request cancel new work.
    pub(crate) fn report_cancel_error(&self, message: &str) {
        let safe: String = message.chars().filter(|c| !c.is_control()).take(300).collect();
        self.shared.inner.lock().cancel_error = Some(if safe.trim().is_empty() {
            "Não foi possível confirmar o cancelamento remoto.".into()
        } else { safe });
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LocalCommand { Pause, Resume }
pub(crate) fn local_command(prompt: &str, has_attachments: bool) -> Option<LocalCommand> {
    if has_attachments { return None; }
    let text = prompt.trim();
    if text.eq_ignore_ascii_case("FINAL") { Some(LocalCommand::Pause) }
    else if text.eq_ignore_ascii_case("INICIAR") { Some(LocalCommand::Resume) }
    else { None }
}
pub(crate) fn content_has_attachments(content: &[Value]) -> bool {
    content.iter().any(|block| block.get("attachment").is_some() || block["type"] != "text")
        || content.len() != 1
}

/// Cleans up only the worker that still owns the session's cancellation signal.
/// In particular, late completion/Drop from FINAL cannot mark a new turn idle after INICIAR.
pub(crate) struct ControlledTurn {
    engine: Weak<crate::QuixiEngine>,
    session: String,
    request: AiRequest,
}
impl ControlledTurn {
    pub(crate) fn signal(&self) -> Arc<AtomicBool> { self.request.signal() }
    pub(crate) fn enter(&self) -> RequestScope { self.request.enter() }
}
impl Drop for ControlledTurn {
    fn drop(&mut self) {
        let Some(engine) = self.engine.upgrade() else { return; };
        let mut turns = engine.turn_cancellations.write();
        if !turns.get(&self.session).is_some_and(|signal| Arc::ptr_eq(signal, &self.request.signal)) { return; }
        if self.request.cancelled() {
            if let Some(session) = engine.agent_sessions.write().get_mut(&self.session) {
                session.is_running = false;
                engine.persist_agent_session(session);
            }
            if let Some(session) = engine.code_sessions.write().get_mut(&self.session) {
                session.turn_running = false;
                engine.persist_code_session(session);
            }
            engine.active_turn_changes.lock().remove(&self.session);
        }
        turns.remove(&self.session);
    }
}

impl crate::QuixiEngine {
    pub fn agent_state(&self) -> Value { self.provider_manager.ai_control.state() }
    pub(crate) fn ai_stopped(&self, signal: &AtomicBool) -> bool {
        signal.load(Ordering::SeqCst) || self.provider_manager.ai_control.is_paused()
    }
    pub(crate) fn is_current_turn(&self, session: &str, signal: &AtomicBool) -> bool {
        self.turn_cancellations.read().get(session).is_some_and(|current| std::ptr::eq(current.as_ref(), signal))
    }
    pub(crate) fn begin_controlled_turn(self: &Arc<Self>, session: &str) -> Result<ControlledTurn, String> {
        let request = self.provider_manager.ai_control.start_request()?;
        self.begin_turn_with_signal(session, request.signal());
        Ok(ControlledTurn { engine: Arc::downgrade(self), session: session.into(), request })
    }
    pub fn set_agent_paused(&self, paused: bool) -> Value {
        let _submission = self.chat_submission.lock();
        let _admission = self.ai_submission.lock();
        let state = if paused { self.provider_manager.ai_control.pause() }
            else { self.provider_manager.ai_control.resume() };
        if state["paused"] == true {
            // Never stop or close the IDE's interactive terminals. Background agent jobs have
            // their own non-blocking cancellation flags and process-group cleanup worker.
            let ids: std::collections::HashSet<String> = self.turn_cancellations.read().keys().cloned()
                .chain(self.agent_sessions.read().keys().cloned())
                .chain(self.code_sessions.read().keys().cloned()).collect();
            for queue in self.pending_messages.lock().values_mut() { queue.clear(); }
            self.jobs.cancel_all();
            for id in &ids {
                self.collaboration.set_goal_armed(id, false);
                self.cancel_turn_signal(id);
            }
            for session in self.agent_sessions.write().values_mut() {
                session.is_running = false;
                session.pending_tool_permissions.clear();
                self.persist_agent_session(session);
            }
            for session in self.code_sessions.write().values_mut() {
                session.turn_running = false;
                self.persist_code_session(session);
            }
            // A paused schedule must require an explicit manual run/re-arm after INICIAR.
            for (id, task) in self.scheduled_tasks.write().iter_mut() {
                if task["enabled"] == true {
                    let mut next = task.clone();
                    next["enabled"] = json!(false);
                    next["lastRunStatus"] = json!("blocked");
                    next["lastRunMessage"] = json!(PAUSED_MESSAGE);
                    let path = self.data_dir.join("scheduled_tasks").join(format!("{id}.json"));
                    if crate::storage::atomic_write(&path, &serde_json::to_vec_pretty(&next).expect("JSON serializes")).is_err() {
                        self.provider_manager.ai_control.report_cancel_error("IA pausada, mas não foi possível salvar o bloqueio de uma tarefa agendada.");
                    }
                    *task = next;
                }
            }
            self.active_turn_changes.lock().clear();
            self.last_turn_usage.write().clear();
            for id in &ids {
                let service = if self.agent_sessions.read().contains_key(id) { "LocalAgentModeSessions" } else { "LocalSessions" };
                self.emit_eipc("quixi.web", service, "onEvent", json!({"type":"session_updated","sessionId":id}));
            }
        }
        let state = self.agent_state();
        self.emit_eipc("quixi.web", "Providers", "onAgentStateChanged", state.clone());
        state
    }
    /// Local notices are not user/assistant turns and are excluded from provider history.
    fn local_ai_notice(&self, session: &str, message: &str, state: &Value, command: Option<&str>) {
        let notice = json!({"type":"system","subtype":"agent_control","uuid":uuid::Uuid::new_v4().to_string(),
            "session_id":session,"timestamp":chrono::Utc::now().to_rfc3339(),"message":{"role":"system","content":[{"type":"text","text":message}]},
            "agentState":state,"localCommand":command});
        let service = if let Some(s) = self.agent_sessions.write().get_mut(session) {
            s.buffered_messages.push(notice.clone());
            self.persist_agent_session(s);
            "LocalAgentModeSessions"
        } else {
            if self.code_sessions.read().contains_key(session) {
                self.code_transcripts.write().entry(session.into()).or_default().push(notice.clone());
                if let Some(s) = self.code_sessions.read().get(session) { self.persist_code_session(s); }
            }
            "LocalSessions"
        };
        self.emit_eipc("quixi.web", service, "onEvent", json!({"type":"message","sessionId":session,"message":notice}));
        self.emit_eipc("quixi.web", service, "onEvent", json!({"type":"agent_control","sessionId":session,"agentState":state,"message":message}));
    }
    pub(crate) fn handle_local_ai_input(&self, session: &str, prompt: &str, attachments: bool) -> Option<Value> {
        if let Some(command) = local_command(prompt, attachments) {
            let paused = command == LocalCommand::Pause;
            let state = self.set_agent_paused(paused);
            let text = if state["status"] == "cancel_error" { state["message"].as_str().unwrap_or(STORAGE_ERROR) }
                else if paused { PAUSED_MESSAGE } else { "IA disponível. INICIAR permite novas tarefas; nenhuma tarefa foi retomada automaticamente." };
            self.local_ai_notice(session, text, &state, Some(if paused { "FINAL" } else { "INICIAR" }));
            return Some(json!({"dispatched":false,"localCommand":true,"agentState":state,"message":text}));
        }
        if self.provider_manager.ai_control.is_paused() {
            let state = self.agent_state();
            self.local_ai_notice(session, PAUSED_MESSAGE, &state, None);
            return Some(json!({"dispatched":false,"paused":true,"agentState":state,"message":PAUSED_MESSAGE}));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn directory() -> PathBuf {
        std::env::temp_dir().join(format!("quixi-ai-control-{}", uuid::Uuid::new_v4()))
    }
    #[test]
    fn isolated_commands_only() {
        for prompt in ["FINAL", " final \n", "\tFiNaL\r\n"] {
            assert_eq!(local_command(prompt, false), Some(LocalCommand::Pause));
        }
        for prompt in ["INICIAR", " iniciar ", "InIcIaR"] {
            assert_eq!(local_command(prompt, false), Some(LocalCommand::Resume));
        }
        for prompt in ["use FINAL", "FINAL now", "\"FINAL\"", "'INICIAR'", "`FINAL`", "```\nFINAL\n```", "FINAL\nINICIAR"] {
            assert_eq!(local_command(prompt, false), None);
        }
        assert_eq!(local_command("FINAL", true), None);
        assert_eq!(local_command("INICIAR", true), None);
    }
    #[test]
    fn permission_persists_and_old_requests_stay_cancelled() {
        let root = directory();
        let control = AgentControl::new(&root);
        assert_eq!(control.state()["status"], "available");
        let old = control.start_request().unwrap();
        assert_eq!(control.state()["status"], "processing");
        assert_eq!(control.pause()["status"], "cancelling");
        assert!(old.cancelled());
        assert!(control.start_request().is_err());
        assert!(AgentControl::new(&root).is_paused());
        control.resume();
        let new = control.start_request().unwrap();
        assert!(old.cancelled());
        assert!(!new.cancelled());
        assert!(!AgentControl::new(&root).is_paused());
        drop(old); drop(new);
        assert_eq!(control.state()["status"], "available");
        let disk: Value = serde_json::from_slice(&std::fs::read(root.join("ai-control.json")).unwrap()).unwrap();
        assert_eq!(disk.as_object().unwrap().len(), 3);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn failed_storage_and_corrupt_storage_fail_closed() {
        let root = directory();
        std::fs::create_dir_all(root.join("ai-control.json")).unwrap();
        let control = AgentControl::new(&root);
        assert!(control.is_paused());
        assert_eq!(control.resume()["status"], "cancel_error");
        assert!(control.start_request().is_err());
        std::fs::remove_dir(root.join("ai-control.json")).unwrap();
        std::fs::write(root.join("ai-control.json"), "invalid").unwrap();
        assert!(AgentControl::new(&root).is_paused());
        assert_eq!(control.resume()["status"], "available");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn pause_and_admission_are_linearized() {
        let root = directory();
        let control = AgentControl::new(&root);
        let barrier = Arc::new(std::sync::Barrier::new(9));
        let requests = std::thread::scope(|scope| {
            let mut workers = Vec::new();
            for _ in 0..8 {
                let c = control.clone(); let b = barrier.clone();
                workers.push(scope.spawn(move || { b.wait(); c.start_request() }));
            }
            barrier.wait(); control.pause();
            workers.into_iter().map(|w| w.join().unwrap()).collect::<Vec<_>>()
        });
        for request in requests.into_iter().flatten() { assert!(request.cancelled()); }
        assert!(control.start_request().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cancelled_worker_scope_cannot_admit_nested_paid_requests_after_resume() {
        let root = directory();
        let control = AgentControl::new(&root);
        let old = control.start_request().unwrap();
        let scope = old.enter();
        control.pause(); control.resume();
        assert!(control.start_request().is_err());
        let other = control.clone();
        assert!(std::thread::spawn(move || other.start_request().is_ok()).join().unwrap());
        drop(scope);
        assert!(control.start_request().is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn engine_commands_are_local_in_chat_and_code_and_do_not_restart_work() {
        let root = directory();
        let engine = crate::QuixiEngine::new_with_data_dir(root.join("resources"), root.clone());
        let agent = engine.start_agent_session(&json!({"message":" FINAL ","cwd":root.join("missing-workspace")}));
        assert!(engine.agent_state()["paused"].as_bool().unwrap());
        assert!(!engine.agent_sessions.read()[&agent].is_running);
        assert!(engine.agent_sessions.read()[&agent].initial_message.is_none());
        assert!(engine.agent_sessions.read()[&agent].buffered_messages.iter().all(|m| m["type"] == "system"));
        assert!(crate::turns::provider_history(&engine.agent_sessions.read()[&agent].buffered_messages).is_empty());
        let code = engine.start_code_session(&json!({"message":"ordinary request","cwd":root.join("missing-workspace")}));
        assert!(!engine.code_sessions.read()[&code].turn_running);
        let denied = engine.send_chat_message(&code, &json!({"text":"another task"})).unwrap();
        assert_eq!(denied["paused"], true);
        assert!(denied["message"].as_str().unwrap().contains("IA pausada. Envie INICIAR"));
        let allowed = engine.send_chat_message(&code, &json!({"text":" iniciar "})).unwrap();
        assert_eq!(allowed["localCommand"], true);
        assert_eq!(engine.agent_state()["paused"], false);
        assert!(!engine.code_sessions.read()[&code].turn_running);
        assert!(engine.turn_cancellations.read().is_empty());
        assert!(crate::turns::provider_history(&engine.code_transcripts.read()[&code]).is_empty());
        drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn pause_clears_steering_disarms_schedules_and_guards_late_callbacks_and_tools() {
        let root = directory();
        let engine = crate::QuixiEngine::new_with_data_dir(root.join("resources"), root.clone());
        let sid = engine.start_agent_session(&json!({}));
        let old = engine.begin_controlled_turn(&sid).unwrap();
        let signal = old.signal();
        engine.agent_sessions.write().get_mut(&sid).unwrap().is_running = true;
        engine.pending_messages.lock().entry(sid.clone()).or_default().push_back(json!({"text":"queued","delivery":"steer","held":false}));
        let schedule = engine.create_or_update_scheduled_task(&json!({"name":"Paused task","prompt":"ordinary task","cwd":""}));
        let id = schedule["id"].as_str().unwrap();
        engine.set_agent_paused(true);
        assert!(signal.load(Ordering::SeqCst));
        assert_eq!(engine.get_pending_messages(&sid), json!([]));
        assert_eq!(engine.scheduled_tasks.read()[id]["enabled"], false);
        assert!(engine.run_scheduled_task(id)["error"].as_str().unwrap().contains("IA pausada"));
        let stale = AtomicBool::new(false);
        let (message, failed) = engine.execute_tool_with_approval(&sid, "late-tool", "Write", &json!({"file_path":root.join("should-not-exist"),"content":"late"}), &root.to_string_lossy(), "auto", &stale);
        assert!(failed, "{message}");
        assert!(!root.join("should-not-exist").exists());
        assert!(engine.compact_session_now(&sid)["error"].is_string());
        assert!(engine.run_subagent(&sid, "late-child", &json!({"title":"late","prompt":"no calls"}), "", "auto", "fixture", &stale).1);
        engine.set_agent_paused(false);
        let new = engine.begin_controlled_turn(&sid).unwrap();
        let new_signal = new.signal();
        engine.agent_sessions.write().get_mut(&sid).unwrap().is_running = true;
        engine.record_partial_reply(&sid, &signal, "STALE");
        engine.record_partial_reply(&sid, &new_signal, "current");
        assert_eq!(engine.partial_replies.read()[&sid].1, "current");
        drop(old);
        assert!(engine.agent_sessions.read()[&sid].is_running);
        assert!(engine.is_current_turn(&sid, &new_signal));
        assert!(!new_signal.load(Ordering::SeqCst));
        engine.set_agent_paused(true);
        drop(new); drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cancelled_goal_scope_cannot_enqueue_a_continuation_on_a_new_turn() {
        let root = directory();
        let engine = crate::QuixiEngine::new_with_data_dir(root.join("resources"), root.clone());
        let sid = engine.start_agent_session(&json!({}));
        let old = engine.begin_controlled_turn(&sid).unwrap();
        let scope = old.enter();
        engine.set_agent_paused(true);
        engine.set_agent_paused(false);
        // New work on another admission thread does not inherit the old task's scope.
        let new = {
            let eng = engine.clone();
            let id = sid.clone();
            std::thread::spawn(move || eng.begin_controlled_turn(&id).unwrap()).join().unwrap()
        };
        engine.agent_sessions.write().get_mut(&sid).unwrap().is_running = true;
        let result = engine.send_chat_message(&sid, &json!({"text":"Late automatic goal continuation"}));
        assert!(result.is_err());
        assert!(engine.get_pending_messages(&sid).as_array().unwrap().is_empty());
        assert!(!new.signal().load(Ordering::SeqCst));
        drop(scope);
        engine.set_agent_paused(true);
        drop(old); drop(new); drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn late_provider_tool_result_after_final_and_iniciar_has_no_callbacks_or_edits() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        let root = directory();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let target = root.join("late-write.txt");
        let arguments = json!({"file_path":target,"content":"must not be written"}).to_string();
        let (arrived_tx, arrived_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "mock request never arrived");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("mock accept: {e}"),
                }
            };
            stream.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(15))).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" { break; }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse::<usize>().unwrap();
                }
            }
            let mut bytes = vec![0; length];
            reader.read_exact(&mut bytes).unwrap();
            arrived_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(15)).unwrap();
            let delta = json!({"choices":[{"delta":{"content":"late response","tool_calls":[{"index":0,"id":"late","type":"function","function":{"name":"Write","arguments":arguments}}]},"finish_reason":null}]});
            let finish = json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]});
            let body = format!("data: {delta}\n\ndata: {finish}\n\ndata: [DONE]\n\n");
            // Some adapters close the connection on cancellation; either way the late result
            // must not escape the engine's generation/token guards.
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
        });
        let engine = crate::QuixiEngine::new_with_data_dir(root.join("resources"), root.clone());
        let saved = engine.provider_manager.save_provider(&json!({"providerId":"custom","baseUrl":url,"apiFormat":"openai","selectedModel":"fixture","activate":true}));
        assert!(saved.get("error").is_none(), "{saved}");
        let sid = engine.start_agent_session(&json!({"model":"fixture","cwd":root}));
        let guard = engine.begin_controlled_turn(&sid).unwrap();
        let signal = guard.signal();
        engine.agent_sessions.write().get_mut(&sid).unwrap().is_running = true;
        let callbacks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let callback_count = callbacks.clone();
        let worker_engine = engine.clone();
        let cwd = root.to_string_lossy().into_owned();
        let worker = std::thread::spawn(move || {
            let _guard = guard;
            let _scope = _guard.enter();
            worker_engine.generate_reply_with_tools_streaming("ordinary task", &cwd, &[], Some("fixture"), "auto", &sid, &signal,
                |_| { callback_count.fetch_add(1, Ordering::SeqCst); },
                |_| { callback_count.fetch_add(1, Ordering::SeqCst); })
        });
        arrived_rx.recv_timeout(Duration::from_secs(15)).unwrap();
        engine.set_agent_paused(true);
        engine.set_agent_paused(false);
        release_tx.send(()).unwrap();
        let (answer, _) = worker.join().unwrap();
        server.join().unwrap();
        assert!(answer.is_empty());
        assert_eq!(callbacks.load(Ordering::SeqCst), 0);
        assert!(!target.exists());
        assert_eq!(engine.agent_state()["status"], "available");
        drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn remote_errors_are_transient_and_do_not_cancel_a_new_generation() {
        let root = directory();
        let control = AgentControl::new(&root);
        control.pause(); control.resume();
        let new = control.start_request().unwrap();
        control.report_cancel_error("Cancelamento remoto não confirmado.");
        assert_eq!(control.state()["status"], "cancel_error");
        assert!(!new.cancelled());
        assert_eq!(AgentControl::new(&root).state()["status"], "available");
        std::fs::remove_dir_all(root).unwrap();
    }
}
