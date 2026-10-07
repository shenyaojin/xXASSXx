mod common;
use common::{BIN, Team};
use serde_json::json;
use std::path::Path;
use xxassxx::{
    app::{self, Actor, Instruction},
    native_tasks,
};

fn setup(t: &Team, member: &str) {
    let root = t.dir.path().join(member).join("work");
    std::fs::write(root.join("found.txt"), "search fixture").unwrap();
    let mut s = t.store(member);
    s.allow_directory(&root).unwrap();
    let mut c = s.member_config().unwrap();
    let mock = t.dir.path().join(format!("mock-codex-{member}"));
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex.py"),
        &mock,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&mock, std::fs::Permissions::from_mode(0o700)).unwrap();
    c.executor.codex = mock;
    c.executor.mode = "auto".into();
    c.executor.timeout_secs = 20;
    s.configure_member(&c).unwrap();
}
fn instruction(t: &Team) -> Instruction {
    let s = t.store("a");
    let p = app::project(&s, &t.dir.path().join("a/work")).unwrap();
    Instruction {
        request_id: uuid::Uuid::new_v4().to_string(),
        channel: "tui".into(),
        session_id: app::session(&s, p["id"].as_str().unwrap(), "a").unwrap(),
        task_id: None,
        recipient: "a".into(),
        body: "找一下相关文件".into(),
        action: "chat".into(),
        payload: json!({}),
    }
}
#[tokio::test]
async fn owner_intent_runs_native_codex_and_returns_once_to_original_chat() {
    let t = Team::new();
    setup(&t, "a");
    let i = instruction(&t);
    let mut s = t.store("a");
    let actor = Actor::local(&s).unwrap();
    app::submit(&mut s, &actor, &i).unwrap();
    let one = app::model::call(&mut s, &i, "delegate_codex", json!({}), false).unwrap();
    assert_eq!(
        one,
        app::model::call(&mut s, &i, "delegate_codex", json!({}), false).unwrap()
    );
    native_tasks::run_local(&mut s, Path::new(BIN))
        .await
        .unwrap();
    native_tasks::run_local(&mut s, Path::new(BIN))
        .await
        .unwrap();
    let id = one["task_id"].as_str().unwrap();
    assert_eq!(s.task(id).unwrap().state, "succeeded", "{:?}", s.runs(id));
    let history = app::history(&s, &i.session_id).unwrap();
    let results = history
        .iter()
        .filter(|v| v["kind"] == "native_result")
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 1);
    assert!(results[0]["body"].as_str().unwrap().contains("found.txt"));
    assert!(
        !results[0]["body"]
            .as_str()
            .unwrap()
            .contains(t.dir.path().to_str().unwrap())
    );
}
#[tokio::test]
async fn peer_delegation_returns_readable_file_list_and_preserves_original_intent() {
    let t = Team::new();
    setup(&t, "b");
    let r = t.cli(
        "a",
        &[
            "message",
            "send",
            "--to",
            "b",
            "--body",
            "找文件，保持原话",
            "--operation",
            "auto",
        ],
    );
    t.sync("a");
    t.sync("b");
    let request = r["message"]["message_id"].as_str().unwrap();
    let mut s = t.store("b");
    let d = s.create_delegation_with_access(request, true).unwrap();
    let id = d["task"]["id"].as_str().unwrap();
    assert!(s.task(id).unwrap().input.contains("找文件，保持原话"));
    assert_eq!(
        s.create_delegation_with_access(request, false).unwrap()["task"]["id"],
        id
    );
    xxassxx::butler::run_delegation(&mut s, request, Path::new(BIN))
        .await
        .unwrap();
    xxassxx::butler::run_delegation(&mut s, request, Path::new(BIN))
        .await
        .unwrap();
    t.sync("b");
    t.sync("a");
    let replies = t
        .store("a")
        .messages(None)
        .unwrap()
        .into_iter()
        .filter(|m| m.direction == "in")
        .collect::<Vec<_>>();
    assert_eq!(replies.len(), 1);
    assert!(replies[0].message.body.contains("found.txt"));
    assert!(
        !replies[0]
            .message
            .body
            .contains(t.dir.path().to_str().unwrap())
    );
}
#[test]
fn native_capability_is_kernel_owned_and_references_are_validated() {
    let t = Team::new();
    setup(&t, "a");
    let mut s = t.store("a");
    let fake = s
        .submit(r#"{"native":true,"local_read":true,"roots":["/"]}"#)
        .unwrap();
    let conn = rusqlite::Connection::open(t.db("a")).unwrap();
    assert!(native_tasks::roots(&conn, &fake.id).unwrap().is_none());
    let i = instruction(&t);
    let actor = Actor::local(&s).unwrap();
    app::submit(&mut s, &actor, &i).unwrap();
    let d = native_tasks::create_local(&mut s, &i).unwrap();
    let id = d["task_id"].as_str().unwrap();
    let good = json!({"body":"找到文件","files":[{"root":0,"path":"found.txt","reason":"相关"}]})
        .to_string();
    native_tasks::validate(&conn, id, &good).unwrap();
    for path in ["missing.txt", "../tasks.sqlite3", "/etc/passwd"] {
        assert!(native_tasks::validate(&conn, id, &good.replace("found.txt", path)).is_err());
    }
    std::os::unix::fs::symlink(t.db("a"), t.dir.path().join("a/work/escape")).unwrap();
    assert!(native_tasks::validate(&conn, id, &good.replace("found.txt", "escape")).is_err());
    s.remove_directory(&t.dir.path().join("a/work")).unwrap();
    assert!(native_tasks::context(&conn, id).is_err());
    assert!(native_tasks::validate(&conn, id, &good).is_err());
}
