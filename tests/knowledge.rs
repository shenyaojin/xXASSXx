use std::{
    fs,
    sync::{Arc, Barrier},
};
use xxassxx::{
    knowledge::{Content, valid_relative},
    store::Store,
};

fn setup() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::init(&dir.path().join("state/db.sqlite3")).unwrap();
    s.set_identity("b", "Bob", "lab").unwrap();
    (dir, s)
}
#[test]
fn snapshots_are_independent_binary_and_empty_directories_survive() {
    let (d, mut s) = setup();
    let input = d.path().join("dataset");
    fs::create_dir_all(input.join("empty")).unwrap();
    fs::write(input.join("a.bin"), [0, 255, 13, 10]).unwrap();
    fs::write(input.join("gone.txt"), "old").unwrap();
    s.allow_directory(&input).unwrap();
    let o = s.create_object("Dataset", "dataset", true).unwrap();
    let v1 = s
        .publish(&o.id, None, "one", "V1", Content::Path(&input))
        .unwrap();
    fs::write(input.join("a.bin"), [0, 255, 11]).unwrap();
    fs::remove_file(input.join("gone.txt")).unwrap();
    fs::write(input.join("new.txt"), "new").unwrap();
    let v2 = s
        .publish(&o.id, Some(&v1.id), "two", "V2", Content::Path(&input))
        .unwrap();
    fs::remove_dir_all(input).unwrap();
    let diff = s.diff_versions(&v1.id, &v2.id).unwrap();
    assert_eq!(diff["changed"][0], "a.bin");
    assert_eq!(diff["removed"][0], "gone.txt");
    assert_eq!(diff["added"][0], "new.txt");
    let target = d.path().join("old");
    s.export_version(&v1.id, &target).unwrap();
    assert_eq!(fs::read(target.join("a.bin")).unwrap(), [0, 255, 13, 10]);
    assert!(target.join("empty").is_dir());
    assert!(s.export_version(&v1.id, &target).is_err());
    assert_eq!(s.object(&o.id).unwrap().main, Some(v2.id));
}
#[test]
fn text_idempotency_conflicts_parent_validation_and_immutable_rows() {
    let (_d, mut s) = setup();
    let o = s.create_object("Decision", "decision", false).unwrap();
    let a = s
        .publish(&o.id, None, "one", "note", Content::Text("使用 V2\n"))
        .unwrap();
    assert_eq!(
        a.id,
        s.publish(&o.id, None, "one", "note", Content::Text("使用 V2\n"))
            .unwrap()
            .id
    );
    assert!(
        s.publish(&o.id, None, "one", "note", Content::Text("different"))
            .is_err()
    );
    assert!(
        s.publish(&o.id, None, "another", "note", Content::Text("x"))
            .is_err()
    );
    assert_eq!(s.text_version(&a.id).unwrap(), "使用 V2\n");
    let b = s.create_object("Other", "decision", true).unwrap();
    assert!(
        s.publish(&b.id, Some(&a.id), "bad", "", Content::Text("x"))
            .is_err()
    );
    let conn = rusqlite::Connection::open(&s.path).unwrap();
    assert!(
        conn.execute("UPDATE versions SET note='changed'", [])
            .is_err()
    );
}
#[test]
fn concurrent_publish_compare_and_swap() {
    let (_d, s) = setup();
    let o = s.create_object("x", "dataset", false).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|i| {
            let db = s.path.clone();
            let id = o.id.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut s = Store::open(&db).unwrap();
                barrier.wait();
                s.publish(&id, None, &format!("p{i}"), "", Content::Text("same"))
                    .is_ok()
            })
        })
        .collect();
    assert_eq!(
        workers
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|x| *x)
            .count(),
        1
    );
    assert_eq!(s.history(&o.id).unwrap().len(), 1);
}
#[test]
fn corrupt_missing_content_and_failed_publish_never_succeed() {
    let (d, mut s) = setup();
    let o = s.create_object("x", "dataset", false).unwrap();
    let v = s
        .publish(&o.id, None, "one", "", Content::Text("value"))
        .unwrap();
    let blob = s
        .blob_path(v.manifest[0].sha256.as_deref().unwrap())
        .unwrap();
    fs::write(&blob, "bad").unwrap();
    assert!(s.verify_version(&v.id).is_err());
    assert!(
        s.export_version(&v.id, &d.path().join("bad-export"))
            .is_err()
    );
    assert!(!d.path().join("bad-export").exists());
    assert!(
        s.publish(&o.id, Some(&v.id), "bad", "", Content::Text("value"))
            .is_err()
    );
    assert_eq!(s.history(&o.id).unwrap().len(), 1);
    fs::remove_file(blob).unwrap();
    assert!(s.verify_version(&v.id).is_err());
}
#[test]
fn reject_symlinks_special_files_own_state_and_path_escape() {
    let (d, mut s) = setup();
    s.allow_directory(d.path()).unwrap();
    let o = s.create_object("x", "dataset", false).unwrap();
    assert!(
        s.publish(&o.id, None, "state", "", Content::Path(d.path()))
            .is_err()
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&s.path, d.path().join("link")).unwrap();
        assert!(
            s.publish(
                &o.id,
                None,
                "link",
                "",
                Content::Path(&d.path().join("link"))
            )
            .is_err()
        );
        assert!(
            s.publish(
                &o.id,
                None,
                "special",
                "",
                Content::Path(std::path::Path::new("/dev/null"))
            )
            .is_err()
        );
    }
    for path in [
        "../escape",
        "/absolute",
        "a/../x",
        "a//b",
        "C:\\path",
        "a\\b",
        "",
    ] {
        assert!(valid_relative(path).is_err());
    }
    assert!(s.history(&o.id).unwrap().is_empty());
}
#[test]
fn migration_preserves_phase_one_data_and_repeat_init() {
    let d = tempfile::tempdir().unwrap();
    let db = d.path().join("old.sqlite3");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(include_str!("../src/schema.sql"))
        .unwrap();
    conn.execute("INSERT INTO tasks(id,input,created_at,state,session_id,result) VALUES('old','original',1,'succeeded','session','result')",[]).unwrap();
    conn.execute("INSERT INTO runs(id,task_id,token_hash,workdir,sandbox,deadline,state,session_id,result) VALUES('run','old','hash','/tmp','read-only',1,'succeeded','session','result')",[]).unwrap();
    conn.execute(
        "INSERT INTO events(run_id,event) VALUES('run','{\"type\":\"turn.completed\"}')",
        [],
    )
    .unwrap();
    drop(conn);
    for _ in 0..2 {
        let s = Store::init(&db).unwrap();
        let t = s.task("old").unwrap();
        assert_eq!(t.result.as_deref(), Some("result"));
        assert_eq!(
            s.runs("old").unwrap()[0].session_id.as_deref(),
            Some("session")
        );
        assert_eq!(s.events("run").unwrap().len(), 1);
    }
}
