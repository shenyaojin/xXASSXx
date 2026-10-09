mod common;
use common::{BIN, Team};
use serde_json::{Value, json};
use std::path::Path;
use xxassxx::{
    app::{self, Actor, Instruction},
    store::Store,
    task_coordinator as tasks, task_materials,
};
fn input(t: &Team, body: &str) -> Instruction {
    let s = t.store("a");
    let p = app::project(&s, &t.dir.path().join("a/work")).unwrap();
    Instruction {
        request_id: uuid::Uuid::new_v4().to_string(),
        channel: "test-user-action".into(),
        session_id: app::session(&s, p["id"].as_str().unwrap(), "a").unwrap(),
        task_id: None,
        recipient: "a".into(),
        body: body.into(),
        action: "chat".into(),
        payload: json!({}),
    }
}
fn submit(s: &mut Store, i: &Instruction) -> Value {
    app::submit(s, &Actor::local(s).unwrap(), i).unwrap()
}
fn draft(t: &Team, body: &str) -> (Instruction, String) {
    let mut s = t.store("a");
    let i = input(t, body);
    let c = submit(&mut s, &i);
    let id = c["task_id"].as_str().unwrap().to_owned();
    tasks::prepare(&s,&id,json!({"goal":"选择实验入口并解释参数与差异","deliverables":["入口","参数","候选差异","选择依据"],"constraints":"只读","questions":[],"mode":"analysis"})).unwrap();
    (i, id)
}
fn action(i: &Instruction, id: &str, kind: &str, rev: i64, body: &str) -> Instruction {
    let mut i = i.clone();
    i.request_id = uuid::Uuid::new_v4().to_string();
    i.task_id = Some(id.into());
    i.action = kind.into();
    i.body = body.into();
    i.payload = json!({"revision":rev});
    i
}
fn count(s: &Store, sql: &str) -> i64 {
    rusqlite::Connection::open(&s.path)
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}
fn transfer(from: &Store, to: &mut Store) {
    let owner = to.owner().unwrap();
    for r in from.messages(None).unwrap().into_iter().filter(|r| {
        r.direction == "out" && r.message.operation == "task_v2" && r.message.recipient == owner
    }) {
        to.save_message(&r.message, true).unwrap();
        tasks::ingest(to, &r.message).unwrap();
    }
}
fn files(t: &Team, m: &str) {
    let mut s = t.store(m);
    let dir = t.dir.path().join(format!("{m}/work"));
    s.allow_directory(&dir).unwrap();
    std::fs::write(dir.join("fast.py"), "iterations=10 # fast original").unwrap();
    std::fs::write(dir.join("precise.py"), "iterations=100 # precise original").unwrap();
    let mut cfg = s.member_config().unwrap();
    cfg.executor.mode = "auto".into();
    cfg.executor.codex =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/business_codex.py");
    cfg.executor.timeout_secs = 20;
    s.configure_member(&cfg).unwrap();
}
#[test]
fn drafts_confirmation_revision_fence_and_independent_chats() {
    let t = Team::new();
    let (i, id) = draft(&t, "@b 找实验脚本");
    let mut a = t.store("a");
    assert_eq!(tasks::get(&a, &id).unwrap()["state"], "draft");
    assert_eq!(count(&a, "SELECT count(*) FROM messages"), 0);
    assert_eq!(count(&a, "SELECT count(*) FROM tasks"), 0);
    assert_eq!(submit(&mut a, &i)["task_id"], id);
    assert_eq!(count(&a, "SELECT count(*) FROM app_tasks"), 1);
    let p = app::project(&a, &t.dir.path().join("a/work")).unwrap();
    let other = app::new_session(&a, p["id"].as_str().unwrap()).unwrap();
    assert_ne!(other, i.session_id);
    assert!(app::history(&a, &other).unwrap().is_empty());
    let old = action(&i, &id, "confirm_task", 1, "确认");
    tasks::revise(&a, &action(&i, &id, "revise_task", 1, "只选择精确方案")).unwrap();
    assert!(tasks::confirm(&a, &old).is_err());
    assert_eq!(count(&a, "SELECT count(*) FROM messages"), 0);
    tasks::prepare(
        &a,
        &id,
        json!({"goal":"精确方案","deliverables":["说明"],"questions":[]}),
    )
    .unwrap();
    let confirm = action(&i, &id, "confirm_task", 2, "确认");
    tasks::confirm(&a, &confirm).unwrap();
    tasks::confirm(&a, &confirm).unwrap();
    assert_eq!(count(&a, "SELECT count(*) FROM messages"), 1);
    tasks::cancel(&a, &action(&i, &id, "stop_task", 2, "取消")).unwrap();
    let mut b = t.store("b");
    transfer(&a, &mut b);
    assert_eq!(tasks::get(&b, &id).unwrap()["state"], "cancelled");
    assert_eq!(count(&b, "SELECT count(*) FROM task_executions"), 0);
    tasks::revise(&a, &action(&i, &id, "reopen_task", 2, "重开并比较两种方法")).unwrap();
    assert_eq!(tasks::get(&a, &id).unwrap()["revision"], 3);
    assert_eq!(count(&a, "SELECT count(*) FROM task_revisions"), 3);
}
#[test]
fn bounded_snapshot_survives_live_edits_and_respects_revocation() {
    let t = Team::new();
    files(&t, "a");
    let (_, id) = draft(&t, "@b 查找文件");
    let s = t.store("a");
    let task = tasks::get(&s, &id).unwrap();
    task_materials::bind(&s, &task).unwrap();
    let refs = json!([{"root":0,"path":"fast.py"}]);
    let ex = uuid::Uuid::new_v4().to_string();
    let snapshot = task_materials::freeze(&s, &task, &ex, &refs).unwrap();
    std::fs::write(t.dir.path().join("a/work/fast.py"), "changed live source").unwrap();
    let (dir, manifest) = task_materials::verify(&s, &snapshot).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("root-0/fast.py")).unwrap(),
        "iterations=10 # fast original"
    );
    assert_eq!(manifest[0]["path"], "fast.py");
    std::os::unix::fs::symlink(t.db("b"), t.dir.path().join("a/work/outside")).unwrap();
    assert!(
        task_materials::freeze(&s, &task, "escape", &json!([{"root":0,"path":"outside"}])).is_err()
    );
    assert!(
        task_materials::freeze(
            &s,
            &task,
            "traversal",
            &json!([{"root":0,"path":"../tasks.sqlite3"}])
        )
        .is_err()
    );
    std::fs::write(t.dir.path().join("a/work/binary"), [0, 1, 2]).unwrap();
    assert!(
        task_materials::freeze(&s, &task, "binary", &json!([{"root":0,"path":"binary"}])).is_err()
    );
    assert!(
        task_materials::freeze(
            &s,
            &task,
            "missing",
            &json!([{"root":0,"path":"missing.py"}])
        )
        .is_err()
    );
    let c = rusqlite::Connection::open(&s.path).unwrap();
    c.execute("UPDATE task_settings SET max_bytes=1", [])
        .unwrap();
    assert!(task_materials::freeze(&s, &task, "oversized", &refs).is_err());
    c.execute("UPDATE task_settings SET max_bytes=2097152,max_files=1", [])
        .unwrap();
    assert!(
        task_materials::freeze(
            &s,
            &task,
            "too-many",
            &json!([{"root":0,"path":"fast.py"},{"root":0,"path":"precise.py"}])
        )
        .is_err()
    );
    let secrets = t.dir.path().join("a/work/private.env");
    std::fs::write(&secrets, "TEST_PRIVATE=synthetic-no-real-secret").unwrap();
    let mut s = s;
    let mut cfg = s.member_config().unwrap();
    cfg.secrets_file = Some(secrets);
    s.configure_member(&cfg).unwrap();
    assert!(
        task_materials::freeze(
            &s,
            &task,
            "private",
            &json!([{"root":0,"path":"private.env"}])
        )
        .is_err()
    );
    s.remove_directory(&t.dir.path().join("a/work")).unwrap();
    assert!(task_materials::verify(&s, &snapshot).is_err());
}
async fn model_server() -> (String, tokio::task::JoinHandle<()>) {
    model_server_with_invalid_reviews(0).await
}
async fn model_server_with_invalid_reviews(
    invalid_reviews: usize,
) -> (String, tokio::task::JoinHandle<()>) {
    use std::sync::{Arc, Mutex};
    let remaining = Arc::new(Mutex::new(invalid_reviews));
    let router=axum::Router::new().route("/chat/completions",axum::routing::post(move |axum::Json(v):axum::Json<Value>| {
        let remaining=remaining.clone();
        async move {
        let ctx:Value=serde_json::from_str(v["messages"][1]["content"].as_str().unwrap()).unwrap();
        let body=ctx["extra"]["body"].as_str().unwrap_or("");
        assert_eq!(ctx["collaboration"]["executor_members"], json!(["b"]));
        let mut result=match ctx["stage"].as_str().unwrap(){
            "resolve" if body.starts_with("请调查") && ctx["task"]["original"].as_str().unwrap().contains("跨端调查回归") && ctx["local_member"]=="b" => json!({"action":"escalate","body":body}),
            "resolve" if body.starts_with("请调查")=>{
                if body.contains("补充原目录") {
                    assert_eq!(ctx["extra"]["question_execution"]["phase"], "analyze");
                    assert_eq!(ctx["extra"]["material_discovery_scope"]["rediscovery_available"], true);
                    assert_eq!(ctx["extra"]["material_discovery_scope"]["member"], "b");
                    assert_eq!(ctx["extra"]["material_discovery_scope"]["source_roots"].as_array().unwrap().len(), 1);
                }
                json!({"action":"investigate","body":"读取本地脚本确认 iterations 参数，不猜测。"})
            },
            "resolve" if body=="本地成员是谁？"=>json!({"action":"answer","body":"b"}),
            "resolve" if body=="实验模式标签是什么？"&&ctx["local_member"]=="a"=>json!({"action":"answer","body":"实验模式 MARINER"}),
            "resolve"=>json!({"action":"escalate","body":body}),
            "followup" => json!({"action":"reply","body":"这个问题是想知道你更看重速度还是准确。按 CtrlB 选问题后告诉我即可。"}),
            "review" if ctx["task"]["original"].as_str().unwrap().contains("跨端调查回归") && !ctx["task"]["history"].as_array().unwrap().iter().any(|h| h["kind"]=="agent_review") => json!({"action":"investigate","body":"请再次核对两种脚本的参数"}),
            "review" if ctx["extra"]["candidates"][0]["result"]["body"].as_str().unwrap().contains("缺参数")=>json!({"action":"continue","body":"补充 precise.py 的 iterations 参数及 fast.py 的差异"}),
            "review"=>json!({"action":"complete","body":"选择 precise.py；iterations=100。fast.py 为10。依据两份固定快照，已补齐所有交付项。"}),
            "prepare"=>json!({"action":"prepare","body":"需求复述","goal":"入口比较","deliverables":["入口","参数"],"questions":[],"constraints":"只读","mode":"analysis"}),
            _=>panic!("unexpected stage {ctx}")};
        if ctx["stage"]=="review" {
            assert_eq!(v["tools"][0]["function"]["parameters"]["properties"]["action"]["enum"],json!(["complete","continue","escalate","investigate"]));
            let mut remaining=remaining.lock().unwrap();
            if *remaining>0 {
                *remaining-=1;
                // Valid JSON, but this belongs to the resolve stage, not review.
                result["action"]=json!("answer");
            }
        }
        axum::Json(json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"decision","type":"function","function":{"name":"task_decision","arguments":result.to_string()}}]}}]}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async { axum::serve(listener, router).await.unwrap() });
    (url, handle)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhausted_review_preserves_candidates_and_recovers_without_restarting_execution() {
    let t = Team::new();
    files(&t, "b");
    let (url, server) = model_server_with_invalid_reviews(3).await;
    for member in ["a", "b"] {
        let mut store = t.store(member);
        let mut cfg = store.member_config().unwrap();
        cfg.model = json!({"provider":"compatible","base_url":url,"model":"simulation","api_key_env":"","allow_insecure_http":true,"thinking":false});
        store.configure_member(&cfg).unwrap();
    }
    let (i, id) = draft(&t, "@b 选择实验入口脚本，说明参数、候选差异与依据");
    let mut a = t.store("a");
    let mut b = t.store("b");
    tasks::confirm(&a, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    for _ in 0..35 {
        transfer(&a, &mut b);
        transfer(&b, &mut a);
        tasks::tick(&b, Path::new(BIN)).await.unwrap();
        tasks::tick(&a, Path::new(BIN)).await.unwrap();
        let task = tasks::get(&a, &id).unwrap();
        if task["state"] == "needs_attention" {
            break;
        }
        for question in task["questions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|q| q["state"] == "user")
        {
            let mut answer = action(&i, &id, "answer_question", 1, "精确结果");
            answer.payload["question_id"] = question["id"].clone();
            tasks::answer(&a, &answer).unwrap();
        }
    }
    let task = tasks::get(&a, &id).unwrap();
    assert_eq!(task["state"], "needs_attention", "{task:#}");
    assert!(
        task["error"]
            .as_str()
            .unwrap()
            .contains("结果检查连续 3 次")
    );
    assert_eq!(task["archived"], false);
    assert_eq!(
        count(
            &a,
            "SELECT count(*) FROM task_model_attempts WHERE stage='review' AND state='failed'"
        ),
        3
    );
    assert_eq!(
        count(
            &a,
            "SELECT count(*) FROM task_model_attempts WHERE stage='review' AND state='succeeded'"
        ),
        0
    );
    assert_eq!(
        count(
            &a,
            "SELECT count(*) FROM task_journal WHERE kind='agent_review'"
        ),
        0
    );
    assert_eq!(count(&a, "SELECT count(*) FROM task_candidates"), 1);
    assert_eq!(
        count(
            &a,
            "SELECT count(*) FROM app_messages WHERE kind='task_completed'"
        ),
        0
    );
    let executions = tasks::get(&b, &id).unwrap()["executions"].clone();
    tasks::recover(&a, &action(&i, &id, "retry_task", 1, "重试结果检查")).unwrap();
    tasks::tick(&a, Path::new(BIN)).await.unwrap();
    assert_eq!(tasks::get(&b, &id).unwrap()["executions"], executions);
    assert_eq!(count(&a, "SELECT count(*) FROM task_executions"), 0);
    for _ in 0..20 {
        transfer(&a, &mut b);
        transfer(&b, &mut a);
        tasks::tick(&b, Path::new(BIN)).await.unwrap();
        tasks::tick(&a, Path::new(BIN)).await.unwrap();
        if tasks::get(&a, &id).unwrap()["state"] == "completed" {
            break;
        }
    }
    let completed = tasks::get(&a, &id).unwrap();
    assert_eq!(completed["state"], "completed", "{completed:#}");
    assert_eq!(completed["revision"], 1);
    assert_eq!(completed["archived"], true);
    assert_eq!(
        count(
            &a,
            "SELECT count(*) FROM app_messages WHERE kind='task_completed'"
        ),
        1
    );
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_round_mcp_resume_review_archive_reopen_and_tui() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let t = Team::new();
    files(&t, "b");
    let (url, server) = model_server_with_invalid_reviews(2).await;
    for m in ["a", "b"] {
        let mut s = t.store(m);
        let mut cfg = s.member_config().unwrap();
        cfg.model = json!({"provider":"compatible","base_url":url,"model":"simulation","api_key_env":"","allow_insecure_http":true,"thinking":false});
        s.configure_member(&cfg).unwrap();
    }
    let (i, id) = draft(&t, "@b 选择实验入口脚本，说明参数、候选差异与依据");
    let mut a = t.store("a");
    let mut b = t.store("b");
    let p = app::project(&a, &t.dir.path().join("a/work")).unwrap();
    let mut ui = xxassxx::tui::Ui::new(&a, p).unwrap();
    ui.selected_task = Some(id.clone());
    ui.refresh(&a).unwrap();
    ui.key(
        &mut a,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
    )
    .unwrap();
    // Initial draft was prepared by the deterministic test coordinator; skip its
    // queued prepare command by processing it normally through the mock model.
    app::model::process_one(&mut a).await.unwrap();
    app::model::process_one(&mut a).await.unwrap();
    assert_eq!(tasks::get(&a, &id).unwrap()["state"], "queued");
    let mut answered = false;
    let mut live_changed = false;
    for _ in 0..35 {
        transfer(&a, &mut b);
        transfer(&b, &mut a);
        tasks::tick(&b, Path::new(BIN)).await.unwrap();
        tasks::tick(&a, Path::new(BIN)).await.unwrap();
        let bt = tasks::get(&b, &id).unwrap();
        if !live_changed && !bt["materials"].as_array().unwrap().is_empty() {
            std::fs::write(
                t.dir.path().join("b/work/precise.py"),
                "iterations=999 changed live",
            )
            .unwrap();
            live_changed = true;
        }
        let at = tasks::get(&a, &id).unwrap();
        if at["state"] == "waiting" && at["waiting_for"] == "等待用户回答" && !answered {
            assert_ne!(at["state"], "completed");
            ui.refresh(&a).unwrap();
            ui.key(
                &mut a,
                KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
            )
            .unwrap();
            ui.key(&mut a, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                .unwrap();
            ui.input.insert("精确结果");
            ui.key(&mut a, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                .unwrap();
            app::model::process_one(&mut a).await.unwrap();
            answered = true;
        }
        if at["state"] == "completed" {
            break;
        }
    }
    let final_task = tasks::get(&a, &id).unwrap();
    assert_eq!(final_task["state"], "completed", "{final_task:#}");
    assert_eq!(
        count(
            &a,
            "SELECT count(*) FROM task_model_attempts WHERE stage='review' AND state='failed'"
        ),
        2
    );
    assert!(answered && live_changed);
    assert_eq!(final_task["archived"], true);
    assert_eq!(
        count(
            &a,
            "SELECT count(*) FROM app_messages WHERE kind='task_completed'"
        ),
        1
    );
    let bt = tasks::get(&b, &id).unwrap();
    let analysis = bt["executions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["phase"] == "analyze")
        .unwrap();
    let runs = b.runs(analysis["id"].as_str().unwrap()).unwrap();
    assert_eq!(runs.len(), 5);
    assert!(
        runs.iter()
            .skip(1)
            .all(|r| r.resumed_session_id == runs[0].session_id)
    );
    assert_eq!(
        count(
            &a,
            "SELECT count(*) FROM task_questions WHERE state='answered'"
        ),
        2
    ); // B resolved its own identity.
    let snap = bt["materials"][0]["id"].as_str().unwrap();
    let (_, manifest) = task_materials::verify(&b, snap).unwrap();
    assert_eq!(manifest.as_array().unwrap().len(), 2);
    // Reopen through the same UI/application action, preserving global identity.
    ui.refresh(&a).unwrap();
    ui.key(
        &mut a,
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
    )
    .unwrap();
    ui.input.insert("改为比较快速方案");
    ui.key(&mut a, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    app::model::process_one(&mut a).await.unwrap();
    let reopened = tasks::get(&a, &id).unwrap();
    assert_eq!(reopened["revision"], 2);
    assert_eq!(reopened["state"], "draft");
    assert_eq!(reopened["revisions"].as_array().unwrap().len(), 2);
    transfer(&b, &mut a);
    assert_eq!(tasks::get(&a, &id).unwrap()["state"], "draft");
    assert_eq!(
        count(
            &a,
            "SELECT count(*) FROM app_messages WHERE kind='task_completed'"
        ),
        1
    );
    ui.refresh(&a).unwrap();
    ui.key(
        &mut a,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
    )
    .unwrap();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 35)).unwrap();
    terminal.draw(|f| ui.render(f, &a)).unwrap();
    assert!(format!("{:?}", terminal.backend().buffer()).contains("工作目录"));
    server.abort();
    let _ = i;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_waiting_tasks_cannot_cross_answer_or_accept_late_revision() {
    use xxassxx::team::{Message, stable_id};
    let t = Team::new();
    let (url, server) = model_server().await;
    let mut a = t.store("a");
    let mut cfg = a.member_config().unwrap();
    cfg.model =
        json!({"provider":"compatible","base_url":url,"model":"simulation","api_key_env":""});
    a.configure_member(&cfg).unwrap();
    let mut cards = Vec::new();
    for body in ["@b first independent job", "@b second independent job"] {
        let (i, id) = draft(&t, body);
        tasks::confirm(&a, &action(&i, &id, "confirm_task", 1, "confirm")).unwrap();
        let question = uuid::Uuid::new_v4().to_string();
        let m=Message{message_id:uuid::Uuid::new_v4().to_string(),conversation_id:stable_id(&id,"a:b"),sender:"b".into(),recipient:"a".into(),kind:"request".into(),operation:"task_v2".into(),body:serde_json::to_string(&tasks::Wire{version:2,task_id:id.clone(),revision:1,sequence:1,event:"question".into(),data:json!({"id":question,"body":"用户要快速还是精确结果？","reason":"范围内的必要事实","known":"两种候选"})}).unwrap(),created_at:1,reply_to:None,object_id:None,version_id:None,workflow:None};
        a.save_message(&m, true).unwrap();
        tasks::ingest(&a, &m).unwrap();
        tasks::tick(&a, Path::new(BIN)).await.unwrap();
        cards.push((i, id, question, m));
    }
    assert!(
        cards
            .iter()
            .all(|(_, id, _, _)| tasks::get(&a, id).unwrap()["waiting_for"] == "等待用户回答")
    );
    let mut answer = action(&cards[0].0, &cards[0].1, "answer_task", 1, "精确");
    answer.payload["question_id"] = json!(cards[1].2);
    assert!(tasks::answer(&a, &answer).is_err());
    answer.payload["question_id"] = json!(cards[0].2);
    tasks::answer(&a, &answer).unwrap();
    tasks::answer(&a, &answer).unwrap();
    assert_eq!(
        tasks::get(&a, &cards[1].1).unwrap()["questions"][0]["state"],
        "user"
    );
    let reopen = action(&cards[0].0, &cards[0].1, "revise_task", 1, "改变交付范围");
    tasks::revise(&a, &reopen).unwrap();
    assert!(tasks::answer(&a, &answer).is_err());
    tasks::ingest(&a, &cards[0].3).unwrap();
    assert_eq!(tasks::get(&a, &cards[0].1).unwrap()["revision"], 2);
    assert_eq!(tasks::get(&a, &cards[0].1).unwrap()["state"], "draft");
    server.abort();
}
#[tokio::test]
async fn chosen_directory_is_bound_before_ui_switch_and_invalid_scope_blocks() {
    let t = Team::new();
    let mut s = t.store("a");
    let chosen = t.dir.path().join("a/work");
    let first = t.dir.path().join("a/aaa");
    std::fs::create_dir_all(&first).unwrap();
    s.allow_directory(&first).unwrap();
    s.allow_directory(&chosen).unwrap();
    let mut i = input(&t, "只读查找");
    i.action = "create_task".into();
    let id = submit(&mut s, &i)["task_id"].as_str().unwrap().to_owned();
    tasks::prepare(
        &s,
        &id,
        json!({"goal":"查找","deliverables":["文件"],"questions":[],"mode":"listing"}),
    )
    .unwrap();
    tasks::confirm(&s, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    let other = app::project(&s, &first).unwrap();
    app::select_working_directory(&s, other["id"].as_str().unwrap()).unwrap();
    tasks::tick(&s, Path::new(BIN)).await.unwrap();
    let task = tasks::get(&s, &id).unwrap();
    let ex = task["executions"][0]["id"].as_str().unwrap();
    let conn = rusqlite::Connection::open(&s.path).unwrap();
    assert_eq!(
        xxassxx::native_tasks::working_directory(&conn, ex)
            .unwrap()
            .unwrap(),
        chosen.canonicalize().unwrap()
    );
    assert_eq!(
        task["project"],
        chosen.canonicalize().unwrap().to_str().unwrap()
    );
    s.remove_directory(&chosen).unwrap();
    assert!(tasks::execution_context(&conn, ex).is_err());
    assert_eq!(count(&s, "SELECT count(*) FROM runs"), 0);
}

#[test]
fn local_task_facts_use_authenticated_relay_and_survive_lost_ack() {
    let t = Team::new();
    let mut s = t.store("a");
    let mut i = input(&t, "只读列出本地脚本");
    i.action = "create_task".into();
    let id = submit(&mut s, &i)["task_id"].as_str().unwrap().to_owned();
    tasks::prepare(
        &s,
        &id,
        json!({"goal":"列出脚本","deliverables":["路径清单"],"questions":[],"mode":"listing"}),
    )
    .unwrap();
    assert_eq!(count(&s, "SELECT count(*) FROM task_fact_outbox"), 0);
    tasks::confirm(&s, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    t.sync("a");
    let relay = rusqlite::Connection::open(t.dir.path().join("relay.sqlite3")).unwrap();
    assert_eq!(
        relay
            .query_row("SELECT count(*) FROM relay_tasks", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        relay
            .query_row("SELECT count(*) FROM relay_messages", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    rusqlite::Connection::open(&s.path)
        .unwrap()
        .execute("UPDATE task_fact_outbox SET sent=0", [])
        .unwrap();
    t.sync("a");
    assert_eq!(
        relay
            .query_row("SELECT count(*) FROM relay_local_facts", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    tasks::cancel(&s, &action(&i, &id, "stop_task", 1, "取消")).unwrap();
    t.sync("a");
    assert_eq!(
        relay
            .query_row("SELECT state FROM relay_tasks", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "cancelled"
    );
    assert_eq!(count(&s, "SELECT count(*) FROM tasks"), 0);
}

#[tokio::test]
async fn uncertain_restart_stops_and_meeting_cannot_replay_execution() {
    let t = Team::new();
    files(&t, "b");
    let (i, id) = draft(&t, "@b 比较脚本");
    let a = t.store("a");
    let mut b = t.store("b");
    tasks::confirm(&a, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    transfer(&a, &mut b);
    let c = rusqlite::Connection::open(&b.path).unwrap();
    c.execute("UPDATE task_jobs SET state='running',attempts=1", [])
        .unwrap();
    drop(b);
    let b = t.store("b");
    tasks::tick(&b, Path::new(BIN)).await.unwrap();
    assert_eq!(tasks::get(&b, &id).unwrap()["state"], "needs_attention");
    assert_eq!(count(&b, "SELECT count(*) FROM runs"), 0);
    let event = json!({"id":uuid::Uuid::new_v4().to_string(),"kind":"meeting_invitation","payload":{"source":"system","meeting_id":uuid::Uuid::new_v4().to_string(),"task_id":id,"revision":1}});
    let report = xxassxx::task_schedule::local_event(&b, &event)
        .unwrap()
        .unwrap();
    assert_eq!(report["state"], "needs_attention");
    xxassxx::task_schedule::local_event(&b, &event).unwrap();
    tasks::tick(&b, Path::new(BIN)).await.unwrap();
    assert_eq!(count(&b, "SELECT count(*) FROM task_system_events"), 1);
    assert_eq!(count(&b, "SELECT count(*) FROM runs"), 0);
    let mut retry = action(&i, &id, "retry_task", 1, "显式重试");
    retry.recipient = "b".into();
    tasks::recover(&b, &retry).unwrap();
    assert_eq!(tasks::get(&b, &id).unwrap()["state"], "processing");
    assert_eq!(
        count(&b, "SELECT count(*) FROM task_jobs WHERE state='pending'"),
        1
    );
}

#[tokio::test]
async fn cancelled_task_rejects_new_submissions_and_old_progress_is_historical() {
    use xxassxx::team::{Message, stable_id};
    let t = Team::new();
    files(&t, "b");
    let (i, id) = draft(&t, "@b 比较脚本");
    let a = t.store("a");
    let mut b = t.store("b");
    let mut cfg = b.member_config().unwrap();
    cfg.executor.mode = "queue".into();
    b.configure_member(&cfg).unwrap();
    tasks::confirm(&a, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    transfer(&a, &mut b);
    tasks::tick(&b, Path::new(BIN)).await.unwrap();
    let ex = tasks::get(&b, &id).unwrap()["executions"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let m = |seq, event, data| Message {
        message_id: uuid::Uuid::new_v4().to_string(),
        conversation_id: stable_id(&id, "a:b"),
        sender: "b".into(),
        recipient: "a".into(),
        kind: "request".into(),
        operation: "task_v2".into(),
        body: serde_json::to_string(&tasks::Wire {
            version: 2,
            task_id: id.clone(),
            revision: 1,
            sequence: seq,
            event,
            data,
        })
        .unwrap(),
        created_at: 1,
        reply_to: None,
        object_id: None,
        version_id: None,
        workflow: None,
    };
    let newer = m(
        100,
        "progress".into(),
        json!({"state":"needs_attention","body":"配置丢失"}),
    );
    tasks::ingest(&a, &newer).unwrap();
    let late = m(99, "accepted".into(), json!({"body":"已接收"}));
    tasks::ingest(&a, &late).unwrap();
    assert_eq!(tasks::get(&a, &id).unwrap()["waiting_for"], "配置丢失");
    tasks::cancel(&a, &action(&i, &id, "stop_task", 1, "取消")).unwrap();
    transfer(&a, &mut b);
    let c = rusqlite::Connection::open(&b.path).unwrap();
    assert!(
        tasks::validate_question(
            &c,
            &ex,
            &json!({"outcome":"question","body":"?","reason":"missing","known":"none"})
        )
        .is_err()
    );
    assert!(
        xxassxx::native_tasks::validate(
            &c,
            &ex,
            &json!({"body":"old result","files":[]}).to_string()
        )
        .is_err()
    );
    tasks::tick(&b, Path::new(BIN)).await.unwrap();
    assert_eq!(count(&b, "SELECT count(*) FROM runs"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_resolve_and_review_investigations_never_switch_executor() {
    let t = Team::new();
    files(&t, "a"); // A has readable files too: that is not permission to change executor.
    files(&t, "b");
    let (url, server) = model_server().await;
    for m in ["a", "b"] {
        let mut s = t.store(m);
        let mut cfg = s.member_config().unwrap();
        cfg.model =
            json!({"provider":"compatible","base_url":url,"model":"simulation","api_key_env":""});
        s.configure_member(&cfg).unwrap();
    }
    let mut a = t.store("a");
    let mut b = t.store("b");
    let (i, id) = draft(&t, "@b 跨端调查回归：比较脚本参数");
    tasks::confirm(&a, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    for _ in 0..18 {
        transfer(&a, &mut b);
        transfer(&b, &mut a);
        tasks::tick(&b, Path::new(BIN)).await.unwrap();
        tasks::tick(&a, Path::new(BIN)).await.unwrap();
        if tasks::get(&a, &id).unwrap()["state"] == "completed" {
            break;
        }
    }
    let view = tasks::get(&a, &id).unwrap();
    assert_eq!(view["state"], "completed", "{view:#}");
    assert_eq!(count(&a, "SELECT count(*) FROM task_executions"), 0);
    assert_eq!(count(&a, "SELECT count(*) FROM task_bindings"), 0);
    assert_eq!(count(&a, "SELECT count(*) FROM runs"), 0);
    assert_eq!(
        count(&b, "SELECT count(*) FROM runs WHERE state='succeeded'"),
        4
    );
    assert_eq!(view["questions"][0]["state"], "superseded");
    assert!(
        view["history"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["kind"] == "agent_review" && h["payload"]["action"] == "investigate")
    );
    transfer(&a, &mut b);
    assert_eq!(tasks::get(&b, &id).unwrap()["state"], "completed");
    server.abort();
}

#[tokio::test]
async fn task_chat_and_question_answer_are_separate_and_view_matches_target() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let t = Team::new();
    let (i, first) = draft(&t, "@b 旧任务独有内容");
    let (_, second) = draft(&t, "@b 新任务独有内容");
    let mut a = t.store("a");
    tasks::confirm(&a, &action(&i, &first, "confirm_task", 1, "确认")).unwrap();
    let c = rusqlite::Connection::open(&a.path).unwrap();
    let q = uuid::Uuid::new_v4().to_string();
    c.execute("INSERT INTO task_questions(id,task_id,revision,asker,body,reason,known,state,created_at) VALUES(?1,?2,1,'b','要快还是精确？','需要选择','','user',1)",rusqlite::params![q,first]).unwrap();
    let p = app::project(&a, &t.dir.path().join("a/work")).unwrap();
    let mut ui = xxassxx::tui::Ui::new(&a, p).unwrap();
    ui.session = i.session_id.clone();
    ui.selected_task = Some(first.clone());
    ui.refresh(&a).unwrap();
    assert!(ui.visible_messages().iter().all(|m| m["task_id"] == first));
    assert!(
        !ui.visible_messages()
            .iter()
            .any(|m| m["body"].as_str().unwrap().contains("新任务独有内容"))
    );
    ui.input.insert("要我回答什么？");
    ui.key(&mut a, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    let (id, action, task): (String, String, String) = c
        .query_row(
            "SELECT id,action,task_id FROM app_commands ORDER BY rowid DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(action, "chat");
    assert_eq!(task, first);
    assert_eq!(
        count(&a, "SELECT count(*) FROM task_questions WHERE state='user'"),
        1
    );
    // A command may receive its new task ID only after background model processing.
    ui.refresh(&a).unwrap();
    c.execute(
        "UPDATE app_commands SET task_id=?2,state='done' WHERE id=?1",
        rusqlite::params![id, second],
    )
    .unwrap();
    ui.refresh(&a).unwrap();
    assert_eq!(ui.selected_task, Some(second.clone()));
    assert!(ui.visible_messages().iter().all(|m| m["task_id"] == second));
    ui.selected_task = Some(first.clone());
    ui.refresh(&a).unwrap();
    ui.key(
        &mut a,
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
    )
    .unwrap();
    ui.key(&mut a, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    ui.input.insert("精确结果");
    ui.key(&mut a, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    let (action, payload): (String, String) = c
        .query_row(
            "SELECT action,payload FROM app_commands ORDER BY rowid DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(action, "answer_task");
    assert_eq!(
        serde_json::from_str::<Value>(&payload).unwrap()["payload"]["question_id"],
        q
    );
}

#[tokio::test]
async fn configured_automatic_budget_blocks_without_running_codex() {
    let t = Team::new();
    files(&t, "b");
    let (i, id) = draft(&t, "@b 比较脚本");
    let a = t.store("a");
    let mut b = t.store("b");
    tasks::confirm(&a, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    transfer(&a, &mut b);
    let mut cfg = b.member_config().unwrap();
    cfg.executor.mode = "queue".into();
    b.configure_member(&cfg).unwrap();
    tasks::tick(&b, Path::new(BIN)).await.unwrap();
    let c = rusqlite::Connection::open(&b.path).unwrap();
    c.execute("UPDATE task_settings SET max_rounds=1", [])
        .unwrap();
    c.execute("UPDATE task_executions SET round=1", []).unwrap();
    cfg.executor.mode = "auto".into();
    b.configure_member(&cfg).unwrap();
    tasks::tick(&b, Path::new(BIN)).await.unwrap();
    assert_eq!(tasks::get(&b, &id).unwrap()["state"], "needs_attention");
    assert_eq!(count(&b, "SELECT count(*) FROM runs"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_analysis_materials_start_new_discovery_preserving_old_snapshot() {
    let t = Team::new();
    files(&t, "b");
    let (url, server) = model_server().await;
    let a = t.store("a");
    let mut b = t.store("b");
    let mut cfg = b.member_config().unwrap();
    cfg.model =
        json!({"provider":"compatible","base_url":url,"model":"simulation","api_key_env":""});
    b.configure_member(&cfg).unwrap();
    let (i, id) = draft(&t, "@b 补充材料回归：比较两个脚本");
    tasks::confirm(&a, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    transfer(&a, &mut b);
    tasks::tick(&b, Path::new(BIN)).await.unwrap();
    let c = rusqlite::Connection::open(&b.path).unwrap();
    let original: String = c
        .query_row("SELECT id FROM task_snapshots", [], |r| r.get(0))
        .unwrap();
    let (old_dir, _) = task_materials::verify(&b, &original).unwrap();
    std::fs::write(
        t.dir.path().join("b/work/fast.py"),
        "iterations=20 # new source",
    )
    .unwrap();
    tasks::tick(&b, Path::new(BIN)).await.unwrap();
    assert_eq!(
        tasks::get(&b, &id).unwrap()["questions"][0]["state"],
        "coordinating"
    );
    tasks::tick(&b, Path::new(BIN)).await.unwrap();
    assert_eq!(count(&b, "SELECT count(*) FROM task_snapshots"), 2);
    assert_eq!(
        count(
            &b,
            "SELECT count(*) FROM task_executions WHERE phase='analyze' AND state='historical'"
        ),
        1
    );
    assert_eq!(count(&b, "SELECT count(*) FROM file_roots"), 1);
    assert_eq!(
        std::fs::read_to_string(old_dir.join("root-0/fast.py")).unwrap(),
        "iterations=10 # fast original"
    );
    tasks::tick(&b, Path::new(BIN)).await.unwrap();
    let view = tasks::get(&b, &id).unwrap();
    assert_eq!(view["questions"][0]["state"], "answered");
    let answer = view["questions"][0]["answer"].as_str().unwrap();
    assert!(answer.contains("iterations=100") && answer.contains("iterations=20"));
    assert_eq!(
        count(&b, "SELECT count(*) FROM runs WHERE state='succeeded'"),
        4
    );
    assert_eq!(
        count(
            &b,
            "SELECT count(*) FROM runs WHERE resumed_session_id IS NOT NULL"
        ),
        0
    );
    assert_eq!(count(&b, "SELECT count(DISTINCT session_id) FROM runs"), 4);
    assert_eq!(count(&b, "SELECT count(*) FROM task_candidates"), 0);
    let candidates = b
        .messages(None)
        .unwrap()
        .into_iter()
        .filter_map(|r| tasks::Wire::parse(&r.message).ok())
        .filter(|w| w.event == "candidate")
        .collect::<Vec<_>>();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].data["sources"].as_array().unwrap().len(), 2);
    assert_eq!(count(&b, "SELECT max_bytes FROM task_settings"), 2097152);
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_path_echo_and_text_confirmation_cross_member_delivery() {
    let t = Team::new();
    files(&t, "a");
    files(&t, "b");
    let local = t.dir.path().join("a/work").to_string_lossy().into_owned();
    let echoed = local.clone();
    let router = axum::Router::new().route("/chat/completions", axum::routing::post(move |axum::Json(v): axum::Json<Value>| {
        let echoed = echoed.clone();
        async move {
            let ctx: Value = serde_json::from_str(v["messages"][1]["content"].as_str().unwrap()).unwrap();
            assert_eq!(ctx["collaboration"]["executor_members"], json!(["b"]));
            assert_eq!(ctx["local_resources"]["member"], "a");
            assert_eq!(ctx["local_resources"]["working_directory"], "member://a/work");
            assert!(!ctx.to_string().contains(&echoed));
            // Deliberately emulate the live failure even though the prompt says not to.
            let decision = json!({"action":"prepare","body":"请 b 在自己的目录只读清点","goal":"了解远端 MOOSE 进展","deliverables":["相关文件和进展依据"],"constraints":format!("只读；工作范围为 {echoed}；不执行脚本"),"known_context":format!("嵌套路径 ({echoed}/notes.md)"),"questions":[],"mode":"analysis"});
            axum::Json(json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"draft","type":"function","function":{"name":"task_decision","arguments":decision.to_string()}}]}}]}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async { axum::serve(listener, router).await.unwrap() });
    let mut a = t.store("a");
    let mut cfg = a.member_config().unwrap();
    cfg.model = json!({"provider":"compatible","base_url":url,"model":"simulation","api_key_env":"","allow_insecure_http":true,"thinking":false});
    a.configure_member(&cfg).unwrap();
    let i = input(
        &t,
        "@b MOOSE 在你那边的 scripts；只做只读清点，没有指定文件。",
    );
    let id = submit(&mut a, &i)["task_id"].as_str().unwrap().to_owned();
    app::model::process_one(&mut a).await.unwrap();
    assert_eq!(count(&a, "SELECT count(*) FROM messages"), 0);
    assert!(
        !tasks::get(&a, &id).unwrap()["draft"]
            .to_string()
            .contains(&local)
    );
    let confirm = action(&i, &id, "chat", 1, "确认。就是这个任务");
    submit(&mut a, &confirm);
    app::model::process_one(&mut a).await.unwrap();
    assert_eq!(
        app::command(&a, &confirm.request_id).unwrap()["state"],
        "done"
    );
    let task = tasks::get(&a, &id).unwrap();
    assert_eq!(task["revision"], 1);
    assert_eq!(task["confirmed"], 1);
    assert_eq!(task["state"], "queued");
    submit(&mut a, &confirm); // Retry the same request ID without recapturing a version.
    let duplicate = action(&i, &id, "chat", 1, "确认");
    submit(&mut a, &duplicate);
    app::model::process_one(&mut a).await.unwrap();
    assert_eq!(count(&a, "SELECT count(*) FROM messages"), 1);
    let wire = a.messages(None).unwrap().pop().unwrap().message;
    assert!(!wire.body.contains(&local));
    wire.validate().unwrap();
    t.sync("a");
    t.sync("b");
    let mut b = t.store("b");
    app::bridge::reconcile(&mut b).unwrap();
    let remote = tasks::get(&b, &id).unwrap();
    assert_eq!(
        remote["project"],
        t.dir
            .path()
            .join("b/work")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
    );
    task_materials::bind(&b, &remote).unwrap();
    let connection = rusqlite::Connection::open(&b.path).unwrap();
    let (bound, _) = task_materials::check_binding(&connection, &id, 1).unwrap();
    assert_eq!(bound, t.dir.path().join("b/work").canonicalize().unwrap());
    server.abort();
}

#[tokio::test]
async fn queued_text_confirmation_cannot_confirm_a_later_revision() {
    let t = Team::new();
    let (i, id) = draft(&t, "@b 查找文件");
    let mut a = t.store("a");
    let confirm = action(&i, &id, "chat", 1, "确认");
    submit(&mut a, &confirm);
    tasks::revise(&a, &action(&i, &id, "revise_task", 1, "改变交付内容")).unwrap();
    tasks::prepare(
        &a,
        &id,
        json!({"goal":"新目标","deliverables":["新结果"],"constraints":"只读","questions":[]}),
    )
    .unwrap();
    assert!(
        tasks::task_chat(&a, &confirm)
            .await
            .unwrap_err()
            .to_string()
            .contains("过期")
    );
    assert_eq!(tasks::get(&a, &id).unwrap()["confirmed"], Value::Null);
    assert_eq!(count(&a, "SELECT count(*) FROM messages"), 0);
    for body in [
        "不要确认",
        "确认之前先解释",
        "他说确认",
        "确认，但改成运行脚本",
        "好的",
    ] {
        assert!(!tasks::is_confirmation(body));
    }
}

#[test]
fn already_saved_path_bearing_draft_can_be_dispatched_without_rewriting_history() {
    let t = Team::new();
    let (i, id) = draft(&t, "@b 清点远端文件");
    let a = t.store("a");
    let local = t.dir.path().join("a/work").to_string_lossy().into_owned();
    let old = json!({"goal":"检查文件","deliverables":["结果"],"constraints":format!("只读 {local}"),"questions":[]}).to_string();
    let c = rusqlite::Connection::open(&a.path).unwrap();
    c.execute(
        "UPDATE app_tasks SET draft=?2 WHERE id=?1",
        rusqlite::params![id, old],
    )
    .unwrap();
    c.execute(
        "UPDATE task_revisions SET draft=?2 WHERE task_id=?1",
        rusqlite::params![id, old],
    )
    .unwrap();
    tasks::confirm(&a, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    assert!(
        tasks::get(&a, &id).unwrap()["draft"]
            .to_string()
            .contains(&local)
    );
    assert!(!a.messages(None).unwrap()[0].message.body.contains(&local));
}

#[tokio::test]
async fn material_budget_rejects_before_acceptance_and_recovery_keeps_run_history() {
    let t = Team::new();
    files(&t, "a");
    let mut i = input(&t, "清点代码并说明依据");
    i.action = "create_task".into();
    let mut s = t.store("a");
    let id = submit(&mut s, &i)["task_id"].as_str().unwrap().to_owned();
    tasks::prepare(&s, &id, json!({"goal":"清点代码","deliverables":["依据"],"constraints":"只读","questions":[],"mode":"analysis"})).unwrap();
    let mut cfg = s.member_config().unwrap();
    cfg.executor.mode = "queue".into();
    s.configure_member(&cfg).unwrap();
    tasks::confirm(&s, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    tasks::tick(&s, Path::new(BIN)).await.unwrap();
    let ex = tasks::get(&s, &id).unwrap()["executions"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let c = rusqlite::Connection::open(&s.path).unwrap();
    c.execute("UPDATE task_settings SET max_bytes=50", [])
        .unwrap();
    let lease = s
        .claim(&ex, false, &t.dir.path().join("a/work"), 30)
        .unwrap();
    s.initialized(&ex, &lease.run_id, &lease.token).unwrap();
    let context = s.read_task(&ex, &lease.run_id, &lease.token).unwrap();
    let native: Value = serde_json::from_str(context["input"].as_str().unwrap()).unwrap();
    assert_eq!(
        native["business_task"]["material_budget"]["max_total_bytes"],
        50
    );
    let bad = json!({"body":"候选文件","files":[{"root":0,"path":"fast.py","reason":"候选"},{"root":0,"path":"precise.py","reason":"候选"}]}).to_string();
    assert!(
        s.submit_result(&ex, &lease.run_id, &lease.token, &lease.run_id, &bad)
            .unwrap_err()
            .to_string()
            .contains("本次提交未接受")
    );
    assert!(s.runs(&ex).unwrap()[0].result.is_none());
    let good = json!({"body":"精简候选；其他未覆盖","files":[{"root":0,"path":"fast.py","reason":"候选"}]}).to_string();
    s.submit_result(&ex, &lease.run_id, &lease.token, &lease.run_id, &good)
        .unwrap();
    let session = uuid::Uuid::new_v4().to_string();
    s.event(
        &lease,
        &json!({"type":"thread.started","thread_id":session}),
    )
    .unwrap();
    s.event(&lease, &json!({"type":"turn.completed"})).unwrap();
    s.finish(&lease, Some(0), None).unwrap();
    // A live file can grow between validation and freezing. Keep the freeze-time
    // check too, and ensure explicit recovery can correct a formerly accepted run.
    std::fs::write(t.dir.path().join("a/work/fast.py"), "x".repeat(51)).unwrap();
    cfg.executor.mode = "auto".into();
    s.configure_member(&cfg).unwrap();
    tasks::tick(&s, Path::new(BIN)).await.unwrap();
    assert_eq!(tasks::get(&s, &id).unwrap()["state"], "needs_attention");
    tasks::recover(&s, &action(&i, &id, "retry_task", 1, "恢复")).unwrap();
    assert_eq!(s.task(&ex).unwrap().state, "pending");
    assert_eq!(
        s.task(&ex).unwrap().session_id.as_deref(),
        Some(session.as_str())
    );
    assert_eq!(
        s.runs(&ex).unwrap()[0].result.as_deref(),
        Some(good.as_str())
    );
    assert_eq!(
        tasks::get(&s, &id).unwrap()["executions"][0]["state"],
        "pending"
    );
}

#[tokio::test]
async fn remote_retry_stays_on_worker_and_legacy_wrong_host_execution_is_blocked() {
    let t = Team::new();
    files(&t, "a");
    files(&t, "b");
    let (i, id) = draft(&t, "@b 查看代码结构");
    let mut a = t.store("a");
    let mut b = t.store("b");
    let mut cfg = b.member_config().unwrap();
    cfg.executor.mode = "queue".into();
    b.configure_member(&cfg).unwrap();
    tasks::confirm(&a, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    transfer(&a, &mut b);
    tasks::tick(&b, Path::new(BIN)).await.unwrap();
    let cb = rusqlite::Connection::open(&b.path).unwrap();
    cb.execute(
        "UPDATE task_executions SET state='failed' WHERE task_id=?1",
        [&id],
    )
    .unwrap();
    cb.execute("UPDATE app_tasks SET state='needs_attention',error='timeout',waiting_reason='timeout',next_owner='b' WHERE id=?1",[&id]).unwrap();
    let ca = rusqlite::Connection::open(&a.path).unwrap();
    ca.execute("UPDATE app_tasks SET state='needs_attention',next_owner='b',waiting_reason='timeout' WHERE id=?1",[&id]).unwrap();
    tasks::recover(&a, &action(&i, &id, "retry_task", 1, "重试")).unwrap();
    transfer(&a, &mut b);
    transfer(&b, &mut a);
    assert_eq!(tasks::get(&b, &id).unwrap()["state"], "processing");
    assert_eq!(
        count(
            &b,
            "SELECT count(*) FROM task_executions WHERE state='pending'"
        ),
        1
    );
    assert_eq!(count(&a, "SELECT count(*) FROM tasks"), 0);
    // Old versions could save an A-side execution. Reject it even with a valid binding.
    task_materials::bind(&a, &tasks::get(&a, &id).unwrap()).unwrap();
    let ex = uuid::Uuid::new_v4().to_string();
    ca.execute(
        "INSERT INTO tasks(id,input,created_at) VALUES(?1,'{}',1)",
        [&ex],
    )
    .unwrap();
    ca.execute("INSERT INTO task_executions(id,task_id,revision,phase,state,created_at) VALUES(?1,?2,1,'discover','pending',1)",rusqlite::params![ex,id]).unwrap();
    assert!(
        tasks::check_execution(&ca, &ex)
            .unwrap_err()
            .to_string()
            .contains("指定成员")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_run_requires_local_revision_grant_and_returns_real_artifacts() {
    use xxassxx::task_workspace::{self, Command};
    let t = Team::new();
    files(&t, "b");
    let (i, id) = draft(&t, "@b 在独立目录修改脚本并运行");
    let mut a = t.store("a");
    tasks::prepare(&a,&id,json!({"goal":"修改副本并计算","deliverables":["计算结果和日志"],"mode":"execute","constraints":"原文件只读"})).unwrap();
    // A local grant cannot precede the immutable user confirmation.
    assert!(
        task_workspace::execute(
            &a,
            Command::Allow {
                task: id.clone(),
                revision: 1,
                parent: t.dir.path().join("a/work"),
                read_dirs: vec![],
                timeout_secs: 20
            }
        )
        .is_err()
    );
    tasks::confirm(&a, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    let mut b = t.store("b");
    transfer(&a, &mut b);
    tasks::tick(&b, Path::new(BIN)).await.unwrap();
    assert_eq!(tasks::get(&b, &id).unwrap()["state"], "waiting");
    for _ in 0..5 {
        tasks::tick(&b, Path::new(BIN)).await.unwrap();
    }
    assert_eq!(count(&b, "SELECT sum(attempts) FROM task_jobs"), 0);
    assert_eq!(
        count(&b, "SELECT count(*) FROM runs"),
        0,
        "read roots must never confer write permission"
    );
    transfer(&b, &mut a);
    assert_eq!(
        tasks::get(&a, &id).unwrap()["waiting_for"],
        "等待执行方允许写入和运行"
    );
    assert!(tasks::get(&a, &id).unwrap()["execution_permission"].is_null());
    // A real UI decision on B, no retry or new command on A.
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let p = app::project(&b, &t.dir.path().join("b/work")).unwrap();
    let mut ui = xxassxx::tui::Ui::new(&b, p).unwrap();
    let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
    ui.key(&mut b, ctrl('t')).unwrap();
    ui.key(&mut b, key(KeyCode::Enter)).unwrap();
    let mut screen = ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 50)).unwrap();
    screen.draw(|f| ui.render(f, &b)).unwrap();
    assert!(format!("{:?}", screen.backend().buffer()).contains("允许并开始"));
    assert_eq!(count(&b, "SELECT count(*) FROM task_workspaces"), 0);
    ui.key(&mut b, key(KeyCode::Esc)).unwrap();
    assert_eq!(count(&b, "SELECT count(*) FROM task_workspaces"), 0);
    ui.key(&mut b, ctrl('g')).unwrap();
    ui.key(&mut b, key(KeyCode::Enter)).unwrap();
    let grant = serde_json::to_value(
        task_workspace::binding(&rusqlite::Connection::open(&b.path).unwrap(), &id, 1).unwrap(),
    )
    .unwrap();
    transfer(&b, &mut a);
    assert_eq!(tasks::get(&a, &id).unwrap()["state"], "processing");
    assert_eq!(count(&a, "SELECT count(*) FROM tasks"), 0);
    // Reopening the worker preserves the queued start and permission.
    drop(b);
    let b = t.store("b");
    tasks::tick(&b, Path::new(BIN)).await.unwrap();
    let bt = tasks::get(&b, &id).unwrap();
    assert_eq!(bt["executions"][0]["phase"], "execute", "{bt:#}");
    assert_eq!(bt["executions"][0]["state"], "candidate", "{bt:#}");
    tasks::tick(&b, Path::new(BIN)).await.unwrap();
    assert_eq!(
        count(&b, "SELECT count(*) FROM runs"),
        1,
        "no repeated execution"
    );
    let workspace = Path::new(grant["directory"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(workspace.join("result.txt")).unwrap(),
        "55\n"
    );
    assert_eq!(
        std::fs::read_to_string(t.dir.path().join("b/work/fast.py")).unwrap(),
        "iterations=10 # fast original"
    );
    let conn = rusqlite::Connection::open(&b.path).unwrap();
    transfer(&b, &mut a);
    assert!(app::history(&a, &i.session_id).unwrap().iter().any(|m| {
        m["body"]
            .as_str()
            .is_some_and(|text| text.contains("已开始在独立任务目录"))
    }));
    let a_conn = rusqlite::Connection::open(&a.path).unwrap();
    let raw: String = a_conn
        .query_row(
            "SELECT result FROM task_candidates WHERE task_id=?1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    let candidate: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(candidate["sources"].as_array().unwrap().len(), 3);
    assert_eq!(candidate["sources"][1]["bytes"], 3);
    let execution = bt["executions"][0]["id"].as_str().unwrap();
    assert_eq!(b.runs(execution).unwrap()[0].sandbox, "task-workspace");
    // Result paths cannot escape through a symlink, and revoked grants are live fences.
    std::os::unix::fs::symlink(
        t.dir.path().join("b/work/fast.py"),
        workspace.join("escape.py"),
    )
    .unwrap();
    assert!(
        task_workspace::sources(&b, execution, &json!([{"root":0,"path":"escape.py"}])).is_err()
    );
    task_workspace::execute(
        &b,
        Command::Revoke {
            task: id.clone(),
            revision: 1,
        },
    )
    .unwrap();
    assert!(tasks::check_execution(&conn, execution).is_err());
    assert!(workspace.join("result.txt").exists());
    assert!(task_workspace::binding(&conn, &id, 2).is_err());
    assert_eq!(
        count(&a, "SELECT count(*) FROM tasks"),
        0,
        "executor remains B"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoking_write_access_stops_running_process_before_later_writes() {
    use xxassxx::task_workspace::{self, Command};
    let t = Team::new();
    files(&t, "b");
    let (i, id) = draft(&t, "@b 撤销执行测试");
    let a = t.store("a");
    tasks::prepare(
        &a,
        &id,
        json!({"goal":"运行独立副本","deliverables":["结果"],"mode":"execute"}),
    )
    .unwrap();
    tasks::confirm(&a, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    let mut b = t.store("b");
    transfer(&a, &mut b);
    let grant = task_workspace::execute(
        &b,
        Command::Allow {
            task: id.clone(),
            revision: 1,
            parent: t.dir.path().join("b/work"),
            read_dirs: vec![],
            timeout_secs: 30,
        },
    )
    .unwrap();
    let workspace = std::path::PathBuf::from(grant["directory"].as_str().unwrap());
    let start = std::time::Instant::now();
    let (result, ()) = tokio::join!(tasks::tick(&b, Path::new(BIN)), async {
        for _ in 0..100 {
            if workspace.join("calculation.py").exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            workspace.join("calculation.py").exists(),
            "executor must have started writing before revocation"
        );
        task_workspace::execute(
            &b,
            Command::Revoke {
                task: id.clone(),
                revision: 1,
            },
        )
        .unwrap();
    });
    result.unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(15));
    assert!(!workspace.join("late.txt").exists());
    assert!(!workspace.join("result.txt").exists());
    let bt = tasks::get(&b, &id).unwrap();
    assert_eq!(bt["state"], "needs_attention");
    assert_eq!(bt["runs"][0]["error_code"], "authorization_changed");
    assert_eq!(count(&a, "SELECT count(*) FROM task_candidates"), 0);
}

#[tokio::test]
async fn task_permission_screen_rejects_stale_revision_and_cancelled_task() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use xxassxx::{
        task_workspace::{self, Command},
        tui::Ui,
    };
    let t = Team::new();
    files(&t, "b");
    let (i, id) = draft(&t, "@b 写测试副本");
    let a = t.store("a");
    tasks::prepare(
        &a,
        &id,
        json!({"goal":"生成测试文件","mode":"execute","deliverables":["文件"]}),
    )
    .unwrap();
    tasks::confirm(&a, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    let mut b = t.store("b");
    transfer(&a, &mut b);
    tasks::tick(&b, Path::new(BIN)).await.unwrap();
    let p = app::project(&b, &t.dir.path().join("b/work")).unwrap();
    let mut ui = Ui::new(&b, p).unwrap();
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    ui.key(
        &mut b,
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
    )
    .unwrap();
    ui.key(&mut b, enter).unwrap();
    // The form remains revision 1 even while the task changes in the background.
    tasks::revise(&a, &action(&i, &id, "revise_task", 1, "改为生成另一份文件")).unwrap();
    tasks::prepare(
        &a,
        &id,
        json!({"goal":"生成另一份文件","mode":"execute","deliverables":["文件"]}),
    )
    .unwrap();
    tasks::confirm(&a, &action(&i, &id, "confirm_task", 2, "确认")).unwrap();
    transfer(&a, &mut b);
    ui.refresh(&b).unwrap();
    assert!(ui.key(&mut b, enter).is_err());
    assert_eq!(count(&b, "SELECT count(*) FROM task_workspaces"), 0);
    assert_eq!(count(&b, "SELECT count(*) FROM runs"), 0);
    tasks::cancel(&a, &action(&i, &id, "stop_task", 2, "取消")).unwrap();
    transfer(&a, &mut b);
    assert!(
        task_workspace::execute(
            &b,
            Command::Allow {
                task: id.clone(),
                revision: 2,
                parent: t.dir.path().join("b/work"),
                read_dirs: vec![],
                timeout_secs: 20
            }
        )
        .is_err()
    );
    assert_eq!(count(&b, "SELECT count(*) FROM task_workspaces"), 0);
}

#[test]
fn completed_remote_task_card_has_no_waiting_or_initiator_directory() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let t = Team::new();
    let (_, id) = draft(&t, "@b 检查远程材料");
    let mut a = t.store("a");
    let conn = rusqlite::Connection::open(&a.path).unwrap();
    conn.execute("UPDATE app_tasks SET state='completed',waiting_reason='等待对方',next_owner='b' WHERE id=?1",[&id]).unwrap();
    let p = app::project(&a, &t.dir.path().join("a/work")).unwrap();
    let mut ui = xxassxx::tui::Ui::new(&a, p).unwrap();
    ui.key(
        &mut a,
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
    )
    .unwrap();
    let mut screen = ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 50)).unwrap();
    screen.draw(|f| ui.render(f, &a)).unwrap();
    let text = format!("{:?}", screen.backend().buffer());
    assert!(text.contains("已完成"));
    assert!(text.contains("执行成员：@b"));
    assert!(!text.contains("等待对方"));
    assert!(!text.contains("已完成 · 等待"));
    assert!(!text.contains(&format!(
        "工作目录：{}",
        t.dir.path().join("a/work").display()
    )));
}

#[test]
fn file_task_waits_for_peer_capability_and_resumes_when_registered() {
    let t = Team::new();
    let (i, id) = draft(&t, "@b 把实际文件发来");
    let a = t.store("a");
    tasks::prepare(
        &a,
        &id,
        json!({"goal":"获取文件","deliverables":["原始文件"],"questions":[],"mode":"files"}),
    )
    .unwrap();
    tasks::confirm(&a, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    t.cli("a", &["butler", "sync"]);
    assert_eq!(tasks::get(&a, &id).unwrap()["state"], "waiting");
    assert!(
        tasks::get(&a, &id).unwrap()["waiting_for"]
            .as_str()
            .unwrap()
            .contains("新版")
    );
    assert!(t.store("b").messages(None).unwrap().is_empty());
    t.cli("b", &["files", "sync"]);
    t.cli("a", &["butler", "sync"]);
    t.cli("b", &["butler", "sync"]);
    assert!(
        t.store("b")
            .messages(None)
            .unwrap()
            .iter()
            .any(|m| m.message.operation == "task_v2")
    );
}

#[test]
fn result_archives_are_immutable_per_revision_after_reopening() {
    let t = Team::new();
    let (i, id) = draft(&t, "@b 检查结果");
    let mut a = t.store("a");
    let c = rusqlite::Connection::open(&a.path).unwrap();
    c.execute(
        "UPDATE app_tasks SET state='completed',result=?2 WHERE id=?1",
        rusqlite::params![id, json!({"body":"first","revision":1}).to_string()],
    )
    .unwrap();
    app::bridge::reconcile(&mut a).unwrap();
    let first = tasks::get(&a, &id).unwrap()["artifact"]
        .as_str()
        .unwrap()
        .to_owned();
    let original = std::fs::read(&first).unwrap();
    tasks::revise(&a, &action(&i, &id, "reopen_task", 1, "再次检查")).unwrap();
    c.execute(
        "UPDATE app_tasks SET state='completed',result=?2 WHERE id=?1",
        rusqlite::params![id, json!({"body":"second","revision":2}).to_string()],
    )
    .unwrap();
    app::bridge::reconcile(&mut a).unwrap();
    app::bridge::reconcile(&mut a).unwrap();
    let second = tasks::get(&a, &id).unwrap()["artifact"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(first, second);
    assert_eq!(std::fs::read(first).unwrap(), original);
    let result: Value = serde_json::from_slice(&std::fs::read(second).unwrap()).unwrap();
    assert_eq!(result["result"]["body"], "second");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepared_attachment_waits_for_receipt_before_model_can_request_more_work() {
    let team = Team::new();
    let (i, id) = draft(&team, "@b 请发一张现有图片");
    let a = team.store("a");
    tasks::prepare(
        &a,
        &id,
        json!({"goal":"接收原图","deliverables":["一张图片"],"mode":"files"}),
    )
    .unwrap();
    tasks::confirm(&a, &action(&i, &id, "confirm_task", 1, "确认")).unwrap();
    let mut b = team.store("b");
    transfer(&a, &mut b);
    let path = team.dir.path().join("b/work/image.png");
    std::fs::write(&path, b"immutable attachment fixture").unwrap();
    let path = path.canonicalize().unwrap();
    let batch = uuid::Uuid::new_v4().to_string();
    xxassxx::transfers::prepare(
        &b,
        xxassxx::transfers::Prepare {
            id: batch.clone(),
            recipient: "a".into(),
            note: "prepared; not yet sent".into(),
            paths: vec![path],
            session: None,
            task: Some(xxassxx::transfers::TaskRef {
                id: id.clone(),
                revision: 1,
            }),
            replaces: None,
        },
        true,
    )
    .unwrap();
    let manifest = xxassxx::transfers::manifest(&b, &batch).unwrap();
    let candidate = json!({"body":"原图已准备，尚未授权发送","attachment":{"transfer_id":batch,"manifest":manifest,"state":"draft"}});
    let db = rusqlite::Connection::open(&a.path).unwrap();
    db.execute(
        "INSERT INTO task_candidates(task_id,revision,member,result) VALUES(?1,1,'b',?2)",
        rusqlite::params![id, candidate.to_string()],
    )
    .unwrap();
    db.execute("INSERT INTO task_jobs(id,task_id,revision,kind,payload,created_at) VALUES(?1,?2,1,'review','{}',0)",rusqlite::params![uuid::Uuid::new_v4().to_string(),id]).unwrap();
    // No model is configured yet: even a reviewer that would misinterpret draft
    // transport state must not be called before a receiver has the actual bytes.
    tasks::tick(&a, Path::new(BIN)).await.unwrap();
    let waiting = tasks::get(&a, &id).unwrap();
    assert_eq!(waiting["state"], "waiting");
    assert_eq!(waiting["next_owner"], "b");
    assert_eq!(count(&a, "SELECT count(*) FROM task_model_attempts"), 0);
    assert_eq!(count(&a, "SELECT count(*) FROM task_executions"), 0);
    tasks::tick(&a, Path::new(BIN)).await.unwrap();
    assert_eq!(count(&a, "SELECT count(*) FROM task_model_attempts"), 0);
    team.cli("a", &["files", "sync"]); // Advertise receiver capability before upload.
    team.cli("b", &["files", "allow", &batch]);
    team.cli("b", &["files", "sync"]);
    team.cli("a", &["files", "sync"]);
    assert_eq!(
        team.cli("a", &["files", "show", &batch])["state"],
        "received"
    );
    let router=axum::Router::new().route("/chat/completions",axum::routing::post(|axum::Json(v):axum::Json<Value>| async move {
        let ctx:Value=serde_json::from_str(v["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(ctx["extra"]["delivery"]["ready"],true);
        // The immutable candidate still truthfully records its preparation time.
        assert_eq!(ctx["extra"]["candidates"][0]["result"]["attachment"]["state"],"draft");
        axum::Json(json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"receipt-review","type":"function","function":{"name":"task_decision","arguments":json!({"action":"complete","body":"原图已收到"}).to_string()}}]}}]}))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async { axum::serve(listener, router).await.unwrap() });
    let mut a = team.store("a");
    let mut cfg = a.member_config().unwrap();
    cfg.model = json!({"provider":"compatible","base_url":url,"model":"simulation","api_key_env":"","allow_insecure_http":true,"thinking":false});
    a.configure_member(&cfg).unwrap();
    tasks::tick(&a, Path::new(BIN)).await.unwrap();
    assert_eq!(tasks::get(&a, &id).unwrap()["state"], "completed");
    assert_eq!(count(&a, "SELECT count(*) FROM task_model_attempts"), 1);
    assert_eq!(count(&a, "SELECT count(*) FROM task_executions"), 0);
    tasks::tick(&a, Path::new(BIN)).await.unwrap();
    assert_eq!(count(&a, "SELECT count(*) FROM task_model_attempts"), 1);
    server.abort();
}
