mod common;
use common::Team;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use xxassxx::{
    app::{self, Actor, Instruction},
    tui::Ui,
};
fn instruction(t: &Team, m: &str, recipient: &str, body: &str) -> Instruction {
    let s = t.store(m);
    let p = app::project(&s, &t.dir.path().join(m).join("work")).unwrap();
    Instruction {
        request_id: uuid::Uuid::new_v4().to_string(),
        channel: "tui".into(),
        session_id: app::session(&s, p["id"].as_str().unwrap(), recipient).unwrap(),
        task_id: None,
        recipient: recipient.into(),
        body: body.into(),
        action: "chat".into(),
        payload: json!({}),
    }
}
fn submit(t: &Team, m: &str, i: &Instruction) {
    let mut s = t.store(m);
    let a = Actor::local(&s).unwrap();
    app::submit(&mut s, &a, i).unwrap();
}
fn process(t: &Team, m: &str) {
    t.cli(m, &["app", "process"]);
}
fn task(t: &Team, m: &str, i: &Instruction) -> Value {
    let s = t.store(m);
    let c = app::command(&s, &i.request_id).unwrap();
    app::tasks::get(&s, c["task_id"].as_str().unwrap()).unwrap()
}
fn mock(t: &Team, m: &str) {
    let path = t.dir.path().join("codex-mock");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/collaborator.py"),
        &path,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut s = t.store(m);
    let mut c = s.member_config().unwrap();
    c.executor.codex = path;
    c.executor.mode = "auto".into();
    c.executor.timeout_secs = 10;
    s.configure_member(&c).unwrap();
}
fn count(t: &Team, m: &str, sql: &str) -> i64 {
    rusqlite::Connection::open(t.db(m))
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}
#[test]
fn trusted_actor_idempotent_queue_second_channel_and_events() {
    let t = Team::new();
    let mut i = instruction(&t, "a", "a", "只读分析");
    i.action = "create_task".into();
    submit(&t, "a", &i);
    submit(&t, "a", &i);
    process(&t, "a");
    let task = task(&t, "a", &i);
    assert_eq!(task["state"], "awaiting_authorization");
    let mut second = instruction(&t, "a", "a", "只看脚本，不运行");
    second.channel = "tg-test".into();
    second.task_id = Some(task["id"].as_str().unwrap().into());
    submit(&t, "a", &second);
    let mut s = t.store("a");
    assert!(Actor::bound_member(&s, "b").is_err());
    let actor = Actor::bound_member(&s, "a").unwrap();
    app::submit(&mut s, &actor, &second).unwrap();
    app::model::call(
        &mut s,
        &second,
        "amend_task",
        json!({"requirement":"只看脚本，不运行"}),
        false,
    )
    .unwrap();
    app::model::call(
        &mut s,
        &second,
        "amend_task",
        json!({"requirement":"只看脚本，不运行"}),
        false,
    )
    .unwrap();
    assert_eq!(
        app::tasks::get(&s, task["id"].as_str().unwrap()).unwrap()["goal"],
        "只读分析\n用户补充：只看脚本，不运行"
    );
    let history = app::history(&s, &i.session_id).unwrap();
    assert_eq!(history.iter().filter(|x| x["kind"] == "user").count(), 2);
    assert!(history.iter().all(|x| x["task_id"] == task["id"]));
    let mut changed = i.clone();
    changed.body = "different".into();
    assert!(app::submit(&mut s, &actor, &changed).is_err());
    let before = app::events(&s, &i.session_id, 0).unwrap();
    app::bridge::reconcile(&mut s).unwrap();
    assert_eq!(before, app::events(&s, &i.session_id, 0).unwrap());
    let seq = before.last().unwrap()["seq"].as_i64().unwrap();
    app::mark_read(&s, &i.session_id, seq).unwrap();
    drop(s);
    let s = t.store("a");
    assert_eq!(app::snapshot(&s, &i.session_id).unwrap()["unread"], 0);
    assert_eq!(count(&t, "a", "SELECT count(*) FROM workflows"), 0);
}
#[test]
fn ui_routing_unicode_render_and_project_are_independent_of_grants() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let t = Team::new();
    let mut s = t.store("a");
    s.set_identity("a", "沈尧 Shenyao \"Keith\" Jin", "lab")
        .unwrap();
    let project = app::project(&s, t.dir.path()).unwrap();
    let mut ui = Ui::new(&s, project).unwrap();
    for _ in 0..7 {
        ui.key(&mut s, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(ui.recipient(&s).unwrap(), "a");
    }
    assert!(s.messages(None).unwrap().is_empty());
    assert!(s.file_roots().unwrap().is_empty());
    ui.input.insert("@");
    assert_eq!(ui.candidates(), vec!["a", "b", "c"]);
    ui.input.clear();
    ui.input.insert("@b 材料有哪些");
    assert_eq!(ui.recipient(&s).unwrap(), "b");
    for text in ["mail a@example.org", "引用：@b", "\"@b quoted\""] {
        assert_eq!(app::addressing(&s, text).unwrap().0, "a");
    }
    assert!(app::addressing(&s, "@unknown x").is_err());
    assert!(app::addressing(&s, "@b @c x").is_err());
    assert!(app::addressing(&s, "@codex x").is_err());
    ui.input.clear();
    ui.input.insert("中文🙂e\u{301}\n宽字符");
    ui.input.left();
    ui.input.backspace();
    for (w, h) in [(100, 30), (38, 12), (10, 4)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| ui.render(f, &s)).unwrap();
        if w == 100 {
            let screen = format!("{:?}", terminal.backend().buffer());
            assert!(screen.contains("Keith"));
        }
    }
    assert_eq!(count(&t, "a", "SELECT count(*) FROM app_commands"), 0);
}
#[test]
fn startup_directory_choice_is_explicit_persistent_and_scoped() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let t = Team::new();
    let mut s = t.store("a");
    let path = t.dir.path().join("a/work");
    let project = app::project(&s, &path).unwrap();
    let open = |store: &xxassxx::store::Store, project: &Value| {
        let mut ui = Ui::new(store, project.clone()).unwrap();
        ui.prompt_project_access(store).unwrap();
        ui
    };
    let screen = |ui: &Ui, store: &xxassxx::store::Store| {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
        terminal.draw(|f| ui.render(f, store)).unwrap();
        format!("{:?}", terminal.backend().buffer())
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut ui = open(&s, &project);
    assert!(screen(&ui, &s).contains("是否将启动文件夹"));
    assert!(s.file_roots().unwrap().is_empty());
    // Enter defaults to declining. Even after reopening the database it stays declined.
    ui.key(&mut s, key(KeyCode::Enter)).unwrap();
    assert!(s.file_roots().unwrap().is_empty());
    drop(s);
    let mut s = t.store("a");
    assert!(!screen(&open(&s, &project), &s).contains("是否将启动文件夹"));
    let other = t.dir.path().join("b/work");
    let other_project = app::project(&s, &other).unwrap();
    let mut ui = open(&s, &other_project);
    assert!(screen(&ui, &s).contains("是否将启动文件夹"));
    ui.key(&mut s, key(KeyCode::Up)).unwrap();
    assert!(s.file_roots().unwrap().is_empty());
    ui.key(&mut s, key(KeyCode::Enter)).unwrap();
    assert_eq!(s.file_roots().unwrap(), vec![other.canonicalize().unwrap()]);
    assert!(!screen(&open(&s, &other_project), &s).contains("是否将启动文件夹"));
    let nested = other.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let nested_project = app::project(&s, &nested).unwrap();
    assert!(!screen(&open(&s, &nested_project), &s).contains("是否将启动文件夹"));
    s.remove_directory(&other).unwrap();
    assert!(screen(&open(&s, &other_project), &s).contains("是否将启动文件夹"));
    assert!(s.file_roots().unwrap().is_empty());
    assert_eq!(count(&t, "a", "SELECT count(*) FROM app_commands"), 0);
    assert_eq!(count(&t, "a", "SELECT count(*) FROM versions"), 0);
    assert_eq!(count(&t, "a", "SELECT count(*) FROM tasks"), 0);
}

#[test]
fn authorized_local_task_uses_real_rust_mcp_fixed_snapshot_and_one_execution() {
    let t = Team::new();
    mock(&t, "a");
    let file = t.dir.path().join("a/work/sample.csv");
    std::fs::write(&file, "x,value\na,4\n").unwrap();
    let mut i = instruction(&t, "a", "a", "只读分析样本");
    i.action = "create_task".into();
    submit(&t, "a", &i);
    process(&t, "a");
    let card = task(&t, "a", &i);
    let id = card["id"].as_str().unwrap();
    let mut auth = instruction(&t, "a", "a", "授权此文件");
    auth.action = "authorize".into();
    auth.task_id = Some(id.into());
    auth.payload = json!({"files":[file]});
    submit(&t, "a", &auth);
    process(&t, "a");
    assert_eq!(
        app::command(&t.store("a"), &auth.request_id).unwrap()["state"],
        "failed"
    );
    assert_eq!(count(&t, "a", "SELECT count(*) FROM workflows"), 0);
    t.store("a")
        .allow_directory(&t.dir.path().join("a/work"))
        .unwrap();
    auth.request_id = uuid::Uuid::new_v4().to_string();
    submit(&t, "a", &auth);
    submit(&t, "a", &auth);
    process(&t, "a");
    let card = app::tasks::get(&t.store("a"), id).unwrap();
    let wf = card["workflow_id"].as_str().unwrap();
    std::fs::write(&file, "changed after grant").unwrap();
    t.cli("a", &["collaboration", "run", wf]);
    app::bridge::reconcile(&mut t.store("a")).unwrap();
    let card = app::tasks::get(&t.store("a"), id).unwrap();
    assert_eq!(card["state"], "completed");
    assert!(card["result"].to_string().contains("a,4"));
    assert!(!card["result"]["sources"].as_array().unwrap().is_empty());
    assert!(Path::new(card["artifact"].as_str().unwrap()).is_file());
    submit(&t, "a", &auth);
    process(&t, "a");
    t.cli("a", &["butler", "tick"]);
    assert_eq!(count(&t, "a", "SELECT count(*) FROM workflow_runs"), 1);
}
#[test]
fn remote_owner_authorization_clarification_resume_and_result_share_task_history() {
    let t = Team::new();
    mock(&t, "b");
    let file = t.dir.path().join("b/work/sample.csv");
    std::fs::write(&file, "item,amount\nx,4\ny,5\n").unwrap();
    let mut i = instruction(&t, "a", "a", "请分析对方的样本");
    i.action = "create_task".into();
    i.payload = json!({"peer":"b"});
    submit(&t, "a", &i);
    process(&t, "a");
    let card = task(&t, "a", &i);
    let id = card["id"].as_str().unwrap();
    assert_eq!(card["state"], "waiting_peer_authorization");
    t.sync("a");
    t.sync("b");
    app::bridge::reconcile(&mut t.store("b")).unwrap();
    let peer = app::tasks::list(&t.store("b"), None).unwrap().remove(0);
    assert_eq!(peer["state"], "awaiting_authorization");
    assert!(t.store("b").workflow_ids().unwrap().is_empty());
    t.store("b")
        .allow_directory(&t.dir.path().join("b/work"))
        .unwrap();
    let mut auth = instruction(&t, "b", "b", "我明确授权样本");
    auth.session_id = peer["session_id"].as_str().unwrap().into();
    auth.task_id = peer["id"].as_str().map(str::to_owned);
    auth.action = "authorize".into();
    auth.payload = json!({"files":[file]});
    submit(&t, "b", &auth);
    process(&t, "b");
    t.sync("b");
    t.sync("a");
    app::bridge::reconcile(&mut t.store("a")).unwrap();
    assert_eq!(
        app::tasks::get(&t.store("a"), id).unwrap()["state"],
        "offer_available"
    );
    let mut accept = instruction(&t, "a", "a", "使用对方材料");
    accept.action = "use_offer".into();
    accept.task_id = Some(id.into());
    submit(&t, "a", &accept);
    process(&t, "a");
    t.sync("a");
    t.cli("b", &["butler", "tick"]);
    t.sync("a");
    app::bridge::reconcile(&mut t.store("a")).unwrap();
    assert_eq!(
        app::tasks::get(&t.store("a"), id).unwrap()["state"],
        "waiting_user"
    );
    let mut answer = instruction(&t, "a", "a", "factor=3");
    answer.task_id = Some(id.into());
    answer.channel = "second-test".into();
    submit(&t, "a", &answer);
    app::model::call(
        &mut t.store("a"),
        &answer,
        "answer_peer",
        json!({"body":"{\"factor\":3}"}),
        false,
    )
    .unwrap();
    t.sync("a");
    t.cli("b", &["butler", "tick"]);
    t.sync("a");
    app::bridge::reconcile(&mut t.store("a")).unwrap();
    let result = app::tasks::get(&t.store("a"), id).unwrap();
    assert_eq!(result["state"], "completed", "{result}");
    assert!(result["result"].to_string().contains("27"));
    let before = app::events(&t.store("a"), &i.session_id, 0).unwrap();
    t.sync("a");
    app::bridge::reconcile(&mut t.store("a")).unwrap();
    assert_eq!(
        before,
        app::events(&t.store("a"), &i.session_id, 0).unwrap()
    );
    assert_eq!(
        count(
            &t,
            "b",
            "SELECT count(DISTINCT session_id) FROM runs JOIN workflow_runs ON runs.id=workflow_runs.run_id"
        ),
        1
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persistent_owner_model_followup_and_tool_failure_cannot_claim_success() {
    let t = Team::new();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let log = requests.clone();
    let router=axum::Router::new().route("/chat/completions",axum::routing::post(move |axum::Json(body):axum::Json<Value>|{let log=log.clone();async move{log.lock().unwrap().push(body.clone());let messages=body["messages"].as_array().unwrap();let current=messages.iter().rev().find(|x|x["role"]=="user").unwrap()["content"].as_str().unwrap();let response=if messages.iter().any(|m|m["role"]=="tool"){json!({"role":"assistant","content":"done"})}else{let(name,args)=if current=="bad"{("send_peer",json!({"member_id":"unknown","body":"hello"}))}else if current.contains("那个"){assert!(messages.iter().any(|m|m["content"].as_str().is_some_and(|s|s.contains("read_me.py"))));("reply_user",json!({"text":"你指的是 read_me.py；仅阅读，不运行。","question":false}))}else{("reply_user",json!({"text":"记住了 read_me.py，想了解哪部分？","question":true}))};json!({"role":"assistant","content":null,"tool_calls":[{"id":"one","type":"function","function":{"name":name,"arguments":args.to_string()}}]})};axum::Json(json!({"choices":[{"finish_reason":if response.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":response}]}))}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut s = t.store("a");
    let mut cfg = s.member_config().unwrap();
    cfg.model = json!({"provider":"compatible","base_url":base,"model":"mock","api_key_env":""});
    s.configure_member(&cfg).unwrap();
    let one = instruction(&t, "a", "a", "我想了解 read_me.py");
    submit(&t, "a", &one);
    let two = instruction(&t, "a", "a", "只看那个脚本，不运行");
    submit(&t, "a", &two);
    app::model::process_one(&mut s).await.unwrap();
    assert!(
        !requests.lock().unwrap()[0]["messages"]
            .to_string()
            .contains("那个"),
        "future queued input must not steer the current turn"
    );
    app::model::process_one(&mut s).await.unwrap();
    assert!(
        app::history(&s, &one.session_id).unwrap().last().unwrap()["body"]
            .as_str()
            .unwrap()
            .contains("仅阅读")
    );
    let bad = instruction(&t, "a", "a", "bad");
    submit(&t, "a", &bad);
    app::model::process_one(&mut s).await.unwrap();
    assert_eq!(
        app::command(&s, &bad.request_id).unwrap()["state"],
        "failed"
    );
    assert!(
        app::model::call(
            &mut s,
            &bad,
            "reply_user",
            json!({"text":"已完成","question":false}),
            true
        )
        .is_err()
    );
    assert!(s.messages(None).unwrap().is_empty());
    assert_eq!(requests.lock().unwrap().len(), 5);
    server.abort();
}
#[test]
fn presence_offline_old_clients_and_expiry_do_not_call_models() {
    let t = Team::new();
    let s = t.store("a");
    assert_eq!(
        xxassxx::presence::view(&s).unwrap()["members"][0]["presence"]["state"],
        "unknown"
    );
    let conn = rusqlite::Connection::open(t.db("a")).unwrap();
    conn.execute(
        "INSERT INTO app_presence VALUES('b',1,31,30,?1)",
        [xxassxx::store::now()],
    )
    .unwrap();
    xxassxx::presence::network(&s, "unreachable", Some("offline")).unwrap();
    let view = xxassxx::presence::view(&s).unwrap();
    assert_eq!(view["members"][0]["presence"]["state"], "expired");
    assert_eq!(view["members"][1]["presence"]["state"], "unknown");
    let i = instruction(&t, "a", "b", "offline queued");
    submit(&t, "a", &i);
    process(&t, "a");
    assert_eq!(count(&t, "a", "SELECT count(*) FROM app_model_runs"), 0);
    assert_eq!(s.messages(None).unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn receiving_and_presence_continue_while_owner_model_waits_and_restart_is_safe() {
    use std::time::{Duration, Instant};
    let t = Team::new();
    let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mark = entered.clone();
    let router=axum::Router::new().route("/chat/completions",axum::routing::post(move||{let mark=mark.clone();async move{mark.store(true,std::sync::atomic::Ordering::SeqCst);tokio::time::sleep(Duration::from_secs(4)).await;axum::Json(json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"no effect"}}]}))}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut s = t.store("a");
    let mut cfg = s.member_config().unwrap();
    cfg.model = json!({"provider":"compatible","base_url":base,"model":"slow","api_key_env":"","timeout_secs":10});
    s.configure_member(&cfg).unwrap();
    let i = instruction(&t, "a", "a", "慢慢考虑");
    submit(&t, "a", &i);
    let mut daemon = t
        .command("a", &["butler", "run", "--poll-secs", "1", "--reconnect"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !entered.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    t.store("b")
        .new_request(
            "a",
            "receive-during-owner-model",
            "metadata",
            (None, None),
            None,
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
    t.sync("b");
    while t.store("a").messages(None).unwrap().is_empty() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert_eq!(
        app::command(&t.store("a"), &i.request_id).unwrap()["state"],
        "processing"
    );
    let response = reqwest::Client::new()
        .get(format!("{}/v1/presence", t.url))
        .bearer_auth(common::TOKEN)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(
        response["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["member_id"] == "a" && m["activity"] == "busy")
    );
    // Kill only this test child while its model request is in flight; service recovery must not repeat it.
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    let before = count(&t, "a", "SELECT sum(calls) FROM app_model_runs");
    let mut again = t
        .command("a", &["butler", "run", "--poll-secs", "1", "--reconnect"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    while app::command(&t.store("a"), &i.request_id).unwrap()["state"] == "processing" {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert_eq!(
        app::command(&t.store("a"), &i.request_id).unwrap()["state"],
        "needs_attention"
    );
    assert_eq!(
        count(&t, "a", "SELECT sum(calls) FROM app_model_runs"),
        before
    );
    unsafe {
        libc::kill(again.id() as i32, libc::SIGTERM);
    };
    again.wait().unwrap();
    server.abort();
}

#[tokio::test]
async fn old_mailbox_presence_is_unknown_instead_of_inferred_online() {
    let t = Team::new();
    let router = axum::Router::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut s = t.store("a");
    let mut cfg = s.member_config().unwrap();
    let secrets = t.dir.path().join("a/private.env");
    std::fs::write(&secrets, format!("TEST_A_TOKEN={}\n", common::TOKEN)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&secrets, std::fs::Permissions::from_mode(0o600)).unwrap();
    cfg.mailbox_url = base;
    cfg.secrets_file = Some(secrets);
    s.configure_member(&cfg).unwrap();
    rusqlite::Connection::open(t.db("a"))
        .unwrap()
        .execute(
            "INSERT INTO app_presence VALUES('b',0,0,30,?1)",
            [xxassxx::store::now()],
        )
        .unwrap();
    xxassxx::presence::heartbeat(&mut s).await.unwrap();
    let v = xxassxx::presence::view(&s).unwrap();
    assert_eq!(v["connection"]["presence_supported"], false);
    assert_eq!(v["members"][0]["presence"]["state"], "unknown");
    assert_eq!(count(&t, "a", "SELECT count(*) FROM app_model_runs"), 0);
    server.abort();
}
#[test]
fn tui_file_picker_authorizes_without_pasting_ids_and_stop_is_real() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let t = Team::new();
    let mut s = t.store("a");
    let root = t.dir.path().join("a/work");
    std::fs::write(root.join("tiny.csv"), "x,y\n1,2\n").unwrap();
    let p = app::project(&s, &root).unwrap();
    let mut ui = Ui::new(&s, p).unwrap();
    let key = |ui: &mut Ui, s: &mut xxassxx::store::Store, c: char| {
        ui.key(s, KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
            .unwrap()
    };
    key(&mut ui, &mut s, 'n');
    ui.input.insert("仅读取样本");
    ui.key(&mut s, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    process(&t, "a");
    ui.refresh(&s).unwrap();
    let id = ui.selected_task.clone().unwrap();
    assert_eq!(
        app::tasks::get(&s, &id).unwrap()["state"],
        "awaiting_authorization"
    );
    key(&mut ui, &mut s, 'r');
    ui.key(
        &mut s,
        KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
    )
    .unwrap();
    ui.input.insert(root.to_str().unwrap());
    ui.key(&mut s, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    process(&t, "a");
    ui.refresh(&s).unwrap();
    assert_eq!(s.file_roots().unwrap().len(), 1);
    key(&mut ui, &mut s, 'g');
    ui.key(&mut s, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    ui.key(
        &mut s,
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
    )
    .unwrap();
    key(&mut ui, &mut s, 's');
    process(&t, "a");
    ui.refresh(&s).unwrap();
    assert_eq!(app::tasks::get(&s, &id).unwrap()["state"], "ready");
    assert_eq!(count(&t, "a", "SELECT count(*) FROM workflow_grants"), 1);
    key(&mut ui, &mut s, 'x');
    ui.key(&mut s, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    process(&t, "a");
    ui.refresh(&s).unwrap();
    assert_eq!(app::tasks::get(&s, &id).unwrap()["state"], "stopped");
    assert_eq!(count(&t, "a", "SELECT count(*) FROM runs"), 0);
    ui.key(&mut s, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();
    ui.refresh(&s).unwrap();
    assert!(
        ui.selected_task.is_none(),
        "do not reattach an unrelated task after Esc"
    );
}
