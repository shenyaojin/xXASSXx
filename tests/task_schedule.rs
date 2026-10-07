use chrono::TimeZone;
use rusqlite::{Connection, params};
use serde_json::json;
use xxassxx::{
    task_coordinator::Wire,
    task_schedule::{self, Config},
    team::{Message, stable_id},
};
fn at(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp()
}
fn setup() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE relay_members(id TEXT PRIMARY KEY); INSERT INTO relay_members VALUES('a'),('b'),('c');").unwrap();
    task_schedule::migrate(&c, &Config::default()).unwrap();
    c
}
fn message(task: &str, event: &str, rev: i64, seq: i64, data: serde_json::Value) -> Message {
    Message {
        message_id: stable_id(task, &format!("{rev}:{seq}")),
        conversation_id: stable_id(task, "pair"),
        sender: "a".into(),
        recipient: "b".into(),
        kind: "request".into(),
        operation: "task_v2".into(),
        body: serde_json::to_string(&Wire {
            version: 2,
            task_id: task.into(),
            revision: rev,
            sequence: seq,
            event: event.into(),
            data,
        })
        .unwrap(),
        created_at: 0,
        reply_to: None,
        object_id: None,
        version_id: None,
        workflow: None,
    }
}
fn propose(c: &Connection, id: &str, time: i64) {
    task_schedule::accept(c,&message(id,"proposal",1,1,json!({"initiator":"a","participants":["a","b"],"confirmed":1,"draft":{"goal":"测试","deliverables":["脚本"]}})),time).unwrap();
}
fn count(c: &Connection, sql: &str) -> i64 {
    c.query_row(sql, [], |r| r.get(0)).unwrap()
}
#[test]
fn meetings_persist_merge_offline_report_and_stop_without_execution() {
    let c = setup();
    let id = uuid::Uuid::new_v4().to_string();
    let start = at("2026-10-07T13:00:00Z");
    propose(&c, &id, start);
    task_schedule::tick(&c, start + 7199).unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM relay_meetings"), 0);
    task_schedule::tick(&c, start + 7200).unwrap();
    task_schedule::tick(&c, start + 7200).unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM relay_meetings"), 1);
    let invite = task_schedule::pending(&c, "b")
        .unwrap()
        .into_iter()
        .find(|v| v["kind"] == "meeting_invitation")
        .unwrap();
    let report = json!({"meeting_id":invite["payload"]["meeting_id"],"task_id":id,"revision":1,"state":"waiting","blocking":"等待用户回答","next_owner":"a","updated_at":start});
    assert!(task_schedule::report(&c, "c", &report, start + 7202).is_err());
    task_schedule::report(&c, "b", &report, start + 7202).unwrap();
    task_schedule::report(&c, "b", &report, start + 7202).unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM relay_reports"), 1);
    task_schedule::tick(&c, start + 7501).unwrap();
    let summary = task_schedule::pending(&c, "b")
        .unwrap()
        .into_iter()
        .find(|v| v["kind"] == "meeting_summary")
        .unwrap();
    assert_eq!(summary["payload"]["absent"], json!(["a"]));
    task_schedule::report(&c, "a", &report, start + 7600).unwrap();
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM relay_system_events WHERE kind='meeting_supplement'"
        ),
        2
    );
    task_schedule::migrate(&c, &Config::default()).unwrap();
    task_schedule::tick(&c, start + 7200 * 100).unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM relay_meetings"), 2);
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM relay_system_events WHERE kind='meeting_invitation' AND received=0"
        ),
        2
    );
    task_schedule::accept(
        &c,
        &message(&id, "complete", 1, 2, json!({"body":"完成"})),
        start + 7200 * 100 + 1,
    )
    .unwrap();
    task_schedule::tick(&c, start + 7200 * 103).unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM relay_meetings"), 2);
    assert!(task_schedule::pending(&c, "c").unwrap().is_empty());
}
#[test]
fn daily_timezone_dst_and_restart_deduplicate_participant_delivery() {
    let cfg = Config::default();
    let (d, t) = task_schedule::scheduled_cutoff(&cfg, at("2026-03-08T23:59:00Z")).unwrap();
    assert_eq!(d, "2026-03-07");
    assert_eq!(t, at("2026-03-08T01:00:00Z"));
    let (d, t) = task_schedule::scheduled_cutoff(&cfg, at("2026-03-09T00:00:00Z")).unwrap();
    assert_eq!(d, "2026-03-08");
    assert_eq!(t, at("2026-03-09T00:00:00Z"));
    let (_, t) = task_schedule::scheduled_cutoff(&cfg, at("2026-11-02T01:00:00Z")).unwrap();
    assert_eq!(t, at("2026-11-02T01:00:00Z"));
    let gap = Config {
        daily_time: "02:30".into(),
        ..Default::default()
    };
    let (_, t) = task_schedule::scheduled_cutoff(&gap, at("2026-03-08T10:00:00Z")).unwrap();
    assert_eq!(t, at("2026-03-08T09:00:00Z"));
    let overlap = Config {
        daily_time: "01:30".into(),
        ..Default::default()
    };
    let (_, t) = task_schedule::scheduled_cutoff(&overlap, at("2026-11-01T10:00:00Z")).unwrap();
    assert_eq!(t, at("2026-11-01T07:30:00Z"));
    let c = setup();
    let id = uuid::Uuid::new_v4().to_string();
    propose(&c, &id, at("2026-10-07T20:00:00Z"));
    let cutoff = at("2026-10-08T00:00:00Z");
    task_schedule::tick(&c, cutoff).unwrap();
    task_schedule::tick(&c, cutoff + 1).unwrap();
    task_schedule::migrate(&c, &cfg).unwrap();
    task_schedule::tick(&c, cutoff + 2).unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM relay_daily"), 2);
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM relay_system_events WHERE kind='daily_summary'"
        ),
        2
    );
    task_schedule::tick(&c, cutoff + 86400 * 30).unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM relay_daily"), 4);
    assert!(task_schedule::pending(&c, "c").unwrap().is_empty());
    let events = task_schedule::pending(&c, "b").unwrap();
    let report = events
        .iter()
        .rfind(|v| v["kind"] == "daily_summary")
        .unwrap();
    assert_eq!(report["payload"]["from_cutoff"], cutoff);
    assert_eq!(
        report["payload"]["tasks"][0]["updated_at"],
        at("2026-10-07T20:00:00Z")
    );
    let _ = chrono_tz::America::Denver.timestamp_opt(cutoff, 0).unwrap();
}
#[test]
fn authentication_revision_and_system_origin_cannot_be_forged() {
    let c = setup();
    let id = uuid::Uuid::new_v4().to_string();
    propose(&c, &id, 100);
    let mut fake = message(&id, "complete", 1, 9, json!({"body":"fake"}));
    fake.sender = "b".into();
    fake.recipient = "a".into();
    assert!(task_schedule::accept(&c, &fake, 101).is_err());
    let unsupported = message(&id, "system_daily", 1, 10, json!({}));
    assert!(Wire::parse(&unsupported).is_err());
    task_schedule::accept(
        &c,
        &message(
            &id,
            "proposal",
            2,
            1,
            json!({"initiator":"a","participants":["a","b"],"confirmed":2}),
        ),
        110,
    )
    .unwrap();
    task_schedule::accept(
        &c,
        &message(&id, "complete", 1, 99, json!({"body":"old result"})),
        111,
    )
    .unwrap();
    assert_eq!(
        c.query_row(
            "SELECT state FROM relay_tasks WHERE id=?1",
            params![id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "queued"
    );
}
