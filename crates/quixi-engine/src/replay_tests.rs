//! Keyless recorded-session SSE replay harness inspired by DeepSeek Harness (`dsh-llm-replay`).
//!
//! Exercises multi-round DeepSeek `reasoning_content` preservation, real token/cache usage,
//! per-turn shadow-Git `workspace_changes` + `Present`, `PlanMode` + `ExitPlanMode` review,
//! timed `AskUserQuestion` continuation, spill recovery, overflow auto-compaction, and
//! parallel read-only `Task` fan-out without any live API credentials.

use super::*;
use std::io::{BufRead, Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

struct ReplayStep {
    status: &'static str,
    sse_events: Vec<Value>,
}

impl ReplayStep {
    fn ok(events: Vec<Value>) -> Self {
        Self {
            status: "200 OK",
            sse_events: events,
        }
    }
    fn http_err(status: &'static str, body: Value) -> Self {
        Self {
            status,
            sse_events: vec![body],
        }
    }
}

fn spawn_replay_server(
    engine: &QuixiEngine,
    provider_id: &str,
    model_id: &str,
    steps: Vec<ReplayStep>,
) -> std::thread::JoinHandle<Vec<Value>> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    {
        let mut st = engine.provider_manager.state.write();
        st.active_provider = provider_id.to_string();
        st.active_model = model_id.to_string();
        let entry = st
            .providers
            .iter_mut()
            .find(|p| p.id == provider_id)
            .unwrap();
        entry.configured = true;
        entry.auth_mode = "api_key".into();
        entry.api_key_raw = "sk-replay-fixture".into();
        entry.api_format = "openai".into();
        entry.base_url = format!("http://{addr}/v1");
        entry.selected_model = model_id.to_string();
    }

    std::thread::spawn(move || {
        let mut captured_requests = Vec::new();
        for step in steps {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                if let Ok((s, _)) = listener.accept() {
                    break s;
                }
                assert!(Instant::now() < deadline, "replay server timed out waiting for request");
                std::thread::sleep(Duration::from_millis(10));
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut content_len = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_len = rest.trim().parse().unwrap_or(0);
                }
            }
            let mut body_bytes = vec![0u8; content_len];
            reader.read_exact(&mut body_bytes).unwrap();
            captured_requests.push(serde_json::from_slice(&body_bytes).unwrap());

            if step.status.starts_with("200") {
                let mut sse = String::new();
                for ev in step.sse_events {
                    sse.push_str(&format!("data: {ev}\n\n"));
                }
                sse.push_str("data: [DONE]\n\n");
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    sse.len(),
                    sse
                )
                .unwrap();
            } else {
                let err_body = step.sse_events[0].to_string();
                write!(
                    stream,
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    step.status,
                    err_body.len(),
                    err_body
                )
                .unwrap();
            }
        }
        captured_requests
    })
}

fn fixture_engine() -> (Arc<QuixiEngine>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let resources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources");
    let engine = QuixiEngine::new_with_data_dir(resources, dir.path().join("data"));
    (engine, dir)
}

#[test]
fn deepseek_multi_round_replays_reasoning_content_usage_and_records_turn_deliverables() {
    let (engine, dir) = fixture_engine();
    let repo = dir.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    assert!(Command::new("git").arg("init").current_dir(&repo).status().unwrap().success());
    assert!(Command::new("git")
        .args(["config", "user.email", "replay@example.com"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["config", "user.name", "Replay"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    fs::write(repo.join("lib.rs"), "pub fn answer() -> i32 { 41 }\n").unwrap();
    assert!(Command::new("git").args(["add", "lib.rs"]).current_dir(&repo).status().unwrap().success());
    assert!(Command::new("git").args(["commit", "-m", "init"]).current_dir(&repo).status().unwrap().success());

    let sid = engine.start_agent_session(&json!({
        "cwd": repo.to_string_lossy(),
        "userSelectedFolders": [repo.to_string_lossy()],
        "permissionMode": "auto",
        "model": "deepseek-flash"
    }));

    let server = spawn_replay_server(
        &engine,
        "deepseek",
        "deepseek-flash",
        vec![
            ReplayStep::ok(vec![
                json!({
                    "choices": [{
                        "delta": {
                            "reasoning_content": "First update TodoWrite and edit lib.rs.",
                            "tool_calls": [
                                {
                                    "index": 0,
                                    "id": "call_todo",
                                    "type": "function",
                                    "function": {
                                        "name": "TodoWrite",
                                        "arguments": json!({"todos": [{"content": "Update answer to 42", "status": "in_progress"}]}).to_string()
                                    }
                                },
                                {
                                    "index": 1,
                                    "id": "call_edit",
                                    "type": "function",
                                    "function": {
                                        "name": "Edit",
                                        "arguments": json!({"file_path": "lib.rs", "old_string": "41", "new_string": "42"}).to_string()
                                    }
                                },
                                {
                                    "index": 2,
                                    "id": "call_present",
                                    "type": "function",
                                    "function": {
                                        "name": "Present",
                                        "arguments": json!({"files": [{"path": "lib.rs", "description": "Updated answer()"}]}).to_string()
                                    }
                                }
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }],
                    "usage": {
                        "prompt_tokens": 1200,
                        "completion_tokens": 90,
                        "prompt_cache_hit_tokens": 1000
                    }
                }),
            ]),
            ReplayStep::ok(vec![
                json!({
                    "choices": [{
                        "delta": {
                            "reasoning_content": "Verified edit succeeded.",
                            "content": "Updated `lib.rs` to return 42."
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 1400,
                        "completion_tokens": 40,
                        "prompt_cache_hit_tokens": 1200
                    }
                }),
            ]),
        ],
    );

    engine
        .send_chat_message(&sid, &json!({"text": "Fix answer() in @lib.rs"}))
        .unwrap();
    for _ in 0..200 {
        if !engine.agent_sessions.read()[&sid].is_running {
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    assert!(!engine.agent_sessions.read()[&sid].is_running);

    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2);

    // Round 2 MUST carry `reasoning_content` on the assistant tool_calls message!
    let round2_msgs = requests[1]["messages"].as_array().unwrap();
    let assistant_tc = round2_msgs
        .iter()
        .find(|m| m["role"] == "assistant" && m.get("tool_calls").is_some())
        .expect("assistant tool_calls message must exist in round 2");
    assert_eq!(
        assistant_tc["reasoning_content"],
        "First update TodoWrite and edit lib.rs."
    );

    // Verify TodoWrite, ContextMeter, and TurnDeliverableRecord (shadow-Git diff + Present)!
    let view = engine.collaboration.client_view(&sid);
    assert_eq!(view["todos"].as_array().unwrap().len(), 1);
    assert_eq!(view["contextMeter"]["cacheReadTokens"], 1200);
    assert_eq!(view["contextMeter"]["inputTokens"], 200); // 1400 - 1200 disjoint!
    let deliverables = view["deliverables"].as_array().unwrap();
    assert_eq!(deliverables.len(), 1);
    assert_eq!(deliverables[0]["presentedFiles"][0]["path"], "lib.rs");
    assert_eq!(deliverables[0]["workspaceChanges"]["files"][0]["display"], "lib.rs");
    assert_eq!(deliverables[0]["workspaceChanges"]["added"], 1);
    assert_eq!(deliverables[0]["workspaceChanges"]["deleted"], 1);
}

#[test]
fn plan_mode_blocks_mutations_until_exit_plan_mode_approved_and_timed_question_continues() {
    let (engine, dir) = fixture_engine();
    let work = dir.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let sid = engine.start_agent_session(&json!({
        "cwd": work.to_string_lossy(),
        "userSelectedFolders": [work.to_string_lossy()],
        "permissionMode": "auto"
    }));
    engine.collaboration.set_plan_mode(&sid, true);

    // 1. Write is blocked while PlanMode is active even in `auto` permission mode:
    let (blocked_msg, is_err) = engine.execute_tool_with_approval(
        &sid,
        "call_w1",
        "Write",
        &json!({"file_path": "should_not_exist.txt", "content": "nope"}),
        work.to_str().unwrap(),
        "auto",
        &AtomicBool::new(false),
    );
    assert!(is_err);
    assert!(blocked_msg.contains("Blocked by Plan Mode"));
    assert!(!work.join("should_not_exist.txt").exists());

    // 2. ExitPlanMode waits for user approval and then exits PlanMode:
    let eng_clone = engine.clone();
    let sid_clone = sid.clone();
    let reviewer = std::thread::spawn(move || {
        eng_clone.execute_tool_with_approval(
            &sid_clone,
            "call_plan",
            "ExitPlanMode",
            &json!({"plan": "# Implementation Plan\n- Create file"}),
            "",
            "auto",
            &AtomicBool::new(false),
        )
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    let req_id = loop {
        let pending = engine.pending_tool_approvals(&sid);
        if let Some(first) = pending.as_array().and_then(|a| a.first()) {
            assert_eq!(first["kind"], "plan_review");
            break first["requestId"].as_str().unwrap().to_string();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(engine.resolve_tool_approval(&sid, &req_id, "approve")["ok"], true);
    let (plan_res, plan_err) = reviewer.join().unwrap();
    assert!(!plan_err, "{plan_res}");
    assert!(!engine.collaboration.is_plan_active(&sid));

    // 3. Timed AskUserQuestion with 1s timeout transitions to `continued` when unanswered:
    let (q_out, q_err) = engine.execute_tool_with_approval(
        &sid,
        "call_q1",
        "AskUserQuestion",
        &json!({
            "questions": [{"id": "db", "question": "Which DB?", "options": [{"label": "SQLite"}, {"label": "Postgres"}]}],
            "timeout_seconds": 1
        }),
        "",
        "auto",
        &AtomicBool::new(false),
    );
    assert!(!q_err);
    let q_val: Value = serde_json::from_str(&q_out).unwrap();
    assert_eq!(q_val["pending"], true);
    assert_eq!(
        engine.collaboration.get(&sid).questions[0].state,
        "continued"
    );
}

#[test]
fn context_overflow_triggers_auto_compaction_and_parallel_readonly_tasks_run_concurrently() {
    let (engine, dir) = fixture_engine();
    let work = dir.path().join("work");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("a.txt"), "ALPHA_VALUE").unwrap();
    fs::write(work.join("b.txt"), "BETA_VALUE").unwrap();

    let sid = engine.start_agent_session(&json!({
        "cwd": work.to_string_lossy(),
        "userSelectedFolders": [work.to_string_lossy()],
        "permissionMode": "default",
        "model": "custom-model"
    }));
    // Seed earlier history so compaction has a balanced split point:
    {
        let mut sessions = engine.agent_sessions.write();
        let sess = sessions.get_mut(&sid).unwrap();
        for i in 0..6 {
            sess.buffered_messages.push(json!({
                "type": if i % 2 == 0 { "user" } else { "assistant" },
                "uuid": format!("seed-{i}"),
                "message": { "content": [{ "type": "text", "text": format!("Historical turn {i}") }] }
            }));
        }
    }

    let server = spawn_replay_server(
        &engine,
        "custom",
        "custom-model",
        vec![
            // Step 1: Simulate HTTP 400 context_length_exceeded!
            ReplayStep::http_err(
                "400 Bad Request",
                json!({"error": {"code": "context_length_exceeded", "message": "maximum context length exceeded"}}),
            ),
            // Step 2: Summarizer call from force_overflow_compaction:
            ReplayStep::ok(vec![json!({
                "choices": [{"delta": {"content": "## Primary Request and Intent\n- Parallel inspect"}, "finish_reason": "stop"}]
            })]),
            // Step 3: Retried main request emits 2 parallel read-only Task calls!
            ReplayStep::ok(vec![json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [
                            {"index": 0, "id": "t1", "type": "function", "function": {"name": "Task", "arguments": json!({"title": "Read A", "prompt": "Check a.txt"}).to_string()}},
                            {"index": 1, "id": "t2", "type": "function", "function": {"name": "Task", "arguments": json!({"title": "Read B", "prompt": "Check b.txt"}).to_string()}}
                        ]
                    },
                    "finish_reason": "tool_calls"
                }]
            })]),
            // Step 4 & 5: Two concurrent subagent completions!
            ReplayStep::ok(vec![json!({"choices": [{"delta": {"content": "Subagent 1 done"}, "finish_reason": "stop"}]})]),
            ReplayStep::ok(vec![json!({"choices": [{"delta": {"content": "Subagent 2 done"}, "finish_reason": "stop"}]})]),
            // Step 6: Parent final answer:
            ReplayStep::ok(vec![json!({"choices": [{"delta": {"content": "Both subagents finished after overflow recovery."}, "finish_reason": "stop"}]})]),
        ],
    );

    let (reply, _) = engine.generate_reply_with_tools_streaming(
        "Inspect both files",
        work.to_str().unwrap(),
        &[],
        Some("custom-model"),
        "default",
        &sid,
        &AtomicBool::new(false),
        |_| {},
        |_| {},
    );
    assert_eq!(reply, "Both subagents finished after overflow recovery.");
    let reqs = server.join().unwrap();
    assert_eq!(reqs.len(), 6);
    assert_eq!(engine.list_subagents(&sid).as_array().unwrap().len(), 2);
}

#[test]
fn ralph_fresh_agent_loop_smart_auto_reviewer_and_job_settlement_notices_work() {
    let (engine, dir) = fixture_engine();
    let work = dir.path().join("work");
    let sub = work.join("nested");
    fs::create_dir_all(&sub).unwrap();

    let sid = engine.start_agent_session(&json!({
        "cwd": work.to_string_lossy(),
        "userSelectedFolders": [work.to_string_lossy()],
        "permissionMode": "smartAuto",
        "model": "custom-model"
    }));

    // 1. WorkingDirectory switches session cwd inside workspace without approval:
    let (wd_out, wd_err) = engine.execute_tool_with_approval(
        &sid,
        "call_wd",
        "WorkingDirectory",
        &json!({"action": "set", "path": "nested"}),
        work.to_str().unwrap(),
        "smartAuto",
        &AtomicBool::new(false),
    );
    assert!(!wd_err, "{wd_out}");
    assert!(engine.agent_sessions.read()[&sid].cwd.ends_with("nested"));

    // 2. Smart Auto safety reviewer allows a safe `cargo check` command without opening human approval:
    //    AND Ralph runs 2 sequential fresh-context rounds passing `<ralph-handoff>` between rounds!
    let server = spawn_replay_server(
        &engine,
        "custom",
        "custom-model",
        vec![
            // Smart Auto safety review call (`ALLOW:`):
            ReplayStep::ok(vec![json!({
                "choices": [{"delta": {"content": "ALLOW: non-destructive echo check"}, "finish_reason": "stop"}]
            })]),
            // Ralph Round 1 fresh subagent (`status: continue`):
            ReplayStep::ok(vec![json!({
                "choices": [{
                    "delta": {
                        "content": "Completed phase 1.\n<ralph-handoff>\n{\"status\":\"continue\",\"summary\":\"Phase 1 verified\",\"next_step\":\"Finish phase 2\"}\n</ralph-handoff>"
                    },
                    "finish_reason": "stop"
                }]
            })]),
            // Ralph Round 2 fresh subagent (`status: complete`):
            ReplayStep::ok(vec![json!({
                "choices": [{
                    "delta": {
                        "content": "Completed phase 2.\n<ralph-handoff>\n{\"status\":\"complete\",\"summary\":\"Both phases complete\",\"next_step\":\"\"}\n</ralph-handoff>"
                    },
                    "finish_reason": "stop"
                }]
            })]),
        ],
    );

    let (bash_out, bash_err) = engine.execute_tool_with_approval(
        &sid,
        "call_smart_bash",
        "Bash",
        &json!({"command": "printf smart-auto-ok", "description": "Safe print"}),
        work.to_str().unwrap(),
        "smartAuto",
        &AtomicBool::new(false),
    );
    assert!(!bash_err, "{bash_out}");
    assert!(bash_out.contains("smart-auto-ok"));

    let (ralph_out, ralph_err) = engine.run_ralph_loop(
        &sid,
        "call_ralph",
        &json!({"objective": "Complete two-phase refactor", "max_rounds": 4, "allow_edits": false}),
        work.to_str().unwrap(),
        "smartAuto",
        "custom-model",
        &AtomicBool::new(false),
    );
    assert!(!ralph_err, "{ralph_out}");
    let ralph_val: Value = serde_json::from_str(&ralph_out).unwrap();
    assert_eq!(ralph_val["status"], "complete");
    assert_eq!(ralph_val["roundsUsed"], 2);

    let reqs = server.join().unwrap();
    assert_eq!(reqs.len(), 3);
    // Round 2 prompt received Round 1's handoff (`Phase 1 verified`):
    assert!(reqs[2].to_string().contains("Phase 1 verified"));

    // 3. Background job settlement notice drains once when unawaited job finishes:
    let job = engine
        .jobs
        .start_bash(&sid, "bg echo", &work, "printf done_bg", dir.path(), None)
        .unwrap();
    for _ in 0..50 {
        let list = engine.jobs.list(&sid);
        if list.iter().any(|j| j.id == job.id && j.status == "completed") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let notices = engine.jobs.take_settlement_notices(&sid);
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains(&job.id));
    assert!(engine.jobs.take_settlement_notices(&sid).is_empty());

    // 4. Markdown session export works cleanly:
    let exported = engine.export_session_markdown(&sid);
    assert!(exported["markdown"].as_str().unwrap().contains(&sid));
}

#[test]
fn postmortem_regressions_64kb_agents_md_posix_bash_heredoc_and_cumulative_telemetry() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("with_like_repo");
    fs::create_dir_all(&ws).unwrap();

    // 1. 64 KB AGENTS.md fits whole without losing the bottom section:
    let body_64k = format!("{}\nBOTTOM_GATE_RULE_MARKER", "a".repeat(64_000));
    fs::write(ws.join("AGENTS.md"), &body_64k).unwrap();
    let loaded = prompt_context::workspace_instructions(ws.to_str().unwrap());
    assert!(loaded.contains("BOTTOM_GATE_RULE_MARKER"));
    assert!(!loaded.contains("[TRUNCATED:"));

    // Over 96 KB gets an explicit truncation notice with line offset:
    let body_110k = format!("{}\nBEYOND_CAP", "line_of_rules\n".repeat(8_000));
    fs::write(ws.join("AGENTS.md"), &body_110k).unwrap();
    let loaded_big = prompt_context::workspace_instructions(ws.to_str().unwrap());
    assert!(loaded_big.contains("[TRUNCATED: showing first 96000"));

    // 2. Bash tool executes POSIX bash heredoc (`<< 'EOF'`) + `$?` regardless of login $SHELL:
    let (heredoc_out, heredoc_err) = agent_tools::execute(
        "Bash",
        &json!({"command": "cat << 'EOF'\nPOSIX_HEREDOC_OK\nEOF\necho status=$?"}),
        ws.to_str().unwrap(),
        "auto",
        &quixi_mcp::McpHost::new(),
    );
    assert!(!heredoc_err, "{heredoc_out}");
    assert!(heredoc_out.contains("POSIX_HEREDOC_OK"));
    assert!(heredoc_out.contains("status=0"));

    // 3. Overflow/auto-compaction emits durable `compaction_checkpoint` via `on_message`,
    //    and multi-round usage accumulates across rounds instead of overwriting with last round!
    let (engine, eng_dir) = fixture_engine();
    let work = eng_dir.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let sid = engine.start_agent_session(&json!({
        "cwd": work.to_string_lossy(),
        "userSelectedFolders": [work.to_string_lossy()],
        "permissionMode": "auto",
        "model": "custom-model"
    }));
    {
        let mut sessions = engine.agent_sessions.write();
        let s = sessions.get_mut(&sid).unwrap();
        for i in 0..6 {
            s.buffered_messages.push(json!({
                "type": if i % 2 == 0 { "user" } else { "assistant" },
                "uuid": format!("m-{i}"),
                "message": { "content": [{ "type": "text", "text": format!("Turn {i}") }] }
            }));
        }
    }

    let server = spawn_replay_server(
        &engine,
        "custom",
        "custom-model",
        vec![
            ReplayStep::http_err(
                "400 Bad Request",
                json!({"error": {"code": "context_length_exceeded", "message": "context window exceeded"}}),
            ),
            ReplayStep::ok(vec![json!({
                "choices": [{"delta": {"content": "## Primary Request and Intent\n- Durable checkpoint"}, "finish_reason": "stop"}]
            })]),
            ReplayStep::ok(vec![json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{"index": 0, "id": "c1", "type": "function", "function": {"name": "ListDirectory", "arguments": "{}"}}]
                    },
                    "finish_reason": "tool_calls"
                }],
                "usage": { "prompt_tokens": 500, "completion_tokens": 50 }
            })]),
            ReplayStep::ok(vec![json!({
                "choices": [{"delta": {"content": "Done."}, "finish_reason": "stop"}],
                "usage": { "prompt_tokens": 600, "completion_tokens": 30 }
            })]),
        ],
    );

    let mut emitted = Vec::new();
    let _ = engine.generate_reply_with_tools_streaming(
        "Continue work",
        work.to_str().unwrap(),
        &[],
        Some("custom-model"),
        "auto",
        &sid,
        &AtomicBool::new(false),
        |_| {},
        |m| emitted.push(m),
    );
    server.join().unwrap();

    // Durable compaction_checkpoint was emitted to session storage:
    assert!(emitted.iter().any(|m| m["type"] == "compaction_checkpoint"));
    // Cumulative usage summed 500 + 600 = 1100 input_tokens and 50 + 30 = 80 output_tokens across 2 rounds:
    let tel = engine.last_turn_usage.read().get(&sid).cloned().unwrap();
    assert_eq!(tel.rounds_used, 3);
    assert_eq!(tel.usage.input_tokens, 1100);
    assert_eq!(tel.usage.output_tokens, 80);
}

#[test]
fn tests_a_b_c_g_write_read_edit_and_all_10_tools_execute_physically_with_openrouter_and_explorer() {
    let (engine, eng_dir) = fixture_engine();
    let work = eng_dir.path().join("proj_explorer");
    fs::create_dir_all(&work).unwrap();

    // Start a session WITHOUT legacy cwd; register the workspace folder via Explorer!
    let sid = engine.start_agent_session(&json!({
        "permissionMode": "auto",
        "model": "anthropic/claude-sonnet-4.6"
    }));
    let added = engine
        .explorer
        .add_source(&sid, &json!({ "path": work.to_string_lossy() }));
    assert!(added.get("error").is_none(), "{added}");

    let server = spawn_replay_server(
        &engine,
        "openrouter",
        "anthropic/claude-sonnet-4.6",
        vec![
            // Round 1 (Teste A): write_file creates TESTE_AGENTE.txt
            ReplayStep::ok(vec![json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_write_a",
                            "type": "function",
                            "function": {
                                "name": "write_file",
                                "arguments": json!({
                                    "file_path": "TESTE_AGENTE.txt",
                                    "content": "FERRAMENTA DE ESCRITA FUNCIONANDO"
                                }).to_string()
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })]),
            // Round 2 (Teste B): read_file reads TESTE_AGENTE.txt
            ReplayStep::ok(vec![json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_read_b",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": json!({
                                    "file_path": "TESTE_AGENTE.txt"
                                }).to_string()
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })]),
            // Round 3 (Teste C): edit_file updates TESTE_AGENTE.txt
            ReplayStep::ok(vec![json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_edit_c",
                            "type": "function",
                            "function": {
                                "name": "edit_file",
                                "arguments": json!({
                                    "file_path": "TESTE_AGENTE.txt",
                                    "old_string": "FERRAMENTA DE ESCRITA FUNCIONANDO",
                                    "new_string": "FERRAMENTA DE EDICAO FUNCIONANDO"
                                }).to_string()
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })]),
            // Round 4: read_file again + create_directory + move_file + apply_patch + list_directory + search_files
            ReplayStep::ok(vec![json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_read_c2",
                                "type": "function",
                                "function": {
                                    "name": "read_file",
                                    "arguments": json!({"file_path": "TESTE_AGENTE.txt"}).to_string()
                                }
                            },
                            {
                                "index": 1,
                                "id": "call_mkdir",
                                "type": "function",
                                "function": {
                                    "name": "create_directory",
                                    "arguments": json!({"path": "src/components"}).to_string()
                                }
                            },
                            {
                                "index": 2,
                                "id": "call_patch",
                                "type": "function",
                                "function": {
                                    "name": "apply_patch",
                                    "arguments": json!({
                                        "patch": "*** Begin Patch\n*** Add File: src/components/temp.rs\n+pub const OK: bool = true;\n*** End Patch"
                                    }).to_string()
                                }
                            },
                            {
                                "index": 3,
                                "id": "call_move",
                                "type": "function",
                                "function": {
                                    "name": "move_file",
                                    "arguments": json!({
                                        "source": "src/components/temp.rs",
                                        "destination": "src/components/explorer.rs"
                                    }).to_string()
                                }
                            },
                            {
                                "index": 4,
                                "id": "call_list",
                                "type": "function",
                                "function": {
                                    "name": "list_directory",
                                    "arguments": json!({"path": "src/components"}).to_string()
                                }
                            },
                            {
                                "index": 5,
                                "id": "call_search",
                                "type": "function",
                                "function": {
                                    "name": "search_files",
                                    "arguments": json!({"query": "EDICAO FUNCIONANDO"}).to_string()
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }]
            })]),
            // Round 5: final response
            ReplayStep::ok(vec![json!({
                "choices": [{
                    "delta": { "content": "Todas as operações físicas concluídas e verificadas." },
                    "finish_reason": "stop"
                }]
            })]),
        ],
    );

    let (reply, _) = engine.generate_reply_with_tools_streaming(
        "Crie o arquivo TESTE_AGENTE.txt, leia-o e edite-o.",
        "",
        &[],
        Some("anthropic/claude-sonnet-4.6"),
        "auto",
        &sid,
        &AtomicBool::new(false),
        |_| {},
        |_| {},
    );
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 5);
    assert!(reply.contains("operações físicas concluídas"));

    // Verify physical state on disk:
    assert_eq!(
        fs::read_to_string(work.join("TESTE_AGENTE.txt")).unwrap(),
        "FERRAMENTA DE EDICAO FUNCIONANDO"
    );
    assert!(work.join("src/components").is_dir());
    assert!(!work.join("src/components/temp.rs").exists());
    assert_eq!(
        fs::read_to_string(work.join("src/components/explorer.rs")).unwrap(),
        "pub const OK: bool = true;\n"
    );

    // Verify Round 3 received the exact Read output from Round 2 ("FERRAMENTA DE ESCRITA FUNCIONANDO"):
    assert!(requests[2]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "tool"
            && m["tool_call_id"] == "call_read_b"
            && m["content"]
                .as_str()
                .unwrap()
                .contains("FERRAMENTA DE ESCRITA FUNCIONANDO")));

    // Verify Round 5 received the updated Read output from Round 4 ("FERRAMENTA DE EDICAO FUNCIONANDO"):
    assert!(requests[4]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "tool"
            && m["tool_call_id"] == "call_read_c2"
            && m["content"]
                .as_str()
                .unwrap()
                .contains("FERRAMENTA DE EDICAO FUNCIONANDO")));
}

#[test]
fn test_d_isolation_prevents_cross_project_modifications_or_reads() {
    let (engine, eng_dir) = fixture_engine();
    let proj_a = eng_dir.path().join("proj_a");
    let proj_b = eng_dir.path().join("proj_b");
    fs::create_dir_all(&proj_a).unwrap();
    fs::create_dir_all(&proj_b).unwrap();
    fs::write(proj_b.join("intacto.txt"), "CONTEUDO_ORIGINAL_B").unwrap();

    let sid_a = engine.start_agent_session(&json!({ "permissionMode": "auto" }));
    let sid_b = engine.start_agent_session(&json!({ "permissionMode": "auto" }));
    engine
        .explorer
        .add_source(&sid_a, &json!({ "path": proj_a.to_string_lossy() }));
    engine
        .explorer
        .add_source(&sid_b, &json!({ "path": proj_b.to_string_lossy() }));

    let cancelled = AtomicBool::new(false);
    for (tool, args) in [
        (
            "write_file",
            json!({"file_path": proj_b.join("invadido.txt"), "content": "NAO PODE"}),
        ),
        (
            "write_file",
            json!({"file_path": "../proj_b/invadido.txt", "content": "NAO PODE"}),
        ),
        (
            "edit_file",
            json!({"file_path": proj_b.join("intacto.txt"), "old_string": "CONTEUDO_ORIGINAL_B", "new_string": "HACKEADO"}),
        ),
        (
            "read_file",
            json!({"file_path": proj_b.join("intacto.txt")}),
        ),
        (
            "delete_file",
            json!({"file_path": proj_b.join("intacto.txt")}),
        ),
    ] {
        let (out, is_err) =
            engine.execute_tool_with_approval(&sid_a, "iso-call", tool, &args, "", "auto", &cancelled);
        assert!(is_err, "Tool {tool} must fail across projects: {out}");
        assert!(
            out.contains("OPERAÇÃO NÃO AUTORIZADA"),
            "Expected OPERAÇÃO NÃO AUTORIZADA, got: {out}"
        );
    }

    assert!(!proj_b.join("invadido.txt").exists());
    assert_eq!(
        fs::read_to_string(proj_b.join("intacto.txt")).unwrap(),
        "CONTEUDO_ORIGINAL_B"
    );
}

#[test]
fn test_e_delete_file_requires_explicit_approval_and_backs_up_to_trash() {
    let (engine, eng_dir) = fixture_engine();
    let work = eng_dir.path().join("work_del");
    fs::create_dir_all(&work).unwrap();
    let target = work.join("para_excluir.txt");
    fs::write(&target, "DADOS_RECUPERAVEIS").unwrap();

    // Even in `acceptEdits` mode (where Write/Edit are pre-approved), `delete_file` MUST ask!
    let sid = engine.start_agent_session(&json!({
        "cwd": work.to_string_lossy(),
        "userSelectedFolders": [work.to_string_lossy()],
        "permissionMode": "acceptEdits"
    }));
    let cancelled = engine.begin_turn(&sid);

    // 1. Deny deletion:
    {
        let eng = engine.clone();
        let sid_cl = sid.clone();
        let cwd_cl = work.to_string_lossy().to_string();
        let sig_cl = cancelled.clone();
        let handle = std::thread::spawn(move || {
            eng.execute_tool_with_approval(
                &sid_cl,
                "del-1",
                "delete_file",
                &json!({"file_path": "para_excluir.txt"}),
                &cwd_cl,
                "acceptEdits",
                &sig_cl,
            )
        });
        let req_id = loop {
            let pending = engine.pending_tool_approvals(&sid);
            if let Some(first) = pending.as_array().and_then(|a| a.first()) {
                assert_eq!(first["toolName"], "delete_file");
                break first["requestId"].as_str().unwrap().to_string();
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(target.exists(), "File must not be deleted before approval");
        engine.resolve_tool_approval(&sid, &req_id, "deny");
        let (out, is_err) = handle.join().unwrap();
        assert!(is_err);
        assert!(out.contains("OPERAÇÃO NÃO AUTORIZADA"));
        assert!(target.exists(), "Denied deletion must leave file on disk");
    }

    // 2. Approve deletion (`allow_once`):
    {
        let eng = engine.clone();
        let sid_cl = sid.clone();
        let cwd_cl = work.to_string_lossy().to_string();
        let sig_cl = cancelled.clone();
        let handle = std::thread::spawn(move || {
            eng.execute_tool_with_approval(
                &sid_cl,
                "del-2",
                "delete_file",
                &json!({"file_path": "para_excluir.txt"}),
                &cwd_cl,
                "acceptEdits",
                &sig_cl,
            )
        });
        let req_id = loop {
            let pending = engine.pending_tool_approvals(&sid);
            if let Some(first) = pending.as_array().and_then(|a| a.first()) {
                break first["requestId"].as_str().unwrap().to_string();
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        engine.resolve_tool_approval(&sid, &req_id, "allow_once");
        let (out, is_err) = handle.join().unwrap();
        assert!(!is_err, "{out}");
        assert!(!target.exists(), "Approved deletion must remove file from workspace");
        // Verify backup copy in session trash:
        let trash_dir = engine.data_dir.join("trash").join(&sid);
        let backups: Vec<_> = fs::read_dir(&trash_dir).unwrap().flatten().collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            fs::read_to_string(backups[0].path()).unwrap(),
            "DADOS_RECUPERAVEIS"
        );
    }
}

#[test]
fn test_h_failures_and_false_conclusion_prevention() {
    let (engine, eng_dir) = fixture_engine();
    let work = eng_dir.path().join("work_fail");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join(".env"), "API_SECRET=top-secret-123").unwrap();

    let sid = engine.start_agent_session(&json!({
        "cwd": work.to_string_lossy(),
        "userSelectedFolders": [work.to_string_lossy()],
        "permissionMode": "auto",
        "model": "custom-model"
    }));
    let cwd = work.to_str().unwrap();
    let cancelled = AtomicBool::new(false);

    // 1. Unknown tool -> ERRO: FERRAMENTA INDISPONÍVEL
    let (out, err) = engine.execute_tool_with_approval(
        &sid,
        "f1",
        "ferramenta_inexistente",
        &json!({}),
        cwd,
        "auto",
        &cancelled,
    );
    assert!(err);
    assert!(out.contains("ERRO: FERRAMENTA INDISPONÍVEL"), "{out}");

    // 2. Invalid arguments -> error
    for (tool, bad_args) in [
        ("write_file", json!({"content": "missing path"})),
        ("edit_file", json!({"file_path": "a.txt", "old_string": "a"})),
        ("move_file", json!({"source": "a.txt"})),
        ("delete_file", json!({})),
        ("search_files", json!({})),
    ] {
        let (out, err) =
            engine.execute_tool_with_approval(&sid, "f2", tool, &bad_args, cwd, "auto", &cancelled);
        assert!(err, "Invalid args for {tool} must fail: {out}");
    }

    // 3. Sensitive secret file (.env) blocked unless explicitly selected in Explorer
    let (env_out, env_err) = engine.execute_tool_with_approval(
        &sid,
        "f3",
        "read_file",
        &json!({"file_path": ".env"}),
        cwd,
        "auto",
        &cancelled,
    );
    assert!(env_err);
    assert!(env_out.contains("OPERAÇÃO NÃO AUTORIZADA"), "{env_out}");

    // Selecting the folder as a whole still does NOT unlock `.env`:
    let added = engine
        .explorer
        .add_source(&sid, &json!({"path": work.to_string_lossy()}));
    let source_id = added["sourceId"].as_str().unwrap().to_string();
    engine.explorer.set_source_selected(&sid, &source_id, true);
    let (_, env_err2) = engine.execute_tool_with_approval(
        &sid,
        "f3b",
        "read_file",
        &json!({"file_path": ".env"}),
        cwd,
        "auto",
        &cancelled,
    );
    assert!(env_err2);

    // Explicit individual selection of `.env` in Explorer unlocks reading it:
    engine
        .explorer
        .set_file_selected(&sid, &source_id, ".env", true);
    let (env_ok_out, env_ok_err) = engine.execute_tool_with_approval(
        &sid,
        "f3c",
        "read_file",
        &json!({"file_path": ".env"}),
        cwd,
        "auto",
        &cancelled,
    );
    assert!(!env_ok_err, "{env_ok_out}");
    assert!(env_ok_out.contains("API_SECRET=top-secret-123"));

    // 4. Nonexistent file -> error
    for (tool, args) in [
        ("read_file", json!({"file_path": "nao_existe.txt"})),
        (
            "edit_file",
            json!({"file_path": "nao_existe.txt", "old_string": "a", "new_string": "b"}),
        ),
        ("delete_file", json!({"file_path": "nao_existe.txt"})),
        (
            "move_file",
            json!({"source": "nao_existe.txt", "destination": "outro.txt"}),
        ),
    ] {
        let (out, err) =
            engine.execute_tool_with_approval(&sid, "f4", tool, &args, cwd, "auto", &cancelled);
        assert!(err, "Missing file for {tool} must fail: {out}");
    }

    // 5. Command timeout -> error
    let (timeout_out, timeout_err) = engine.execute_tool_with_approval(
        &sid,
        "f5",
        "run_command",
        &json!({"command": "sleep 3", "timeout_seconds": 1}),
        cwd,
        "auto",
        &cancelled,
    );
    assert!(timeout_err, "Timed out command must report error: {timeout_out}");

    // 6. False conclusion prevention: model claims it created a file without calling any tool
    let server = spawn_replay_server(
        &engine,
        "custom",
        "custom-model",
        vec![ReplayStep::ok(vec![json!({
            "choices": [{
                "delta": { "content": "Criei o arquivo FALSO.txt com sucesso no projeto." },
                "finish_reason": "stop"
            }]
        })])],
    );
    let (reply, _) = engine.generate_reply_with_tools_streaming(
        "Crie FALSO.txt",
        cwd,
        &[],
        Some("custom-model"),
        "auto",
        &sid,
        &AtomicBool::new(false),
        |_| {},
        |_| {},
    );
    server.join().unwrap();
    assert!(!work.join("FALSO.txt").exists());
    assert!(
        reply.contains("ERRO: FERRAMENTA INDISPONÍVEL"),
        "Unverified file creation claim must be flagged: {reply}"
    );
}
