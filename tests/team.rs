mod common;
use common::Team;
use xxassxx::{knowledge::Content, team::Message};

#[test]
fn independent_databases_real_http_offline_restart_reply_followup_and_third_member() {
    let mut t = Team::new();
    let mut b = t.store("b");
    let object = b.create_object("Dataset", "dataset", true).unwrap();
    let v1 = b
        .publish(&object.id, None, "v1", "", Content::Text("one"))
        .unwrap();
    let v2 = b
        .publish(&object.id, Some(&v1.id), "v2", "", Content::Text("two"))
        .unwrap();
    let request = t.cli(
        "a",
        &[
            "message",
            "send",
            "--to",
            "b",
            "--body",
            "Which dataset version?",
            "--operation",
            "metadata",
            "--object-id",
            &object.id,
        ],
    );
    let id = request["message"]["message_id"].as_str().unwrap();
    let conv = request["message"]["conversation_id"].as_str().unwrap();
    assert!(t.store("b").messages(None).unwrap().is_empty());
    t.sync("a");
    assert!(t.store("b").messages(None).unwrap().is_empty());
    t.restart();
    t.sync("c");
    assert!(t.store("c").messages(None).unwrap().is_empty());
    t.sync("b");
    t.sync("b");
    assert_eq!(t.store("b").messages(None).unwrap().len(), 1);
    assert_eq!(b.pin_message_version(id).unwrap(), Some(v2.id.clone()));
    b.publish(&object.id, Some(&v2.id), "v3", "", Content::Text("three"))
        .unwrap();
    assert_eq!(b.pin_message_version(id).unwrap(), Some(v2.id.clone()));
    let reply = t.cli(
        "b",
        &[
            "message",
            "reply",
            id,
            "--body",
            "The resolved version is V2.",
        ],
    );
    assert_eq!(reply["message"]["version_id"], v2.id);
    t.sync("b");
    t.sync("a");
    let reply_id = reply["message"]["message_id"].as_str().unwrap();
    assert_eq!(t.store("a").messages(Some(conv)).unwrap().len(), 2);
    t.cli(
        "a",
        &[
            "message",
            "send",
            "--to",
            "b",
            "--body",
            "Please explain that version",
            "--reply-to",
            reply_id,
            "--object-id",
            &object.id,
            "--version-id",
            &v2.id,
        ],
    );
    t.sync("a");
    t.sync("b");
    assert_eq!(t.store("b").messages(Some(conv)).unwrap().len(), 3);
    assert_ne!(t.db("a"), t.db("b"));
    assert!(t.store("a").object(&object.id).is_err());
}

#[tokio::test]
async fn relay_auth_dedup_and_lost_ack() {
    let t = Team::new();
    let request = t.cli("a", &["message", "send", "--to", "b", "--body", "Hello"]);
    let msg: Message = serde_json::from_value(request["message"].clone()).unwrap();
    let c = reqwest::Client::new();
    let url = format!("{}/v1/messages", t.url);
    assert_eq!(c.post(&url).json(&msg).send().await.unwrap().status(), 401);
    for _ in 0..2 {
        assert!(
            c.post(&url)
                .bearer_auth(common::TOKEN)
                .json(&msg)
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
    }
    let mut changed = msg.clone();
    changed.body = "different".into();
    assert_eq!(
        c.post(&url)
            .bearer_auth(common::TOKEN)
            .json(&changed)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    changed.sender = "b".into();
    assert_eq!(
        c.post(&url)
            .bearer_auth(common::TOKEN)
            .json(&changed)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        c.post(format!("{}/v1/ack/{}", t.url, msg.message_id))
            .bearer_auth("local-test-credential-cccccccc")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    // Emulate a crash after committing the local receipt, before relay ack.
    t.store("b").save_message(&msg, true).unwrap();
    t.sync("b");
    t.sync("b");
    assert_eq!(t.store("b").messages(None).unwrap().len(), 1);
    assert!(t.store("c").messages(None).unwrap().is_empty());
}

#[test]
fn model_off_and_closed_conversation_keep_original_messages() {
    let t = Team::new();
    let request = t.cli("a", &["message", "send", "--to", "b", "--body", "original"]);
    let id = request["message"]["message_id"].as_str().unwrap();
    let conv = request["message"]["conversation_id"].as_str().unwrap();
    t.sync("a");
    t.sync("b");
    t.cli("b", &["conversation", "close", conv]);
    let out = t
        .command("b", &["message", "reply", id, "--body", "reply"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(t.store("b").message(id).unwrap().message.body, "original");
    assert_eq!(
        t.store("b").member_config().unwrap().model["provider"],
        "off"
    );
}

#[test]
fn prose_slash_survives_delivery_while_local_paths_are_rejected() {
    let t = Team::new();
    let request = t.cli(
        "a",
        &[
            "message",
            "send",
            "--to",
            "b",
            "--body",
            "请查询 main / 当前版本",
        ],
    );
    let id = request["message"]["message_id"].as_str().unwrap();
    t.sync("a");
    t.sync("b");
    let body = "这是 main / 当前解析版本；仅返回对象元数据。";
    t.cli("b", &["message", "reply", id, "--body", body]);
    t.sync("b");
    t.sync("a");
    let messages = t.store("a").messages(None).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].message.body, body);
    for body in [
        "请读取 /Users/example/data.csv",
        "读取 C:\\data\\file.csv",
        "file:///Users/example/data.csv",
    ] {
        let out = t
            .command("a", &["message", "send", "--to", "b", "--body", body])
            .output()
            .unwrap();
        assert!(!out.status.success());
    }
    assert_eq!(t.store("a").messages(None).unwrap().len(), 2);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn butler_sigint_during_http_tick_is_not_lost() {
    use axum::{Json, Router, routing::get};
    use serde_json::json;
    use std::{
        process::Stdio,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    // Block whoami during the second tick (after the first Ctrl-C listener
    // existed), then during the very first tick. Neither signal may be lost.
    for blocked_request in [2, 0] {
        let t = Team::new();
        let arrived = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let count = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route(
                "/v1/whoami",
                get({
                    let arrived = arrived.clone();
                    let release = release.clone();
                    move || {
                        let arrived = arrived.clone();
                        let release = release.clone();
                        let count = count.clone();
                        async move {
                            if count.fetch_add(1, Ordering::SeqCst) == blocked_request {
                                arrived.notify_one();
                                release.notified().await;
                            }
                            Json(json!({"team_id":"lab","member_id":"a"}))
                        }
                    }
                }),
            )
            .route(
                "/v1/inbox",
                get(|| async { Json(json!({"team_id":"lab","messages":[]})) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut store = t.store("a");
        let mut config = store.member_config().unwrap();
        config.mailbox_url = format!("http://{}", listener.local_addr().unwrap());
        store.configure_member(&config).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut child =
            tokio::process::Command::from(t.command("a", &["butler", "run", "--poll-secs", "1"]))
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
        tokio::time::timeout(Duration::from_secs(10), arrived.notified())
            .await
            .unwrap();
        assert_eq!(
            unsafe { libc::kill(child.id().unwrap() as i32, libc::SIGINT) },
            0
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        release.notify_one();
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        if status.is_err() {
            child.kill().await.unwrap();
            child.wait().await.unwrap();
        }
        server.abort();
        assert!(
            status
                .expect("SIGINT was lost during HTTP synchronization")
                .unwrap()
                .success()
        );
    }
}
