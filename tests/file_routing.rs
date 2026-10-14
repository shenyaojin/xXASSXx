mod common;
use common::Team;
use serde_json::{Value, json};
use xxassxx::{
    access_mode::{self, Mode},
    app::{self, Actor, Instruction},
};

fn input(team: &Team, recipient: &str, body: &str) -> Instruction {
    let store = team.store("a");
    let project = app::project(&store, &team.dir.path().join("a/work")).unwrap();
    Instruction {
        request_id: uuid::Uuid::new_v4().to_string(),
        channel: "tui".into(),
        session_id: app::session(&store, project["id"].as_str().unwrap(), recipient).unwrap(),
        task_id: None,
        recipient: recipient.into(),
        body: body.into(),
        action: "chat".into(),
        payload: json!({}),
    }
}

#[tokio::test]
async fn addressed_local_push_uses_owner_router_and_respects_both_permission_modes() {
    let team = Team::new();
    let router = axum::Router::new().route("/chat/completions", axum::routing::post(|axum::Json(v): axum::Json<Value>| async move {
        let ctx: Value = serde_json::from_str(v["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(ctx["addressed_member"], "b");
        let message = if v["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool") {
            json!({"role":"assistant","content":"done"})
        } else {
            json!({"role":"assistant","tool_calls":[{"id":"push","type":"function","function":{"name":"prepare_file_send","arguments":json!({"member_id":"b","files":["研究 结果.txt"],"note":"文件交换测试"}).to_string()}}]})
        };
        axum::Json(json!({"choices":[{"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":message}]}))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut store = team.store("a");
    let root = team.dir.path().join("a/work");
    std::fs::write(root.join("研究 结果.txt"), "ROUTING-42").unwrap();
    store.allow_directory(&root).unwrap();
    let mut cfg = store.member_config().unwrap();
    cfg.model = json!({"provider":"compatible","model":"mock","base_url":base,"api_key_env":""});
    store.configure_member(&cfg).unwrap();
    for (recipient, mode) in [("a", Mode::AskForApproval), ("b", Mode::FullAccess)] {
        access_mode::set(&store, mode).unwrap();
        let i = input(&team, recipient, "@b 把我电脑里的『研究 结果.txt』传给你");
        let actor = Actor::local(&store).unwrap();
        let queued = app::submit(&mut store, &actor, &i).unwrap();
        assert!(
            queued["task_id"].is_null(),
            "addressing is not executor selection"
        );
        app::model::process_one(&mut store).await.unwrap();
        let command = app::command(&store, &i.request_id).unwrap();
        assert_eq!(command["state"], "done", "{command}");
        app::model::process_one(&mut store).await.unwrap();
        let transfers = xxassxx::transfers::list(&store).unwrap();
        assert!(
            transfers.iter().any(|t| t["state"]
                == if mode == Mode::FullAccess {
                    "queued"
                } else {
                    "draft"
                }),
            "{transfers:#?}"
        );
        assert!(app::tasks::list(&store, None).unwrap().is_empty());
        assert!(
            store.messages(None).unwrap().is_empty(),
            "no remote search task was sent"
        );
        let snapshot = app::snapshot(&store, &i.session_id).unwrap();
        assert!(
            snapshot["timeline"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["kind"] != "operation_prepare_file_send")
        );
        assert!(
            app::history(&store, &i.session_id)
                .unwrap()
                .iter()
                .any(|m| m["kind"] == "operation_prepare_file_send")
        );
    }
    server.abort();
}

#[test]
fn remote_requests_are_created_only_after_routing_and_capability_questions_need_no_task() {
    let team = Team::new();
    let mut store = team.store("a");
    for body in ["@b 可以传文件吗？", "@b 请把你那边的报告发来"] {
        let i = input(&team, "a", body);
        let actor = Actor::local(&store).unwrap();
        assert!(app::submit(&mut store, &actor, &i).unwrap()["task_id"].is_null());
        if body.contains("可以") {
            app::model::call(
                &mut store,
                &i,
                "reply_user",
                json!({"text":"支持文件发送。","question":false}),
                false,
            )
            .unwrap();
            assert!(app::tasks::list(&store, None).unwrap().is_empty());
        } else {
            let task = app::model::call(
                &mut store,
                &i,
                "send_peer",
                json!({"member_id":"b","body":body}),
                false,
            )
            .unwrap();
            assert_eq!(task["participants"], json!(["a", "b"]));
            assert_eq!(
                app::command(&store, &i.request_id).unwrap()["task_id"],
                task["id"]
            );
        }
    }
}

#[test]
fn questions_with_unbound_paths_remain_deliverable_without_inventing_ownership() {
    let team = Team::new();
    let mut store = team.store("a");
    let mut i = input(&team, "a", "@b 检查材料");
    i.action = "create_task".into();
    let actor = Actor::local(&store).unwrap();
    let task = app::submit(&mut store, &actor, &i).unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let body = r#"未找到 /home/other/报告.zip；检查过 /tmp /mnt /media。引用 (/opt/data/file.csv) 与 file:///var/cache/item.txt、C:\data\x.txt；链接 https://example.org/a/b?path=/tmp 和 member://b/work/a.txt； main / current"#;
    let value = xxassxx::task_prose::portable(
        &rusqlite::Connection::open(&store.path).unwrap(),
        "a",
        &task,
        json!({"body":body,"nested":["/unknown/path"]}),
    )
    .unwrap();
    let body = value["body"].as_str().unwrap();
    assert!(body.contains("unbound://absolute/tmp"));
    assert!(!body.contains("member://a/home/other"));
    assert!(body.contains("https://example.org/a/b?path=/tmp"));
    assert!(body.contains("member://b/work/a.txt"));
    assert!(body.contains("main / current"));
    assert_eq!(
        xxassxx::task_prose::portable(
            &rusqlite::Connection::open(&store.path).unwrap(),
            "a",
            &task,
            value.clone()
        )
        .unwrap(),
        value
    );
    let work = team.dir.path().join("a/work").display().to_string();
    let url = format!("https://example.org/?first={work}&again={work}");
    assert_eq!(
        xxassxx::task_prose::portable(
            &rusqlite::Connection::open(&store.path).unwrap(),
            "a",
            &task,
            json!(url)
        )
        .unwrap(),
        json!(url)
    );
    xxassxx::task_coordinator::prepare(&store, &task, json!({"goal":body,"known_context":value,"mode":"files","start_from_request":true,"deliverables":["材料"],"questions":[]})).unwrap();
    let messages = store.messages(None).unwrap();
    assert_eq!(messages.len(), 1);
    messages[0].message.validate().unwrap();
}

/// Real provider evaluation: decision quality only; no file/task tools execute.
#[tokio::test]
#[ignore = "requires XXASSXX_ROUTING_MODEL_CONFIG and a live model API"]
async fn live_owner_file_routing_matrix() {
    let config: Value = serde_json::from_slice(
        &std::fs::read(std::env::var("XXASSXX_ROUTING_MODEL_CONFIG").unwrap()).unwrap(),
    )
    .unwrap();
    let model =
        xxassxx::model::HttpModel::new(xxassxx::model::ModelConfig::parse(&config).unwrap())
            .unwrap();
    let cases = [
        (
            "@b /Users/alice/Library/CloudStorage/资料/研究 结果.zip 能否把这个文件传给你所在机器",
            "prepare_file_send",
        ),
        (
            "@b 把我的 ~/Downloads/report.zip 发给你",
            "prepare_file_send",
        ),
        (
            "Send my local file file:///tmp/study%20notes.txt to b.",
            "prepare_file_send",
        ),
        (
            "@b 请把当前目录的结果.csv 传到你的服务器",
            "prepare_file_send",
        ),
        (
            "@b 给你两个本机文件：/tmp/a.txt 和 /tmp/b.txt",
            "prepare_file_send",
        ),
        (
            "把我本机 /home/alice/output.zip 发给 b",
            "prepare_file_send",
        ),
        ("@b 把你电脑上的最新报告发给我", "send_peer"),
        (
            "@b 你服务器 /home/ubuntu/project/out.zip 发到我这里",
            "send_peer",
        ),
        ("@b 请总结你机器上的项目进展", "send_peer"),
        ("@b 能传文件吗？先别发送，我想了解功能", "reply_user"),
        (
            "示例提示词是‘@b 发本机文件给你’，这样写可以吗？不要执行",
            "reply_user",
        ),
        ("帮我发文件", "reply_user"),
    ];
    let mut records = Vec::new();
    for mode in ["ask_for_approval", "full_access"] {
        for (body, expected) in cases {
            let context = json!({"identity":{"member_id":"a"},"contacts":[{"member_id":"b"}],"addressed_member":if body.starts_with("@b") {json!("b")}else{Value::Null},"working_directory":"/Users/alice/project","allowed_directories":["/Users/alice/project"],"access_mode":mode,"tasks":[],"recent_file_tasks":[],"selected_task_id":null});
            let mut messages = vec![
                json!({"role":"system","content":app::model::OWNER_PROMPT}),
                json!({"role":"system","content":context.to_string()}),
                json!({"role":"user","content":body}),
            ];
            let mut decision = Value::Null;
            for _ in 0..3 {
                let response = model
                    .complete_with_requirement(&messages, &app::model::tools(), true)
                    .await
                    .unwrap();
                messages.push(response.clone());
                for call in response["tool_calls"].as_array().unwrap() {
                    if call["function"]["name"] == "context" {
                        messages.push(json!({"role":"tool","tool_call_id":call["id"],"content":json!({"ok":true,"value":context}).to_string()}));
                    } else {
                        decision = call.clone();
                    }
                }
                if !decision.is_null() {
                    break;
                }
            }
            let passed = decision["function"]["name"] == expected;
            records.push(json!({"mode":mode,"request":body,"expected":expected,"decision":decision,"passed":passed}));
            if let Ok(path) = std::env::var("XXASSXX_ROUTING_EVIDENCE") {
                std::fs::write(path, serde_json::to_vec_pretty(&records).unwrap()).unwrap();
            }
        }
    }
    assert!(
        records.iter().all(|r| r["passed"] == true),
        "{}",
        serde_json::to_string_pretty(&records).unwrap()
    );
}
