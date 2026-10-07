mod common;
use axum::{Json, Router, http::StatusCode, routing::post};
use common::Team;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use xxassxx::knowledge::Content;

async fn fake(mode: &str) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let history = seen.clone();
    let mode = mode.to_owned();
    let router=Router::new().route("/chat/completions",post(move|Json(body):Json<Value>|{
        let history=history.clone();let mode=mode.clone();async move {
            let n={let mut h=history.lock().unwrap();h.push(body.clone());h.len()};
            if mode=="timeout"{tokio::time::sleep(std::time::Duration::from_secs(3)).await;}
            if mode=="quota"{return (StatusCode::TOO_MANY_REQUESTS,Json(json!({"error":"vendor error body must not enter business logs"})));}
            if mode=="invalid_response"{return (StatusCode::OK,Json(json!({"not":"completion"})));}
            let context:Value=serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
            let object=context["current_request"]["message"]["object_id"].as_str().unwrap();
            let tool=|id:&str,name:&str,args:Value|json!({"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}});
            let response=if mode=="invalid_tool" && n==1 {json!({"role":"assistant","content":null,"tool_calls":[tool("bad","knowledge_metadata",json!({"object_id":object,"shell":"rm"}))]})}
            else if mode=="limit" {json!({"role":"assistant","content":null,"tool_calls":[tool(&format!("round-{n}"),"contacts",json!({}))]})}
            else if mode=="delegate" && n==1 {json!({"role":"assistant","content":null,"tool_calls":[tool("delegate1","delegate_codex",json!({})),tool("delegate2","delegate_codex",json!({}))]})}
            else if n==1 {json!({"role":"assistant","content":null,"reasoning_content":"mock thinking field retained","tool_calls":[tool("facts","knowledge_metadata",json!({"object_id":object})),tool("contacts","contacts",json!({}))]})}
            else if mode=="success" && n==2 {
                assert_eq!(body["messages"][2]["reasoning_content"],"mock thinking field retained");
                assert_eq!(body["messages"][3]["tool_call_id"],"facts");assert_eq!(body["messages"][4]["tool_call_id"],"contacts");
                let result:Value=serde_json::from_str(body["messages"][3]["content"].as_str().unwrap()).unwrap();let version=result["value"]["version"]["id"].as_str().unwrap();
                json!({"role":"assistant","content":null,"tool_calls":[tool("reply","submit_reply",json!({"body":format!("Source member b; object {object}; main / resolved version {version}."),"version_id":version}))]})
            }else{json!({"role":"assistant","content":"Additional model summary"})};
            let finish=if response.get("tool_calls").is_some(){"tool_calls"}else{"stop"};
            (StatusCode::OK,Json(json!({"choices":[{"finish_reason":finish,"message":response}]})))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (url, seen, handle)
}
fn request(t: &Team, operation: &str) -> String {
    let mut b = t.store("b");
    let object = b.create_object("Shared dataset", "dataset", true).unwrap();
    b.publish(
        &object.id,
        None,
        &object.id,
        "released",
        Content::Text("DO_NOT_UPLOAD_FILE_CONTENT"),
    )
    .unwrap();
    let r = t.cli(
        "a",
        &[
            "message",
            "send",
            "--to",
            "b",
            "--body",
            "Please explain the dataset's published version",
            "--operation",
            operation,
            "--object-id",
            &object.id,
        ],
    );
    t.sync("a");
    t.sync("b");
    r["message"]["message_id"].as_str().unwrap().into()
}
fn configure(t: &Team, url: &str) {
    let mut s = t.store("b");
    let mut cfg = s.member_config().unwrap();
    cfg.model = json!({"provider":"compatible","base_url":url,"model":"test-model","api_key_env":"","timeout_secs":1,"max_model_calls":4,"max_tool_rounds":2});
    s.configure_member(&cfg).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_http_tool_loop_preserves_ids_reasoning_and_no_file_content() {
    let t = Team::new();
    let id = request(&t, "auto");
    let (url, history, server) = fake("success").await;
    configure(&t, &url);
    t.cli("b", &["butler", "process", &id]);
    t.sync("b");
    t.sync("a");
    let s = t.store("b");
    assert_eq!(s.message(&id).unwrap().state, "replied");
    let runs = s.model_history(&id).unwrap();
    assert_eq!(runs[0]["state"], "succeeded");
    assert_eq!(runs[0]["steps"].as_array().unwrap().len(), 3);
    assert_eq!(runs[0]["steps"][2]["tool_call_id"], "reply");
    assert_eq!(runs[0]["summary"], "Additional model summary");
    let h = history.lock().unwrap();
    assert_eq!(h.len(), 3);
    assert!(
        !serde_json::to_string(&*h)
            .unwrap()
            .contains("DO_NOT_UPLOAD_FILE_CONTENT")
    );
    assert!(
        !serde_json::to_string(&*h)
            .unwrap()
            .contains(t.dir.path().to_str().unwrap())
    );
    assert_eq!(t.store("a").messages(None).unwrap().len(), 2);
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failures_limits_invalid_tools_and_retry_are_visible() {
    for (mode, error) in [
        ("quota", "model_quota_limited"),
        ("timeout", "model_timeout"),
        ("invalid_response", "model_invalid_response"),
        ("invalid_tool", "model_no_valid_action"),
        ("limit", "model_tool_round_limit"),
    ] {
        let t = Team::new();
        let id = request(&t, "auto");
        let (url, _, server) = fake(mode).await;
        configure(&t, &url);
        let out = t
            .command("b", &["butler", "process", &id])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{mode}");
        let record = t.store("b").message(&id).unwrap();
        assert_eq!(record.state, "failed");
        assert_eq!(record.error.as_deref(), Some(error));
        assert_eq!(
            record.message.body,
            "Please explain the dataset's published version"
        );
        assert_eq!(t.store("b").messages(None).unwrap().len(), 1);
        server.abort();
        let (url, _, server) = fake("success").await;
        configure(&t, &url);
        t.cli("b", &["butler", "process", &id]);
        assert_eq!(
            t.store("b")
                .model_history(&id)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
        server.abort();
    }
}

#[test]
fn explicit_metadata_needs_no_model_and_missing_key_is_visible() {
    let t = Team::new();
    let id = request(&t, "metadata");
    t.cli("b", &["butler", "process", &id]);
    assert_eq!(t.store("b").message(&id).unwrap().state, "replied");
    assert!(
        t.store("b")
            .model_history(&id)
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    let id2 = request(&t, "auto");
    let mut s = t.store("b");
    let mut cfg = s.member_config().unwrap();
    cfg.model = json!({"provider":"deepseek","model":"configured-model","api_key_env":"XXASSXX_NONEXISTENT_TEST_KEY"});
    s.configure_member(&cfg).unwrap();
    let out = t
        .command("b", &["butler", "process", &id2])
        .env_remove("XXASSXX_NONEXISTENT_TEST_KEY")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(
        s.message(&id2).unwrap().error.as_deref(),
        Some("missing_model_key")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_delegation_tool_is_transactionally_idempotent() {
    let t = Team::new();
    let id = request(&t, "auto");
    let (url, _, server) = fake("delegate").await;
    configure(&t, &url);
    t.cli("b", &["butler", "process", &id]);
    let s = t.store("b");
    let task = s.delegation(&id).unwrap()["task"]["id"].clone();
    assert_eq!(s.list().unwrap().len(), 1);
    t.cli("b", &["butler", "process", &id]);
    assert_eq!(s.delegation(&id).unwrap()["task"]["id"], task);
    assert_eq!(s.message(&id).unwrap().state, "delegated");
    server.abort();
}
