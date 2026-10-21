mod common;
use common::Team;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};
use xxassxx::{
    app::{self, Instruction},
    store::Store,
    task_coordinator as tasks,
    task_recovery::{self, Reason},
    tui::Ui,
};

fn key(ui: &mut Ui, store: &mut Store, code: KeyCode) {
    ui.key(store, KeyEvent::new(code, KeyModifiers::NONE))
        .unwrap();
}

fn fixture(team: &Team) -> (Store, Ui, String) {
    let store = team.store("a");
    let c = rusqlite::Connection::open(&store.path).unwrap();
    let project = app::project(&store, &team.dir.path().join("a/work")).unwrap();
    let session = app::session(&store, project["id"].as_str().unwrap(), "a").unwrap();
    let input = Instruction {
        request_id: uuid::Uuid::new_v4().to_string(),
        channel: "test-user-action".into(),
        session_id: session,
        task_id: None,
        recipient: "a".into(),
        body: "查找报告并说明已有依据".into(),
        action: "create_task".into(),
        payload: json!({}),
    };
    let id = tasks::create(&c, &input, "a", None).unwrap();
    tasks::prepare(&store, &id, json!({"goal":"查找报告并说明已有依据","deliverables":["报告清单与已有依据"],"constraints":"只读检查已有资料","mode":"analysis","questions":[]})).unwrap();
    let task = tasks::get(&store, &id).unwrap();
    task_recovery::record(
        &c,
        &task,
        "failure-a",
        Reason::ExecutionError,
        "legacy",
        "failed-step",
        "文件 report.txt 在读取期间发生变化",
        "a",
    )
    .unwrap();
    c.execute("UPDATE app_tasks SET state='needs_attention',confirmed=revision,error='文件 report.txt 在读取期间发生变化' WHERE id=?1", [&id]).unwrap();
    let mut ui = Ui::new(&store, project).unwrap();
    ui.selected_task = Some(id.clone());
    (store, ui, id)
}

fn command_count(store: &Store) -> i64 {
    rusqlite::Connection::open(&store.path)
        .unwrap()
        .query_row("SELECT count(*) FROM app_commands", [], |r| r.get(0))
        .unwrap()
}

fn last_input(store: &Store) -> Value {
    let raw: String = rusqlite::Connection::open(&store.path)
        .unwrap()
        .query_row(
            "SELECT payload FROM app_commands ORDER BY rowid DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn open(ui: &mut Ui, store: &mut Store) {
    ui.key(
        store,
        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
    )
    .unwrap();
}

#[test]
fn recovery_requires_explicit_choice_and_binds_correction_and_resume() {
    let team = Team::new();
    let (mut store, mut ui, id) = fixture(&team);
    open(&mut ui, &mut store);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 32)).unwrap();
    terminal.draw(|f| ui.render(f, &store)).unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("文件 report.txt 在读取期间发生变化"));
    assert!(screen.contains("补充事实或纠正"));
    assert!(screen.contains("从失败步骤有限续接"));
    let before = command_count(&store);
    key(&mut ui, &mut store, KeyCode::Enter);
    assert_eq!(
        command_count(&store),
        before,
        "opening correction must not retry"
    );
    ui.input.insert("忽略尚未写完的报告，复用已保存的依据");
    key(&mut ui, &mut store, KeyCode::Tab);
    key(&mut ui, &mut store, KeyCode::Enter);
    let sent = last_input(&store);
    assert_eq!(sent["action"], "supplement_task");
    assert_eq!(sent["task_id"], id);
    assert_eq!(sent["payload"]["revision"], 1);
    assert_eq!(sent["payload"]["resume"], true);
    assert_eq!(
        sent["payload"]["block_id"],
        tasks::get(&store, &id).unwrap()["blocker"]["id"]
    );
    assert_eq!(
        tasks::get(&store, &id).unwrap()["state"],
        "needs_attention",
        "enqueue is not completed recovery"
    );
}

#[test]
fn correction_defaults_to_save_and_stale_resume_preserves_unsent_text() {
    let team = Team::new();
    let (mut store, mut ui, id) = fixture(&team);
    open(&mut ui, &mut store);
    key(&mut ui, &mut store, KeyCode::Char('s'));
    ui.input.insert("只补充现有资料来源");
    key(&mut ui, &mut store, KeyCode::Enter);
    assert!(last_input(&store)["payload"]["resume"].is_null());
    open(&mut ui, &mut store);
    key(&mut ui, &mut store, KeyCode::Char('s'));
    ui.input.insert("核对原记录后续接");
    key(&mut ui, &mut store, KeyCode::Tab);
    let task = tasks::get(&store, &id).unwrap();
    task_recovery::record(
        &rusqlite::Connection::open(&store.path).unwrap(),
        &task,
        "failure-b",
        Reason::ExecutionUncertain,
        "execution",
        "new-step",
        "当前运行状态已变化",
        "a",
    )
    .unwrap();
    ui.refresh(&store).unwrap();
    let before = command_count(&store);
    let error = ui
        .key(
            &mut store,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .unwrap_err();
    assert!(error.to_string().contains("失败步骤已变化"));
    assert_eq!(ui.input.text, "核对原记录后续接");
    assert_eq!(command_count(&store), before);
}

#[test]
fn recovery_back_keeps_task_and_retries_only_the_selected_block() {
    let team = Team::new();
    let (mut store, mut ui, id) = fixture(&team);
    open(&mut ui, &mut store);
    key(&mut ui, &mut store, KeyCode::Char('d'));
    key(&mut ui, &mut store, KeyCode::Esc);
    key(&mut ui, &mut store, KeyCode::Esc);
    assert_eq!(ui.selected_task.as_deref(), Some(id.as_str()));
    assert_eq!(command_count(&store), 0);
    open(&mut ui, &mut store);
    key(&mut ui, &mut store, KeyCode::Char('r'));
    let sent = last_input(&store);
    assert_eq!(sent["action"], "retry_task");
    assert_eq!(sent["task_id"], id);
    assert_eq!(
        sent["payload"]["block_id"],
        tasks::get(&store, &id).unwrap()["blocker"]["id"]
    );
}

#[test]
fn continuation_receipt_switches_to_the_existing_task_conversation() {
    let team = Team::new();
    let (mut store, mut ui, id) = fixture(&team);
    let task_session = ui.session.clone();
    ui.selected_task = None;
    ui.input.insert("/new");
    key(&mut ui, &mut store, KeyCode::Enter);
    assert_ne!(ui.session, task_session);
    ui.input.insert("继续刚才的任务");
    key(&mut ui, &mut store, KeyCode::Enter);
    let sent = last_input(&store);
    // Simulate the authenticated continuation tool's command receipt. This
    // test covers the UI transition; the tool's routing has separate tests.
    rusqlite::Connection::open(&store.path)
        .unwrap()
        .execute(
            "UPDATE app_commands SET task_id=?2,session_id=?3,state='processing' WHERE id=?1",
            rusqlite::params![sent["request_id"].as_str(), id, task_session],
        )
        .unwrap();
    ui.refresh(&store).unwrap();
    assert_eq!(ui.session, task_session);
    assert_eq!(ui.selected_task.as_deref(), Some(id.as_str()));
}

#[test]
fn granting_permission_after_a_failed_execution_returns_to_recovery_without_replaying() {
    let team = Team::new();
    let (mut store, mut ui, id) = fixture(&team);
    let work = team.dir.path().join("a/work");
    store.allow_directory(&work).unwrap();
    let retained = work.join("existing-result.txt");
    std::fs::write(&retained, "83\n").unwrap();
    let c = rusqlite::Connection::open(&store.path).unwrap();
    let execution = uuid::Uuid::new_v4().to_string();
    // Seed a prior failed attempt. Granting permission must leave this attempt
    // paused; the existing authorization regression covers unstarted tasks.
    c.execute(
        "UPDATE app_tasks SET draft=json_set(draft,'$.mode','execute') WHERE id=?1",
        [&id],
    )
    .unwrap();
    c.execute(
        "INSERT INTO tasks(id,input,created_at) VALUES(?1,'{}',1)",
        [&execution],
    )
    .unwrap();
    c.execute("INSERT INTO task_executions(id,task_id,revision,phase,state,created_at) VALUES(?1,?2,1,'execute','uncertain',1)", rusqlite::params![execution,id]).unwrap();
    let task = tasks::get(&store, &id).unwrap();
    task_recovery::record(
        &c,
        &task,
        "permission-followup",
        Reason::ExecutionUncertain,
        "execution",
        &execution,
        "上次运行中断；权限更新后仍需先核对已有成果",
        "a",
    )
    .unwrap();
    ui.refresh(&store).unwrap();
    assert_eq!(
        tasks::get(&store, &id).unwrap()["execution_permission"]["can_allow"],
        true
    );
    open(&mut ui, &mut store);
    key(&mut ui, &mut store, KeyCode::Char('a'));
    key(&mut ui, &mut store, KeyCode::Enter);
    assert!(
        xxassxx::task_workspace::binding(&c, &id, 1)
            .unwrap()
            .full_access
    );
    assert_eq!(tasks::get(&store, &id).unwrap()["state"], "needs_attention");
    let state: String = c
        .query_row(
            "SELECT state FROM task_executions WHERE id=?1",
            [&execution],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "uncertain");
    let grants: i64 = c
        .query_row("SELECT count(*) FROM task_recovery_grants", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(grants, 0, "permission alone must not grant retry attempts");
    assert_eq!(std::fs::read_to_string(retained).unwrap(), "83\n");
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 32)).unwrap();
    terminal.draw(|f| ui.render(f, &store)).unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("处理当前任务"));
    assert!(screen.contains("原失败步骤仍暂停"));
    assert!(screen.contains("先核对已有成果与副作用"));
    assert!(!screen.contains("助手会自动开始"));
}

#[test]
fn opening_recovery_from_task_list_restores_session_and_revision_edits_are_bound() {
    let team = Team::new();
    let (mut store, mut ui, id) = fixture(&team);
    let task_session = ui.session.clone();
    let task_project = ui.project.clone();
    ui.selected_task = None;
    ui.input.insert("/new");
    key(&mut ui, &mut store, KeyCode::Enter);
    assert_ne!(ui.session, task_session);
    ui.project = json!({"id":"other-project","path":"/other-project","name":"other"});
    ui.key(
        &mut store,
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
    )
    .unwrap();
    open(&mut ui, &mut store);
    assert_eq!(ui.session, task_session);
    assert_eq!(ui.project["id"], task_project["id"]);
    assert_eq!(ui.project["path"], task_project["path"]);
    key(&mut ui, &mut store, KeyCode::Char('e'));
    ui.input.insert("保留已完成部分，只修订缺失的交付标准");
    let c = rusqlite::Connection::open(&store.path).unwrap();
    c.execute(
        "UPDATE app_tasks SET revision=revision+1 WHERE id=?1",
        [&id],
    )
    .unwrap();
    ui.refresh(&store).unwrap();
    let before = command_count(&store);
    let error = ui
        .key(
            &mut store,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .unwrap_err();
    assert!(error.to_string().contains("任务版本已变化"));
    assert_eq!(ui.input.text, "保留已完成部分，只修订缺失的交付标准");
    assert_eq!(command_count(&store), before);
}
