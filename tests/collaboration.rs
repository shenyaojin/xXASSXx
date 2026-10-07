mod common;
use common::Team;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
};
use xxassxx::knowledge::Content;

fn mock_codex(t: &Team, auto: bool) {
    let executable = t.dir.path().join("mock-codex");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex.py"),
        &executable,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut s = t.store("b");
    let mut cfg = s.member_config().unwrap();
    cfg.executor.codex = executable;
    cfg.executor.mode = if auto { "auto" } else { "queue" }.into();
    s.configure_member(&cfg).unwrap();
}
pub fn mcp(t: &Team, member: &str, name: &str, args: Value) -> Value {
    let mut cmd = Command::new(common::BIN);
    Team::credentials(&mut cmd);
    let mut child = cmd
        .arg("--db")
        .arg(t.db(member))
        .args(["butler", "mcp-serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"initiating-codex-test","version":"1"}}})).unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":args}})).unwrap();
    input.flush().unwrap();
    line.clear();
    out.read_line(&mut line).unwrap();
    drop(input);
    assert!(child.wait().unwrap().success());
    let response: Value = serde_json::from_str(&line).unwrap();
    response["result"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_to_codex_to_http_reply_read_by_initiator_mcp() {
    let mut t = Team::new();
    mock_codex(&t, true);
    let router=axum::Router::new().route("/chat/completions",axum::routing::post(|axum::Json(body):axum::Json<Value>|async move{
        let message=if body["messages"].as_array().unwrap().iter().any(|m|m["role"]=="tool"){json!({"role":"assistant","content":"Delegation queued; not completed yet."})}else{json!({"role":"assistant","content":null,"tool_calls":[{"id":"delegate-current","type":"function","function":{"name":"delegate_codex","arguments":"{}"}}]})};
        axum::Json(json!({"choices":[{"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":message}]}))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let mut b = t.store("b");
    let mut cfg = b.member_config().unwrap();
    cfg.model = json!({"provider":"compatible","base_url":base,"model":"mock","api_key_env":""});
    b.configure_member(&cfg).unwrap();
    let dataset = t.dir.path().join("dataset");
    std::fs::create_dir(&dataset).unwrap();
    b.allow_directory(&dataset).unwrap();
    std::fs::write(dataset.join("data.csv"), "value\n1\n").unwrap();
    let object = b.create_object("Dataset", "dataset", true).unwrap();
    let v1 = b
        .publish(&object.id, None, "data-v1", "V1", Content::Path(&dataset))
        .unwrap();
    std::fs::write(dataset.join("data.csv"), "value\n1\n2\n").unwrap();
    let v2 = b
        .publish(
            &object.id,
            Some(&v1.id),
            "data-v2",
            "V2",
            Content::Path(&dataset),
        )
        .unwrap();
    let request_id = uuid::Uuid::new_v4().to_string();
    let requested = mcp(
        &t,
        "a",
        "collaboration_send_request",
        json!({"to":"b","body":"Please analyze the changes in the dataset","message_id":request_id,"object_id":object.id}),
    );
    assert_eq!(requested["isError"], false);
    t.sync("a");
    t.restart(); // B offline while the relay persists the request.
    t.sync("b");
    let result = t.cli("b", &["butler", "process", &request_id]);
    assert_eq!(result["state"], "completed");
    let task = result["task"]["id"].as_str().unwrap();
    assert_eq!(result["task"]["state"], "succeeded");
    assert_eq!(b.runs(task).unwrap().len(), 1);
    assert!(
        b.events(&b.runs(task).unwrap()[0].id)
            .unwrap()
            .iter()
            .any(|e| e["item"]["tool"] == "submit_task_result")
    );
    t.sync("b");
    t.sync("a");
    let replies = mcp(
        &t,
        "a",
        "collaboration_read_replies",
        json!({"message_id":request_id}),
    );
    assert_eq!(replies["isError"], false);
    let reply = &replies["structuredContent"]["replies"][0]["message"];
    assert_eq!(reply["reply_to"], request_id);
    assert_eq!(reply["version_id"], v2.id);
    assert!(reply["body"].as_str().unwrap().contains("MOCK_RESULT"));
    // Restart/retry never creates another task or reply, or runs successful work again.
    t.cli("b", &["delegation", "run", &request_id]);
    t.cli("b", &["butler", "process", &request_id]);
    assert_eq!(b.list().unwrap().len(), 1);
    assert_eq!(b.runs(task).unwrap().len(), 1);
    assert_eq!(b.messages(None).unwrap().len(), 2);
    let denied = mcp(
        &t,
        "a",
        "submit_task_result",
        json!({"task_id":task,"result":"forged"}),
    );
    assert_eq!(denied["isError"], true);
    server.abort();
}

#[test]
fn failed_codex_cannot_reply_and_retry_resumes_bound_session() {
    let t = Team::new();
    mock_codex(&t, false);
    let r = t.cli(
        "a",
        &[
            "message",
            "send",
            "--to",
            "b",
            "--body",
            "Professional judgment please",
            "--operation",
            "analysis",
        ],
    );
    let id = r["message"]["message_id"].as_str().unwrap();
    t.sync("a");
    t.sync("b");
    t.cli("b", &["butler", "process", id]);
    let out = t
        .command("b", &["delegation", "run", id])
        .env("XXASSXX_MOCK_MODE", "fail_after_result")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let s = t.store("b");
    assert_eq!(s.messages(None).unwrap().len(), 1);
    let session = s.delegation(id).unwrap()["task"]["session_id"].clone();
    assert!(session.is_string());
    let done = t.cli("b", &["delegation", "run", id]);
    assert_eq!(done["state"], "completed");
    assert_eq!(done["executions"][1]["resumed_session_id"], session);
    assert_eq!(s.list().unwrap().len(), 1);
}

#[test]
fn closed_conversation_does_not_run_model_or_delegate() {
    let t = Team::new();
    let r = t.cli(
        "a",
        &[
            "message",
            "send",
            "--to",
            "b",
            "--body",
            "Please analyze",
            "--operation",
            "analysis",
        ],
    );
    t.sync("a");
    t.sync("b");
    let id = r["message"]["message_id"].as_str().unwrap();
    let conv = r["message"]["conversation_id"].as_str().unwrap();
    t.cli("b", &["conversation", "close", conv]);
    t.cli("b", &["butler", "tick"]);
    assert_eq!(t.store("b").delegation(id).unwrap()["state"], "none");
    assert!(
        t.store("b")
            .model_history(id)
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn peer_delegation_never_receives_owner_mcp_capability() {
    let t = Team::new();
    mock_codex(&t, false);
    let request = t.cli(
        "a",
        &[
            "message",
            "send",
            "--to",
            "b",
            "--body",
            "Analyze the authorized context",
            "--operation",
            "analysis",
        ],
    );
    let id = request["message"]["message_id"].as_str().unwrap();
    t.sync("a");
    t.sync("b");
    t.cli("b", &["butler", "process", id]);
    let capture = t.dir.path().join("argv.json");
    let output = t
        .command("b", &["delegation", "run", id])
        .env("XXASSXX_MOCK_RECORD", &capture)
        .output()
        .unwrap();
    assert!(output.status.success());
    let args: Value = serde_json::from_slice(&std::fs::read(&capture).unwrap()).unwrap();
    assert!(!args["argv"].to_string().contains("xxassxx_butler"));
    // Even a repeat model tool call after an execution returns no local paths.
    let mut store = t.store("b");
    let tool_result =
        xxassxx::butler::call_tool(&mut store, id, "delegate_codex", json!({})).unwrap();
    assert!(tool_result["task_id"].is_string());
    assert!(!tool_result.to_string().contains("workdir"));

    let task = t.cli("b", &["submit", "Owner-authored collaboration task"]);
    let config = store.member_config().unwrap();
    let output = t
        .command(
            "b",
            &[
                "run-once",
                task["id"].as_str().unwrap(),
                "--codex",
                config.executor.codex.to_str().unwrap(),
                "--workdir",
                config.executor.workdir.to_str().unwrap(),
            ],
        )
        .env("XXASSXX_MOCK_RECORD", &capture)
        .output()
        .unwrap();
    assert!(output.status.success());
    let args: Value = serde_json::from_slice(&std::fs::read(capture).unwrap()).unwrap();
    assert!(
        args["argv"]
            .to_string()
            .contains("mcp_servers.xxassxx_butler.command")
    );
}
