mod common;
use common::{BIN, Team};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use xxassxx::{
    app::{self, Actor, Instruction},
    local_files::{self, Scope},
    task_coordinator as tasks,
};

fn input(t: &Team, action: &str) -> Instruction {
    let s = t.store("a");
    let p = app::project(&s, &t.dir.path().join("a/work")).unwrap();
    Instruction {
        request_id: uuid::Uuid::new_v4().to_string(),
        channel: "tui".into(),
        session_id: app::session(&s, p["id"].as_str().unwrap(), "a").unwrap(),
        task_id: None,
        recipient: "a".into(),
        body: "查找白名单中的报告，读取依据后继续处理".into(),
        action: action.into(),
        payload: json!({}),
    }
}
fn tool(id: usize, name: &str, args: Value) -> Value {
    json!({"id":format!("call_{id}"),"type":"function","function":{"name":name,"arguments":args.to_string()}})
}
fn response(calls: Vec<Value>) -> axum::Json<Value> {
    axum::Json(
        json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","tool_calls":calls}}]}),
    )
}

#[test]
fn bounded_reads_search_multiple_roots_and_enforce_live_permissions() {
    let t = Team::new();
    let mut s = t.store("a");
    let work = t.dir.path().join("a/work");
    let second = t.dir.path().join("reports");
    std::fs::create_dir(&second).unwrap();
    std::fs::write(work.join("old.txt"), "old draft").unwrap();
    std::fs::write(second.join("研究 结果.txt"), "final version\nanswer=83\n").unwrap();
    std::fs::write(second.join("binary.zip"), [0, 255, 0, 42]).unwrap();
    s.allow_directory(&work).unwrap();
    s.allow_directory(&second).unwrap();
    let call = |name: &str, args: Value| local_files::call(&s, Scope::Owner(&work), name, args);
    assert!(
        call("list_local_files", json!({"path":t.dir.path()})).is_err(),
        "shared parent is not authorized"
    );
    assert!(
        call("search_local_files", json!({"path":work,"query":"结果"})).unwrap()["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let found = call(
        "search_local_files",
        json!({"path":second,"query":"answer=83","content":true}),
    )
    .unwrap();
    assert_eq!(found["entries"].as_array().unwrap().len(), 1);
    assert_eq!(found["entries"][0]["line"], 2);
    let read = call(
        "read_local_file",
        json!({"path":second.join("研究 结果.txt"),"start_line":2,"max_lines":1}),
    )
    .unwrap();
    assert_eq!(read["content"], "2: answer=83");
    assert_eq!(read["truncated"], true);
    assert_eq!(read["sha256"].as_str().unwrap().len(), 64);
    assert!(call("read_local_file", json!({"path":second.join("binary.zip")})).is_err());
    let outside = t.dir.path().join("outside.txt");
    std::fs::write(&outside, "not allowed").unwrap();
    std::os::unix::fs::symlink(&outside, work.join("escape")).unwrap();
    assert!(call("read_local_file", json!({"path":"escape"})).is_err());
    assert!(call("read_local_file", json!({"path":"../../outside.txt"})).is_err());
    s.remove_directory(&second).unwrap();
    assert!(
        local_files::call(
            &s,
            Scope::Owner(&work),
            "read_local_file",
            json!({"path":second.join("研究 结果.txt")})
        )
        .is_err()
    );
    s.allow_directory(t.dir.path()).unwrap();
    assert!(
        local_files::call(
            &s,
            Scope::Owner(&work),
            "read_local_file",
            json!({"path":s.path})
        )
        .is_err(),
        "broad roots do not expose the database"
    );
    let secret = t.dir.path().join("key.env");
    std::fs::write(&secret, "secret marker").unwrap();
    let mut cfg = s.member_config().unwrap();
    cfg.secrets_file = Some(secret.clone());
    s.configure_member(&cfg).unwrap();
    assert!(
        local_files::call(
            &s,
            Scope::Owner(&work),
            "read_local_file",
            json!({"path":secret})
        )
        .is_err()
    );
}

#[tokio::test]
async fn owner_can_recover_a_read_error_read_text_and_answer_without_codex() {
    let t = Team::new();
    let mut s = t.store("a");
    let work = t.dir.path().join("a/work");
    s.allow_directory(&work).unwrap();
    std::fs::write(work.join("report.txt"), "actual answer=83").unwrap();
    let router=axum::Router::new().route("/chat/completions",axum::routing::post(|axum::Json(v):axum::Json<Value>|async move{
        let tools=v["messages"].as_array().unwrap().iter().filter(|m|m["role"]=="tool").collect::<Vec<_>>();
        match tools.len(){0=>response(vec![tool(1,"read_local_file",json!({"path":"missing.txt"}))]),1=>{assert!(tools[0]["content"].as_str().unwrap().contains("false"));response(vec![tool(2,"read_local_file",json!({"path":"report.txt"}))])},2=>{assert!(!v["tools"].as_array().unwrap().iter().any(|t|t["function"]["name"]=="read_local_file"));assert!(tools[1]["content"].as_str().unwrap().contains("actual answer=83"));response(vec![tool(3,"reply_user",json!({"text":"report.txt 第1行记录 answer=83。","question":false}))])},_=>axum::Json(json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"done"}}]}))}
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut cfg = s.member_config().unwrap();
    cfg.model = json!({"provider":"compatible","model":"mock","base_url":url,"api_key_env":"","max_model_calls":4,"max_tool_rounds":3});
    s.configure_member(&cfg).unwrap();
    let i = input(&t, "chat");
    app::submit(&mut s, &Actor::local(&t.store("a")).unwrap(), &i).unwrap();
    app::model::process_one(&mut s).await.unwrap();
    let command = app::command(&s, &i.request_id).unwrap();
    assert_eq!(command["state"], "done", "{command}");
    assert!(app::tasks::list(&s, None).unwrap().is_empty());
    assert!(s.list().unwrap().is_empty());
    assert!(
        app::history(&s, &i.session_id)
            .unwrap()
            .iter()
            .any(|v| v["body"].as_str().unwrap_or("").contains("answer=83"))
    );
    server.abort();
}

#[tokio::test]
async fn discovery_evidence_and_original_request_follow_owner_handoff() {
    let t = Team::new();
    let mut s = t.store("a");
    let work = t.dir.path().join("a/work");
    let second = t.dir.path().join("reports");
    std::fs::create_dir(&second).unwrap();
    for root in [&work, &second] {
        s.allow_directory(root).unwrap();
    }
    std::fs::write(second.join("report.txt"), "selected report=83").unwrap();
    let router=axum::Router::new().route("/chat/completions",axum::routing::post(|axum::Json(v):axum::Json<Value>|async move{
        let ctx:Value=serde_json::from_str(v["messages"][1]["content"].as_str().unwrap()).unwrap();let completed=v["messages"].as_array().unwrap().iter().filter(|m|m["role"]=="tool").collect::<Vec<_>>();
        if completed.is_empty(){response(ctx["allowed_directories"].as_array().unwrap().iter().enumerate().map(|(n,p)|tool(n,"search_local_files",json!({"path":p,"query":"report"}))).collect())}
        else if completed.len()==2 {let candidate=completed.iter().find_map(|m|{let x:Value=serde_json::from_str(m["content"].as_str().unwrap()).unwrap();x["value"]["entries"][0]["path"].as_str().map(str::to_owned)}).unwrap();response(vec![tool(3,"read_local_file",json!({"path":candidate}))])}
        else if completed.len()==3 {response(vec![tool(4,"delegate_codex",json!({"reason":"Need specialized parsing of the selected report"}))])}else{axum::Json(json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"done"}}]}))}
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut cfg = s.member_config().unwrap();
    cfg.model = json!({"provider":"compatible","model":"mock","base_url":url,"api_key_env":""});
    s.configure_member(&cfg).unwrap();
    let i = input(&t, "chat");
    app::submit(&mut s, &Actor::local(&t.store("a")).unwrap(), &i).unwrap();
    app::model::chat(&mut s, &i).await.unwrap();
    let command = app::command(&s, &i.request_id).unwrap();
    let id = command["task_id"].as_str().unwrap();
    let task = tasks::get(&s, id).unwrap();
    assert_eq!(task["original"], i.body);
    let conn = rusqlite::Connection::open(&s.path).unwrap();
    let handoff = xxassxx::local_agent::handoff_context(&conn, "not-started", id).unwrap();
    assert_eq!(handoff["owner_delegated"], true);
    assert_eq!(
        handoff["owner"]["observations"].as_array().unwrap().len(),
        3
    );
    assert!(handoff.to_string().contains("selected report=83"));
    assert!(
        handoff["owner"]["reason"]
            .as_str()
            .unwrap()
            .contains("specialized parsing")
    );
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_agent_can_finish_without_codex_or_handoff_in_the_same_execution() {
    for handoff in [false, true] {
        let t = Team::new();
        let mut s = t.store("a");
        let work = t.dir.path().join("a/work");
        s.allow_directory(&work).unwrap();
        std::fs::write(work.join("fast.py"), "iterations=10 # fast original").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let router=axum::Router::new().route("/chat/completions",axum::routing::post(move |axum::Json(v):axum::Json<Value>|{let observed=observed.clone();async move{
            assert_eq!(v["messages"][0]["content"],xxassxx::local_agent::PROMPT);let n=observed.fetch_add(1,Ordering::SeqCst);
            if n==0 {response(vec![tool(1,"read_local_file",json!({"path":"fast.py"}))])}
            else {assert!(v["messages"].to_string().contains("iterations=10"));if handoff {response(vec![tool(2,"delegate_codex",json!({"reason":"Need detailed toolchain verification of the located script"}))])}else{response(vec![tool(2,"submit_task_result",json!({"result":json!({"body":"fast.py 记录 iterations=10；未运行程序。","files":[{"root":0,"path":"fast.py","reason":"read actual parameter"}]}).to_string()}))])}}
        }}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut cfg = s.member_config().unwrap();
        cfg.model = json!({"provider":"compatible","model":"mock","base_url":url,"api_key_env":""});
        cfg.executor.mode = "auto".into();
        cfg.executor.timeout_secs = 30;
        cfg.executor.codex = if handoff {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/business_codex.py")
        } else {
            work.join("codex-must-not-be-needed")
        };
        s.configure_member(&cfg).unwrap();
        let mut i = input(&t, "create_task");
        let command = app::submit(&mut s, &Actor::local(&t.store("a")).unwrap(), &i).unwrap();
        let id = command["task_id"].as_str().unwrap();
        tasks::prepare(&s,id,json!({"goal":"找到脚本并说明参数","mode":"listing","deliverables":["文件与参数"],"questions":[]})).unwrap();
        i.task_id = Some(id.into());
        i.payload = json!({"revision":1});
        i.request_id = uuid::Uuid::new_v4().to_string();
        tasks::confirm(&s, &i).unwrap();
        tasks::tick(&s, Path::new(BIN)).await.unwrap();
        let task = tasks::get(&s, id).unwrap();
        assert_eq!(task["executions"][0]["state"], "candidate", "{task:#}");
        let execution = task["executions"][0]["id"].as_str().unwrap();
        let runs = s.runs(execution).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, "succeeded");
        assert_eq!(runs[0].mcp_initialized, handoff);
        assert_eq!(runs[0].session_id.is_some(), handoff);
        assert_eq!(runs[0].codex_version.is_some(), handoff);
        let events = s.events(&runs[0].id).unwrap();
        assert!(
            events
                .iter()
                .any(|e| e["type"] == "xxassxx.local_agent.tool")
        );
        if handoff {
            let ctx = xxassxx::local_agent::handoff_context(
                &rusqlite::Connection::open(&s.path).unwrap(),
                execution,
                id,
            )
            .unwrap();
            assert!(ctx.to_string().contains("iterations=10"));
        } else {
            assert_eq!(runs[0].exit_code, None);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        server.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_delivery_member_is_corrected_before_any_task_effect() {
    let t = Team::new();
    let mut s = t.store("a");
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let router = axum::Router::new().route("/chat/completions", axum::routing::post(move |axum::Json(v): axum::Json<Value>| {
        let seen = seen.clone();
        async move {
            assert_eq!(v["tools"][0]["function"]["parameters"]["properties"]["deliver_to"]["enum"], json!(["b", "c"]));
            let n = seen.fetch_add(1, Ordering::SeqCst);
            if n > 0 {
                assert!(v["messages"][0]["content"].as_str().unwrap().contains("contacts.member_id"));
            }
            response(vec![tool(n,"task_decision",json!({"action":"prepare","body":"核对后交付","goal":"核对后把图片发给 b","deliverables":["图片与核对说明"],"questions":[],"mode":"files","start_from_request":false,"deliver_to":if n == 0 {"Display Name"}else{"b"}}))])
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut cfg = s.member_config().unwrap();
    cfg.model = json!({"provider":"compatible","base_url":url,"model":"mock","api_key_env":""});
    s.configure_member(&cfg).unwrap();
    let i = input(&t, "create_task");
    let command = app::submit(&mut s, &Actor::local(&t.store("a")).unwrap(), &i).unwrap();
    let id = command["task_id"].as_str().unwrap();
    tasks::prepare_model(&s, id).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(tasks::get(&s, id).unwrap()["draft"]["deliver_to"], "b");
    assert!(s.messages(None).unwrap().is_empty());
    assert!(xxassxx::transfers::list(&s).unwrap().is_empty());
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_processing_delivers_to_the_named_contact_and_waits_for_receipt() {
    use xxassxx::access_mode::{self, Mode};
    for mode in [Mode::AskForApproval, Mode::FullAccess] {
        let t = Team::new();
        let mut s = t.store("a");
        let work = t.dir.path().join("a/work");
        s.allow_directory(&work).unwrap();
        std::fs::write(work.join("status.json"), r#"{"latest":"result.png"}"#).unwrap();
        std::fs::write(work.join("result.png"), b"\x89PNG\r\n\x1a\nfixture").unwrap();
        let mut cfg = s.member_config().unwrap();
        cfg.executor.mode = "auto".into();
        cfg.executor.codex =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/business_codex.py");
        cfg.executor.timeout_secs = 30;
        s.configure_member(&cfg).unwrap();
        access_mode::set(&s, mode).unwrap();
        let mut i = input(&t, "create_task");
        let command = app::submit(&mut s, &Actor::local(&t.store("a")).unwrap(), &i).unwrap();
        let id = command["task_id"].as_str().unwrap();
        let draft = json!({"goal":"本机核对后把图片发给 b","mode":"files","deliver_to":"b","deliverables":["核对依据和实际图片"],"questions":[]});
        let mut invalid = draft.clone();
        invalid["deliver_to"] = json!("a");
        assert!(tasks::prepare(&s, id, invalid).is_err());
        tasks::prepare(&s, id, draft).unwrap();
        if mode == Mode::AskForApproval {
            i.task_id = Some(id.into());
            i.payload = json!({"revision":1});
            i.request_id = uuid::Uuid::new_v4().to_string();
            tasks::confirm(&s, &i).unwrap();
        } else {
            assert_eq!(tasks::get(&s, id).unwrap()["confirmed"], 1);
        }
        tasks::tick(&s, Path::new(BIN)).await.unwrap();
        let task = tasks::get(&s, id).unwrap();
        let raw: String = rusqlite::Connection::open(&s.path)
            .unwrap()
            .query_row(
                "SELECT result FROM task_candidates WHERE task_id=?1 AND revision=1 AND member='a'",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        let candidates =
            json!([{"member":"a","result":serde_json::from_str::<Value>(&raw).unwrap()}]);
        let batch = xxassxx::transfers::list(&s).unwrap().remove(0);
        let transfer = batch["id"].as_str().unwrap();
        let manifest = xxassxx::transfers::manifest(&s, transfer).unwrap();
        assert_eq!(manifest.sender, "a");
        assert_eq!(manifest.recipient, "b");
        assert!(!xxassxx::transfers::delivery_ready(&s, &task, &candidates).unwrap());
        assert_eq!(
            xxassxx::transfers::delivery_status(&s, &task, &candidates).unwrap()["next_owner"],
            "a"
        );
        if mode == Mode::AskForApproval {
            assert_eq!(batch["state"], "draft");
            xxassxx::transfers::allow(&s, transfer).unwrap();
        } else {
            assert_eq!(batch["state"], "queued");
        }
        // The destination receives an attachment, not an execution assignment.
        assert!(s.messages(None).unwrap().is_empty());
        t.cli("b", &["files", "sync"]);
        t.cli("a", &["files", "sync"]);
        t.cli("b", &["files", "sync"]);
        t.cli("a", &["files", "sync"]);
        assert!(xxassxx::transfers::delivery_ready(&s, &task, &candidates).unwrap());
        let status = xxassxx::transfers::delivery_status(&s, &task, &candidates).unwrap();
        assert_eq!(status["next_owner"], Value::Null);
        assert!(status["body"].as_str().unwrap().contains("@b"));
        assert_eq!(
            xxassxx::transfers::get(&t.store("b"), transfer).unwrap()["state"],
            "received"
        );
        assert!(app::tasks::list(&t.store("b"), None).unwrap().is_empty());
    }
}
