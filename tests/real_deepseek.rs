mod common;
use serde_json::json;
use xxassxx::knowledge::Content;

#[test]
#[ignore = "REAL DEEPSEEK: explicit opt-in, uses DEEPSEEK_API_KEY and actual quota"]
fn real_deepseek_tools_and_traceable_reply() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("smoke-output")
        .join(format!("deepseek-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let report = dir.join("report.json");
    if std::env::var("DEEPSEEK_API_KEY")
        .ok()
        .is_none_or(|s| s.is_empty())
    {
        std::fs::write(
            &report,
            json!({"status":"not_run","reason":"DEEPSEEK_API_KEY missing","real_deepseek":true})
                .to_string(),
        )
        .unwrap();
        println!("NOT RUN: missing DEEPSEEK_API_KEY; {}", report.display());
        return;
    }
    std::fs::write(
        &report,
        json!({"status":"running","real_deepseek":true}).to_string(),
    )
    .unwrap();
    let team = common::Team::new();
    let mut b = team.store("b");
    let object = b
        .create_object("DeepSeek smoke dataset", "dataset", true)
        .unwrap();
    let version = b
        .publish(
            &object.id,
            None,
            "smoke",
            "test release",
            Content::Text("file content must not be uploaded"),
        )
        .unwrap();
    let mut cfg = b.member_config().unwrap();
    cfg.model = json!({"provider":"deepseek","model":std::env::var("DEEPSEEK_MODEL").unwrap_or("deepseek-flash".into()),"timeout_secs":60,"max_model_calls":5,"max_tool_rounds":4});
    b.configure_member(&cfg).unwrap();
    let r=team.cli("a",&["message","send","--to","b","--body","请调用 knowledge_metadata 查询这个数据集的发布版本，然后用 submit_reply 回复，注明来源成员、对象 ID 和具体版本 ID，不需要分析文件。","--object-id",&object.id]);
    let id = r["message"]["message_id"].as_str().unwrap();
    team.sync("a");
    team.sync("b");
    let out = team
        .command("b", &["butler", "process", id])
        .output()
        .unwrap();
    let history = b.model_history(id).unwrap();
    let record = b.message(id).unwrap();
    let verified = out.status.success()
        && record.state == "replied"
        && history.as_array().unwrap().iter().any(|run| {
            run["state"] == "succeeded"
                && run["steps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s["name"] == "knowledge_metadata" && s["result"]["ok"] == true)
        });
    let delivered = verified
        && team
            .command("b", &["butler", "sync"])
            .output()
            .unwrap()
            .status
            .success()
        && team
            .command("a", &["butler", "sync"])
            .output()
            .unwrap()
            .status
            .success()
        && team.store("a").messages(None).unwrap().iter().any(|m| {
            m.message.kind == "reply" && m.message.version_id.as_deref() == Some(&version.id)
        });
    let evidence = json!({"status":if verified && delivered{"passed"}else{"failed"},"real_deepseek":true,"delivered":delivered,"model":cfg.model["model"],"request_id":id,"object_id":object.id,"version_id":version.id,"model_runs":history,"messages":b.messages(None).unwrap()});
    std::fs::write(&report, serde_json::to_string_pretty(&evidence).unwrap()).unwrap();
    assert!(
        verified && delivered,
        "real DeepSeek did not produce a verified tool-backed reply; inspect {}",
        report.display()
    );
    println!("REAL DEEPSEEK PASSED: {}", report.display());
}
