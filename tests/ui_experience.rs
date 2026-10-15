mod common;
use common::Team;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend, style::Color};
use rusqlite::params;
use serde_json::json;
use xxassxx::{
    app::{self, task_archive},
    store::Store,
    tui::Ui,
};

fn fixture() -> (Team, Store, Ui) {
    let team = Team::new();
    let store = team.store("a");
    let project = app::project(&store, &team.dir.path().join("a/work")).unwrap();
    let ui = Ui::new(&store, project).unwrap();
    (team, store, ui)
}

#[test]
fn access_picker_is_local_persistent_and_does_not_send_model_commands() {
    use xxassxx::access_mode::{self, Mode};
    let (team, mut s, mut ui) = fixture();
    let conn = rusqlite::Connection::open(&s.path).unwrap();
    assert_eq!(access_mode::get(&conn).unwrap(), Mode::AskForApproval);
    slash(&mut ui, &mut s, "/access");
    let shown = screen(&ui, &s, 120, 32);
    assert!(
        shown.contains("Ask for approval")
            && shown.contains("Full access")
            && shown.contains("联网")
    );
    key(&mut ui, &mut s, KeyCode::Down);
    key(&mut ui, &mut s, KeyCode::Esc);
    assert_eq!(access_mode::get(&conn).unwrap(), Mode::AskForApproval);
    slash(&mut ui, &mut s, "/access");
    key(&mut ui, &mut s, KeyCode::Down);
    key(&mut ui, &mut s, KeyCode::Enter);
    assert_eq!(
        access_mode::get(&rusqlite::Connection::open(&Store::open(&s.path).unwrap().path).unwrap())
            .unwrap(),
        Mode::FullAccess
    );
    assert_eq!(
        access_mode::get(&rusqlite::Connection::open(team.db("b")).unwrap()).unwrap(),
        Mode::AskForApproval
    );
    assert!(screen(&ui, &s, 140, 32).contains("Full access · /access"));
    slash(&mut ui, &mut s, "/access");
    key(&mut ui, &mut s, KeyCode::Up);
    key(&mut ui, &mut s, KeyCode::Enter);
    assert_eq!(access_mode::get(&conn).unwrap(), Mode::AskForApproval);
    slash(&mut ui, &mut s, "/reconnect");
    assert!(screen(&ui, &s, 140, 32).contains("正在重新连接"));
    assert_eq!(
        conn.query_row("SELECT count(*) FROM app_commands", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
fn task(store: &Store, ui: &Ui, id: &str, state: &str) {
    rusqlite::Connection::open(&store.path).unwrap().execute("INSERT INTO app_tasks(id,session_id,project_id,title,goal,state,created_at) VALUES(?1,?2,?3,?4,'查看测试材料',?5,1)",params![id,ui.session,ui.project["id"].as_str(),format!("任务-{id}"),state]).unwrap();
    rusqlite::Connection::open(&store.path).unwrap().execute("INSERT INTO app_messages(id,session_id,task_id,sender,recipient,kind,body,created_at) VALUES(?1,?2,?1,'butler','a','reply',?3,1)",params![id,ui.session,format!("原始结果-{id}")]).unwrap();
}
fn key(ui: &mut Ui, s: &mut Store, k: KeyCode) {
    ui.key(s, KeyEvent::new(k, KeyModifiers::NONE)).unwrap();
}
fn slash(ui: &mut Ui, s: &mut Store, text: &str) {
    key(ui, s, KeyCode::Esc);
    ui.input.insert(text);
    key(ui, s, KeyCode::Enter);
}
fn screen(ui: &Ui, s: &Store, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui.render(f, s)).unwrap();
    let b = term.backend().buffer();
    use unicode_width::UnicodeWidthStr;
    (0..h)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0;
            while x < w {
                let symbol = b[(x, y)].symbol();
                line.push_str(symbol);
                x += symbol.width().max(1) as u16;
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}
#[test]
fn archive_is_local_persistent_reversible_and_never_changes_task_execution() {
    let (_team, s, ui) = fixture();
    task(&s, &ui, "done", "completed");
    task(&s, &ui, "running", "processing");
    let before = app::tasks::get(&s, "done").unwrap();
    assert!(task_archive::set(&s, "running", true).is_err());
    task_archive::set(&s, "done", true).unwrap();
    assert_eq!(before, app::tasks::get(&s, "done").unwrap());
    let reopen = Store::open(&s.path).unwrap();
    assert!(task_archive::ids(&reopen).unwrap().contains("done"));
    assert_eq!(task_archive::finished_all(&s).unwrap(), 0);
    assert_eq!(app::history(&s, &ui.session).unwrap().len(), 2);
    assert!(s.file_roots().unwrap().is_empty());
    assert_eq!(
        rusqlite::Connection::open(&s.path)
            .unwrap()
            .query_row("SELECT count(*) FROM app_commands", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    task_archive::set(&s, "done", false).unwrap();
    assert!(task_archive::ids(&s).unwrap().is_empty());
    task_archive::set(&s, "done", true).unwrap();
    rusqlite::Connection::open(&s.path)
        .unwrap()
        .execute(
            "UPDATE app_tasks SET revision=1,state='processing' WHERE id='done'",
            [],
        )
        .unwrap();
    assert!(task_archive::ids(&s).unwrap().is_empty());
    rusqlite::Connection::open(&s.path)
        .unwrap()
        .execute("UPDATE app_tasks SET state='completed' WHERE id='done'", [])
        .unwrap();
    assert!(
        task_archive::ids(&s).unwrap().is_empty(),
        "an old archived revision must not hide a new completed result"
    );
}
#[test]
fn slash_completion_and_task_archive_flow_use_no_model_or_send_queue() {
    let (_team, mut s, mut ui) = fixture();
    task(&s, &ui, "done", "completed");
    task(&s, &ui, "active", "processing");
    ui.refresh(&s).unwrap();
    ui.input.insert("/ta");
    assert!(screen(&ui, &s, 100, 30).contains("/tasks"));
    key(&mut ui, &mut s, KeyCode::Tab);
    assert_eq!(ui.input.text, "/tasks");
    assert_eq!(
        rusqlite::Connection::open(&s.path)
            .unwrap()
            .query_row("SELECT count(*) FROM app_commands", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    key(&mut ui, &mut s, KeyCode::Enter);
    let text = screen(&ui, &s, 100, 30);
    assert!(text.contains("任务-active"));
    assert!(!text.contains("任务-done"));
    key(&mut ui, &mut s, KeyCode::Tab);
    assert!(screen(&ui, &s, 100, 30).contains("任务-done"));
    key(&mut ui, &mut s, KeyCode::Char('a'));
    assert!(task_archive::ids(&s).unwrap().contains("done"));
    assert!(!screen(&ui, &s, 100, 30).contains("任务-done"));
    key(&mut ui, &mut s, KeyCode::Tab);
    assert!(screen(&ui, &s, 100, 30).contains("任务-done"));
    key(&mut ui, &mut s, KeyCode::Enter);
    assert!(screen(&ui, &s, 100, 30).contains("原始结果-done"));
    slash(&mut ui, &mut s, "/archived");
    key(&mut ui, &mut s, KeyCode::Char('a'));
    assert!(task_archive::ids(&s).unwrap().is_empty());
    slash(&mut ui, &mut s, "/identity");
    assert!(screen(&ui, &s, 100, 30).contains("我的身份"));
    slash(&mut ui, &mut s, "/status");
    assert!(screen(&ui, &s, 100, 30).contains("连接状态"));
    key(&mut ui, &mut s, KeyCode::Esc);
    ui.input.insert("/unknown");
    assert!(
        ui.key(&mut s, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .is_err()
    );
    assert_eq!(
        rusqlite::Connection::open(&s.path)
            .unwrap()
            .query_row("SELECT count(*) FROM app_commands", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn archived_task_messages_remain_unread_and_accessible_in_inbox() {
    let (_team, mut s, mut ui) = fixture();
    task(&s, &ui, "done", "completed");
    let row = json!({"id":"thread","kind":"task","task_id":"done","subject":"已归档任务的新留言","unread":true,"last_entry":{"sender":"b","recipient":"a","body":"还需要讨论"}});
    rusqlite::Connection::open(&s.path)
        .unwrap()
        .execute(
            "INSERT INTO member_inbox_threads VALUES('thread',?1)",
            [row.to_string()],
        )
        .unwrap();
    task_archive::set(&s, "done", true).unwrap();
    ui.refresh(&s).unwrap();
    assert!(!ui.visible_messages().iter().any(|m| m["task_id"] == "done"));
    slash(&mut ui, &mut s, "/inbox");
    assert!(!screen(&ui, &s, 100, 30).contains("还需要讨论"));
    key(&mut ui, &mut s, KeyCode::Tab);
    assert!(screen(&ui, &s, 100, 30).contains("还需要讨论"));
    assert_eq!(
        rusqlite::Connection::open(&s.path)
            .unwrap()
            .query_row("SELECT count(*) FROM member_inbox_viewed", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn menu_scroll_colors_and_narrow_terminals_preserve_input() {
    let (_team, mut s, mut ui) = fixture();
    ui.input.insert("/");
    key(&mut ui, &mut s, KeyCode::Up); // Wrap to the last command and scroll it into view.
    for (w, h) in [(100, 30), (45, 12), (10, 4)] {
        let _ = screen(&ui, &s, w, h);
    }
    assert!(screen(&ui, &s, 100, 30).contains("/update"));
    key(&mut ui, &mut s, KeyCode::Esc);
    assert_eq!(ui.input.text, "/");
    key(&mut ui, &mut s, KeyCode::Char('h'));
    key(&mut ui, &mut s, KeyCode::Tab);
    assert_eq!(ui.input.text, "/help");
    key(&mut ui, &mut s, KeyCode::Esc);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| ui.render(f, &s)).unwrap();
    assert!(
        term.backend()
            .buffer()
            .content
            .iter()
            .any(|c| c.fg == Color::Cyan)
    );
    assert!(
        term.backend()
            .buffer()
            .content
            .iter()
            .any(|c| c.fg == Color::DarkGray)
    );
    assert!(screen(&ui, &s, 100, 30).contains("直接输入，或用 / 选择操作"));
}

#[test]
fn timeline_collapses_transfer_events_by_id_keeps_time_and_preserves_evidence() {
    let (_team, s, mut ui) = fixture();
    let conn = rusqlite::Connection::open(&s.path).unwrap();
    let states = ["draft", "queued", "uploading", "available", "delivered"];
    for id in ["batch-a", "batch-b"] {
        let manifest = json!({"protocol":1,"id":id,"sender":"a","recipient":"b","note":"Internal verification detail", "files":[{"id":"f","name":"report.csv","bytes":9437184,"sha256":"x"}],"task":null,"replaces":null});
        conn.execute("INSERT INTO file_transfers(id,direction,session_id,manifest,state,created_at,updated_at) VALUES(?1,'out',?2,?3,'delivered',1791918300,1791918320)",params![id,ui.session,manifest.to_string()]).unwrap();
        for (n, state) in states.iter().enumerate() {
            let mid = xxassxx::team::stable_id(id, &format!("attachment:{state}"));
            conn.execute("INSERT INTO app_messages(id,session_id,sender,recipient,kind,body,created_at) VALUES(?1,?2,'butler','a','attachment',?3,?4)",params![mid,ui.session,format!("Old transport body {state}"),1791918300+n as i64]).unwrap();
        }
    }
    conn.execute("INSERT INTO app_commands(id,actor,channel,session_id,recipient,body,action,payload,state,created_at) VALUES('allow-batch','a','tui',?1,'a','允许发送','allow_files',?2,'completed',1791918325)",params![ui.session,json!({"payload":{"transfer_id":"batch-a"}}).to_string()]).unwrap();
    for (id, kind) in [("user-allow", "user"), ("ack-allow", "attachment")] {
        conn.execute("INSERT INTO app_messages(id,session_id,sender,recipient,kind,body,command_id,created_at) VALUES(?1,?2,'a','a',?3,'允许发送','allow-batch',1791918325)",params![id,ui.session,kind]).unwrap();
    }
    ui.refresh(&s).unwrap();
    assert_eq!(app::history(&s, &ui.session).unwrap().len(), 12);
    assert_eq!(
        ui.visible_messages().len(),
        2,
        "identical names in separate batches must remain distinct"
    );
    assert_eq!(ui.visible_messages()[0]["created_at"], 1791918300);
    let rendered = screen(&ui, &s, 145, 38);
    assert_eq!(rendered.matches("report.csv").count(), 2);
    assert!(rendered.contains("你发给 @b 的文件"));
    assert!(rendered.contains("2026-10-") && rendered.contains("9.0 MiB"));
    assert!(!rendered.contains("Internal verification") && !rendered.contains("Old transport"));
    assert!(!rendered.contains("助手 · 消息"));
    conn.execute(
        "UPDATE file_transfers SET state='failed',error='网络断开' WHERE id='batch-a'",
        [],
    )
    .unwrap();
    ui.refresh(&s).unwrap();
    assert!(screen(&ui, &s, 145, 38).contains("网络断开"));
    conn.execute(
        "INSERT INTO app_preferences VALUES('hidden_transfer:batch-a','1')",
        [],
    )
    .unwrap();
    ui.refresh(&s).unwrap();
    assert_eq!(ui.visible_messages().len(), 1);
    assert_eq!(
        app::snapshot(&s, &ui.session).unwrap()["attachments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(app::history(&s, &ui.session).unwrap().len(), 12);
    assert_eq!(
        conn.query_row("SELECT count(*) FROM file_transfers", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn attachment_approval_keeps_actions_visible_and_long_notes_in_details() {
    let (_team, mut s, mut ui) = fixture();
    let conn = rusqlite::Connection::open(&s.path).unwrap();
    let files: Vec<_> = (0..40).map(|n| json!({"id":format!("f-{n}"),"name":format!("image-{n:02}.png"),"bytes":258255,"sha256":"x"})).collect();
    let note = "Internal search transcript. ".repeat(100);
    let manifest = json!({"protocol":1,"id":"batch","sender":"a","recipient":"b","note":note,"files":files,"task":null,"replaces":null});
    conn.execute("INSERT INTO file_transfers(id,direction,session_id,manifest,state,created_at,updated_at) VALUES('batch','out',?1,?2,'draft',1,1)",params![ui.session,manifest.to_string()]).unwrap();
    ui.refresh(&s).unwrap();
    ui.key(
        &mut s,
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
    )
    .unwrap();
    key(&mut ui, &mut s, KeyCode::Enter);
    for _ in 0..39 {
        key(&mut ui, &mut s, KeyCode::Down);
    }
    for (w, h) in [(145, 38), (85, 24)] {
        let view = screen(&ui, &s, w, h);
        assert!(view.contains("image-39.png"));
        assert!(view.contains("a 允许发送") && view.contains("i 说明和路径"));
        assert!(
            !view.contains("Internal search")
                && !view.contains("已传 0")
                && !view.contains("讨论当前任务")
        );
    }
    key(&mut ui, &mut s, KeyCode::Char('i'));
    assert!(screen(&ui, &s, 145, 38).contains("Internal search"));
    key(&mut ui, &mut s, KeyCode::Esc);
    assert!(screen(&ui, &s, 145, 38).contains("image-39.png"));
    assert!(
        !conn
            .query_row(
                "SELECT authorized FROM file_transfers WHERE id='batch'",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
}

fn business_task(s: &mut Store, ui: &Ui, title: &str) -> String {
    let i = app::Instruction {
        request_id: uuid::Uuid::new_v4().to_string(),
        channel: "test".into(),
        session_id: ui.session.clone(),
        task_id: None,
        recipient: "a".into(),
        body: title.into(),
        action: "create_task".into(),
        payload: json!({}),
    };
    let actor = app::Actor::local(s).unwrap();
    app::submit(s, &actor, &i).unwrap()["task_id"]
        .as_str()
        .unwrap()
        .into()
}
#[test]
fn retry_uses_the_highlighted_task_instead_of_a_stale_chat_selection() {
    let (_team, mut s, mut ui) = fixture();
    let first = business_task(&mut s, &ui, "第一个任务");
    let second = business_task(&mut s, &ui, "第二个任务");
    ui.refresh(&s).unwrap();
    ui.selected_task = Some(first);
    ui.key(
        &mut s,
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
    )
    .unwrap();
    let index = ui.data["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .position(|t| t["id"] == second)
        .unwrap();
    for _ in 0..index {
        key(&mut ui, &mut s, KeyCode::Down);
    }
    ui.key(
        &mut s,
        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
    )
    .unwrap();
    let conn = rusqlite::Connection::open(&s.path).unwrap();
    let task: String = conn
        .query_row(
            "SELECT task_id FROM app_commands WHERE action='retry_task'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(task, second);
}
#[test]
fn stop_form_preserves_input_has_two_choices_and_archives_with_reason() {
    let (_team, mut s, mut ui) = fixture();
    let id = business_task(&mut s, &ui, "查看当前材料");
    ui.refresh(&s).unwrap();
    ui.selected_task = Some(id.clone());
    ui.input.insert("还没发出的补充");
    ui.key(
        &mut s,
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
    )
    .unwrap();
    let visible = screen(&ui, &s, 145, 38);
    assert!(
        visible.contains("停止并归档")
            && visible.contains("仅停止")
            && visible.contains("停止原因")
    );
    assert!(
        ui.key(&mut s, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .is_err()
    );
    key(&mut ui, &mut s, KeyCode::Esc);
    assert_eq!(ui.input.text, "还没发出的补充");
    assert_eq!(ui.selected_task.as_deref(), Some(id.as_str()));
    assert_eq!(app::tasks::get(&s, &id).unwrap()["state"], "draft");
    ui.input.clear();
    ui.input.insert("结束这个任务。");
    key(&mut ui, &mut s, KeyCode::Enter);
    assert!(screen(&ui, &s, 145, 38).contains("停止原因"));
    ui.input.insert("换了方案，本次先收起");
    key(&mut ui, &mut s, KeyCode::Tab);
    key(&mut ui, &mut s, KeyCode::Tab);
    for (w, h) in [(100, 30), (45, 12), (10, 4)] {
        let _ = screen(&ui, &s, w, h);
    }
    key(&mut ui, &mut s, KeyCode::Enter);
    assert!(task_archive::ids(&s).unwrap().contains(&id));
    assert_eq!(app::tasks::get(&s, &id).unwrap()["state"], "cancelled");
    slash(&mut ui, &mut s, "/archived");
    key(&mut ui, &mut s, KeyCode::Enter);
    let visible = screen(&ui, &s, 145, 38);
    assert!(visible.contains("停止记录") && visible.contains("换了方案"));
    assert!(!visible.contains("已完成"));
}
#[test]
fn stop_uses_highlighted_task_and_rejects_a_stale_open_form() {
    let (_team, mut s, mut ui) = fixture();
    let first = business_task(&mut s, &ui, "第一项");
    let second = business_task(&mut s, &ui, "第二项");
    rusqlite::Connection::open(&s.path)
        .unwrap()
        .execute(
            "UPDATE app_tasks SET created_at=CASE WHEN id=?1 THEN 1 ELSE 2 END",
            [&first],
        )
        .unwrap();
    ui.refresh(&s).unwrap();
    ui.selected_task = Some(first.clone());
    // Active-task list's current row, not an old open conversation, is the target.
    ui.input.insert("/tasks");
    key(&mut ui, &mut s, KeyCode::Enter);
    let visible = screen(&ui, &s, 145, 38);
    assert!(visible.contains("第二项"));
    ui.key(
        &mut s,
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
    )
    .unwrap();
    ui.input.insert("只停止当前选中项");
    key(&mut ui, &mut s, KeyCode::Tab);
    key(&mut ui, &mut s, KeyCode::Enter);
    let stopped = ui.selected_task.clone().unwrap();
    assert_ne!(stopped, first);
    assert_eq!(stopped, second);
    assert!(task_archive::ids(&s).unwrap().is_empty());
    assert_eq!(app::tasks::get(&s, &first).unwrap()["state"], "draft");
    ui.selected_task = Some(first.clone());
    ui.input.insert("/stop");
    key(&mut ui, &mut s, KeyCode::Enter);
    ui.input.insert("旧版本的停止原因");
    let revise = app::Instruction {
        request_id: uuid::Uuid::new_v4().to_string(),
        channel: "test".into(),
        session_id: ui.session.clone(),
        task_id: Some(first.clone()),
        recipient: "a".into(),
        body: "新的范围".into(),
        action: "revise_task".into(),
        payload: json!({"revision":1}),
    };
    xxassxx::task_coordinator::revise(&s, &revise).unwrap();
    ui.refresh(&s).unwrap();
    assert!(
        ui.key(&mut s, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .is_err()
    );
    assert_eq!(app::tasks::get(&s, &first).unwrap()["revision"], 2);
    assert_eq!(app::tasks::get(&s, &first).unwrap()["state"], "draft");
    assert!(ui.input.text.contains("旧版本"));
}

#[test]
fn codex_directory_picker_can_cancel_or_reject_outside_path_without_changing_chat() {
    let (team, mut s, mut ui) = fixture();
    let original_session = ui.session.clone();
    slash(&mut ui, &mut s, "/codex");
    assert!(screen(&ui, &s, 140, 32).contains("白名单为空"));
    key(&mut ui, &mut s, KeyCode::Esc);
    s.allow_directory(&team.dir.path().join("a/work")).unwrap();
    slash(&mut ui, &mut s, "/codex");
    assert!(screen(&ui, &s, 140, 32).contains("在哪个目录启动 Codex"));
    key(&mut ui, &mut s, KeyCode::Char('p'));
    key(&mut ui, &mut s, KeyCode::End);
    for _ in 0..300 {
        key(&mut ui, &mut s, KeyCode::Backspace);
    }
    key(&mut ui, &mut s, KeyCode::Char('/'));
    key(&mut ui, &mut s, KeyCode::Enter);
    assert!(screen(&ui, &s, 140, 32).contains("必须在当前白名单内"));
    key(&mut ui, &mut s, KeyCode::Esc);
    key(&mut ui, &mut s, KeyCode::Esc);
    assert_eq!(ui.session, original_session);
    let conn = rusqlite::Connection::open(&s.path).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM app_commands", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
