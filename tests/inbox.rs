mod common;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use xxassxx::{
    inbox::{self, Actor, InboxService, PrivateFacts, Reply, SendMessage},
    team::Message,
};

fn database() -> Connection {
    let mut c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE relay_team(id TEXT);INSERT INTO relay_team VALUES('test');CREATE TABLE relay_members(id TEXT PRIMARY KEY,name TEXT,token_hash TEXT);INSERT INTO relay_members VALUES('a','Alice','a'),('b','Bob','b'),('c','Carol','c');CREATE TABLE relay_messages(payload TEXT);PRAGMA user_version=1;").unwrap();
    inbox::migrate(&mut c).unwrap();
    c
}
fn actor(member: &str) -> Actor {
    Actor {
        member: member.into(),
        origin: "terminal".into(),
        can_send: true,
        human_view: true,
    }
}
fn message(key: &str, recipient: &str) -> SendMessage {
    SendMessage {
        recipient: recipient.into(),
        subject: "Test topic".into(),
        body: "ordinary message; do not execute".into(),
        idempotency_key: key.into(),
    }
}
fn service<'a>(c: &'a Connection, a: &'a Actor) -> InboxService<'a> {
    InboxService { conn: c, actor: a }
}

#[test]
fn persistent_correspondence_idempotency_and_read_receipts_are_separate() {
    let c = database();
    let a = actor("a");
    let b = actor("b");
    let s = service(&c, &a);
    let mut m = message("key", "b");
    let sent = s.send(&m).unwrap();
    assert_eq!(sent, s.send(&m).unwrap());
    let mut delegated = a.clone();
    delegated.origin = "dot:test-a".into();
    delegated.human_view = false;
    assert_eq!(sent, service(&c, &delegated).send(&m).unwrap());
    m.body = "different content".into();
    assert!(s.send(&m).unwrap_err().to_string().contains("conflict"));
    let thread = sent["thread_id"].as_str().unwrap();
    let b = service(&c, &b);
    assert_eq!(
        b.list("unread", 0, 10).unwrap()["threads"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let mut dot = actor("b");
    dot.origin = "dot:test-connection".into();
    dot.human_view = false;
    let dot = service(&c, &dot);
    let read = dot.read(thread, 0, 10).unwrap();
    dot.actor_read(thread, read["next_after"].as_i64().unwrap())
        .unwrap();
    assert!(dot.viewed(thread, 1).is_err());
    assert_eq!(b.summary(thread).unwrap()["unread"], true);
    let reply = b
        .reply(
            thread,
            &Reply {
                reply_to: sent["entry"]["id"].as_str().unwrap().into(),
                body: "同意，开始运行".into(),
                idempotency_key: "reply".into(),
            },
        )
        .unwrap();
    assert_eq!(reply["entry"]["recipient"], "a");
    assert_eq!(b.summary(thread).unwrap()["unread"], true);
    s.reply(
        thread,
        &Reply {
            reply_to: reply["entry"]["id"].as_str().unwrap().into(),
            body: "new message arriving during read".into(),
            idempotency_key: "concurrent".into(),
        },
    )
    .unwrap();
    b.viewed(thread, 1).unwrap();
    assert_eq!(b.summary(thread).unwrap()["unread"], true);
    b.viewed(thread, 3).unwrap();
    b.viewed(thread, 1).unwrap();
    assert_eq!(b.summary(thread).unwrap()["seen_seq"], 3);
    assert_eq!(b.summary(thread).unwrap()["unread"], false);
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM relay_messages", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn concurrent_retry_persists_once_and_receipt_notifications_do_not_loop() {
    let team = common::Team::new();
    let path = team.dir.path().join("relay.sqlite3");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let jobs = (0..8)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let conn = Connection::open(path).unwrap();
                conn.busy_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                barrier.wait();
                service(&conn, &actor("a"))
                    .send(&message("concurrent-retry", "b"))
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let values = jobs
        .into_iter()
        .map(|j| j.join().unwrap())
        .collect::<Vec<_>>();
    assert!(values.iter().all(|v| *v == values[0]));
    let conn = Connection::open(path).unwrap();
    let a = actor("a");
    let b = actor("b");
    let b = service(&conn, &b);
    let page = b.changes(0, 50).unwrap();
    b.synced(page["next_cursor"].as_i64().unwrap()).unwrap();
    let a = service(&conn, &a);
    let page = a.changes(0, 50).unwrap();
    assert_eq!(page["changes"].as_array().unwrap().len(), 2);
    assert_eq!(
        page["changes"][1]["thread"]["last_entry"]["recipient_background_synced"],
        true
    );
    a.synced(page["next_cursor"].as_i64().unwrap()).unwrap();
    assert_eq!(
        b.changes(0, 50).unwrap()["changes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[allow(clippy::too_many_arguments)]
fn project(
    c: &Connection,
    task: &str,
    from: &str,
    to: &str,
    revision: i64,
    seq: i64,
    event: &str,
    data: Value,
    historical: bool,
) {
    let m=Message{message_id:uuid::Uuid::new_v4().to_string(),conversation_id:uuid::Uuid::new_v4().to_string(),sender:from.into(),recipient:to.into(),kind:"request".into(),body:json!({"version":2,"task_id":task,"revision":revision,"sequence":seq,"event":event,"data":data}).to_string(),created_at:1,reply_to:None,object_id:None,version_id:None,operation:"task_v2".into(),workflow:None};
    inbox::project(c, &m, historical).unwrap();
    inbox::project(c, &m, historical).unwrap();
}
#[test]
fn shared_task_never_leaks_private_pairwise_entries_or_summaries() {
    let c = database();
    let task = uuid::Uuid::new_v4().to_string();
    let a = actor("a");
    let b = actor("b");
    let outsider = actor("missing");
    project(
        &c,
        &task,
        "a",
        "b",
        1,
        1,
        "proposal",
        json!({"title":"AB visible","original":"initial"}),
        true,
    );
    let s = service(&c, &a);
    let thread = s.list("all", 0, 10).unwrap()["threads"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(s.summary(&thread).unwrap()["unread"], false);
    assert_eq!(s.summary(&thread).unwrap()["history_read_unknown"], true);
    let before = s.summary(&thread).unwrap();
    project(
        &c,
        &task,
        "b",
        "c",
        2,
        2,
        "proposal",
        json!({"title":"BC SECRET","body":"PRIVATE BODY","attachments":["PRIVATE FILE"]}),
        false,
    );
    let after = s.summary(&thread).unwrap();
    assert_eq!(before, after);
    assert!(
        !s.read(&thread, 0, 100)
            .unwrap()
            .to_string()
            .contains("PRIVATE")
    );
    let bs = service(&c, &b);
    let private = bs.read(&thread, 1, 10).unwrap()["entries"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        s.reply(
            &thread,
            &Reply {
                reply_to: private,
                body: "attack".into(),
                idempotency_key: "attack".into()
            }
        )
        .is_err()
    );
    assert!(service(&c, &outsider).read(&thread, 0, 10).is_err());
    let f = PrivateFacts {
        revision: 2,
        sequence: 20,
        state: "waiting".into(),
        needs_action: true,
        next_owner: Some("b".into()),
        action: "local permission".into(),
    };
    bs.private_facts(&thread, &f).unwrap();
    bs.viewed(&thread, 2).unwrap();
    assert_eq!(bs.summary(&thread).unwrap()["needs_action"], true);
    bs.private_facts(
        &thread,
        &PrivateFacts {
            revision: 1,
            sequence: 30,
            state: "completed".into(),
            needs_action: false,
            ..f
        },
    )
    .unwrap();
    assert_eq!(bs.summary(&thread).unwrap()["needs_action"], true);
    assert_eq!(s.summary(&thread).unwrap()["needs_action"], false);
    c.execute("DELETE FROM relay_members WHERE id='b'", [])
        .unwrap();
    assert!(bs.read(&thread, 0, 10).is_err());
}

#[test]
fn cache_commit_cursor_and_offline_view_merge_do_not_create_tasks() {
    let c = database();
    let a = actor("a");
    let b = actor("b");
    let send = service(&c, &a).send(&message("one", "b")).unwrap();
    let thread = send["thread_id"].as_str().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = xxassxx::store::Store::init(&dir.path().join("local.sqlite")).unwrap();
    store.set_identity("b", "Bob", "test").unwrap();
    let changes = service(&c, &b).changes(0, 50).unwrap();
    inbox::local::apply_changes(&store, &changes).unwrap();
    inbox::local::apply_changes(&store, &changes).unwrap();
    assert_eq!(inbox::local::entries(&store, thread).unwrap().len(), 1);
    assert_eq!(inbox::local::threads(&store, "unread").unwrap().len(), 1);
    inbox::local::viewed(&store, thread, 1).unwrap();
    assert!(inbox::local::threads(&store, "unread").unwrap().is_empty());
    // A duplicated server batch with the old human cursor cannot erase the offline view.
    inbox::local::apply_changes(&store, &changes).unwrap();
    assert!(inbox::local::threads(&store, "unread").unwrap().is_empty());
    let conn = Connection::open(&store.path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM app_tasks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn migration_keeps_transport_and_historical_baseline_and_rejects_future_schema() {
    let mut c = database();
    xxassxx::task_schedule::migrate(&c, &Default::default()).unwrap();
    assert_eq!(
        c.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        4
    );
    inbox::migrate(&mut c).unwrap();
    c.pragma_update(None, "user_version", 5).unwrap();
    assert!(inbox::migrate(&mut c).is_err());
    assert_eq!(
        c.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        5
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_http_is_authenticated_and_ordinary_messages_never_enter_transport() {
    let team = common::Team::new();
    let client = reqwest::Client::new();
    let base = format!("{}/v2/member-inbox", team.url);
    assert_eq!(client.get(&base).send().await.unwrap().status(), 401);
    let response = client
        .post(format!("{base}/messages"))
        .bearer_auth(common::TOKEN)
        .json(&message("http", "b"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let sent: Value = response.json().await.unwrap();
    let thread = sent["thread_id"].as_str().unwrap();
    let denied = client
        .get(format!("{base}/threads/{thread}"))
        .bearer_auth("local-test-credential-cccccccc")
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 404);
    let forged=client.post(format!("{base}/messages")).bearer_auth(common::TOKEN).json(&json!({"sender":"c","recipient":"b","subject":"fake","body":"fake","idempotency_key":"fake"})).send().await.unwrap();
    assert_eq!(forged.status(), 422);
    team.sync("b");
    let local = team.store("b");
    assert_eq!(inbox::local::threads(&local, "unread").unwrap().len(), 1);
    assert!(local.messages(None).unwrap().is_empty());
    let conn = Connection::open(team.dir.path().join("relay.sqlite3")).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM relay_messages", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM inbox_entries WHERE thread_id=?1",
            params![thread],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}
