use rusqlite::{Connection, params};
use xxassxx::{app, store::Store};
#[test]
fn schema_six_is_incremental_idempotent_and_does_not_forge_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    let c = Connection::open(&path).unwrap();
    for sql in [
        include_str!("../src/schema.sql"),
        include_str!("../src/schema_v2.sql"),
        include_str!("../src/schema_v3.sql"),
        include_str!("../src/schema_v4.sql"),
        include_str!("../src/schema_v5.sql"),
        include_str!("../src/schema_v6.sql"),
    ] {
        c.execute_batch(sql).unwrap();
    }
    c.execute(
        "INSERT INTO identity VALUES(1,'a','Alice','lab','{\"preserve\":true}')",
        [],
    )
    .unwrap();
    c.execute("INSERT INTO file_roots VALUES(?1)", [dir.path().to_str()])
        .unwrap();
    let p = uuid::Uuid::new_v4().to_string();
    let s = uuid::Uuid::new_v4().to_string();
    let task = uuid::Uuid::new_v4().to_string();
    c.execute(
        "INSERT INTO app_projects VALUES(?1,?2,'work')",
        params![p, dir.path().to_str()],
    )
    .unwrap();
    c.execute(
        "INSERT INTO app_sessions VALUES(?1,?2,'a','old chat',0,100)",
        params![s, p],
    )
    .unwrap();
    c.execute("INSERT INTO app_tasks(id,session_id,project_id,title,goal,state,created_at) VALUES(?1,?2,?3,'old','old goal','completed',100)",params![task,s,p]).unwrap();
    let pending = uuid::Uuid::new_v4().to_string();
    c.execute("INSERT INTO app_tasks(id,session_id,project_id,title,goal,state,error,created_at) VALUES(?1,?2,?3,'pending','old goal','waiting','keep original diagnostic',100)",params![pending,s,p]).unwrap();
    c.execute(
        "INSERT INTO tasks(id,input,created_at) VALUES(?1,'old pending intent',100)",
        [&task],
    )
    .unwrap();
    c.execute(
        "INSERT INTO native_tasks(task_id,roots) VALUES(?1,'[]')",
        [&task],
    )
    .unwrap();
    drop(c);
    let store = Store::open(&path).unwrap();
    let other = app::new_session(&store, &p).unwrap();
    assert_ne!(other, s);
    assert_eq!(store.file_roots().unwrap().len(), 1);
    assert_eq!(
        app::tasks::get(&store, &task).unwrap()["state"],
        "completed"
    );
    assert!(app::tasks::get(&store, &task).unwrap()["error"].is_null());
    let old = app::tasks::get(&store, &pending).unwrap();
    assert_eq!(old["state"], "needs_attention");
    assert!(
        old["error"]
            .as_str()
            .unwrap()
            .starts_with("keep original diagnostic")
    );
    drop(store);
    Store::open(&path).unwrap();
    let c = Connection::open(&path).unwrap();
    assert_eq!(
        c.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        8
    );
    assert_eq!(
        c.query_row("SELECT config FROM identity", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "{\"preserve\":true}"
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM task_revisions", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM legacy_holds", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
}
