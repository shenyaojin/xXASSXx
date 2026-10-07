use axum::{Json, Router, http::HeaderMap, routing::post};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use xxassxx::{
    model::{HttpModel, ModelConfig},
    setup,
    store::Store,
};

fn join(root: &Path, invite: &Path) {
    setup::init_client(root, invite, "off", None, None, None, "codex".into(), &[]).unwrap();
}

#[tokio::test]
async fn private_invitations_join_and_authenticate_without_exported_tokens() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("server");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let output = setup::init_server(
        &root,
        "lab",
        &url,
        &["alice=Alice".into(), "bob=Bob".into()],
    )
    .unwrap();
    let cfg = toml::from_str(&std::fs::read_to_string(root.join("server.toml")).unwrap()).unwrap();
    let router = xxassxx::mailbox::router(&root.join("mailbox.sqlite3"), &cfg).unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let a = temp.path().join("alice");
    let b = temp.path().join("bob");
    join(&a, &root.join("invitations/alice.json"));
    join(&b, &root.join("invitations/bob.json"));
    let mut alice = Store::open(&a.join("member.sqlite3")).unwrap();
    let mut bob = Store::open(&b.join("member.sqlite3")).unwrap();
    let a_key = alice.member_config().unwrap().credential().unwrap();
    let b_key = bob.member_config().unwrap().credential().unwrap();
    assert!(!output.to_string().contains(&a_key));
    assert!(!output.to_string().contains(&b_key));
    assert!(a_key != b_key);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [
            a.join("secrets.env"),
            root.join("secrets.env"),
            root.join("invitations/bob.json"),
        ] {
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    let req = alice
        .new_request(
            "bob",
            "Hello from Alice",
            "metadata",
            (None, None),
            None,
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
    xxassxx::team::sync(&mut alice).await.unwrap();
    xxassxx::team::sync(&mut bob).await.unwrap();
    assert_eq!(
        bob.message(&req.message.message_id).unwrap().message.body,
        "Hello from Alice"
    );
    assert!(
        setup::init_server(
            &root,
            "lab",
            &url,
            &["alice=Alice".into(), "bob=Bob".into()]
        )
        .is_err()
    );
    let now = Store::open(&a.join("member.sqlite3")).unwrap();
    assert!(now.member_config().unwrap().credential().unwrap() == a_key);
    let doctor = setup::doctor(&alice, false).await.unwrap();
    assert_eq!(doctor["mailbox"]["reachable_and_identity_matches"], true);
    server.abort();
}

#[test]
fn invalid_server_or_invitation_does_not_create_installation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("server");
    for members in [
        vec!["alice=A".into()],
        vec!["../alice=A".into(), "bob=B".into()],
        vec!["alice=A".into(), "alice=B".into()],
    ] {
        assert!(setup::init_server(&root, "lab", "https://team.example", &members).is_err());
        assert!(!root.exists());
    }
    assert!(
        setup::init_server(
            &root,
            "lab",
            "http://public.example",
            &["a=A".into(), "b=B".into()]
        )
        .is_err()
    );
    let invite = temp.path().join("invite.json");
    std::fs::write(&invite, r#"{"credential":"do-not-print-this"}"#).unwrap();
    let err = setup::init_client(&root, &invite, "off", None, None, None, "codex".into(), &[])
        .unwrap_err();
    assert!(!err.to_string().contains("do-not-print-this"));
    assert!(!root.exists());
}

#[tokio::test]
async fn ollama_defaults_and_tool_roundtrip_use_v1_without_key_or_vendor_fields() {
    let default =
        ModelConfig::parse(&json!({"provider":"ollama","model":"installed-local-model"})).unwrap();
    assert_eq!(default.base_url, "http://127.0.0.1:11434/v1");
    assert!(default.api_key_env.is_empty());
    assert!(ModelConfig::parse(&json!({"provider":"ollama"})).is_err());
    let seen = Arc::new(Mutex::new(0));
    let counter = seen.clone();
    let router=Router::new().route("/v1/chat/completions",post(move |headers:HeaderMap,Json(body):Json<Value>| {
        let counter=counter.clone();
        async move {
            assert!(headers.get("authorization").is_none());
            assert!(body.get("thinking").is_none());
            assert_eq!(body["stream"],false);
            let mut n=counter.lock().unwrap(); *n+=1;
            let message=if *n==1 {
                let prompt=body["messages"][0]["content"].as_str().unwrap();
                let nonce=prompt.split("nonce ").nth(1).unwrap().split('.').next().unwrap();
                json!({"role":"assistant","content":"","tool_calls":[{"id":"probe-1","type":"function","function":{"name":"connection_probe","arguments":json!({"nonce":nonce}).to_string()}}]})
            } else {
                assert_eq!(body["messages"][2]["tool_call_id"],"probe-1");
                let tool:Value=serde_json::from_str(body["messages"][2]["content"].as_str().unwrap()).unwrap();
                json!({"role":"assistant","content":tool["receipt"]})
            };
            Json(json!({"choices":[{"finish_reason":if *n==1 {"tool_calls"} else {"stop"},"message":message}]}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let config = ModelConfig::parse(
        &json!({"provider":"ollama","model":"installed-local-model","base_url":url}),
    )
    .unwrap();
    HttpModel::new(config).unwrap().probe_tools().await.unwrap();
    assert_eq!(*seen.lock().unwrap(), 2);
    server.abort();
}

#[tokio::test]
async fn tool_probe_rejects_chat_only_model() {
    let router=Router::new().route("/v1/chat/completions",post(||async {Json(json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"I can use tools"}}]}))}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let config =
        ModelConfig::parse(&json!({"provider":"ollama","model":"chat-only","base_url":url}))
            .unwrap();
    assert!(
        HttpModel::new(config)
            .unwrap()
            .probe_tools()
            .await
            .unwrap_err()
            .to_string()
            .contains("no_call")
    );
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnect_survives_transport_failure_beyond_failure_budget() {
    let temp = tempfile::tempdir().unwrap();
    let server_root = temp.path().join("server");
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    drop(socket);
    setup::init_server(
        &server_root,
        "lab",
        &format!("http://{address}"),
        &["a=A".into(), "b=B".into()],
    )
    .unwrap();
    let root = temp.path().join("a");
    join(&root, &server_root.join("invitations/a.json"));
    let db = root.join("member.sqlite3");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_xxassxx"))
        .arg("--db")
        .arg(&db)
        .args([
            "butler",
            "run",
            "--poll-secs",
            "1",
            "--max-failures",
            "1",
            "--reconnect",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let store = Store::open(&db).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if xxassxx::service::status(&store).unwrap()["state"] == "backoff" {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(child.try_wait().unwrap().is_none());
    let cfg =
        toml::from_str(&std::fs::read_to_string(server_root.join("server.toml")).unwrap()).unwrap();
    let router = xxassxx::mailbox::router(&server_root.join("mailbox.sqlite3"), &cfg).unwrap();
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    loop {
        if xxassxx::service::status(&store).unwrap()["state"] == "running" {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    xxassxx::service::execute(&store, xxassxx::service::Command::Stop { wait_secs: 5 })
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(!xxassxx::team::transient_transport_error("http_status_401"));
    assert!(!xxassxx::team::transient_transport_error(
        "model_quota_limited"
    ));
    server.abort();
}
