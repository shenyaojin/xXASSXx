mod common;
use common::Team;
use serde_json::Value;
use xxassxx::{
    team::stable_id,
    transfers::{self, Prepare},
};
fn file(t: &Team, member: &str, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let p = t.dir.path().join(member).join("work").join(name);
    std::fs::write(&p, bytes).unwrap();
    p.canonicalize().unwrap()
}
fn sync(t: &Team, member: &str) {
    t.cli(member, &["files", "sync"]);
}
fn send(t: &Team, path: &std::path::Path) -> String {
    let v = t.cli("a", &["files", "send", "--to", "b", path.to_str().unwrap()]);
    v["id"].as_str().unwrap().into()
}
fn state(t: &Team, member: &str, id: &str) -> Value {
    t.cli(member, &["files", "show", id])
}
#[test]
fn fixed_binary_offline_receipt_and_no_overwrite() {
    let t = Team::new();
    sync(&t, "b");
    let data = b"\0\xffbinary\n";
    let source = file(&t, "a", "图像.bin", data);
    let id = send(&t, &source);
    std::fs::write(&source, b"changed").unwrap();
    sync(&t, "a");
    assert_eq!(state(&t, "a", &id)["state"], "available");
    assert!(t.store("b").path.exists());
    sync(&t, "b");
    sync(&t, "a");
    let received = state(&t, "b", &id);
    assert_eq!(received["state"], "received");
    assert_eq!(state(&t, "a", &id)["state"], "delivered");
    let fid = received["manifest"]["files"][0]["id"].as_str().unwrap();
    let dest = t.dir.path().join("saved.bin");
    t.cli("b", &["files", "save", &id, fid, dest.to_str().unwrap()]);
    assert_eq!(std::fs::read(&dest).unwrap(), data);
    assert!(
        !t.command("b", &["files", "save", &id, fid, dest.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        t.cli("c", &["files", "list"])
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn resume_after_mailbox_restart_and_lost_receipt() {
    let mut t = Team::new();
    sync(&t, "b");
    let bytes = vec![37; transfers::CHUNK as usize * 5 + 7];
    let p = file(&t, "a", "result.bin", &bytes);
    let id = send(&t, &p);
    sync(&t, "a");
    assert_eq!(state(&t, "a", &id)["state"], "uploading");
    t.restart();
    sync(&t, "a");
    sync(&t, "b");
    assert_eq!(state(&t, "b", &id)["state"], "offered");
    t.cli("b", &["files", "receive", &id]);
    sync(&t, "b");
    assert_eq!(state(&t, "b", &id)["state"], "downloading");
    t.restart();
    sync(&t, "b");
    sync(&t, "a");
    assert_eq!(state(&t, "a", &id)["state"], "delivered");
    let r = state(&t, "b", &id);
    let local = r["local_files"][0]["path"].as_str().unwrap();
    assert_eq!(std::fs::read(local).unwrap(), bytes);
    sync(&t, "b");
    assert_eq!(t.cli("b", &["files", "list"]).as_array().unwrap().len(), 1);
}
#[test]
fn prepared_selection_requires_owner_and_freezes_exact_bytes() {
    let t = Team::new();
    let p = file(&t, "a", "data.csv", b"x,y\n1,2\n");
    let s = t.store("a");
    s.allow_directory(p.parent().unwrap()).unwrap();
    let id = stable_id("test", "pending");
    let v = transfers::prepare(
        &s,
        Prepare {
            id: id.clone(),
            recipient: "b".into(),
            note: "for analysis".into(),
            paths: vec![p],
            session: None,
            task: None,
            replaces: None,
        },
        false,
    )
    .unwrap();
    assert_eq!(v["state"], "draft");
    sync(&t, "b");
    sync(&t, "a");
    sync(&t, "b");
    assert!(
        t.cli("b", &["files", "list"])
            .as_array()
            .unwrap()
            .is_empty()
    );
    t.cli("a", &["files", "allow", &id]);
    sync(&t, "a");
    sync(&t, "b");
    assert_eq!(state(&t, "b", &id)["state"], "received");
}
#[test]
fn revoke_denies_future_download_and_keeps_history() {
    let t = Team::new();
    sync(&t, "b");
    t.cli("b", &["files", "preferences", "--auto-receive", "0"]);
    let id = send(&t, &file(&t, "a", "private.dat", b"payload"));
    sync(&t, "a");
    sync(&t, "b");
    assert_eq!(state(&t, "b", &id)["state"], "offered");
    t.cli("a", &["files", "revoke", &id]);
    assert_eq!(state(&t, "a", &id)["state"], "revoking");
    sync(&t, "a");
    sync(&t, "b");
    assert_eq!(state(&t, "b", &id)["state"], "revoked");
    assert!(
        !t.command("b", &["files", "receive", &id])
            .status()
            .unwrap()
            .success()
    );
}
#[test]
fn rejects_links_private_state_and_oversized_selection() {
    let t = Team::new();
    let p = file(&t, "a", "data", b"safe");
    let link = p.parent().unwrap().join("link");
    std::os::unix::fs::symlink(&p, &link).unwrap();
    assert!(
        !t.command("a", &["files", "send", "--to", "b", link.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    let db = t.db("a").canonicalize().unwrap();
    assert!(
        !t.command("a", &["files", "send", "--to", "b", db.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    t.cli(
        "a",
        &[
            "files",
            "preferences",
            "--max-file",
            "2",
            "--max-batch",
            "2",
            "--auto-receive",
            "0",
        ],
    );
    assert!(
        !t.command("a", &["files", "send", "--to", "b", p.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn relay_checks_owner_recipient_chunk_integrity_and_idempotency() {
    let t = Team::new();
    sync(&t, "b");
    let id = send(&t, &file(&t, "a", "evidence.bin", b"original"));
    let local = state(&t, "a", &id);
    let m = &local["manifest"];
    let fid = m["files"][0]["id"].as_str().unwrap();
    let c = reqwest::Client::new();
    let endpoint = format!("{}/v1/files", t.url);
    assert_eq!(
        c.post(&endpoint).json(m).send().await.unwrap().status(),
        401
    );
    assert!(
        c.post(&endpoint)
            .bearer_auth(common::TOKEN)
            .json(m)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let chunk = format!("{}/{}/{}/chunks/0", endpoint, id, fid);
    assert_eq!(
        c.get(format!("{endpoint}/{id}"))
            .bearer_auth("local-test-credential-cccccccc")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        c.get(&chunk)
            .bearer_auth("local-test-credential-cccccccc")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        c.put(&chunk)
            .bearer_auth(common::TOKEN)
            .header("x-content-sha256", transfers::digest(b"original"))
            .body("corrupt!")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    for _ in 0..2 {
        assert!(
            c.put(&chunk)
                .bearer_auth(common::TOKEN)
                .header("x-content-sha256", transfers::digest(b"original"))
                .body("original")
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
    }
    assert_eq!(
        c.put(&chunk)
            .bearer_auth(common::TOKEN)
            .header("x-content-sha256", transfers::digest(b"different"))
            .body("different")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        c.put(&chunk)
            .bearer_auth(common::TOKEN)
            .header("x-content-sha256", transfers::digest(b"changed!"))
            .body("changed!")
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert!(
        c.post(format!("{endpoint}/{id}/complete"))
            .bearer_auth(common::TOKEN)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert_eq!(
        c.get(&chunk)
            .bearer_auth("local-test-credential-cccccccc")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let body = c
        .get(&chunk)
        .bearer_auth("local-test-credential-bbbbbbbb")
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&body[..], b"original");
}

#[test]
fn attachment_analysis_binds_only_selected_fixed_files() {
    use serde_json::json;
    use xxassxx::{app, task_coordinator, task_materials};
    let t = Team::new();
    sync(&t, "b");
    let id = send(&t, &file(&t, "a", "pressure.csv", b"p\n10\n20\n30\n"));
    sync(&t, "a");
    sync(&t, "b");
    let m = state(&t, "b", &id);
    let fid = m["manifest"]["files"][0]["id"].as_str().unwrap().to_owned();
    let s = t.store("b");
    let p = app::project(&s, &t.dir.path().join("b/work")).unwrap();
    let session = app::session(&s, p["id"].as_str().unwrap(), "b").unwrap();
    let i = app::Instruction {
        request_id: uuid::Uuid::new_v4().to_string(),
        channel: "test".into(),
        session_id: session,
        task_id: None,
        recipient: "b".into(),
        body: "求均值".into(),
        action: "analyze_attachment".into(),
        payload: json!({}),
    };
    let task = transfers::analysis_task(&s, &i, &id, &[fid]).unwrap();
    let tid = task["id"].as_str().unwrap();
    task_materials::bind(&s, &task).unwrap();
    let (dir, roots) =
        task_materials::check_binding(&rusqlite::Connection::open(&s.path).unwrap(), tid, 1)
            .unwrap();
    assert_eq!(roots, vec![dir]);
    assert!(s.file_roots().unwrap().is_empty());
    assert_eq!(task_coordinator::get(&s, tid).unwrap()["state"], "draft");
}

#[test]
fn delivery_requires_matching_candidate_sender_revision_and_receipt() {
    use serde_json::json;
    let t = Team::new();
    sync(&t, "b");
    let id = send(&t, &file(&t, "a", "result.csv", b"v\n4\n"));
    sync(&t, "a");
    sync(&t, "b");
    let s = t.store("b");
    let mut m = transfers::manifest(&s, &id).unwrap();
    let tid = uuid::Uuid::new_v4().to_string();
    m.task = Some(transfers::TaskRef {
        id: tid.clone(),
        revision: 1,
    });
    rusqlite::Connection::open(&s.path)
        .unwrap()
        .execute(
            "UPDATE file_transfers SET manifest=?2 WHERE id=?1",
            rusqlite::params![id, serde_json::to_string(&m).unwrap()],
        )
        .unwrap();
    let task = json!({"id":tid,"revision":1,"initiator":"b","participants":["a","b"],"draft":{"mode":"execute","deliver_files":true}});
    let mut candidates =
        json!([{"member":"a","result":{"attachment":{"transfer_id":id,"manifest":m}}}]);
    assert!(transfers::delivery_ready(&s, &task, &candidates).unwrap());
    rusqlite::Connection::open(&s.path)
        .unwrap()
        .execute(
            "UPDATE file_transfers SET state='downloading' WHERE id=?1",
            [&id],
        )
        .unwrap();
    assert!(!transfers::delivery_ready(&s, &task, &candidates).unwrap());
    rusqlite::Connection::open(&s.path)
        .unwrap()
        .execute(
            "UPDATE file_transfers SET state='received' WHERE id=?1",
            [&id],
        )
        .unwrap();
    candidates[0]["member"] = json!("c");
    assert!(!transfers::delivery_ready(&s, &task, &candidates).unwrap());
    candidates[0]["member"] = json!("a");
    candidates[0]["result"]["attachment"]["manifest"]["task"]["revision"] = json!(2);
    assert!(!transfers::delivery_ready(&s, &task, &candidates).unwrap());
}

#[test]
fn expiration_denies_download_without_waiting_for_cleanup() {
    let t = Team::new();
    sync(&t, "b");
    let id = send(&t, &file(&t, "a", "expired.txt", b"expires"));
    sync(&t, "a");
    let c = rusqlite::Connection::open(t.dir.path().join("relay.sqlite3")).unwrap();
    c.execute(
        "UPDATE relay_file_transfers SET expires_at=0 WHERE id=?1",
        [&id],
    )
    .unwrap();
    sync(&t, "b");
    assert_eq!(state(&t, "b", &id)["state"], "expired");
    assert!(
        state(&t, "b", &id)["local_files"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    sync(&t, "a");
    assert_eq!(state(&t, "a", &id)["state"], "expired");
}

#[tokio::test]
async fn inbox_paginates_and_completed_history_does_not_hide_pending_files() {
    use serde_json::json;
    let t = Team::new();
    sync(&t, "b");
    let id = send(&t, &file(&t, "a", "old-pending.bin", b"keep"));
    let s = t.store("a");
    let m = transfers::manifest(&s, &id).unwrap();
    for _ in 0..201 {
        let mut history = m.clone();
        history.id = uuid::Uuid::new_v4().to_string();
        rusqlite::Connection::open(&s.path).unwrap().execute("INSERT INTO file_transfers(id,direction,manifest,state,created_at,updated_at) VALUES(?1,'out',?2,'delivered',1,1)",rusqlite::params![history.id,json!(history).to_string()]).unwrap();
    }
    sync(&t, "a");
    sync(&t, "b");
    assert_eq!(state(&t, "b", &id)["state"], "received");
    let relay = rusqlite::Connection::open(t.dir.path().join("relay.sqlite3")).unwrap();
    for _ in 0..50 {
        let mut history = m.clone();
        history.id = uuid::Uuid::new_v4().to_string();
        relay.execute("INSERT INTO relay_file_transfers(id,sender,recipient,manifest,state,bytes,expires_at,purge_at,created_at) VALUES(?1,'a','b',?2,'revoked',4,0,0,0)",rusqlite::params![history.id,json!(history).to_string()]).unwrap();
    }
    let c = reqwest::Client::new();
    let mut cursor = 0;
    let mut count = 0;
    loop {
        let page: Value = c
            .get(format!("{}/v1/files?after={cursor}", t.url))
            .bearer_auth("local-test-credential-bbbbbbbb")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        count += page["items"].as_array().unwrap().len();
        if let Some(next) = page["next"].as_i64() {
            assert!(next > cursor);
            cursor = next;
        } else {
            break;
        }
    }
    assert_eq!(count, 51);
}
