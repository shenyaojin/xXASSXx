#![allow(dead_code)]
use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};
use xxassxx::{
    store::Store,
    team::{Contact, ExecutorConfig, MemberConfig},
};
pub const BIN: &str = env!("CARGO_BIN_EXE_xxassxx");
pub const TOKEN: &str = "local-test-credential-aaaaaaaa";
pub struct Team {
    pub dir: tempfile::TempDir,
    pub relay: Child,
    pub url: String,
}
impl Drop for Team {
    fn drop(&mut self) {
        let _ = self.relay.kill();
        let _ = self.relay.wait();
    }
}
impl Team {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("relay.toml"),"team_id = 'lab'\n[[members]]\nmember_id='a'\ndisplay_name='Alice'\ncredential_env='TEST_A_TOKEN'\n[[members]]\nmember_id='b'\ndisplay_name='Bob'\ncredential_env='TEST_B_TOKEN'\n[[members]]\nmember_id='c'\ndisplay_name='Carol'\ncredential_env='TEST_C_TOKEN'\n").unwrap();
        let (relay, url) = Self::start(dir.path());
        let t = Self { dir, relay, url };
        for member in ["a", "b", "c"] {
            let mut store = Store::init(&t.db(member)).unwrap();
            store.set_identity(member, member, "lab").unwrap();
            std::fs::create_dir_all(t.dir.path().join(member).join("work")).unwrap();
            store
                .configure_member(&MemberConfig {
                    mailbox_url: t.url.clone(),
                    credential_env: format!("TEST_{}_TOKEN", member.to_uppercase()),
                    secrets_file: None,
                    allow_insecure_http: false,
                    contacts: ["a", "b", "c"]
                        .into_iter()
                        .filter(|p| *p != member)
                        .map(|p| Contact {
                            member_id: p.into(),
                            display_name: p.into(),
                        })
                        .collect(),
                    executor: ExecutorConfig {
                        workdir: t.dir.path().join(member).join("work"),
                        ..Default::default()
                    },
                    model: serde_json::json!({"provider":"off"}),
                })
                .unwrap();
        }
        t
    }
    fn start(root: &Path) -> (Child, String) {
        let mut command = Command::new(BIN);
        Self::credentials(&mut command);
        let mut child = command
            .arg("--db")
            .arg(root.join("relay.sqlite3"))
            .args(["mailbox", "serve", "--listen", "127.0.0.1:0", "--config"])
            .arg(root.join("relay.toml"))
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&line).expect("relay must start and report address");
        (
            child,
            format!("http://{}", value["listening"].as_str().unwrap()),
        )
    }
    pub fn restart(&mut self) {
        self.relay.kill().unwrap();
        self.relay.wait().unwrap();
        let (child, url) = Self::start(self.dir.path());
        self.relay = child;
        self.url = url;
        for member in ["a", "b", "c"] {
            let mut s = self.store(member);
            let mut cfg = s.member_config().unwrap();
            cfg.mailbox_url = self.url.clone();
            s.configure_member(&cfg).unwrap();
        }
    }
    pub fn credentials(cmd: &mut Command) {
        cmd.env("TEST_A_TOKEN", TOKEN)
            .env("TEST_B_TOKEN", "local-test-credential-bbbbbbbb")
            .env("TEST_C_TOKEN", "local-test-credential-cccccccc");
    }
    pub fn db(&self, member: &str) -> PathBuf {
        self.dir.path().join(member).join("tasks.sqlite3")
    }
    pub fn store(&self, member: &str) -> Store {
        Store::open(&self.db(member)).unwrap()
    }
    pub fn command(&self, member: &str, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        Self::credentials(&mut c);
        c.arg("--db").arg(self.db(member)).args(args);
        c
    }
    pub fn cli(&self, member: &str, args: &[&str]) -> serde_json::Value {
        let out = self.command(member, args).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    pub fn sync(&self, member: &str) {
        self.cli(member, &["butler", "sync"]);
    }
}
