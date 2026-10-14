//! Synthetic OAuth introspection + real HTTP MCP. This is not a dot/OAuth login test.
mod common;
use axum::{Form, Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

async fn inspect(
    State(tokens): State<Arc<Mutex<HashMap<String, Value>>>>,
    headers: axum::http::HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Json<Value> {
    assert_eq!(
        headers["authorization"],
        "Basic aHR0cHMlM0ElMkYlMkZyZXNvdXJjZS5leGFtcGxlJTJGbWNwK2NsaWVudCUyQjE6aXNvbGF0ZWQlMkJ0ZXN0JTNBJUU1JUFGJTg2JUU5JTkyJUE1"
    );
    Json(
        tokens
            .lock()
            .unwrap()
            .get(&form["token"])
            .cloned()
            .unwrap_or(json!({"active":false})),
    )
}
async fn call(client: &reqwest::Client, url: &str, token: &str, name: &str, args: Value) -> Value {
    let response=client.post(format!("{url}/mcp")).bearer_auth(token)
        .header("MCP-Protocol-Version","2025-06-18").header("Accept","application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})).send().await.unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    assert!(status.is_success(), "{status}: {text}");
    serde_json::from_str(&text).unwrap()
}
#[tokio::test(flavor = "multi_thread")]
async fn member_mcp_auth_visibility_revocation_and_message_boundary() {
    let team = common::Team::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let provider = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", provider.local_addr().unwrap());
    let tokens = Arc::new(Mutex::new(HashMap::<String, Value>::new()));
    let provider_app = Router::new()
        .route("/introspect", post(inspect))
        .with_state(tokens.clone());
    let provider_task =
        tokio::spawn(async move { axum::serve(provider, provider_app).await.unwrap() });
    let connections = team.dir.path().join("connections.toml");
    let bindings=["a","b","c"].map(|m|format!("[[connections]]\nid='test-{m}'\nsubject='subject-{m}'\nclient_id='dot-test'\nmember_id='{m}'\ncan_send=true\n")).join("");
    std::fs::write(&connections, &bindings).unwrap();
    let secret = team.dir.path().join("oauth.env");
    std::fs::write(&secret, "INTROSPECTION_SECRET=isolated+test:密钥\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let config = xxassxx::member_auth::Config {
        resource: format!("{url}/mcp"),
        issuer: issuer.clone(),
        introspection_url: format!("{issuer}/introspect"),
        introspection_client_id: "https://resource.example/mcp client+1".into(),
        secret_env: "INTROSPECTION_SECRET".into(),
        secrets_file: secret,
        connections_file: connections.clone(),
        allowed_origins: vec![],
        allow_loopback_http: true,
    };
    for m in ["a", "b", "c"] {
        tokens.lock().unwrap().insert(m.into(),json!({"active":true,"iss":issuer,"aud":config.resource,"exp":xxassxx::store::now()+300,"sub":format!("subject-{m}"),"client_id":"dot-test","scope":"inbox.read inbox.send"}));
    }
    let app =
        xxassxx::member_mcp::router(team.dir.path().join("relay.sqlite3"), config.clone()).unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    let metadata: Value = client
        .get(format!("{url}/.well-known/oauth-protected-resource/mcp"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(metadata["resource"], config.resource);
    for invalid in ["invalid", common::TOKEN] {
        let r = client
            .post(format!("{url}/mcp"))
            .bearer_auth(invalid)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401);
        assert!(
            r.headers()["www-authenticate"]
                .to_str()
                .unwrap()
                .contains("oauth-protected-resource")
        );
    }
    let init:Value=client.post(format!("{url}/mcp")).bearer_auth("a").header("Accept","application/json, text/event-stream").json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"isolated-test","version":"1"}}})).send().await.unwrap().json().await.unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "xxassxx-member-inbox");
    let list: Value = client
        .post(format!("{url}/mcp"))
        .bearer_auth("a")
        .header("MCP-Protocol-Version", "2025-06-18")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 6);
    assert_eq!(
        call(&client, &url, "a", "get_identity", json!({})).await["result"]["structuredContent"]["member_id"],
        "a"
    );
    let forged=call(&client,&url,"a","send_message",json!({"sender":"b","recipient":"b","subject":"test","body":"test","idempotency_key":"forged"})).await;
    assert!(forged.get("error").is_some());
    let contacts = call(&client, &url, "a", "list_contacts", json!({})).await;
    assert_eq!(
        contacts["result"]["structuredContent"]["contacts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let payload = json!({"recipient":"b","subject":"MCP test","body":"同意，开始运行","idempotency_key":"send-1"});
    let sent = call(&client, &url, "a", "send_message", payload.clone()).await;
    assert_eq!(
        sent,
        call(&client, &url, "a", "send_message", payload).await
    );
    let data = &sent["result"]["structuredContent"];
    let thread = data["thread_id"].as_str().unwrap();
    let listed = call(&client, &url, "b", "list_inbox", json!({"filter":"unread"})).await;
    assert_eq!(
        listed["result"]["structuredContent"]["threads"][0]["id"],
        thread
    );
    let read = call(
        &client,
        &url,
        "b",
        "read_thread",
        json!({"thread_id":thread}),
    )
    .await;
    assert_eq!(
        read["result"]["structuredContent"]["thread"]["unread"],
        true
    );
    assert_eq!(
        read["result"]["structuredContent"]["entries"][0]["origin"],
        "dot:test-a"
    );
    let hidden = call(
        &client,
        &url,
        "c",
        "read_thread",
        json!({"thread_id":thread}),
    )
    .await;
    assert_eq!(hidden["result"]["isError"], true);
    let reply=call(&client,&url,"b","reply_to_thread",json!({"thread_id":thread,"reply_to":data["entry"]["id"],"body":"收到；这是普通回复","idempotency_key":"reply-1"})).await;
    assert_eq!(
        reply["result"]["structuredContent"]["entry"]["recipient"],
        "a"
    );
    let db = rusqlite::Connection::open(team.dir.path().join("relay.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM relay_messages", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row(
            "SELECT seen_seq FROM inbox_member_state WHERE member='b'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM inbox_actor_receipts WHERE member='b'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    // Provider scope, audience, issuer, expiry, revocation, and subject/client binding are independent checks.
    let original = tokens.lock().unwrap()["a"].clone();
    for (field, value, status) in [
        ("aud", json!("another-resource"), 401),
        ("iss", json!("another-issuer"), 401),
        ("exp", json!(0), 401),
        ("active", json!(false), 401),
        ("sub", json!("unbound"), 403),
        ("client_id", json!("other-client"), 403),
        ("scope", json!("inbox.send"), 403),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        tokens.lock().unwrap().insert("a".into(), changed);
        assert_eq!(
            client
                .post(format!("{url}/mcp"))
                .bearer_auth("a")
                .send()
                .await
                .unwrap()
                .status()
                .as_u16(),
            status,
            "{field}"
        );
    }
    let mut readonly = original.clone();
    readonly["scope"] = json!("inbox.read");
    tokens.lock().unwrap().insert("a".into(), readonly);
    let denied=call(&client,&url,"a","send_message",json!({"recipient":"b","subject":"readonly","body":"cannot send","idempotency_key":"readonly"})).await;
    assert_eq!(
        denied["result"]["structuredContent"]["error"],
        "send_forbidden"
    );
    tokens.lock().unwrap().insert("a".into(), original);
    std::fs::write(
        &connections,
        bindings.replace("id='test-a'", "revoked=true\nid='test-a'"),
    )
    .unwrap();
    assert_eq!(
        client
            .post(format!("{url}/mcp"))
            .bearer_auth("a")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    db.execute("DELETE FROM relay_members WHERE id='b'", [])
        .unwrap();
    assert_eq!(
        client
            .post(format!("{url}/mcp"))
            .bearer_auth("b")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    task.abort();
    provider_task.abort();
}
