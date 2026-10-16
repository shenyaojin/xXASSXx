mod common;
use common::{BIN, Team};
use rusqlite::{Connection, params};
use serde_json::json;
use std::path::Path;
use xxassxx::{
    app::{self, Actor, Instruction},
    store::Store,
    task_coordinator as tasks, task_roles as roles,
    team::Message,
};

fn create(team: &Team, executor: &str) -> (Instruction, String) {
    let mut s = team.store("a");
    let p = app::project(&s, &team.dir.path().join("a/work")).unwrap();
    let i = Instruction {
        request_id: uuid::Uuid::new_v4().to_string(),
        channel: "test-user-action".into(),
        session_id: app::session(&s, p["id"].as_str().unwrap(), "a").unwrap(),
        task_id: None,
        recipient: "a".into(),
        body: "核对合成文件并交付结果".into(),
        action: "create_task".into(),
        payload: json!({"peer":executor}),
    };
    let actor = Actor::local(&s).unwrap();
    let v = app::submit(&mut s, &actor, &i).unwrap();
    let id = v["task_id"].as_str().unwrap().to_owned();
    prepare(&s, &id);
    (i, id)
}
fn prepare(s: &Store, id: &str) {
    tasks::prepare(s,id,json!({"goal":"核对合成文件","deliverables":["结果"],"mode":"execute","deliver_files":true,"questions":[]})).unwrap();
}
fn command(i: &Instruction, id: &str, rev: i64, body: &str) -> Instruction {
    let mut i = i.clone();
    i.request_id = uuid::Uuid::new_v4().to_string();
    i.task_id = Some(id.into());
    i.payload = json!({"revision":rev});
    i.body = body.into();
    i
}
fn messages(s: &Store, event: &str) -> Vec<Message> {
    s.messages(None)
        .unwrap()
        .into_iter()
        .filter(|r| {
            r.direction == "out"
                && r.message.operation == "task_v2"
                && tasks::Wire::parse(&r.message).unwrap().event == event
        })
        .map(|r| r.message)
        .collect()
}
fn receive(s: &mut Store, m: &Message) {
    s.save_message(m, true).unwrap();
    tasks::ingest(s, m).unwrap();
}
fn count(s: &Store, sql: &str) -> i64 {
    Connection::open(&s.path)
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

#[tokio::test]
async fn replacement_waits_for_old_member_and_tombstone_defeats_reordered_proposal() {
    let team = Team::new();
    let (i, id) = create(&team, "b");
    let mut a = team.store("a");
    tasks::confirm(&a, &command(&i, &id, 1, "确认")).unwrap();
    let old = messages(&a, "proposal").pop().unwrap();
    tasks::supplement(
        &a,
        &command(&i, &id, 1, "所有长度单位为米，这是换人后仍有效的事实"),
    )
    .unwrap();
    let mut change = command(&i, &id, 1, "执行 @c 原成员尚未开始，替换执行分配");
    change.payload["executor"] = json!("c");
    assert!(roles::change(&a, &change).unwrap());
    tasks::prepare_model(&a, &id).await.unwrap();
    assert_eq!(
        tasks::get(&a, &id).unwrap()["draft"]["deliverables"],
        json!(["结果"])
    );
    tasks::confirm(&a, &command(&i, &id, 2, "确认新安排")).unwrap();
    assert_eq!(
        messages(&a, "proposal").len(),
        1,
        "offline old member must block new dispatch"
    );
    assert_eq!(tasks::get(&a, &id).unwrap()["roles"]["executor"], "c");
    let stop = messages(&a, "stop_assignment").pop().unwrap();
    let mut b = team.store("b");
    receive(&mut b, &stop);
    receive(&mut b, &stop);
    receive(&mut b, &old);
    assert_eq!(count(&b, "SELECT count(*) FROM task_executions"), 0);
    assert_eq!(
        count(&b, "SELECT count(*) FROM app_tasks"),
        0,
        "late proposal is history only"
    );
    assert_eq!(messages(&b, "assignment_stopped").len(), 1);
    let ack = messages(&b, "assignment_stopped").pop().unwrap();
    receive(&mut a, &ack);
    receive(&mut a, &ack);
    tasks::tick(&a, Path::new(BIN)).await.unwrap();
    let new = messages(&a, "proposal")
        .into_iter()
        .find(|m| m.recipient == "c")
        .unwrap();
    let mut c = team.store("c");
    receive(&mut c, &new);
    receive(&mut c, &new);
    tasks::tick(&c, Path::new(BIN)).await.unwrap();
    assert_eq!(
        count(&c, "SELECT count(*) FROM runs"),
        0,
        "new executor has its own approval boundary"
    );
    assert_eq!(
        tasks::get(&c, &id).unwrap()["participants"],
        json!(["a", "c"])
    );
    assert_eq!(
        tasks::get(&c, &id).unwrap()["inputs"][0]["body"],
        "所有长度单位为米，这是换人后仍有效的事实"
    );
    assert_eq!(
        count(&c, "SELECT count(*) FROM task_jobs WHERE kind='start'"),
        1
    );
    assert_eq!(count(&c, "SELECT count(*) FROM task_workspaces"), 0);
    let transition = tasks::get(&c, &id).unwrap()["assignment_change"].clone();
    assert_eq!(transition["previous_executor"], "b");
    assert_eq!(transition["reason"], change.body);
    assert_eq!(transition["stops"][0]["state"], "stopped");
    assert!(
        transition["stops"][0].get("evidence").is_none(),
        "private prior side-effect details are not forwarded"
    );
    assert_eq!(
        messages(&c, "input_receipt").len(),
        1,
        "carried inputs have a new reception receipt"
    );
    let mut stale = old.clone();
    let mut w = tasks::Wire::parse(&stale).unwrap();
    w.event = "candidate".into();
    w.sequence = 20;
    w.data = json!({"body":"旧结果不能完成新安排","sources":[{}]});
    stale.body = serde_json::to_string(&w).unwrap();
    stale.sender = "b".into();
    stale.recipient = "a".into();
    stale.message_id = uuid::Uuid::new_v4().to_string();
    a.save_message(&stale, true).unwrap();
    let _ = tasks::ingest(&a, &stale);
    assert_ne!(tasks::get(&a, &id).unwrap()["state"], "completed");
    assert_eq!(count(&a, "SELECT count(*) FROM task_candidates"), 0);
}

#[test]
fn delivery_recipient_is_not_executor_or_participant_and_old_grant_is_not_reused() {
    let team = Team::new();
    let (i, id) = create(&team, "a");
    let a = team.store("a");
    let mut change = command(&i, &id, 1, "接收 @b 本机处理后只把成果发给 b");
    change.payload["result_recipient"] = json!("b");
    assert!(!roles::change(&a, &change).unwrap());
    let t = tasks::get(&a, &id).unwrap();
    assert_eq!(roles::executors(&t).unwrap(), ["a"]);
    assert_eq!(t["participants"], json!(["a"]));
    let c = Connection::open(&a.path).unwrap();
    roles::approve_delivery(&c, &t).unwrap();
    assert!(roles::delivery_approved(&c, &t).unwrap());
    change.request_id = uuid::Uuid::new_v4().to_string();
    change.payload["result_recipient"] = json!("c");
    change.body = "接收 @c 仅更换成果接收人".into();
    roles::change(&a, &change).unwrap();
    let t = tasks::get(&a, &id).unwrap();
    assert_eq!(t["revision"], 1);
    assert_eq!(t["roles"]["executor"], "a");
    assert!(!roles::delivery_approved(&c, &t).unwrap());
    tasks::confirm(&a, &command(&i, &id, 1, "确认")).unwrap();
    assert_eq!(
        messages(&a, "proposal").len(),
        0,
        "recipient receives no private task contract"
    );
    assert_eq!(
        count(&a, "SELECT count(*) FROM task_jobs WHERE kind='start'"),
        1
    );
    assert_eq!(count(&team.store("c"), "SELECT count(*) FROM app_tasks"), 0);
}

#[test]
fn text_result_is_a_separate_authorized_delivery_without_private_history() {
    let team = Team::new();
    let (i, id) = create(&team, "a");
    let a = team.store("a");
    tasks::prepare(&a,&id,json!({"goal":"回答合成问题并发给 b","deliverables":["文字答案"],"mode":"analysis","deliver_to":"b","questions":[]})).unwrap();
    tasks::confirm(&a, &command(&i, &id, 1, "确认")).unwrap();
    let t = tasks::get(&a, &id).unwrap();
    let output = json!({"body":"合成结果：83","files":[],"deliveries":[],"run_id":uuid::Uuid::new_v4().to_string()});
    let offered = xxassxx::transfers::task_offer(&a, &t, "test-execution", &output)
        .unwrap()
        .unwrap();
    let same = xxassxx::transfers::task_offer(&a, &t, "test-execution", &output)
        .unwrap()
        .unwrap();
    assert_eq!(offered, same);
    assert_eq!(offered["state"], "draft");
    let candidates = json!([{"member":"a","result":{"attachment":offered}}]);
    assert!(!xxassxx::transfers::delivery_ready(&a, &t, &candidates).unwrap());
    let transfer = xxassxx::transfers::get(&a, offered["transfer_id"].as_str().unwrap()).unwrap();
    let path = transfer["local_files"][0]["path"].as_str().unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), "合成结果：83");
    assert_eq!(transfer["manifest"]["files"][0]["name"], "result.txt");
    assert_eq!(count(&team.store("b"), "SELECT count(*) FROM app_tasks"), 0);
    assert!(messages(&a, "proposal").is_empty());
}

#[test]
fn executor_may_keep_its_own_result_without_fictitious_transport() {
    let team = Team::new();
    let (_, id) = create(&team, "b");
    let a = team.store("a");
    let mut t = tasks::get(&a, &id).unwrap();
    t["roles"]["recipients"] = json!(["b"]);
    assert!(
        xxassxx::transfers::delivery_ready(
            &a,
            &t,
            &json!([{"member":"b","result":{"body":"已核对本机结果"}}])
        )
        .unwrap()
    );
    assert!(!xxassxx::transfers::delivery_ready(&a, &t, &json!([])).unwrap());
    assert!(!xxassxx::transfers::delivery_ready(&a, &t, &json!([{"member":"c"}])).unwrap());
}

#[test]
fn same_goal_supplement_keeps_assignment_and_approval_and_revisions_replace_requirements() {
    let team = Team::new();
    let (i, id) = create(&team, "b");
    let a = team.store("a");
    tasks::confirm(&a, &command(&i, &id, 1, "确认")).unwrap();
    let t = tasks::get(&a, &id).unwrap();
    let input = command(&i, &id, 1, "原数据三个样本，单位均为米");
    tasks::supplement(&a, &input).unwrap();
    let fresh = tasks::get(&a, &id).unwrap();
    assert_eq!(fresh["roles"], t["roles"]);
    assert_eq!(fresh["revision"], 1);
    assert_eq!(fresh["inputs"][0]["body"], input.body);
    let change = command(&i, &id, 1, "当前完整要求：只交付统计表，取消旧报告");
    tasks::revise(&a, &change).unwrap();
    let fresh = tasks::get(&a, &id).unwrap();
    assert_eq!(fresh["original"], change.body);
    assert_eq!(fresh["revisions"][0]["original"], i.body);
    assert_eq!(fresh["roles"], t["roles"]);
}

#[test]
fn old_client_capability_blocks_new_roles_and_stop_acknowledgement_is_bound() {
    let team = Team::new();
    let (i, id) = create(&team, "b");
    let a = team.store("a");
    tasks::confirm(&a, &command(&i, &id, 1, "确认")).unwrap();
    let m = messages(&a, "proposal").pop().unwrap();
    let c = Connection::open(team.dir.path().join("relay.sqlite3")).unwrap();
    assert!(xxassxx::team_activity::require_online(&c, &m, xxassxx::store::now()).unwrap());
    c.execute(
        "DELETE FROM relay_role_capabilities WHERE member_id='b'",
        [],
    )
    .unwrap();
    assert!(!xxassxx::team_activity::require_online(&c, &m, xxassxx::store::now()).unwrap());
    let mut wrong = m.clone();
    let mut w = tasks::Wire::parse(&wrong).unwrap();
    w.event = "assignment_stopped".into();
    w.data = json!({"roles_protocol":1,"stop_id":xxassxx::team::stable_id(&id,"stop-assignment:1:a:b"),"initiator":"a","executor":"b","evidence":"forged"});
    wrong.sender = "c".into();
    wrong.recipient = "a".into();
    wrong.body = serde_json::to_string(&w).unwrap();
    assert!(roles::ingest_control(&a, &wrong, &w).is_err());
    assert!(roles::relay_control(&c, &wrong, &w).is_err());
}

#[test]
fn schema_thirteen_upgrade_preserves_legacy_multi_executor_contract_and_grants() {
    let team = Team::new();
    let (_i, id) = create(&team, "b");
    let a = team.store("a");
    let c = Connection::open(&a.path).unwrap();
    c.execute(
        "UPDATE app_tasks SET roles=NULL,participants='[\"a\",\"b\",\"c\"]' WHERE id=?1",
        [&id],
    )
    .unwrap();
    c.execute("ALTER TABLE app_tasks DROP COLUMN roles", [])
        .unwrap();
    c.execute("ALTER TABLE task_revisions DROP COLUMN roles", [])
        .unwrap();
    for table in ["task_role_changes", "task_stops", "task_delivery_approvals"] {
        c.execute(&format!("DROP TABLE {table}"), []).unwrap();
    }
    c.pragma_update(None, "user_version", 13).unwrap();
    drop(c);
    drop(a);
    let a = team.store("a");
    let t = tasks::get(&a, &id).unwrap();
    assert!(t["roles"].is_null());
    assert_eq!(roles::executors(&t).unwrap(), ["b", "c"]);
    assert_eq!(count(&a, "SELECT count(*) FROM task_delivery_approvals"), 0);
    assert_eq!(count(&a, "SELECT count(*) FROM task_stops"), 0);
    assert_eq!(
        Connection::open(&a.path)
            .unwrap()
            .query_row(
                "SELECT participants FROM app_tasks WHERE id=?1",
                params![id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "[\"a\",\"b\",\"c\"]"
    );
}

#[test]
fn withdrawal_proves_unsent_assignment_and_other_initiator_cannot_poison_tombstone() {
    let team = Team::new();
    let (i, id) = create(&team, "b");
    let a = team.store("a");
    tasks::confirm(&a, &command(&i, &id, 1, "确认")).unwrap();
    let proposal = messages(&a, "proposal").pop().unwrap();
    let mut change = command(&i, &id, 1, "执行 @c 旧提议还未送达");
    change.payload["executor"] = json!("c");
    roles::change(&a, &change).unwrap();
    let stop = messages(&a, "stop_assignment").pop().unwrap();
    let w = tasks::Wire::parse(&stop).unwrap();
    let relay = Connection::open(team.dir.path().join("relay.sqlite3")).unwrap();
    roles::relay_control(&relay, &stop, &w).unwrap();
    assert!(!xxassxx::team_activity::withdraw(&relay, "a", &proposal).unwrap());
    roles::relay_control(&relay, &stop, &w).unwrap();
    assert_eq!(
        relay
            .query_row("SELECT stopped FROM relay_assignment_stops", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let mut b = team.store("b");
    let mut other = stop.clone();
    let mut foreign = w.clone();
    foreign.data["initiator"] = json!("c");
    foreign.data["stop_id"] = json!(xxassxx::team::stable_id(&id, "stop-assignment:1:c:b"));
    other.sender = "c".into();
    other.conversation_id = xxassxx::team::stable_id(&id, "b:c");
    other.message_id = uuid::Uuid::new_v4().to_string();
    other.body = serde_json::to_string(&foreign).unwrap();
    receive(&mut b, &other);
    assert!(!roles::fenced_for(&Connection::open(&b.path).unwrap(), &id, 1, "a").unwrap());
    receive(&mut b, &proposal);
    assert_eq!(tasks::get(&b, &id).unwrap()["initiator"], "a");
    assert!(!roles::fenced(&Connection::open(&b.path).unwrap(), &id, 1).unwrap());
    receive(&mut b, &stop);
    assert!(roles::fenced(&Connection::open(&b.path).unwrap(), &id, 1).unwrap());
}

#[tokio::test]
async fn task_start_serializes_with_other_local_writers() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let team = Team::new();
    team.store("a")
        .allow_directory(&team.dir.path().join("a/work"))
        .unwrap();
    for _ in 0..16 {
        let (i, id) = create(&team, "a");
        let a = team.store("a");
        tasks::confirm(&a, &command(&i, &id, 1, "确认")).unwrap();
        xxassxx::task_workspace::execute(
            &a,
            xxassxx::task_workspace::Command::Allow {
                task: id,
                revision: 1,
                parent: team.dir.path().join("a/work"),
                read_dirs: vec![],
                timeout_secs: 30,
            },
        )
        .unwrap();
    }
    let done = Arc::new(AtomicBool::new(false));
    let flag = done.clone();
    let path = team.db("a");
    let writer = std::thread::spawn(move || {
        let c = Connection::open(path).unwrap();
        c.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
        while !flag.load(Ordering::SeqCst) {
            c.execute("INSERT INTO app_preferences(key,value) VALUES('concurrent-test','tick') ON CONFLICT(key) DO UPDATE SET value=excluded.value", []).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
    let a = team.store("a");
    let outcome = async {
        for _ in 0..16 {
            tasks::tick(&a, Path::new(BIN)).await?;
        }
        anyhow::Ok(())
    }
    .await;
    done.store(true, Ordering::SeqCst);
    writer.join().unwrap();
    outcome.unwrap();
    assert_eq!(
        count(&a, "SELECT count(*) FROM task_jobs WHERE state='failed'"),
        0
    );
    assert_eq!(count(&a, "SELECT count(*) FROM task_executions"), 16);
}
