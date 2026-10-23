use rusqlite::{Connection, params};
use serde_json::json;
use xxassxx::{
    app::{self, export},
    store::Store,
};

fn fixture() -> (tempfile::TempDir, Store, String) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(&dir.path().join("client.sqlite3")).unwrap();
    store.set_identity("alice", "Alice", "lab").unwrap();
    let project = app::project(&store, dir.path()).unwrap();
    let session = app::session(&store, project["id"].as_str().unwrap(), "alice").unwrap();
    (dir, store, session)
}

#[test]
fn export_is_complete_scoped_verbatim_and_deduplicates_message_events() {
    let (_dir, store, session) = fixture();
    let conn = Connection::open(&store.path).unwrap();
    let project: String = conn
        .query_row(
            "SELECT project_id FROM app_sessions WHERE id=?1",
            [&session],
            |r| r.get(0),
        )
        .unwrap();
    let other = app::new_session(&store, &project).unwrap();
    let text = "第一段原话\n\n第二段，保留缩进：\n  A = 83\n";
    app::add_message(
        &conn, "original", &session, None, "alice", "butler", "user", text, None, None,
    )
    .unwrap();
    app::add_message(
        &conn,
        "unrelated",
        &other,
        None,
        "alice",
        "butler",
        "user",
        "不能导出无关会话",
        None,
        None,
    )
    .unwrap();
    for i in 0..650 {
        app::event(
            &conn,
            &format!("activity-{i}"),
            &session,
            None,
            "progress",
            json!({"body":format!("独立活动-{i}"),"access_token":"never-export-this"}),
        )
        .unwrap();
    }
    conn.execute("INSERT INTO app_tasks(id,session_id,project_id,title,goal,state,created_at,archived) VALUES('task',?1,?2,'已归档任务','核对数据','completed',1,1)", params![session, project]).unwrap();
    conn.execute(
        "INSERT INTO task_journal VALUES('j','task',1,'bob',1,'progress',?1,2)",
        [json!({"result":"旧失败和恢复仍保留"}).to_string()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO task_journal VALUES('missing','task',1,'bob',2,'progress','{}',3)",
        [],
    )
    .unwrap();
    app::task_archive::set(&store, "task", true).unwrap();
    let record = export::conversation(&store, &session).unwrap();
    assert_eq!(record.records, 654);
    assert!(record.text.contains("本机归档"));
    assert_eq!(record.text.matches(text).count(), 1);
    assert!(record.text.contains("独立活动-649"));
    assert!(record.text.contains("旧失败和恢复仍保留"));
    assert!(record.text.contains("已归档任务"));
    assert!(!record.text.contains("不能导出无关会话"));
    assert!(!record.text.contains("never-export-this"));
    assert_eq!(
        conn.query_row("SELECT count(*) FROM app_commands", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn historical_event_time_is_unknown_and_new_events_are_timestamped() {
    let (_dir, mut store, session) = fixture();
    let task = store.submit("测试真实活动落盘时间").unwrap();
    let lease = store
        .claim(&task.id, false, std::path::Path::new("/tmp"), 120)
        .unwrap();
    store.event(&lease, &json!({"type":"item.completed","item":{"type":"command_execution","command":"printf 83","aggregated_output":"83"}})).unwrap();
    let conn = Connection::open(&store.path).unwrap();
    conn.execute(
        "INSERT INTO native_tasks(task_id,roots,command_id) VALUES(?1,'[]',NULL)",
        [&task.id],
    )
    .unwrap();
    let project: String = conn
        .query_row(
            "SELECT project_id FROM app_sessions WHERE id=?1",
            [&session],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute("INSERT INTO app_tasks(id,session_id,project_id,title,goal,state,workflow_id,created_at) VALUES('legacy',?1,?2,'历史任务','检查','completed',?3,1)", params![session,project,task.id]).unwrap();
    conn.execute(
        "INSERT INTO events(run_id,event) VALUES(?1,?2)",
        params![
            lease.run_id,
            json!({"type":"item.completed","item":{"type":"agent_message","text":"历史运行结果"}})
                .to_string()
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO events(run_id,event) VALUES(?1,?2)",
        params![
            lease.run_id,
            json!({"type":"item.completed","item":{"type":"reasoning","text":"不要导出内部思考"}})
                .to_string()
        ],
    )
    .unwrap();
    let record = export::conversation(&store, &session).unwrap();
    assert!(record.text.contains("printf 83") && record.text.contains("历史运行结果"));
    assert!(record.text.contains("时间未知"));
    assert!(!record.text.contains("不要导出内部思考"));
    assert!(
        conn.query_row(
            "SELECT created_at IS NOT NULL FROM events ORDER BY id LIMIT 1",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
}

#[test]
fn save_is_private_atomic_and_never_overwrites() {
    let (dir, store, session) = fixture();
    let record = export::conversation(&store, &session).unwrap();
    let output = dir.path().join("exports");
    let first = export::save(&record, &output).unwrap();
    let second = export::save(&record, &output).unwrap();
    assert_ne!(first, second);
    assert_eq!(std::fs::read_to_string(&first).unwrap(), record.text);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(first).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(export::save(&record, std::path::Path::new("relative")).is_err());
    assert!(export::save(&record, &second).is_err());
    assert_eq!(std::fs::read_dir(output).unwrap().count(), 2);
}

#[test]
fn schema14_upgrade_keeps_historical_event_bytes_and_unknown_time() {
    let (dir, mut store, _session) = fixture();
    let task = store.submit("旧版记录").unwrap();
    let lease = store.claim(&task.id, false, dir.path(), 120).unwrap();
    let conn = Connection::open(&store.path).unwrap();
    let original = "{\"type\":\"item.completed\", \"item\":{\"text\":\"原始结果\"}}";
    conn.execute(
        "INSERT INTO events(run_id,event) VALUES(?1,?2)",
        params![lease.run_id, original],
    )
    .unwrap();
    conn.execute_batch("ALTER TABLE events DROP COLUMN created_at; ALTER TABLE model_steps DROP COLUMN created_at; PRAGMA user_version=14;").unwrap();
    let upgraded = Store::open(&store.path).unwrap();
    let conn = Connection::open(&upgraded.path).unwrap();
    assert_eq!(
        conn.query_row("SELECT event FROM events", [], |r| r.get::<_, String>(0))
            .unwrap(),
        original
    );
    assert!(
        conn.query_row("SELECT created_at IS NULL FROM events", [], |r| r
            .get::<_, bool>(0))
            .unwrap()
    );
    assert_eq!(
        conn.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
            .unwrap(),
        15
    );
    assert_eq!(
        conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}

#[test]
fn terminal_export_can_cancel_retry_save_and_return_without_model_commands() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};
    use xxassxx::tui::Ui;
    let (dir, mut store, _session) = fixture();
    let project = app::project(&store, dir.path()).unwrap();
    let mut ui = Ui::new(&store, project).unwrap();
    let conn = Connection::open(&store.path).unwrap();
    app::add_message(
        &conn,
        "ui-result",
        &ui.session,
        None,
        "butler",
        "alice",
        "reply",
        "核对结果：83。",
        None,
        None,
    )
    .unwrap();
    let key = |ui: &mut Ui, store: &mut Store, code| {
        ui.key(store, KeyEvent::new(code, KeyModifiers::NONE))
            .unwrap();
    };
    ui.input.insert("/export");
    key(&mut ui, &mut store, KeyCode::Enter);
    key(&mut ui, &mut store, KeyCode::Esc);
    assert!(
        std::fs::read_dir(dir.path()).unwrap().all(|f| f
            .unwrap()
            .path()
            .extension()
            .is_none_or(|e| e != "txt"))
    );
    ui.input.insert("/export");
    key(&mut ui, &mut store, KeyCode::Enter);
    key(&mut ui, &mut store, KeyCode::Char('d'));
    key(&mut ui, &mut store, KeyCode::Home);
    for _ in 0..500 {
        key(&mut ui, &mut store, KeyCode::Delete);
    }
    for c in "relative".chars() {
        key(&mut ui, &mut store, KeyCode::Char(c));
    }
    key(&mut ui, &mut store, KeyCode::Enter);
    key(&mut ui, &mut store, KeyCode::Enter);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    term.draw(|f| ui.render(f, &store)).unwrap();
    let screen: String = buffer_text(term.backend().buffer());
    assert!(screen.contains("导出未完成"));
    key(&mut ui, &mut store, KeyCode::Char('d'));
    key(&mut ui, &mut store, KeyCode::Home);
    for _ in 0..500 {
        key(&mut ui, &mut store, KeyCode::Delete);
    }
    let output = dir.path().join("exported");
    for c in output.to_str().unwrap().chars() {
        key(&mut ui, &mut store, KeyCode::Char(c));
    }
    key(&mut ui, &mut store, KeyCode::Enter);
    key(&mut ui, &mut store, KeyCode::Enter);
    let file = std::fs::read_dir(&output)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(
        std::fs::read_to_string(file)
            .unwrap()
            .contains("核对结果：83。")
    );
    term.draw(|f| ui.render(f, &store)).unwrap();
    let screen: String = buffer_text(term.backend().buffer());
    assert!(screen.contains("已导出 TXT"));
    key(&mut ui, &mut store, KeyCode::Enter);
    assert_eq!(
        conn.query_row("SELECT count(*) FROM app_commands", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
    use unicode_width::UnicodeWidthStr;
    let mut text = String::new();
    for y in 0..buffer.area.height {
        let mut x = 0;
        while x < buffer.area.width {
            let symbol = buffer[(x, y)].symbol();
            text.push_str(symbol);
            x += symbol.width().max(1) as u16;
        }
        text.push('\n');
    }
    text
}

#[test]
fn local_file_tool_receipts_include_evidence_and_preserve_unknown_legacy_times() {
    let (_dir, store, session) = fixture();
    let conn = Connection::open(&store.path).unwrap();
    conn.execute("INSERT INTO app_commands(id,actor,channel,session_id,recipient,body,action,payload,created_at) VALUES('command','alice','tui',?1,'alice','核对文件','chat','{}',1)", [&session]).unwrap();
    let trace = json!([
        {"created_at":2,"name":"read_file","arguments":{"path":"values.txt","api_key":"private-value"},"result":{"ok":true,"value":"17\n29\n37\n"}},
        {"name":"legacy_read","result":{"ok":false,"error":"保留旧失败"}}
    ]);
    conn.execute("INSERT INTO app_model_runs(id,command_id,state,trace,created_at) VALUES('model','command','done',?1,1)", [trace.to_string()]).unwrap();
    app::add_message(
        &conn,
        "final-answer",
        &session,
        None,
        "butler",
        "alice",
        "answer",
        "已核对：83。",
        None,
        None,
    )
    .unwrap();
    app::event(&conn,"context-event",&session,None,"task_chat_context",json!({"policy":"static-instruction-not-an-activity","tasks":[{"id":"original-task-reference"}]})).unwrap();
    let record = export::conversation(&store, &session).unwrap();
    assert_eq!(record.records, 4);
    assert!(record.text.contains("助手答复 · butler"));
    assert!(record.text.contains("original-task-reference"));
    assert!(!record.text.contains("static-instruction-not-an-activity"));
    assert!(record.text.contains("read_file（调用回执）"));
    assert!(record.text.contains("17\n29\n37\n"));
    assert!(record.text.contains("保留旧失败"));
    assert!(!record.text.contains("private-value"));
    assert!(record.text.contains("其中时间未知 1 条"));
}
