mod common;
use common::Team;
use serde_json::{Value, json};
use xxassxx::{app, mailbox::RelayConfig, team::Contact, tui::Ui};

#[test]
fn new_member_is_discovered_before_inbox_without_reimporting_invitation() {
    let mut team = Team::new();
    let config = team.dir.path().join("relay.toml");
    let original = std::fs::read_to_string(&config).unwrap();
    let mut two: RelayConfig = toml::from_str(&original).unwrap();
    two.members.retain(|m| m.member_id != "c");
    std::fs::write(&config, toml::to_string(&two).unwrap()).unwrap();
    team.restart();
    team.sync("a");
    let a = team.store("a");
    let session = app::session(
        &a,
        app::project(&a, team.dir.path()).unwrap()["id"]
            .as_str()
            .unwrap(),
        "b",
    )
    .unwrap();
    assert_eq!(a.contacts().unwrap().len(), 1);
    let mut before = serde_json::to_value(a.member_config().unwrap()).unwrap();
    before.as_object_mut().unwrap().remove("contacts");
    before.as_object_mut().unwrap().remove("mailbox_url");
    let mut ui = Ui::new(&a, app::project(&a, team.dir.path()).unwrap()).unwrap();
    ui.input.insert("@");
    assert_eq!(ui.candidates(), vec!["a", "b"]);

    let mut three: RelayConfig = toml::from_str(&original).unwrap();
    three.members[1].display_name = "Bob renamed".into();
    std::fs::write(&config, toml::to_string(&three).unwrap()).unwrap();
    team.restart();
    let message = team.cli(
        "c",
        &[
            "message",
            "send",
            "--to",
            "a",
            "--body",
            "Hello from newly joined Carol",
        ],
    );
    team.sync("c");
    // No client contact mutation or reinitialization after Carol joined.
    assert_eq!(a.contacts().unwrap().len(), 1);
    team.sync("a");
    let a = team.store("a");
    let contacts = serde_json::to_value(a.contacts().unwrap()).unwrap();
    assert_eq!(
        contacts,
        json!([
            {"member_id":"b","display_name":"Bob renamed"},
            {"member_id":"c","display_name":"Carol"}
        ])
    );
    assert_eq!(
        a.message(message["message"]["message_id"].as_str().unwrap())
            .unwrap()
            .message
            .body,
        "Hello from newly joined Carol"
    );
    assert_eq!(a.identity().unwrap()["display_name"], "Alice");
    let cfg = a.member_config().unwrap();
    assert_eq!(serde_json::to_value(&cfg.contacts).unwrap(), contacts);
    let mut after = serde_json::to_value(cfg).unwrap();
    after.as_object_mut().unwrap().remove("contacts");
    after.as_object_mut().unwrap().remove("mailbox_url");
    assert_eq!(before, after);
    assert!(app::snapshot(&a, &session).is_ok());
    ui.refresh(&a).unwrap();
    assert_eq!(ui.candidates(), vec!["a", "b", "c"]);
    assert_eq!(
        xxassxx::presence::view(&a).unwrap()["members"][1]["presence"]["state"],
        "unknown"
    );
    // A restarted client retains the directory even before the next network tick.
    assert_eq!(team.store("a").contacts().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn directory_is_authenticated_and_never_contains_credentials() {
    let team = Team::new();
    let client = reqwest::Client::new();
    let url = format!("{}/v1/whoami", team.url);
    assert_eq!(client.get(&url).send().await.unwrap().status(), 401);
    let value: Value = client
        .get(url)
        .bearer_auth(common::TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(value["member_directory"]["protocol"], 1);
    assert_eq!(
        value["member_directory"]["members"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    for member in value["member_directory"]["members"].as_array().unwrap() {
        assert_eq!(member.as_object().unwrap().len(), 2);
        assert!(member["member_id"].is_string() && member["display_name"].is_string());
    }
    assert!(!value.to_string().contains(common::TOKEN));
    assert!(!value.to_string().contains("token_hash"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_invalid_and_unreachable_servers_preserve_last_good_directory() {
    use axum::{Json, Router, routing::get};
    use std::sync::{Arc, Mutex};
    let team = Team::new();
    let identity = Arc::new(Mutex::new(json!({"team_id":"lab","member_id":"a"})));
    let response = identity.clone();
    let router = Router::new()
        .route(
            "/v1/whoami",
            get(move || {
                let response = response.clone();
                async move { Json(response.lock().unwrap().clone()) }
            }),
        )
        .route(
            "/v1/inbox",
            get(|| async { Json(json!({"team_id":"lab","messages":[]})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut a = team.store("a");
    let mut cfg = a.member_config().unwrap();
    cfg.mailbox_url = format!("http://{}", listener.local_addr().unwrap());
    a.configure_member(&cfg).unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let before = serde_json::to_value(a.contacts().unwrap()).unwrap();
    team.sync("a"); // Legacy server with no directory is still supported.
    assert_eq!(serde_json::to_value(a.contacts().unwrap()).unwrap(), before);
    for value in [
        json!({"team_id":"other","member_id":"a","member_directory":{"protocol":1,"members":[{"member_id":"a","display_name":"Other"}]}}),
        json!({"team_id":"lab","member_id":"a","member_directory":{"protocol":1,"members":[]}}),
        json!({"team_id":"lab","member_id":"a","member_directory":{"protocol":1,"members":[{"member_id":"b","display_name":"Bob"}]}}),
        json!({"team_id":"lab","member_id":"a","member_directory":{"protocol":1,"members":[{"member_id":"a","display_name":"A"},{"member_id":"a","display_name":"Duplicate"}]}}),
        json!({"team_id":"lab","member_id":"a","member_directory":{"protocol":1,"members":[{"member_id":"a","display_name":"A"},{"member_id":"../bad","display_name":"Invalid"}]}}),
    ] {
        *identity.lock().unwrap() = value;
        assert!(
            !team
                .command("a", &["butler", "sync"])
                .output()
                .unwrap()
                .status
                .success()
        );
        assert_eq!(serde_json::to_value(a.contacts().unwrap()).unwrap(), before);
    }
    server.abort();
    assert!(
        !team
            .command("a", &["butler", "sync"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        serde_json::to_value(team.store("a").contacts().unwrap()).unwrap(),
        before
    );
}

#[test]
fn live_directory_insertions_preserve_selected_member_and_mention() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let team = Team::new();
    let mut store = team.store("a");
    let mut ui = Ui::new(&store, app::project(&store, team.dir.path()).unwrap()).unwrap();
    ui.view = 2; // Carol
    ui.input.insert("@");
    ui.key(&mut store, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .unwrap();
    ui.key(&mut store, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .unwrap(); // Carol
    let mut cfg = store.member_config().unwrap();
    cfg.contacts.push(Contact {
        member_id: "bb".into(),
        display_name: "New member".into(),
    });
    store.configure_member(&cfg).unwrap();
    ui.refresh(&store).unwrap();
    assert_eq!(ui.view, 3);
    ui.key(&mut store, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(ui.input.text, "@c ");
    ui.view = 4; // Recent tasks
    cfg.contacts.retain(|c| c.member_id != "bb");
    store.configure_member(&cfg).unwrap();
    ui.refresh(&store).unwrap();
    assert_eq!(ui.view, 3);
}
