#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use xxassxx::{setup, store::Store};

const BIN: &str = env!("CARGO_BIN_EXE_xxassxx");

struct Fixture {
    dir: tempfile::TempDir,
    client: PathBuf,
    project: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("工作项目 with spaces and \"quotes\"");
        fs::create_dir(&project).unwrap();
        let codex = dir.path().join("fake codex");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/interactive_codex.py"),
            &codex,
        )
        .unwrap();
        fs::set_permissions(&codex, fs::Permissions::from_mode(0o700)).unwrap();
        let server = dir.path().join("server");
        setup::init_server(
            &server,
            "lab",
            "http://127.0.0.1:9",
            &["owner=Shenyao \"Keith\" Jin".into(), "test=Test".into()],
        )
        .unwrap();
        let client = dir.path().join("data/xxassxx/client");
        setup::init_client(
            &client,
            &server.join("invitations/owner.json"),
            "off",
            None,
            None,
            None,
            codex,
            &[],
        )
        .unwrap();
        Self {
            dir,
            client,
            project,
        }
    }
    fn command(&self) -> Command {
        let mut c = Command::new(BIN);
        c.current_dir(&self.project)
            .env("XDG_DATA_HOME", self.dir.path().join("data"));
        c
    }
    fn store(&self) -> Store {
        Store::open(&self.client.join("member.sqlite3")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self
            .command()
            .args(["client", "stop"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[test]
fn check_launch_does_not_start_share_or_authorize_and_roundtrips_paths() {
    let f = Fixture::new();
    let out = f
        .command()
        .args(["codex", ".", "--check"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["project"],
        f.project.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(v["identity"]["display_name"], "Shenyao \"Keith\" Jin");
    let cfg: toml::Value = v["codex_args"][3].as_str().unwrap().parse().unwrap();
    assert_eq!(
        cfg["mcp_servers"]["xxassxx_butler"]["args"][1]
            .as_str()
            .unwrap(),
        f.store().path.to_str().unwrap()
    );
    assert!(f.store().file_roots().unwrap().is_empty());
    assert!(f.store().objects(false).unwrap().is_empty());
    assert_eq!(
        xxassxx::service::status(&f.store()).unwrap()["alive"],
        false
    );
    assert!(!f.project.join(".codex").exists());
    assert!(!f.project.join(".xxassxx").exists());
    assert!(
        !f.command()
            .args(["codex", "missing", "--check"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        !f.command()
            .args(["codex", ".", "--check", "--allow-import"])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn no_terminal_or_missing_installation_has_no_startup_side_effects() {
    let f = Fixture::new();
    let out = f.command().output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("交互终端"));
    assert_eq!(
        xxassxx::service::status(&f.store()).unwrap()["alive"],
        false
    );
    let missing = f.dir.path().join("unconfigured");
    let out = f.command().env("XDG_DATA_HOME", &missing).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("尚未初始化"));
    assert!(!missing.exists());
    assert!(
        !f.command()
            .args(["--db", "elsewhere.sqlite3"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!f.project.join("elsewhere.sqlite3").exists());
}

fn terminal_command(mut command: Command) -> std::process::ExitStatus {
    use std::{fs::File, os::fd::FromRawFd};
    let (mut master, mut slave) = (-1, -1);
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    let _master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    for key in [
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "DEEPSEEK_API_KEY",
        "XXASSXX_MEMBER_TOKEN",
        "XXASSXX_RUN_TOKEN",
    ] {
        command.env(key, "do-not-forward-test-value");
    }
    command
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap())
        .stderr(slave)
        .status()
        .unwrap()
}

#[test]
fn foreground_codex_gets_real_butler_mcp_and_daemon_survives_exit() {
    let f = Fixture::new();
    let mut explicit = f.command();
    explicit.arg("codex");
    assert_eq!(terminal_command(explicit).code(), Some(17));
    let first: Value =
        serde_json::from_slice(&fs::read(f.project.join("launch-report.json")).unwrap()).unwrap();
    assert_eq!(first["context"]["contacts"][0]["member_id"], "test");
    assert_eq!(first["context"]["allowed_directories"], json!([]));
    assert_eq!(xxassxx::service::status(&f.store()).unwrap()["alive"], true);
    let mut again = f.command();
    again.args(["codex", ".", "--allow-import"]);
    assert_eq!(terminal_command(again).code(), Some(17));
    let second: Value =
        serde_json::from_slice(&fs::read(f.project.join("launch-report.json")).unwrap()).unwrap();
    assert_eq!(
        first["context"]["service"]["instance"],
        second["context"]["service"]["instance"]
    );
    assert_eq!(
        f.store().file_roots().unwrap(),
        vec![f.project.canonicalize().unwrap()]
    );
    assert!(f.store().objects(false).unwrap().is_empty());
    assert!(f.store().messages(None).unwrap().is_empty());
}
