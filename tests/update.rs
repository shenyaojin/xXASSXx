mod common;
use anyhow::Result;
use ed25519_dalek::{Signer, SigningKey};
use semver::Version;
use serde_json::json;
use std::{collections::BTreeMap, path::Path};
use xxassxx::update::{
    Channel, SCHEMA, install, package,
    release::{self, Asset, Manifest},
    target,
};

fn manifest() -> Manifest {
    Manifest {
        format: 1,
        version: Version::parse(env!("CARGO_PKG_VERSION")).unwrap(),
        schema: SCHEMA,
        notes: "test".into(),
        assets: BTreeMap::new(),
    }
}
fn receipt(exe: &Path) {
    install::write_json(
        &install::sidecar(exe, "install.json").unwrap(),
        &json!({"format":1,"binary":"xxassxx","target":target().unwrap()}),
    )
    .unwrap();
}
#[test]
fn signatures_bind_version_platform_schema_and_hash() {
    let key = SigningKey::from_bytes(&[19; 32]);
    let public = release::hex(key.verifying_key().as_bytes());
    let mut m = manifest();
    m.assets.insert(
        target().unwrap().into(),
        Asset {
            name: format!("xxassxx-{}.tar.gz", target().unwrap()),
            sha256: "aa".repeat(32),
            size: 17,
        },
    );
    let bytes = serde_json::to_vec(&m).unwrap();
    let sig = release::hex(&key.sign(&bytes).to_bytes());
    assert!(release::verify(&bytes, &sig, &public).is_ok());
    assert!(release::verify(&bytes, &sig, release::PUBLIC_KEY).is_err());
    for field in ["schema", "version", "assets"] {
        let mut v = serde_json::to_value(&m).unwrap();
        v[field] = json!("tampered");
        assert!(release::verify(&serde_json::to_vec(&v).unwrap(), &sig, &public).is_err());
    }
    assert!(release::verify(&bytes, "not a signature", &public).is_err());
    assert!(release::unhex::<32>(&"é".repeat(32)).is_err());
}
#[test]
fn channels_use_semver_and_never_downgrade() {
    let current = Version::parse("0.4.0-alpha").unwrap();
    for (candidate, channel, accepted) in [
        ("0.4.0-alpha", Channel::Preview, false),
        ("0.3.9", Channel::Preview, false),
        ("0.5.0-alpha", Channel::Stable, false),
        ("0.5.0-alpha", Channel::Preview, true),
        ("0.4.0", Channel::Stable, true),
        ("0.10.0", Channel::Preview, true),
    ] {
        assert_eq!(
            release::eligible(&Version::parse(candidate).unwrap(), &current, channel),
            accepted
        );
    }
}
fn archive(path: &Path, link: bool, duplicate: bool) {
    let encoder = flate2::write::GzEncoder::new(
        std::fs::File::create(path).unwrap(),
        flate2::Compression::fast(),
    );
    let mut tar = tar::Builder::new(encoder);
    for _ in 0..if duplicate { 2 } else { 1 } {
        let mut header = tar::Header::new_gnu();
        header.set_size(if link { 0 } else { 5 });
        header.set_mode(0o755);
        if link {
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_link_name("/etc/passwd").unwrap();
        }
        header.set_cksum();
        tar.append_data(
            &mut header,
            "xxassxx",
            if link { &b""[..] } else { &b"hello"[..] },
        )
        .unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
}
#[test]
fn extraction_rejects_links_and_duplicate_binaries() {
    let temp = tempfile::tempdir().unwrap();
    for (n, link, duplicate) in [(0, true, false), (1, false, true), (2, false, false)] {
        let path = temp.path().join(format!("{n}.tgz"));
        archive(&path, link, duplicate);
        assert_eq!(
            package::extract(&path, &temp.path().join(format!("bin{n}"))).is_ok(),
            n == 2
        );
    }
}
#[test]
fn leases_serialize_updates_and_prevent_replacement_with_readers() {
    let temp = tempfile::tempdir().unwrap();
    let exe = temp.path().join("xxassxx");
    std::fs::write(&exe, "old").unwrap();
    assert!(install::managed(&exe).is_err());
    receipt(&exe);
    install::managed(&exe).unwrap();
    let reader = install::lock(&exe, "runtime.lock", false).unwrap();
    let reader2 = install::lock(&exe, "runtime.lock", false).unwrap();
    assert!(install::lock(&exe, "runtime.lock", true).is_err());
    drop((reader, reader2));
    let writer = install::lock(&exe, "runtime.lock", true).unwrap();
    assert!(install::lock(&exe, "runtime.lock", false).is_err());
    drop(writer);
    let update = install::lock(&exe, "update.lock", true).unwrap();
    assert!(install::lock(&exe, "update.lock", true).is_err());
    drop(update);
}
#[tokio::test]
async fn corrupt_archive_is_rejected_before_unpacking() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut m = manifest();
    let name = format!("xxassxx-{}.tar.gz", target()?);
    std::fs::write(temp.path().join(&name), b"corrupt")?;
    m.assets.insert(
        target()?.into(),
        Asset {
            name,
            size: 7,
            sha256: "00".repeat(32),
        },
    );
    let stage = tempfile::tempdir()?;
    let release = release::Release {
        manifest: m,
        base: String::new(),
    };
    assert!(
        package::prepare(&release, Some(temp.path()), stage.path())
            .await
            .is_err()
    );
    assert!(!stage.path().join("xxassxx").exists());
    Ok(())
}
#[tokio::test]
async fn install_is_atomic_and_never_opens_incompatible_database() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let exe = temp.path().join("xxassxx");
    let staged = temp.path().join("new");
    let db = temp.path().join("member.sqlite3");
    std::fs::write(&exe, "old")?;
    std::fs::write(&staged, "new")?;
    let conn = rusqlite::Connection::open(&db)?;
    conn.pragma_update(None, "user_version", 8)?;
    drop(conn);
    assert!(
        install::apply(&exe, &staged, &manifest(), &db, 1, false)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&exe)?, "old");
    let conn = rusqlite::Connection::open(&db)?;
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))?,
        8
    );
    drop(conn);
    let missing = temp.path().join("none.sqlite3");
    install::apply(&exe, &staged, &manifest(), &missing, 1, false).await?;
    assert_eq!(std::fs::read_to_string(&exe)?, "new");
    assert_eq!(
        std::fs::read_to_string(install::sidecar(&exe, "previous")?)?,
        "old"
    );
    assert!(!missing.exists());
    Ok(())
}
#[tokio::test]
async fn stop_timeout_never_replaces_the_program() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let exe = temp.path().join("xxassxx");
    let staged = temp.path().join("new");
    std::fs::write(&exe, "old")?;
    std::fs::write(&staged, "new")?;
    let store = xxassxx::store::Store::init(&temp.path().join("member.sqlite3"))?;
    let _runtime = xxassxx::service::Runtime::acquire(&store)?;
    let result = install::apply(&exe, &staged, &manifest(), &store.path, 1, false).await;
    assert!(result.unwrap_err().to_string().contains("后台尚未退出"));
    assert_eq!(std::fs::read_to_string(&exe)?, "old");
    assert!(!install::sidecar(&exe, "previous")?.exists());
    Ok(())
}
#[test]
fn update_from_tui_is_local_and_does_not_enqueue_a_model_command() {
    let team = common::Team::new();
    let mut store = team.store("a");
    let mut ui = xxassxx::tui::Ui::new(
        &store,
        xxassxx::app::project(&store, team.dir.path()).unwrap(),
    )
    .unwrap();
    ui.input.insert("/update");
    assert!(
        ui.key(
            &mut store,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE
            )
        )
        .unwrap()
    );
    let count: u32 = rusqlite::Connection::open(&store.path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM app_commands", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
#[test]
fn probe_needs_no_database_and_managed_processes_honor_runtime_lock() {
    let temp = tempfile::tempdir().unwrap();
    let exe = temp.path().join("xxassxx");
    std::fs::copy(common::BIN, &exe).unwrap();
    receipt(&exe);
    let exclusive = install::lock(&exe, "runtime.lock", true).unwrap();
    assert!(
        std::process::Command::new(&exe)
            .args(["update", "--probe"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let out = std::process::Command::new(&exe)
        .args(["client", "status"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("其他终端"));
    drop(exclusive);
}

#[tokio::test]
async fn failed_new_service_restores_old_binary_without_rewinding_data() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let team = common::Team::new();
    let temp = tempfile::tempdir()?;
    let exe = temp.path().join("xxassxx");
    std::fs::copy(common::BIN, &exe)?;
    receipt(&exe);
    let db = team.db("a");
    let secrets = temp.path().join("secrets.env");
    std::fs::write(&secrets, format!("TEST_A_TOKEN={}\n", common::TOKEN))?;
    std::fs::set_permissions(&secrets, std::fs::Permissions::from_mode(0o600))?;
    let mut store = team.store("a");
    let mut config = store.member_config()?;
    config.secrets_file = Some(secrets);
    store.configure_member(&config)?;
    drop(store);
    let mut command = std::process::Command::new(&exe);
    common::Team::credentials(&mut command);
    let started = command
        .arg("--db")
        .arg(&db)
        .args(["service", "start", "--reconnect"])
        .output()?;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let staged = temp.path().join("bad-service");
    std::fs::write(&staged, "#!/bin/sh\nexit 42\n")?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    let before = xxassxx::store::Store::open(&db)?.identity()?;
    let result = install::apply(&exe, &staged, &manifest(), &db, 5, false).await;
    let state = xxassxx::service::status_path(&db)?;
    xxassxx::service::stop_path(&db, 5).await?;
    assert!(result.unwrap_err().to_string().contains("已恢复旧程序"));
    assert_eq!(state["alive"], true);
    assert_eq!(before, xxassxx::store::Store::open(&db)?.identity()?);
    assert_eq!(std::fs::read(&exe)?, std::fs::read(common::BIN)?);
    Ok(())
}
