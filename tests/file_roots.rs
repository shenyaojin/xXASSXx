use serde_json::Value;
use std::{fs, path::Path, process::Command};
use xxassxx::{knowledge::Content, setup, store::Store};

#[test]
fn imports_default_to_deny_and_removal_keeps_existing_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("data");
    let outside = dir.path().join("data-other");
    fs::create_dir(&allowed).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(allowed.join("read.txt"), "allowed").unwrap();
    fs::write(outside.join("private.txt"), "outside").unwrap();
    let mut store = Store::init(&dir.path().join("state/db.sqlite3")).unwrap();
    store.set_identity("alice", "Alice", "lab").unwrap();
    let object = store.create_object("data", "dataset", false).unwrap();
    assert!(
        store
            .publish(&object.id, None, "denied", "", Content::Path(&allowed))
            .is_err()
    );
    assert!(!store.content_root().exists());
    store.allow_directory(&allowed).unwrap();
    store.allow_directory(&allowed.join(".")).unwrap();
    assert_eq!(store.file_roots().unwrap().len(), 1);
    assert!(
        store
            .publish(&object.id, None, "prefix", "", Content::Path(&outside))
            .is_err()
    );
    let version = store
        .publish(&object.id, None, "ok", "", Content::Path(&allowed))
        .unwrap();
    let path = store.path.clone();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store.file_roots().unwrap(),
        vec![allowed.canonicalize().unwrap()]
    );
    assert!(store.remove_directory(&allowed).unwrap());
    assert!(
        store
            .publish(
                &object.id,
                Some(&version.id),
                "removed",
                "",
                Content::Path(&allowed)
            )
            .is_err()
    );
    assert!(store.verify_version(&version.id).is_ok());
    assert!(!store.object(&object.id).unwrap().shared);
    let saved = store.allow_directory(&outside).unwrap();
    fs::remove_dir_all(&outside).unwrap();
    assert!(store.remove_directory(&saved).unwrap());
    assert!(store.file_roots().unwrap().is_empty());
}

#[test]
#[cfg(unix)]
fn links_cannot_expand_the_allowlist() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    let outside = dir.path().join("outside");
    fs::create_dir(&allowed).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret"), "outside").unwrap();
    let mut store = Store::init(&dir.path().join("db")).unwrap();
    store.set_identity("alice", "Alice", "lab").unwrap();
    let object = store.create_object("data", "dataset", false).unwrap();
    store.allow_directory(&allowed).unwrap();
    symlink(&outside, allowed.join("escape")).unwrap();
    for path in [
        &allowed,
        &allowed.join("escape/secret"),
        &allowed.join("../outside/secret"),
    ] {
        assert!(
            store
                .publish(&object.id, None, "escape", "", Content::Path(path))
                .is_err()
        );
    }
    fs::remove_file(allowed.join("escape")).unwrap();
    fs::remove_dir(&allowed).unwrap();
    symlink(&outside, &allowed).unwrap();
    assert!(
        store
            .publish(
                &object.id,
                None,
                "retarget",
                "",
                Content::Path(&allowed.join("secret"))
            )
            .is_err()
    );
    assert!(
        store
            .remove_directory(&allowed.canonicalize().unwrap())
            .is_ok()
    );
    // Remove using the originally recorded canonical path, not the new link target.
    let saved = store.file_roots().unwrap()[0].clone();
    assert!(store.remove_directory(&saved).unwrap());
    assert!(store.history(&object.id).unwrap().is_empty());
}

#[test]
fn migration_does_not_grant_legacy_databases_access_to_the_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("old.sqlite3");
    let conn = rusqlite::Connection::open(&db).unwrap();
    for schema in [
        include_str!("../src/schema.sql"),
        include_str!("../src/schema_v2.sql"),
        include_str!("../src/schema_v3.sql"),
    ] {
        conn.execute_batch(schema).unwrap();
    }
    conn.execute("INSERT INTO identity(singleton,member_id,display_name,team_id) VALUES(1,'alice','Alice','lab')", []).unwrap();
    conn.execute(
        "INSERT INTO tasks(id,input,created_at) VALUES('old','keep me',1)",
        [],
    )
    .unwrap();
    drop(conn);
    for _ in 0..2 {
        let store = Store::open(&db).unwrap();
        assert_eq!(store.owner().unwrap(), "alice");
        assert_eq!(store.task("old").unwrap().input, "keep me");
        assert!(store.file_roots().unwrap().is_empty());
    }
}

#[test]
fn client_init_and_roots_are_independent_of_startup_directory() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first");
    let other = dir.path().join("other");
    fs::create_dir_all(first.join("project")).unwrap();
    fs::create_dir(&other).unwrap();
    let server = dir.path().join("server");
    setup::init_server(
        &server,
        "lab",
        "https://team.example",
        &["alice=艾丽丝".into(), "bob=Bob".into()],
    )
    .unwrap();
    let invite = server.join("invitations/alice.json");
    let data = dir.path().join("user-data");
    let cli = |cwd: &Path, args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_xxassxx"))
            .current_dir(cwd)
            .env("XDG_DATA_HOME", &data)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let joined = cli(
        &first,
        &[
            "client",
            "init",
            "--invite",
            invite.to_str().unwrap(),
            "--provider",
            "off",
            "--allow-dir",
            "project",
            "--allow-dir",
            ".",
        ],
    );
    assert_eq!(joined["allowed_directories"].as_array().unwrap().len(), 2);
    let who = cli(&other, &["client", "whoami"]);
    assert_eq!(who["username"], "alice");
    assert_eq!(who["display_name"], "艾丽丝");
    assert_eq!(who["team_id"], "lab");
    assert!(who.get("credential").is_none());
    let roots = cli(&other, &["client", "roots", "list"]);
    assert_eq!(roots["directories"].as_array().unwrap().len(), 2);
    cli(&other, &["client", "roots", "add", "."]);
    cli(&other, &["client", "roots", "remove", "."]);
    let store = Store::open(&data.join("xxassxx/client/member.sqlite3")).unwrap();
    assert!(store.objects(false).unwrap().is_empty());
    assert_eq!(store.file_roots().unwrap().len(), 2);
    let invalid = dir.path().join("not-created");
    assert!(
        setup::init_client(
            &invalid,
            &invite,
            "off",
            None,
            None,
            None,
            "codex".into(),
            &[dir.path().join("missing")]
        )
        .is_err()
    );
    assert!(!invalid.exists());
}
