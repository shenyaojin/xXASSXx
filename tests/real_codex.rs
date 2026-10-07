//! Opt-in: uses the existing official login and consumes real Codex usage.
use serde_json::json;
use std::{path::PathBuf, process::Command};
use xxassxx::store::Store;

#[test]
#[ignore = "REAL CODEX: uses network and existing login; run explicitly with --ignored --nocapture"]
fn real_codex_mcp_and_resume() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("smoke-output")
        .join(uuid::Uuid::new_v4().to_string());
    let workdir = root.join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    let report = root.join("report.json");
    std::fs::write(
        &report,
        json!({"status":"running","real_codex":true}).to_string(),
    )
    .unwrap();
    eprintln!("real smoke evidence: {}", root.display());
    let db = root.join("tasks.sqlite3");
    let initialized = Command::new(env!("CARGO_BIN_EXE_xxassxx"))
        .arg("--db")
        .arg(&db)
        .arg("init")
        .output()
        .unwrap();
    assert!(initialized.status.success());
    let submitted = Command::new(env!("CARGO_BIN_EXE_xxassxx"))
        .arg("--db").arg(&db).arg("submit")
        .arg("This is an integration smoke test. Submit exactly the string XXASSXX_REAL_SMOKE_OK as the result through submit_task_result. On a resumed execution read the task again and submit the same string for the new run_id. No file or shell operations are needed.")
        .output().unwrap();
    assert!(submitted.status.success());
    let submitted: serde_json::Value = serde_json::from_slice(&submitted.stdout).unwrap();
    let task_id = submitted["id"].as_str().unwrap().to_owned();
    let store = Store::open(&db).unwrap();
    let codex = std::env::var("XXASSXX_REAL_CODEX").unwrap_or("codex".into());
    let outcome = std::panic::catch_unwind(|| {
        let store = Store::open(&db).unwrap();
        let mut session = None;
        for resume in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_xxassxx"));
            command
                .arg("--db")
                .arg(&db)
                .arg("run-once")
                .arg(&task_id)
                .arg("--workdir")
                .arg(&workdir)
                .arg("--codex")
                .arg(&codex)
                .args(["--timeout-secs", "180"]);
            if resume {
                command.arg("--resume");
            }
            if let Ok(model) = std::env::var("XXASSXX_REAL_MODEL") {
                command.arg("--model").arg(model);
            }
            let output = command.output().unwrap();
            let label = if resume { "resume" } else { "initial" };
            std::fs::write(root.join(format!("{label}.stdout.json")), &output.stdout).unwrap();
            std::fs::write(root.join(format!("{label}.stderr.txt")), &output.stderr).unwrap();
            assert!(
                output.status.success(),
                "{label}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let saved = store.task(&task_id).unwrap();
            assert_eq!(saved.state, "succeeded");
            assert_eq!(saved.result.as_deref(), Some("XXASSXX_REAL_SMOKE_OK"));
            if resume {
                assert_eq!(saved.session_id, session);
            } else {
                session = saved.session_id;
                assert!(session.is_some());
            }
            let runs = store.runs(&task_id).unwrap();
            let run = runs.last().unwrap();
            assert!(
                run.mcp_initialized
                    && run.reads > 0
                    && run.submissions > 0
                    && run.turn_completed
                    && run.exit_code == Some(0)
            );
            let events = store.events(&run.id).unwrap();
            std::fs::write(
                root.join(format!("{label}.events.json")),
                serde_json::to_string_pretty(&events).unwrap(),
            )
            .unwrap();
            // Independent of prose: real CLI structured MCP events AND DB receipts.
            for tool in ["get_task", "submit_task_result"] {
                assert!(
                    events.iter().any(|e| e["type"] == "item.completed"
                        && e["item"]["type"] == "mcp_tool_call"
                        && e["item"]["server"] == "xxassxx"
                        && e["item"]["tool"] == tool
                        && e["item"]["status"] == "completed"),
                    "missing real MCP event: {tool}"
                );
            }
        }
    });
    let evidence = json!({"status":if outcome.is_ok(){"passed"}else{"failed"},"real_codex":true,"task":store.task(&task_id).unwrap(),"runs":store.runs(&task_id).unwrap()});
    std::fs::write(&report, serde_json::to_string_pretty(&evidence).unwrap()).unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
    println!("REAL CODEX PASSED: {}", report.display());
}
