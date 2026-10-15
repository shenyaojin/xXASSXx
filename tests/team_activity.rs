mod common;
use common::Team;
use rusqlite::Connection;
use serde_json::{Value, json};
use xxassxx::{
    store::now,
    task_coordinator::Wire,
    task_schedule,
    team::{Message, stable_id},
    team_activity as activity,
};
fn message(id: &str, sender: &str, event: &str, sequence: i64, data: Value) -> Message {
    Message {
        message_id: uuid::Uuid::new_v4().to_string(),
        conversation_id: stable_id(id, "pair"),
        sender: sender.into(),
        recipient: if sender == "a" { "b" } else { "a" }.into(),
        kind: "request".into(),
        operation: "task_v2".into(),
        body: serde_json::to_string(&Wire {
            version: 2,
            task_id: id.into(),
            revision: 1,
            sequence,
            event: event.into(),
            data,
        })
        .unwrap(),
        created_at: now(),
        reply_to: None,
        object_id: None,
        version_id: None,
        workflow: None,
    }
}
fn proposal(id: &str, shared: bool) -> Message {
    message(
        id,
        "a",
        "proposal",
        1,
        json!({"initiator":"a","participants":["a","b"],"confirmed":1,"team_activity":shared,"title":"Project progress","draft":{"goal":"Summarize allowed project progress"}}),
    )
}
fn project(c: &Connection, m: &Message) {
    let tx = c.unchecked_transaction().unwrap();
    task_schedule::accept(&tx, m, now()).unwrap();
    activity::project(&tx, m, now()).unwrap();
    tx.commit().unwrap();
}
#[test]
fn observer_sees_bounded_shared_progress_and_result_but_not_old_or_private_tasks() {
    let team = Team::new();
    let c = Connection::open(team.dir.path().join("relay.sqlite3")).unwrap();
    let old = uuid::Uuid::new_v4().to_string();
    project(&c, &proposal(&old, false));
    project(
        &c,
        &message(
            &old,
            "b",
            "progress",
            1,
            json!({"team_scope":"whitelist","body":"private legacy result"}),
        ),
    );
    let id = uuid::Uuid::new_v4().to_string();
    project(&c, &proposal(&id, true));
    assert!(
        activity::changes(&c, "c", 0).unwrap()["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    project(
        &c,
        &message(
            &id,
            "a",
            "progress",
            2,
            json!({"team_scope":"whitelist","body":"initiator cannot attest remote scope"}),
        ),
    );
    assert!(
        activity::changes(&c, "c", 0).unwrap()["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let start = message(
        &id,
        "b",
        "progress",
        1,
        json!({"team_scope":"whitelist","stage":"reading","body":"raw internal log must not be shared","debug":{"secret":"PRIVATE"}}),
    );
    project(&c, &start);
    project(&c, &start);
    let progress = activity::changes(&c, "c", 0).unwrap();
    assert_eq!(progress["events"].as_array().unwrap().len(), 1);
    assert!(!progress.to_string().contains("PRIVATE"));
    project(
        &c,
        &message(&id, "b", "candidate", 2, json!({"body":"unreviewed"})),
    );
    let complete = message(
        &id,
        "a",
        "complete",
        3,
        json!({"body":"Three measurements total 83; next step is review of the pending experiment.","candidates":[{"member":"b","result":{"sources":[{"member":"b","root":0,"path":"progress.md","sha256":"abc","bytes":42,"private_path":"SECRET"}]}}],"private":"SECRET"}),
    );
    project(&c, &complete);
    project(&c, &complete);
    project(
        &c,
        &message(&id, "b", "progress", 3, json!({"body":"late progress"})),
    );
    let page = activity::changes(&c, "c", 0).unwrap();
    let events = page["events"].as_array().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[2]["state"], "completed");
    assert!(events[2]["body"].as_str().unwrap().contains("83"));
    assert_eq!(events[2]["sources"][0]["path"], "progress.md");
    assert!(!page.to_string().contains("SECRET"));
    let observer = team.store("c");
    activity::apply(&observer, &page).unwrap();
    assert_eq!(activity::cached(&observer).unwrap().len(), 3);
    assert_eq!(
        Connection::open(&observer.path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM app_tasks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0,
        "reading shared records never creates work"
    );
    c.execute("DELETE FROM relay_members WHERE id='c'", [])
        .unwrap();
    assert!(activity::changes(&c, "c", 0).is_err());
}
#[test]
fn new_revision_and_stale_events_do_not_regress_shared_result() {
    let team = Team::new();
    let c = Connection::open(team.dir.path().join("relay.sqlite3")).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    project(&c, &proposal(&id, true));
    project(
        &c,
        &message(&id, "b", "progress", 4, json!({"team_scope":"whitelist"})),
    );
    project(
        &c,
        &message(&id, "b", "question", 3, json!({"body":"late question"})),
    );
    assert_eq!(
        activity::changes(&c, "c", 0).unwrap()["events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let mut next = proposal(&id, true);
    let mut w = Wire::parse(&next).unwrap();
    w.revision = 2;
    w.data["confirmed"] = json!(2);
    next.body = serde_json::to_string(&w).unwrap();
    project(&c, &next);
    project(
        &c,
        &message(&id, "a", "complete", 9, json!({"body":"old completion"})),
    );
    let mut ready = message(&id, "b", "progress", 1, json!({"team_scope":"whitelist"}));
    let mut w = Wire::parse(&ready).unwrap();
    w.revision = 2;
    ready.body = serde_json::to_string(&w).unwrap();
    project(&c, &ready);
    let page = activity::changes(&c, "c", 0).unwrap();
    assert_eq!(page["events"].as_array().unwrap().len(), 2);
    assert_eq!(page["events"][1]["revision"], 2);
}
#[tokio::test]
async fn offline_proposal_waits_online_retry_is_once_and_observer_cannot_mutate() {
    let team = Team::new();
    let c = Connection::open(team.dir.path().join("relay.sqlite3")).unwrap();
    c.execute("DELETE FROM relay_presence", []).unwrap();
    let client = reqwest::Client::new();
    let id = uuid::Uuid::new_v4().to_string();
    let p = proposal(&id, true);
    let post = || {
        client
            .post(format!("{}/v1/messages", team.url))
            .bearer_auth(common::TOKEN)
            .json(&p)
            .send()
    };
    let r = post().await.unwrap();
    assert_eq!(r.status(), 409);
    assert_eq!(
        r.json::<Value>().await.unwrap()["error"],
        "task_peer_offline"
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM relay_tasks", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    client
        .post(format!("{}/v1/presence", team.url))
        .bearer_auth("local-test-credential-bbbbbbbb")
        .json(&json!({"activity":"idle","team_activity_protocol":1}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    post().await.unwrap().error_for_status().unwrap();
    c.execute("DELETE FROM relay_presence", []).unwrap();
    post().await.unwrap().error_for_status().unwrap();
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM relay_messages", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let forged = message(&id, "c", "progress", 1, json!({"team_scope":"whitelist"}));
    let r = client
        .post(format!("{}/v1/messages", team.url))
        .bearer_auth("local-test-credential-cccccccc")
        .json(&forged)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    assert_eq!(
        client
            .get(format!("{}/v2/team-activity", team.url))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let r = client
        .get(format!("{}/v2/team-activity", team.url))
        .bearer_auth("local-test-credential-cccccccc")
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
}
#[test]
fn migration_does_not_publish_history_and_shared_read_revocation_survives_full_access() {
    use xxassxx::{
        access_mode::{self, Mode},
        app, task_coordinator as tasks, task_materials,
    };
    let team = Team::new();
    let b = team.store("b");
    let white = team.dir.path().join("b/work");
    b.allow_directory(&white).unwrap();
    access_mode::set(&b, Mode::FullAccess).unwrap();
    let project = app::project(&b, &team.dir.path().join("b")).unwrap();
    let session = app::session(&b, project["id"].as_str().unwrap(), "b").unwrap();
    let i = app::Instruction {
        request_id: uuid::Uuid::new_v4().to_string(),
        channel: "test".into(),
        session_id: session,
        task_id: None,
        recipient: "b".into(),
        action: "create_task".into(),
        body: "Read whitelist project".into(),
        payload: json!({}),
    };
    let bc = Connection::open(&b.path).unwrap();
    let id = tasks::create(&bc, &i, "b", None).unwrap();
    tasks::prepare(
        &b,
        &id,
        json!({"goal":"Read","deliverables":["report"],"mode":"analysis"}),
    )
    .unwrap();
    let t = tasks::get(&b, &id).unwrap();
    task_materials::bind(&b, &t).unwrap();
    let (cwd, roots) = task_materials::check_binding(&bc, &id, 1).unwrap();
    assert_eq!(cwd, white.canonicalize().unwrap());
    assert_eq!(roots, vec![cwd]);
    assert!(activity::scoped(&bc, &id, 1).unwrap());
    bc.execute("DELETE FROM file_roots", []).unwrap();
    assert!(task_materials::check_binding(&bc, &id, 1).is_err());
    let c = Connection::open(team.dir.path().join("relay.sqlite3")).unwrap();
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM relay_activity_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    // Schema 10 databases migrate old tasks with sharing off, including drafts.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("member.sqlite3");
    let c = Connection::open(&db).unwrap();
    for schema in [
        include_str!("../src/schema.sql"),
        include_str!("../src/schema_v2.sql"),
        include_str!("../src/schema_v3.sql"),
        include_str!("../src/schema_v4.sql"),
        include_str!("../src/schema_v5.sql"),
        include_str!("../src/schema_v6.sql"),
        include_str!("../src/schema_v7.sql"),
        include_str!("../src/schema_v8.sql"),
        include_str!("../src/schema_v9.sql"),
        include_str!("../src/schema_v10.sql"),
    ] {
        c.execute_batch(schema).unwrap();
    }
    drop(c);
    let old = xxassxx::store::Store::open(&db).unwrap();
    assert_eq!(
        Connection::open(&old.path)
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        11
    );
}

#[tokio::test]
async fn withdrawn_offline_task_cannot_start_later_and_lost_ack_preserves_cancel_delivery() {
    let team = Team::new();
    let client = reqwest::Client::new();
    let id = uuid::Uuid::new_v4().to_string();
    let p = proposal(&id, true);
    let withdraw = || {
        client
            .post(format!("{}/v2/task-withdraw", team.url))
            .bearer_auth(common::TOKEN)
            .json(&p)
            .send()
    };
    assert_eq!(
        withdraw().await.unwrap().json::<Value>().await.unwrap()["persisted"],
        false
    );
    assert_eq!(
        withdraw().await.unwrap().json::<Value>().await.unwrap()["persisted"],
        false
    );
    let r = client
        .post(format!("{}/v1/messages", team.url))
        .bearer_auth(common::TOKEN)
        .json(&p)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 410);
    let accepted = proposal(&uuid::Uuid::new_v4().to_string(), true);
    client
        .post(format!("{}/v1/messages", team.url))
        .bearer_auth(common::TOKEN)
        .json(&accepted)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let r = client
        .post(format!("{}/v2/task-withdraw", team.url))
        .bearer_auth(common::TOKEN)
        .json(&accepted)
        .send()
        .await
        .unwrap();
    assert_eq!(r.json::<Value>().await.unwrap()["persisted"], true);
}
