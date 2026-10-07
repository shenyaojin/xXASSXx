use std::{
    path::PathBuf,
    process::{Command, Output},
    time::Instant,
};
use tempfile::TempDir;
use xxassxx::store::Store;

const BIN: &str = env!("CARGO_BIN_EXE_xxassxx");
const SESSION: &str = "11111111-2222-4333-8444-555555555555";

struct Fixture {
    dir: TempDir,
    db: PathBuf,
    task: String,
    codex: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state with spaces.sqlite3");
        let store = Store::init(&db).unwrap();
        let task = store
            .submit(
                "UNTRUSTED_TASK_TEXT $(touch SHOULD_NOT_EXIST); `echo unsafe`\n引号 ' \" ; --help",
            )
            .unwrap()
            .id;
        let codex = dir.path().join("fake codex");
        std::fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex.py"),
            &codex,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self {
            dir,
            db,
            task,
            codex,
        }
    }
    fn command(&self, mode: &str, resume: bool) -> Command {
        let mut c = Command::new(BIN);
        c.arg("--db")
            .arg(&self.db)
            .arg("run-once")
            .arg(&self.task)
            .arg("--workdir")
            .arg(self.dir.path())
            .arg("--codex")
            .arg(&self.codex)
            .args([
                "--timeout-secs",
                if mode == "timeout" || mode == "preflight_timeout" {
                    "2"
                } else {
                    "20"
                },
            ])
            .env("XXASSXX_MOCK_MODE", mode)
            .env("XXASSXX_MOCK_RECORD", self.dir.path().join("record.json"))
            .env("XXASSXX_MOCK_CODEX_PID", self.dir.path().join("codex.pid"))
            .env("XXASSXX_MOCK_PID", self.dir.path().join("mcp.pid"));
        if resume {
            c.arg("--resume");
        }
        c
    }
    fn run(&self, mode: &str, resume: bool) -> Output {
        self.command(mode, resume).output().unwrap()
    }
    fn store(&self) -> Store {
        Store::open(&self.db).unwrap()
    }
    fn failure(&self, mode: &str, code: &str) {
        let out = self.run(mode, false);
        assert!(
            !out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let store = self.store();
        let runs = store.runs(&self.task).unwrap();
        assert_eq!(
            runs.last().unwrap().error_code.as_deref(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_ne!(store.task(&self.task).unwrap().state, "succeeded");
        assert!(store.task(&self.task).unwrap().result.is_none());
    }
}

#[test]
fn success_and_resume_by_persisted_id() {
    let f = Fixture::new();
    let out = f.run("success", false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        f.store().task(&f.task).unwrap().session_id.as_deref(),
        Some(SESSION)
    );
    assert!(
        !f.run("success", false).status.success(),
        "must require explicit resume"
    );
    let out = f.run("success", true);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let store = f.store();
    let task = store.task(&f.task).unwrap();
    let runs = store.runs(&f.task).unwrap();
    assert_eq!(task.result.as_deref(), Some("MOCK_RESULT"));
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[1].resumed_session_id.as_deref(), Some(SESSION));
    assert_eq!(runs[1].session_id.as_deref(), Some(SESSION));
    for run in runs {
        assert!(run.mcp_initialized && run.reads > 0 && run.submissions == 1 && run.turn_completed);
        assert_eq!(run.exit_code, Some(0));
        assert!(
            store
                .events(&run.id)
                .unwrap()
                .iter()
                .any(|e| e["type"] == "future.compatible.event")
        );
    }
    assert!(!f.dir.path().join("SHOULD_NOT_EXIST").exists());
}

#[test]
fn process_failure() {
    let f = Fixture::new();
    f.failure("failure", "process_failed");
    assert_eq!(f.store().runs(&f.task).unwrap()[0].exit_code, Some(7));
}
#[test]
fn quota_diagnostic() {
    Fixture::new().failure("quota", "quota_limited");
}
#[test]
fn login_diagnostic() {
    Fixture::new().failure("not_logged_in", "not_logged_in");
}
#[test]
fn mcp_initialization_failure() {
    Fixture::new().failure("mcp_init_failure", "mcp_initialization_failed");
}
#[test]
fn prose_completion_is_not_success() {
    Fixture::new().failure("no_mcp", "mcp_initialization_failed");
}
#[test]
fn missing_mcp_result() {
    Fixture::new().failure("missing_result", "missing_result");
}
#[test]
fn missing_completed_event() {
    Fixture::new().failure("no_completion", "incomplete_execution");
}
#[test]
fn submitted_result_plus_failure_is_not_success() {
    Fixture::new().failure("fail_after_result", "process_failed");
}
#[test]
fn malformed_events() {
    Fixture::new().failure("malformed", "protocol_error");
}
#[test]
fn preflight_timeout() {
    Fixture::new().failure("preflight_timeout", "timeout");
}

#[test]
fn duplicate_submission_and_conflicts() {
    let f = Fixture::new();
    let out = f.run("duplicate", false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let runs = f.store().runs(&f.task).unwrap();
    assert_eq!(runs[0].submissions, 2);
    assert_eq!(runs[0].result.as_deref(), Some("MOCK_RESULT"));
}

#[test]
fn rejects_wrong_identity_through_mcp() {
    let f = Fixture::new();
    let out = f.run("wrong_identity", false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn drains_stderr_without_deadlock() {
    let f = Fixture::new();
    let out = f.run("stderr_flood", false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn timeout_and_recovery() {
    let f = Fixture::new();
    let started = Instant::now();
    f.failure("timeout", "timeout");
    assert!(started.elapsed().as_secs() < 8);
    assert_process_stopped(wait_for_pid(&f, "codex.pid"));
    assert_process_stopped(wait_for_pid(&f, "mcp.pid"));
    assert_eq!(f.store().task(&f.task).unwrap().state, "timed_out");
    let out = f.run("success", true);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The killed MCP process cannot retain an active write capability.
    let runs = f.store().runs(&f.task).unwrap();
    assert_eq!(runs[0].submissions, 0);
    assert_eq!(runs[1].resumed_session_id.as_deref(), Some(SESSION));
}

// These regressions exercise real process lifetimes, not just expired DB rows.
struct RunningFixture {
    child: std::process::Child,
    codex_pid: Option<i32>,
}

impl RunningFixture {
    fn start(f: &Fixture, mode: &str) -> Self {
        Self {
            child: f
                .command(mode, false)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
            codex_pid: None,
        }
    }

    fn kill_runner(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

impl Drop for RunningFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // A failed regression must not itself leave the mock running.
        if let Some(pid) = self.codex_pid.filter(|pid| process_running(*pid)) {
            #[cfg(unix)]
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "process supervision condition timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn wait_for_pid(f: &Fixture, name: &str) -> i32 {
    let mut pid = None;
    wait_until(|| {
        pid = std::fs::read_to_string(f.dir.path().join(name))
            .ok()
            .and_then(|s| s.parse().ok());
        pid.is_some()
    });
    pid.unwrap()
}

fn process_running(pid: i32) -> bool {
    #[cfg(unix)]
    if unsafe { libc::kill(pid, 0) } != 0
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    {
        return false;
    }
    // Linux containers may retain orphan zombies until PID 1 reaps them.
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        if stat
            .rsplit_once(')')
            .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'))
        {
            return false;
        }
    }
    true
}

fn assert_process_stopped(pid: i32) {
    wait_until(|| !process_running(pid));
}

#[test]
fn runner_sigkill_stops_codex_and_mcp_before_resume() {
    let f = Fixture::new();
    let mut runner = RunningFixture::start(&f, "hold");
    let codex = wait_for_pid(&f, "codex.pid");
    runner.codex_pid = Some(codex);
    let mcp = wait_for_pid(&f, "mcp.pid");
    runner.kill_runner();
    assert_process_stopped(codex);
    assert_process_stopped(mcp);
    wait_until(|| f.store().task(&f.task).unwrap().state != "running");
    assert_eq!(
        f.store().runs(&f.task).unwrap()[0].error_code.as_deref(),
        Some("interrupted")
    );
    let resumed = f.run("success", true);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(
        f.store().runs(&f.task).unwrap()[1]
            .resumed_session_id
            .as_deref(),
        Some(SESSION)
    );
}

#[test]
fn runner_sigkill_during_preflight_stops_child() {
    let f = Fixture::new();
    let mut runner = RunningFixture::start(&f, "preflight_timeout");
    let codex = wait_for_pid(&f, "codex.pid");
    runner.codex_pid = Some(codex);
    runner.kill_runner();
    assert_process_stopped(codex);
    wait_until(|| f.store().task(&f.task).unwrap().state != "running");
    assert_eq!(
        f.store().runs(&f.task).unwrap()[0].error_code.as_deref(),
        Some("interrupted")
    );
    assert!(f.run("success", false).status.success());
}

#[test]
fn active_worker_blocks_recovery_even_after_lease_expiry() {
    let f = Fixture::new();
    let mut runner = RunningFixture::start(&f, "hold");
    let codex = wait_for_pid(&f, "codex.pid");
    runner.codex_pid = Some(codex);
    let mcp = wait_for_pid(&f, "mcp.pid");
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute("UPDATE runs SET deadline=0", []).unwrap();
    let blocked = f.run("success", true);
    assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("task executor is still alive"));
    assert_eq!(f.store().runs(&f.task).unwrap().len(), 1);
    runner.kill_runner();
    assert_process_stopped(codex);
    assert_process_stopped(mcp);
    wait_until(|| f.store().task(&f.task).unwrap().state != "running");
    assert!(f.run("success", true).status.success());
}

#[test]
#[cfg(unix)]
fn ctrl_c_stops_worker_children_and_records_interruption() {
    let f = Fixture::new();
    let mut runner = RunningFixture::start(&f, "hold");
    let codex = wait_for_pid(&f, "codex.pid");
    runner.codex_pid = Some(codex);
    let mcp = wait_for_pid(&f, "mcp.pid");
    assert_eq!(
        unsafe { libc::kill(runner.child.id() as i32, libc::SIGINT) },
        0
    );
    wait_until(|| runner.child.try_wait().unwrap().is_some());
    assert_process_stopped(codex);
    assert_process_stopped(mcp);
    assert_eq!(
        f.store().runs(&f.task).unwrap()[0].error_code.as_deref(),
        Some("interrupted")
    );
}

#[test]
fn rejects_changed_session_on_resume() {
    let f = Fixture::new();
    assert!(f.run("success", false).status.success());
    assert!(!f.run("wrong_session", true).status.success());
    let store = f.store();
    assert_eq!(
        store.task(&f.task).unwrap().session_id.as_deref(),
        Some(SESSION)
    );
    assert_eq!(
        store.runs(&f.task).unwrap()[1].error_code.as_deref(),
        Some("session_mismatch")
    );
}

#[test]
fn missing_codex_diagnostic() {
    let f = Fixture::new();
    let out = Command::new(BIN)
        .arg("--db")
        .arg(&f.db)
        .arg("run-once")
        .arg(&f.task)
        .arg("--workdir")
        .arg(f.dir.path())
        .args(["--codex", "xxassxx-nonexistent-official-codex"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(
        f.store().runs(&f.task).unwrap()[0].error_code.as_deref(),
        Some("not_installed")
    );
}

#[test]
fn store_rejects_inactive_stale_wrong_and_unread_submissions() {
    let f = Fixture::new();
    let mut s = f.store();
    let lease = s.claim(&f.task, false, f.dir.path(), 10).unwrap();
    assert!(s.claim(&f.task, false, f.dir.path(), 10).is_err());
    assert!(
        s.submit_result(&f.task, &lease.run_id, &lease.token, "k", "result")
            .is_err()
    );
    assert!(s.read_task(&f.task, &lease.run_id, "wrong-token").is_err());
    assert!(
        s.read_task("other-task", &lease.run_id, &lease.token)
            .is_err()
    );
    s.read_task(&f.task, &lease.run_id, &lease.token).unwrap();
    s.submit_result(&f.task, &lease.run_id, &lease.token, "k", "result")
        .unwrap();
    assert!(
        s.finish(&lease, None, Some(("timeout", "test timeout")))
            .is_err()
    );
    assert!(
        s.submit_result(&f.task, &lease.run_id, &lease.token, "k", "result")
            .is_err()
    );
    let next = s.claim(&f.task, false, f.dir.path(), 10).unwrap();
    assert!(s.read_task(&f.task, &lease.run_id, &lease.token).is_err());
    assert!(s.read_task(&f.task, &next.run_id, &lease.token).is_err());
}

#[test]
fn successful_result_retry_is_idempotent() {
    let f = Fixture::new();
    let mut s = f.store();
    let lease = s.claim(&f.task, false, f.dir.path(), 20).unwrap();
    s.initialized(&f.task, &lease.run_id, &lease.token).unwrap();
    s.read_task(&f.task, &lease.run_id, &lease.token).unwrap();
    s.submit_result(&f.task, &lease.run_id, &lease.token, "k", "result")
        .unwrap();
    s.event(
        &lease,
        &serde_json::json!({"type":"thread.started","thread_id":SESSION}),
    )
    .unwrap();
    s.event(&lease, &serde_json::json!({"type":"turn.completed"}))
        .unwrap();
    s.finish(&lease, Some(0), None).unwrap();
    assert!(
        s.submit_result(&f.task, &lease.run_id, &lease.token, "k", "result")
            .unwrap()["duplicate"]
            .as_bool()
            .unwrap()
    );
    assert!(
        s.submit_result(&f.task, &lease.run_id, &lease.token, "k", "changed")
            .is_err()
    );
}

#[test]
fn expired_runner_can_be_recovered_without_accepting_late_result() {
    let f = Fixture::new();
    let mut s = f.store();
    let lease = s.claim(&f.task, false, f.dir.path(), 10).unwrap();
    s.event(
        &lease,
        &serde_json::json!({"type":"thread.started","thread_id":SESSION}),
    )
    .unwrap();
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute("UPDATE runs SET deadline=0 WHERE id=?1", [&lease.run_id])
        .unwrap();
    assert!(s.read_task(&f.task, &lease.run_id, &lease.token).is_err());
    let next = s.claim(&f.task, true, f.dir.path(), 10).unwrap();
    assert_eq!(next.resume_id.as_deref(), Some(SESSION));
    assert!(s.finish(&lease, Some(0), None).is_err());
    assert_eq!(
        s.runs(&f.task).unwrap()[0].error_code.as_deref(),
        Some("lease_expired")
    );
}

#[test]
fn terminal_failure_cannot_be_overridden_by_completion() {
    Fixture::new().failure("terminal_then_completed", "process_failed");
}

#[test]
fn transient_reconnect_can_recover() {
    let f = Fixture::new();
    let out = f.run("transient_recovered", false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn concurrent_claims_have_exactly_one_winner() {
    let f = Fixture::new();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let db = f.db.clone();
            let task = f.task.clone();
            let workdir = f.dir.path().to_path_buf();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = Store::open(&db).unwrap();
                barrier.wait();
                store.claim(&task, false, &workdir, 20).is_ok()
            })
        })
        .collect();
    let winners = handles
        .into_iter()
        .filter_map(|h| h.join().ok())
        .filter(|won| *won)
        .count();
    assert_eq!(winners, 1);
    assert_eq!(f.store().runs(&f.task).unwrap().len(), 1);
}

#[test]
fn stdio_protocol_errors_and_initialization_are_enforced() {
    use std::io::Write;
    use std::process::Stdio;
    let f = Fixture::new();
    let mut store = f.store();
    let lease = store.claim(&f.task, false, f.dir.path(), 20).unwrap();
    let mut child = Command::new(BIN)
        .arg("--db")
        .arg(&f.db)
        .arg("mcp-serve")
        .arg("--task-id")
        .arg(&f.task)
        .arg("--run-id")
        .arg(&lease.run_id)
        .env("XXASSXX_RUN_TOKEN", &lease.token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(input, "not-json").unwrap();
    for request in [
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2099-01-01","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_task","arguments":{"task_id":f.task,"unexpected":true}}}),
        serde_json::json!({"jsonrpc":"2.0","id":4,"method":"unknown"}),
        serde_json::json!({"jsonrpc":"2.0","id":5,"method":"ping"}),
    ] {
        writeln!(input, "{request}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let responses: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(responses.len(), 6); // initialized notification has no response.
    assert_eq!(responses[0]["error"]["code"], -32700);
    assert_eq!(responses[1]["error"]["code"], -32000);
    assert_eq!(responses[2]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(responses[3]["result"]["isError"], true);
    assert_eq!(responses[4]["error"]["code"], -32601);
    assert_eq!(responses[5]["result"], serde_json::json!({}));
    assert_eq!(store.runs(&f.task).unwrap()[0].reads, 0);
}
