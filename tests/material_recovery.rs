use rusqlite::Connection;
use serde_json::{Value, json};
use std::{fs, path::PathBuf};
use xxassxx::{
    app::{self, Actor, Instruction},
    store::Store,
    task_coordinator as tasks, task_materials,
    task_recovery::{self, Reason},
    team::{ExecutorConfig, MemberConfig},
};

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    store: Store,
    task: Value,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("work");
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let mut store = Store::init(&directory.path().join("state/db.sqlite3")).unwrap();
        store.set_identity("owner", "Owner", "lab").unwrap();
        store
            .configure_member(&MemberConfig {
                mailbox_url: "http://127.0.0.1:1".into(),
                credential_env: "MATERIAL_TEST_UNUSED_TOKEN".into(),
                secrets_file: None,
                allow_insecure_http: false,
                contacts: vec![],
                executor: ExecutorConfig {
                    workdir: root.clone(),
                    ..Default::default()
                },
                model: json!({"provider":"off"}),
            })
            .unwrap();
        store.allow_directory(&root).unwrap();
        let project = app::project(&store, &root).unwrap();
        let instruction = Instruction {
            request_id: uuid::Uuid::new_v4().to_string(),
            channel: "test-user-action".into(),
            session_id: app::session(&store, project["id"].as_str().unwrap(), "owner").unwrap(),
            task_id: None,
            recipient: "owner".into(),
            body: "核对现有项目资料并说明证据缺口".into(),
            action: "create_task".into(),
            payload: json!({}),
        };
        let actor = Actor::local(&store).unwrap();
        let command = app::submit(&mut store, &actor, &instruction).unwrap();
        let id = command["task_id"].as_str().unwrap();
        tasks::prepare(
            &store,
            id,
            json!({"goal":"核对已有资料","mode":"analysis","deliverables":["报告"],"questions":[]}),
        )
        .unwrap();
        let task = tasks::get(&store, id).unwrap();
        task_materials::bind(&store, &task).unwrap();
        Self {
            _directory: directory,
            root,
            store,
            task,
        }
    }
    fn connection(&self) -> Connection {
        Connection::open(&self.store.path).unwrap()
    }
    fn feedback(&self, files: Value) -> Value {
        let error = task_materials::freeze(&self.store, &self.task, "candidate-execution", &files)
            .unwrap_err();
        assert_eq!(task_recovery::classify(&error), Reason::MaterialInput);
        let feedback =
            task_materials::correction_feedback(&error.context("freeze failed")).unwrap();
        assert_eq!(feedback["snapshot_created"], false);
        assert_eq!(
            self.connection()
                .query_row("SELECT count(*) FROM task_snapshots", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        feedback
    }
}

#[test]
fn rejected_private_file_preserves_ordinary_candidates_without_partial_snapshot_or_blob() {
    let mut f = Fixture::new();
    fs::write(f.root.join("report.txt"), "verified project progress").unwrap();
    fs::write(
        f.root.join("private.env"),
        "SYNTHETIC_PRIVATE_PAYLOAD=do-not-copy",
    )
    .unwrap();
    let mut cfg = f.store.member_config().unwrap();
    cfg.secrets_file = Some(f.root.join("private.env"));
    f.store.configure_member(&cfg).unwrap();
    let feedback = f.feedback(json!([
        {"root":0,"path":"report.txt"},
        {"root":0,"path":"private.env"}
    ]));
    assert_eq!(feedback["rejected"][0]["code"], "private_material");
    assert_eq!(feedback["rejected"][0]["path"], "private.env");
    assert_eq!(feedback["usable_files"][0]["path"], "report.txt");
    assert_eq!(feedback["usable_files"][0]["bytes"], 25);
    assert!(!feedback.to_string().contains("SYNTHETIC_PRIVATE_PAYLOAD"));
    assert!(!f.store.path.with_extension("task-materials").exists());
    assert!(
        !f.store
            .blob_path(feedback["usable_files"][0]["sha256"].as_str().unwrap())
            .unwrap()
            .exists()
    );
    // Correct the list in the same execution: rejection did not reserve its ID.
    let snapshot = task_materials::freeze(
        &f.store,
        &f.task,
        "candidate-execution",
        &json!([{"root":0,"path":"report.txt"}]),
    )
    .unwrap();
    let (directory, manifest) = task_materials::verify(&f.store, &snapshot).unwrap();
    assert_eq!(manifest.as_array().unwrap().len(), 1);
    assert_eq!(
        fs::read_to_string(directory.join("root-0/report.txt")).unwrap(),
        "verified project progress"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("private.env")).unwrap(),
        "SYNTHETIC_PRIVATE_PAYLOAD=do-not-copy"
    );
}

#[test]
fn all_bad_candidates_get_specific_feedback_and_later_valid_files_are_kept() {
    let f = Fixture::new();
    fs::write(f.root.join("valid.txt"), "evidence").unwrap();
    fs::write(f.root.join("binary.bin"), [0, 1, 2]).unwrap();
    fs::write(f.root.join("invalid-utf8.txt"), [0xff]).unwrap();
    std::os::unix::fs::symlink(f.root.join("valid.txt"), f.root.join("alias.txt")).unwrap();
    fs::create_dir(f.root.join("directory")).unwrap();
    let feedback = f.feedback(json!([
        {"root":0,"path":"missing.txt"},
        {"root":0,"path":"binary.bin"},
        {"root":0,"path":"alias.txt"},
        {"root":0,"path":"../state/db.sqlite3"},
        {"root":100,"path":"valid.txt"},
        {"root":0,"path":"directory"},
        {"root":0,"path":"invalid-utf8.txt"},
        {"root":0,"path":"valid.txt"},
        {"root":0,"path":"valid.txt"}
    ]));
    let codes: Vec<_> = feedback["rejected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["code"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes,
        [
            "unavailable",
            "unsupported_format",
            "symlink",
            "invalid_path",
            "invalid_root",
            "not_regular_file",
            "unsupported_format",
            "duplicate"
        ]
    );
    assert!(
        feedback["rejected"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| !v["next_step"].as_str().unwrap().is_empty())
    );
    assert_eq!(feedback["usable_files"].as_array().unwrap().len(), 1);
    assert_eq!(feedback["usable_files"][0]["path"], "valid.txt");
}

#[test]
fn byte_and_file_limits_are_actionable_without_discarding_other_candidates() {
    let f = Fixture::new();
    fs::write(f.root.join("large.txt"), "123456789").unwrap();
    fs::write(f.root.join("small.txt"), "123").unwrap();
    fs::write(f.root.join("extra.txt"), "45").unwrap();
    f.connection()
        .execute("UPDATE task_settings SET max_bytes=5,max_files=2", [])
        .unwrap();
    let feedback = f.feedback(json!([
        {"root":0,"path":"large.txt"},
        {"root":0,"path":"small.txt"},
        {"root":0,"path":"extra.txt"}
    ]));
    assert_eq!(feedback["rejected"][0]["code"], "byte_budget");
    assert_eq!(feedback["usable_files"].as_array().unwrap().len(), 2);
    assert_eq!(feedback["limits"]["max_total_bytes"], 5);
    f.connection()
        .execute("UPDATE task_settings SET max_files=1", [])
        .unwrap();
    let feedback = f.feedback(json!([{"root":0,"path":"small.txt"},{"root":0,"path":"extra.txt"}]));
    assert_eq!(feedback["rejected"][0]["code"], "file_budget");
    assert_eq!(feedback["limits"]["max_files"], 1);
    assert_eq!(
        f.feedback(json!([]))["rejected"][0]["code"],
        "empty_selection"
    );
    assert_eq!(
        f.feedback(Value::Null)["rejected"][0]["code"],
        "empty_selection"
    );
}

#[test]
fn freeze_rechecks_files_and_revoked_authority_after_submission_validation() {
    let f = Fixture::new();
    let files = json!([{"root":0,"path":"report.txt"}]);
    fs::write(f.root.join("report.txt"), "verified evidence").unwrap();
    task_materials::validate(&f.store, &f.task, &files).unwrap();
    assert!(!f.store.path.with_extension("task-materials").exists());
    fs::write(f.root.join("report.txt"), [0, 1, 2]).unwrap();
    assert_eq!(
        f.feedback(files.clone())["rejected"][0]["code"],
        "unsupported_format"
    );
    fs::write(f.root.join("report.txt"), "verified evidence").unwrap();
    task_materials::validate(&f.store, &f.task, &files).unwrap();
    // A storage problem must remain a real execution error, not an invitation
    // to keep selecting different source files.
    let snapshot_base = f.store.path.with_extension("task-materials");
    fs::write(&snapshot_base, "occupied by an unexpected file").unwrap();
    let error =
        task_materials::freeze(&f.store, &f.task, "candidate-execution", &files).unwrap_err();
    assert_eq!(task_recovery::classify(&error), Reason::ExecutionError);
    assert!(task_materials::correction_feedback(&error).is_none());
    fs::remove_file(snapshot_base).unwrap();
    f.store.remove_directory(&f.root).unwrap();
    let error =
        task_materials::freeze(&f.store, &f.task, "candidate-execution", &files).unwrap_err();
    assert_eq!(task_recovery::classify(&error), Reason::LocalPermission);
    assert!(task_materials::correction_feedback(&error).is_none());
    assert!(!f.store.path.with_extension("task-materials").exists());
    let error = anyhow::anyhow!("unexpected database or disk failure");
    assert_eq!(task_recovery::classify(&error), Reason::ExecutionError);
    assert!(task_materials::correction_feedback(&error).is_none());
}

#[test]
fn interrupted_snapshot_commit_retains_orphan_then_recovers_idempotently() {
    let f = Fixture::new();
    fs::write(
        f.root.join("report.txt"),
        "verified evidence before storage failure",
    )
    .unwrap();
    let files = json!([{"root":0,"path":"report.txt"}]);
    let execution = "interrupted-storage-execution";
    let snapshot = xxassxx::team::stable_id(execution, "materials");
    let base = f.store.path.with_extension("task-materials");
    let directory = base.join(&snapshot);
    let c = f.connection();
    c.execute_batch("CREATE TRIGGER reject_snapshot_commit BEFORE INSERT ON task_snapshots BEGIN SELECT RAISE(ABORT,'injected snapshot insert failure'); END;").unwrap();
    let error = task_materials::freeze(&f.store, &f.task, execution, &files).unwrap_err();
    assert_eq!(task_recovery::classify(&error), Reason::ExecutionError);
    assert!(task_materials::correction_feedback(&error).is_none());
    assert_eq!(
        c.query_row("SELECT count(*) FROM task_snapshots", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        fs::read_to_string(directory.join("root-0/report.txt")).unwrap(),
        "verified evidence before storage failure"
    );
    c.execute_batch("DROP TRIGGER reject_snapshot_commit;")
        .unwrap();
    assert_eq!(
        task_materials::freeze(&f.store, &f.task, execution, &files).unwrap(),
        snapshot
    );
    let retained: Vec<_> = fs::read_dir(base.join("orphans"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(retained.len(), 1);
    let note: Value =
        serde_json::from_slice(&fs::read(retained[0].join("retention.json")).unwrap()).unwrap();
    assert_eq!(note["kind"], "uncommitted_material_directory");
    assert_eq!(note["task_id"], f.task["id"]);
    assert_eq!(note["revision"], f.task["revision"]);
    assert_eq!(note["execution_id"], execution);
    assert_eq!(note["snapshot_id"], snapshot);
    assert_eq!(note["original_directory"], directory.to_str().unwrap());
    assert_eq!(
        fs::read_to_string(retained[0].join("materials/root-0/report.txt")).unwrap(),
        "verified evidence before storage failure"
    );
    let (_, before) = task_materials::verify(&f.store, &snapshot).unwrap();
    fs::write(
        f.root.join("report.txt"),
        "later source change must not replace a committed snapshot",
    )
    .unwrap();
    assert_eq!(
        task_materials::freeze(&f.store, &f.task, execution, &files).unwrap(),
        snapshot
    );
    assert_eq!(
        task_materials::verify(&f.store, &snapshot).unwrap().1,
        before
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM task_snapshots", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(fs::read_dir(base.join("orphans")).unwrap().count(), 1);
    let mut other_task = f.task.clone();
    other_task["id"] = json!("another-task");
    assert!(
        task_materials::freeze(&f.store, &other_task, execution, &files)
            .unwrap_err()
            .to_string()
            .contains("其他任务或版本")
    );
    assert_eq!(
        task_materials::verify(&f.store, &snapshot).unwrap().1,
        before
    );
}

#[test]
fn orphan_recovery_never_moves_a_directory_owned_by_another_snapshot() {
    let f = Fixture::new();
    fs::write(f.root.join("report.txt"), "owned snapshot evidence").unwrap();
    let files = json!([{"root":0,"path":"report.txt"}]);
    let execution = "owned-directory-execution";
    let snapshot = task_materials::freeze(&f.store, &f.task, execution, &files).unwrap();
    let (directory, _) = task_materials::verify(&f.store, &snapshot).unwrap();
    let c = f.connection();
    // Simulate an existing imported/repaired record that owns the same physical
    // directory under a different snapshot ID. It must never be quarantined.
    c.execute(
        "UPDATE task_snapshots SET id='other-owned-snapshot' WHERE id=?1",
        [&snapshot],
    )
    .unwrap();
    let error = task_materials::freeze(&f.store, &f.task, execution, &files).unwrap_err();
    assert!(error.to_string().contains("不能移动或覆盖"));
    assert_eq!(task_recovery::classify(&error), Reason::ExecutionError);
    assert!(task_materials::correction_feedback(&error).is_none());
    assert_eq!(
        fs::read_to_string(directory.join("root-0/report.txt")).unwrap(),
        "owned snapshot evidence"
    );
    assert!(task_materials::verify(&f.store, "other-owned-snapshot").is_ok());
    assert!(
        !f.store
            .path
            .with_extension("task-materials")
            .join("orphans")
            .exists()
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM task_snapshots", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}
