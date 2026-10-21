mod common;
use common::Team;
use rusqlite::params;
use serde_json::{Value, json};
use xxassxx::{
    app::{self, Actor, Instruction},
    store::Store,
    task_coordinator as tasks,
    task_recovery::{self, Reason},
};

fn input(team: &Team, member: &str, body: &str) -> Instruction {
    let s = team.store(member);
    let p = app::project(&s, &team.dir.path().join(member).join("work")).unwrap();
    Instruction {
        request_id: uuid::Uuid::new_v4().to_string(),
        channel: "context-regression".into(),
        session_id: app::session(&s, p["id"].as_str().unwrap(), member).unwrap(),
        task_id: None,
        recipient: member.into(),
        body: body.into(),
        action: "chat".into(),
        payload: json!({}),
    }
}

fn submit(s: &mut Store, i: &Instruction) {
    app::submit(s, &Actor::local(s).unwrap(), i).unwrap();
}

fn incoming(team: &Team) -> String {
    let mut a = team.store("a");
    let mut i = input(team, "a", "@b 查找项目报告并说明已核实的进展");
    i.action = "create_task".into();
    submit(&mut a, &i);
    let id = app::command(&a, &i.request_id).unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    tasks::prepare(&a, &id, json!({"goal":"查找项目报告并说明已核实的进展","deliverables":["进展概述"],"constraints":"只读","questions":[],"mode":"analysis"})).unwrap();
    i.request_id = uuid::Uuid::new_v4().to_string();
    i.task_id = Some(id.clone());
    i.payload = json!({"revision":1});
    tasks::confirm(&a, &i).unwrap();
    let mut b = team.store("b");
    for message in a.messages(None).unwrap().into_iter().filter(|r| {
        r.direction == "out" && r.message.operation == "task_v2" && r.message.recipient == "b"
    }) {
        b.save_message(&message.message, true).unwrap();
        tasks::ingest(&mut b, &message.message).unwrap();
    }
    id
}

fn db(s: &Store) -> rusqlite::Connection {
    rusqlite::Connection::open(&s.path).unwrap()
}

#[test]
fn inbound_followup_keeps_original_words_and_submission_time_context() {
    let team = Team::new();
    let id = incoming(&team);
    let mut b = team.store("b");
    let c = db(&b);
    let task = tasks::get(&b, &id).unwrap();
    c.execute("UPDATE app_tasks SET state='needs_attention',waiting_reason='材料处理暂时停止' WHERE id=?1",[&id]).unwrap();
    let block = task_recovery::record(
        &c,
        &task,
        "first-block",
        Reason::ExecutionError,
        "execution",
        "frozen-step",
        "一个候选文件不能作为材料",
        "b",
    )
    .unwrap();
    let question = uuid::Uuid::new_v4().to_string();
    c.execute("INSERT INTO task_questions(id,task_id,revision,asker,body,reason,known,state,created_at) VALUES(?1,?2,1,'a','报告范围是哪一年','需澄清','','user',1)",params![question,id]).unwrap();
    // Synthetic saved execution evidence: the later blocked step must not make
    // the owner dialogue claim the earlier discovery found nothing.
    c.execute("INSERT INTO tasks(id,input,state,created_at) VALUES('saved-discovery','evidence fixture','succeeded',1)",[]).unwrap();
    c.execute("INSERT INTO task_executions(id,task_id,revision,phase,state,created_at) VALUES('saved-discovery',?1,1,'discover','candidate',1)",[&id]).unwrap();
    c.execute("INSERT INTO runs(id,task_id,token_hash,state,workdir,sandbox,deadline,result) VALUES('saved-run','saved-discovery','fixture','succeeded','/fixture','read-only',1,?1)",[json!({"body":"已找到两份项目报告，尚未核验来源。","files":[{"root":0,"path":"reports/alpha.md"},{"root":0,"path":"reports/beta.md"}]}).to_string()]).unwrap();
    let i = input(
        &team,
        "b",
        "保留已经找到的报告，仅说明有依据的进展；受限部分先不要处理。",
    );
    assert_ne!(i.session_id, task["session_id"]);
    submit(&mut b, &i);
    let context = app::model::context(&b, &i).unwrap();
    let visible = context["submitted_task_context"]["tasks"]
        .as_array()
        .unwrap();
    let saved = visible.iter().find(|t| t["id"] == id).unwrap();
    assert_eq!(saved["blocker"]["id"], block["id"]);
    assert_eq!(saved["saved_findings"][0]["file_count"], 2);
    assert_eq!(
        saved["saved_findings"][0]["files"][0]["path"],
        "reports/alpha.md"
    );
    assert!(
        saved["saved_findings"][0]["body"]
            .as_str()
            .unwrap()
            .contains("尚未核验")
    );
    // A question or blocker created while the command waits is not a user
    // answer or recovery grant for that new state.
    c.execute("INSERT INTO task_questions(id,task_id,revision,asker,body,reason,known,state,created_at) VALUES('later-question',?1,1,'a','后来新问题','需澄清','','user',2)",[&id]).unwrap();
    task_recovery::resolve(&c, &task).unwrap();
    task_recovery::record(
        &c,
        &task,
        "later-block",
        Reason::ExecutionUncertain,
        "execution",
        "other-step",
        "新中断",
        "b",
    )
    .unwrap();
    let result =
        app::model::call(&mut b, &i, "continue_task", json!({"task_id":id}), false).unwrap();
    assert_eq!(result["resumed"], false);
    assert_eq!(
        app::model::call(&mut b, &i, "continue_task", json!({"task_id":id}), false).unwrap(),
        result
    );
    let mut restored = i.clone();
    app::task_context::restore_binding(&b, &mut restored).unwrap();
    assert_eq!(restored.body, i.body);
    assert_eq!(restored.task_id.as_deref(), Some(id.as_str()));
    assert_eq!(restored.session_id, task["session_id"].as_str().unwrap());
    assert_eq!(restored.payload["block_id"], block["id"]);
    assert_eq!(restored.payload["question_ids"], json!([question]));
    let history = app::history(&b, &restored.session_id).unwrap();
    assert!(
        history
            .iter()
            .any(|m| m["command_id"] == i.request_id && m["body"] == i.body && m["task_id"] == id)
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM app_tasks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM task_recovery_grants", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM task_inputs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0,
        "routing alone cannot apply a supplement"
    );
}

#[test]
fn routing_cannot_select_future_tasks_or_reinterpret_new_revisions() {
    let team = Team::new();
    let mut b = team.store("b");
    let early = input(&team, "b", "继续检查现有报告");
    submit(&mut b, &early);
    let id = incoming(&team);
    assert!(
        app::model::call(
            &mut b,
            &early,
            "continue_task",
            json!({"task_id":id}),
            false
        )
        .is_err()
    );
    assert!(app::command(&b, &early.request_id).unwrap()["task_id"].is_null());
    let later = input(&team, "b", "补充限制：仅使用已保存的结果");
    submit(&mut b, &later);
    db(&b)
        .execute("UPDATE app_tasks SET revision=2 WHERE id=?1", [&id])
        .unwrap();
    assert!(
        app::model::call(
            &mut b,
            &later,
            "continue_task",
            json!({"task_id":id}),
            false
        )
        .is_err()
    );
    assert!(app::command(&b, &later.request_id).unwrap()["task_id"].is_null());
}

#[test]
fn selected_task_corrections_cannot_start_another_task() {
    let team = Team::new();
    let id = incoming(&team);
    let mut b = team.store("b");
    let t = tasks::get(&b, &id).unwrap();
    let mut i = input(&team, "b", "刚才用词有误，请按原有项目理解");
    i.session_id = t["session_id"].as_str().unwrap().into();
    i.task_id = Some(id.clone());
    submit(&mut b, &i);
    for (tool, args) in [
        ("send_peer", json!({"member_id":"a","body":"invented work"})),
        (
            "create_read_task",
            json!({"title":"invented","goal":"invented"}),
        ),
        ("delegate_codex", json!({"reason":"invented"})),
    ] {
        assert!(
            app::model::call(&mut b, &i, tool, args, false).is_err(),
            "{tool}"
        );
    }
    assert_eq!(
        db(&b)
            .query_row("SELECT count(*) FROM app_tasks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(app::command(&b, &i.request_id).unwrap()["task_id"], id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_executor_chat_routes_to_original_task_and_keeps_constraints_local() {
    use std::sync::{Arc, Mutex};
    let team = Team::new();
    let id = incoming(&team);
    let mut b = team.store("b");
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let log = requests.clone();
    let selected = id.clone();
    let router=axum::Router::new().route("/chat/completions",axum::routing::post(move |axum::Json(body):axum::Json<Value>|{
        let log=log.clone();
        let selected=selected.clone();
        async move {
            log.lock().unwrap().push(body.clone());
            let routing=body["tools"].as_array().unwrap().iter().any(|tool|tool["function"]["name"]=="continue_task");
            let (name,args)=if routing {
                ("continue_task",json!({"task_id":selected}))
            } else {
                ("task_decision",json!({"action":"supplement","body":"本机约束","resume":false}))
            };
            axum::Json(json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"route-or-followup","type":"function","function":{"name":name,"arguments":args.to_string()}}]}}]}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut cfg = b.member_config().unwrap();
    cfg.model = json!({"provider":"compatible","base_url":url,"model":"routing-regression","api_key_env":"","allow_insecure_http":true,"thinking":false});
    b.configure_member(&cfg).unwrap();
    let c = db(&b);
    c.execute("UPDATE app_tasks SET state='needs_attention',waiting_reason='需要选择可用材料' WHERE id=?1",[&id]).unwrap();
    let before_messages = b.messages(None).unwrap().len();
    let i = input(
        &team,
        "b",
        "本机补充：诊断记录仅供本机排查，结果中不要引用其内容。",
    );
    submit(&mut b, &i);
    app::model::process_one(&mut b).await.unwrap();
    let command = app::command(&b, &i.request_id).unwrap();
    assert_eq!(command["state"], "done", "{command}");
    assert_eq!(command["task_id"], id);
    assert_eq!(
        c.query_row("SELECT count(*) FROM app_tasks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let (kind, body): (String, String) = c
        .query_row(
            "SELECT kind,body FROM task_inputs WHERE id=?1",
            [&i.request_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind, "executor_constraint");
    assert_eq!(
        body, i.body,
        "the model cannot replace the owner's original words"
    );
    assert_eq!(tasks::get(&b, &id).unwrap()["state"], "needs_attention");
    assert_eq!(
        b.messages(None).unwrap().len(),
        before_messages,
        "local constraints cannot become peer messages"
    );
    let requests = requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "routing must lead to followup, never task preparation"
    );
    assert!(requests[1]["messages"].to_string().contains("followup"));
    assert!(
        !requests[1]["messages"]
            .to_string()
            .contains("current stage is prepare")
    );
    drop(requests);
    c.execute("INSERT INTO tasks(id,input,state,created_at) VALUES('constraint-reader','read fixture','pending',1)",[]).unwrap();
    c.execute("INSERT INTO task_executions(id,task_id,revision,phase,state,created_at) VALUES('constraint-reader',?1,1,'discover','pending',1)",[&id]).unwrap();
    xxassxx::task_inputs::read(&c, "constraint-reader", "constraint-run").unwrap();
    xxassxx::task_inputs::read(&c, "constraint-reader", "constraint-run").unwrap();
    assert_eq!(
        b.messages(None).unwrap().len(),
        before_messages,
        "reading a local constraint must not export a receipt"
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM task_input_reads WHERE input_id=?1",
            [&i.request_id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_followup_revises_the_task_created_by_its_captured_predecessor() {
    use std::sync::{Arc, Mutex};
    let team = Team::new();
    let mut a = team.store("a");
    let first = input(&team, "a", "整理已有项目报告并说明当前进展");
    let second = input(&team, "a", "补充纠正：只用已有报告，不重新运行程序");
    let expected = xxassxx::team::stable_id(&first.request_id, "business-task");
    let selected = expected.clone();
    let initial = first.body.clone();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let log = requests.clone();
    let router=axum::Router::new().route("/chat/completions",axum::routing::post(move |axum::Json(body):axum::Json<Value>|{
        let log=log.clone();
        let selected=selected.clone();
        let initial=initial.clone();
        async move {
            log.lock().unwrap().push(body.clone());
            let messages=body["messages"].as_array().unwrap();
            let routing=body["tools"].as_array().unwrap().iter().any(|tool|tool["function"]["name"]=="continue_task");
            let (name,args)=if routing {
                let current=messages.iter().rev().find(|m|m["role"]=="user").unwrap()["content"].as_str().unwrap();
                if current==initial {
                    ("create_read_task",json!({"title":"项目报告","goal":initial}))
                } else {
                    ("continue_task",json!({"task_id":selected}))
                }
            } else {
                let context=messages.iter().filter(|m|m["role"]=="user").filter_map(|m|serde_json::from_str::<Value>(m["content"].as_str()?).ok()).find(|v|v["stage"].is_string()).unwrap();
                match context["stage"].as_str().unwrap() {
                    "prepare"=>("task_decision",json!({"action":"prepare","body":"整理现有报告","goal":context["task"]["original"],"deliverables":["进展概述"],"constraints":"只读","questions":[],"mode":"analysis"})),
                    "followup"=>("task_decision",json!({"action":"revise","body":"纠正当前任务的范围","effective_request":"整理已有项目报告并说明当前进展。只用已有报告，不重新运行程序。"})),
                    other=>panic!("unexpected stage {other}"),
                }
            };
            axum::Json(json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"queued-followup","type":"function","function":{"name":name,"arguments":args.to_string()}}]}}]}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut cfg = a.member_config().unwrap();
    cfg.model = json!({"provider":"compatible","base_url":url,"model":"queued-routing-regression","api_key_env":"","allow_insecure_http":true,"thinking":false});
    a.configure_member(&cfg).unwrap();
    // Both messages are submitted before either has produced a task card.
    submit(&mut a, &first);
    submit(&mut a, &second);
    assert!(app::command(&a, &second.request_id).unwrap()["task_id"].is_null());
    app::model::process_one(&mut a).await.unwrap();
    assert_eq!(
        app::command(&a, &first.request_id).unwrap()["state"],
        "done"
    );
    app::model::process_one(&mut a).await.unwrap();
    let command = app::command(&a, &second.request_id).unwrap();
    assert_eq!(command["state"], "done", "{command}");
    assert_eq!(command["task_id"], expected);
    assert_eq!(
        db(&a)
            .query_row("SELECT count(*) FROM app_tasks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let task = tasks::get(&a, &expected).unwrap();
    assert_eq!(task["revision"], 2);
    assert!(
        task["original"]
            .as_str()
            .unwrap()
            .contains("不重新运行程序")
    );
    assert!(
        app::history(&a, &second.session_id)
            .unwrap()
            .iter()
            .any(|m| m["command_id"] == second.request_id && m["body"] == second.body)
    );
    assert_eq!(
        requests.lock().unwrap().len(),
        5,
        "two owner routes, original preparation, followup and revised preparation"
    );
    server.abort();
}

#[test]
fn predecessor_reference_is_original_revision_only_and_cannot_approve_later_state() {
    let team = Team::new();
    let mut a = team.store("a");
    let first = input(&team, "a", "整理现有文档");
    let next = input(&team, "a", "继续讨论刚才的要求");
    let late_revision = input(&team, "a", "补充刚才的范围");
    let independent = input(&team, "a", "另起一项新任务：整理另一个项目的实验日志");
    submit(&mut a, &first);
    submit(&mut a, &next);
    submit(&mut a, &late_revision);
    submit(&mut a, &independent);
    app::model::call(
        &mut a,
        &first,
        "create_read_task",
        json!({"title":"文档","goal":first.body}),
        false,
    )
    .unwrap();
    let id = app::command(&a, &first.request_id).unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let t = tasks::get(&a, &id).unwrap();
    let c = db(&a);
    task_recovery::record(
        &c,
        &t,
        "newly-created-block",
        Reason::ExecutionError,
        "job",
        "later-step",
        "later issue",
        "a",
    )
    .unwrap();
    c.execute("INSERT INTO task_questions(id,task_id,revision,asker,body,reason,known,state,created_at) VALUES('later-question',?1,1,'a','later question','need input','','user',1)",[&id]).unwrap();
    app::model::call(&mut a, &next, "continue_task", json!({"task_id":id}), false).unwrap();
    let mut restored = next.clone();
    app::task_context::restore_binding(&a, &mut restored).unwrap();
    assert_eq!(restored.payload["captured_before_creation"], true);
    assert!(restored.payload["block_id"].is_null());
    assert_eq!(restored.payload["question_ids"], json!([]));
    c.execute("UPDATE app_tasks SET revision=2 WHERE id=?1", [&id])
        .unwrap();
    assert!(
        app::model::call(
            &mut a,
            &late_revision,
            "continue_task",
            json!({"task_id":id}),
            false
        )
        .is_err()
    );
    // A separate explicit new request may still create its own task; a pending
    // predecessor is context, never a blanket prohibition on new work.
    app::model::call(
        &mut a,
        &independent,
        "create_read_task",
        json!({"title":"separate task","goal":"separate task"}),
        false,
    )
    .unwrap();
    assert_ne!(
        app::command(&a, &independent.request_id).unwrap()["task_id"],
        id
    );
}
