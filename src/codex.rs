//! Official CLI adapter: argv + stdin, never a shell, never credential-file access.
use crate::{
    mcp::MAX_FRAME,
    store::{Lease, Store},
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    time::{Instant, timeout_at},
};

pub struct Options {
    pub codex: PathBuf,
    pub mcp_executable: PathBuf,
    pub workdir: PathBuf,
    pub timeout_secs: u64,
    pub resume: bool,
    pub model: Option<String>,
}

#[derive(Debug)]
struct Failure {
    code: &'static str,
    message: String,
    exit_code: Option<i32>,
}
impl Failure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            exit_code: None,
        }
    }
}

/// Unknown text stays a process failure; never silently assume auth or quota.
pub fn classify(text: &str) -> &'static str {
    let t = text.to_lowercase();
    if [
        "not logged in",
        "authentication required",
        "unauthorized",
        "status 401",
        "please log in",
        "please login",
        "refresh token",
    ]
    .iter()
    .any(|s| t.contains(s))
    {
        "not_logged_in"
    } else if [
        "usage limit",
        "rate limit",
        "rate_limit",
        "insufficient_quota",
        "quota exceeded",
        "status 429",
        "429 too many requests",
        "credits",
    ]
    .iter()
    .any(|s| t.contains(s))
    {
        "quota_limited"
    } else if (t.contains("session")
        && (t.contains("not found")
            || t.contains("no session found")
            || t.contains("does not exist")))
        || t.contains("no rollout found")
    {
        "session_not_found"
    } else if t.contains("mcp")
        && [
            "failed",
            "failure",
            "timed out",
            "initialize",
            "initialization",
        ]
        .iter()
        .any(|s| t.contains(s))
    {
        "mcp_initialization_failed"
    } else {
        "process_failed"
    }
}

// A process group includes Codex and its stdio MCP children. Kill on every path,
// including cancellation and errors, so no detached writer survives a timeout.
struct Group(i32);
impl Drop for Group {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-self.0, libc::SIGKILL);
        }
    }
}

pub(crate) fn command(executable: &Path, cwd: &Path) -> Command {
    let mut c = Command::new(executable);
    if executable.is_absolute() {
        if let Some(parent) = executable.parent() {
            let paths = std::iter::once(parent.to_path_buf())
                .chain(std::env::split_paths(
                    &std::env::var_os("PATH").unwrap_or_default(),
                ))
                .collect::<Vec<_>>();
            if let Ok(path) = std::env::join_paths(paths) {
                c.env("PATH", path);
            }
        }
    }
    c.current_dir(cwd)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    c.process_group(0);
    // Official CLI retains HOME/CODEX_HOME and manages its own existing login.
    // Avoid changing the auth route through API-key overrides inherited by us.
    c.env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("DEEPSEEK_API");
    c
}

async fn probe(
    executable: &Path,
    cwd: &Path,
    args: &[&str],
    deadline: Instant,
) -> std::result::Result<(bool, String), Failure> {
    let mut child = command(executable, cwd).args(args).spawn().map_err(|e| {
        Failure::new(
            if e.kind() == std::io::ErrorKind::NotFound {
                "not_installed"
            } else {
                "process_failed"
            },
            e.to_string(),
        )
    })?;
    let _group = Group(child.id().unwrap() as i32);
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let capture = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut stdout = stdout.take(65536);
        let mut stderr = stderr.take(65536);
        let (_, _, status) = tokio::try_join!(
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err),
            child.wait()
        )?;
        Ok::<_, std::io::Error>((
            status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out),
                String::from_utf8_lossy(&err)
            ),
        ))
    };
    match timeout_at(deadline, capture).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(Failure::new("process_failed", e.to_string())),
        Err(_) => {
            let _ = child.kill().await;
            Err(Failure::new("timeout", "Codex preflight timed out"))
        }
    }
}

fn config(c: &mut Command, key: &str, value: toml::Value) {
    c.arg("-c").arg(format!("{key}={value}"));
}
fn string(s: impl Into<String>) -> toml::Value {
    toml::Value::String(s.into())
}

fn build_command(store: &Store, lease: &Lease, options: &Options) -> Result<Command, Failure> {
    let native = crate::native_tasks::roots(&store.conn, &lease.task_id)
        .map_err(|e| Failure::new("storage_error", e.to_string()))?;
    if native.is_some() {
        crate::native_tasks::context(&store.conn, &lease.task_id)
            .map_err(|e| Failure::new("authorization_changed", e.to_string()))?;
    }
    let bound_cwd = crate::native_tasks::working_directory(&store.conn, &lease.task_id)
        .map_err(|e| Failure::new("working_directory", e.to_string()))?;
    let cwd = bound_cwd.as_ref().unwrap_or(&options.workdir);
    let mut c = command(&options.codex, cwd);
    c.arg("exec");
    if let Some(id) = &lease.resume_id {
        c.arg("resume").arg(id);
    }
    c.args([
        "--json",
        "--ignore-user-config",
        "--ignore-rules",
        "--skip-git-repo-check",
    ]);
    c.env("NO_COLOR", "1");
    config(&mut c, "approval_policy", string("never"));
    let access = crate::task_workspace::for_execution(&store.conn, &lease.task_id)
        .map_err(|e| Failure::new("authorization_changed", e.to_string()))?;
    if let Some(roots) = &native {
        c.arg("--strict-config");
        let profile = if access.is_some() {
            "xxassxx_execute"
        } else {
            "xxassxx_read"
        };
        config(&mut c, "default_permissions", string(profile));
        let mut fs = toml::map::Map::new();
        fs.insert(":minimal".into(), string("read"));
        for root in roots {
            fs.insert(root.to_string_lossy().into_owned(), string("read"));
        }
        if let Some(access) = &access {
            fs.insert(
                access.directory.to_string_lossy().into_owned(),
                string("write"),
            );
        }
        // NVM installs the sandbox helper outside the OS's minimal runtime paths.
        // Permit only its package, never the home directory containing it.
        if let Ok(exe) = options.codex.canonicalize() {
            if let Some(package) = exe.parent().and_then(Path::parent) {
                if package.file_name().is_some_and(|n| n == "codex")
                    && package.join("package.json").is_file()
                {
                    fs.insert(package.to_string_lossy().into_owned(), string("read"));
                }
            }
        }
        // These remain private even if the owner selected a broad source root.
        // Deny the actual private files, not the whole parent: an explicitly
        // selected working directory may be a sibling below that parent. A broad
        // parent denial also prevents Codex from resolving its authorized cwd.
        for path in [store.path.clone(), store.content_root()] {
            fs.insert(path.to_string_lossy().into_owned(), string("deny"));
        }
        for suffix in ["-wal", "-shm", ".service", ".run-locks"] {
            fs.insert(
                format!("{}{suffix}", store.path.to_string_lossy()),
                string("deny"),
            );
        }
        if let Ok(cfg) = store.member_config() {
            if let Some(path) = cfg.secrets_file {
                fs.insert(path.to_string_lossy().into_owned(), string("deny"));
            }
            if let Some(path) = cfg.model["secrets_file"].as_str() {
                fs.insert(path.into(), string("deny"));
            }
        }
        for var in ["HOME", "CODEX_HOME"] {
            if let Some(path) = std::env::var_os(var) {
                let path = PathBuf::from(path);
                if var == "HOME" {
                    for name in [".ssh", ".codex", ".config/xxassxx"] {
                        fs.insert(
                            path.join(name).to_string_lossy().into_owned(),
                            string("deny"),
                        );
                    }
                } else {
                    fs.insert(path.to_string_lossy().into_owned(), string("deny"));
                }
            }
        }
        remove_redundant_denials(&mut fs);
        config(
            &mut c,
            &format!("permissions.{profile}.filesystem"),
            toml::Value::Table(fs),
        );
        config(
            &mut c,
            &format!("permissions.{profile}.network.enabled"),
            false.into(),
        );
        config(&mut c, "shell_environment_policy.inherit", string("none"));
        let mut env = toml::map::Map::from_iter([
            (
                "PATH".into(),
                string("/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin"),
            ),
            ("LC_ALL".into(), string("C")),
        ]);
        if let Some(access) = &access {
            for name in ["TMPDIR", "TMP", "TEMP", "XDG_CACHE_HOME", "MPLCONFIGDIR"] {
                env.insert(
                    name.into(),
                    string(access.directory.join(".tmp").to_string_lossy()),
                );
            }
            env.insert("PYTHONDONTWRITEBYTECODE".into(), string("1"));
            for name in ["OMP_NUM_THREADS", "OPENBLAS_NUM_THREADS", "MKL_NUM_THREADS"] {
                env.insert(name.into(), string("1"));
            }
        }
        config(
            &mut c,
            "shell_environment_policy.set",
            toml::Value::Table(env),
        );
    } else {
        config(&mut c, "sandbox_mode", string("read-only"));
    }
    // Automatic tasks receive explicit versioned instructions via get_task.
    // Do not load mutable project AGENTS files outside the task's read scope.
    // Codex >=0.149 reads these through its filesystem sandbox at startup.
    config(&mut c, "project_doc_max_bytes", 0.into());
    config(&mut c, "features.shell_tool", native.is_some().into());
    config(&mut c, "features.unified_exec", false.into());
    config(&mut c, "features.multi_agent", false.into());
    config(&mut c, "web_search", string("disabled"));
    config(
        &mut c,
        "mcp_servers.xxassxx.command",
        string(options.mcp_executable.to_string_lossy()),
    );
    let args = vec![
        "--db".to_owned(),
        store.path.to_string_lossy().into_owned(),
        "mcp-serve".into(),
        "--task-id".into(),
        lease.task_id.clone(),
        "--run-id".into(),
        lease.run_id.clone(),
    ];
    config(
        &mut c,
        "mcp_servers.xxassxx.args",
        toml::Value::Array(args.into_iter().map(string).collect()),
    );
    config(
        &mut c,
        "mcp_servers.xxassxx.cwd",
        string(cwd.to_string_lossy()),
    );
    config(
        &mut c,
        "mcp_servers.xxassxx.env_vars",
        toml::Value::Array(vec![string("XXASSXX_RUN_TOKEN")]),
    );
    config(&mut c, "mcp_servers.xxassxx.required", true.into());
    config(&mut c, "mcp_servers.xxassxx.startup_timeout_sec", 10.into());
    config(&mut c, "mcp_servers.xxassxx.tool_timeout_sec", 20.into());
    config(
        &mut c,
        "mcp_servers.xxassxx.default_tools_approval_mode",
        string("approve"),
    );
    // A peer's delegated request receives only its authorized task context.
    // The owner-local server may read private metadata and other conversations,
    // so it is available only to tasks submitted locally by the owner.
    let delegated: bool = store
        .conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM delegations WHERE task_id=?1)",
            [&lease.task_id],
            |r| r.get(0),
        )
        .map_err(|e| Failure::new("storage_error", e.to_string()))?;
    if let Ok(member) = store.member_config() {
        if !delegated
            && native.is_none()
            && crate::workflow::for_task(&store.conn, &lease.task_id)
                .map_err(|e| Failure::new("storage_error", e.to_string()))?
                .is_none()
        {
            config(
                &mut c,
                "mcp_servers.xxassxx_butler.command",
                string(options.mcp_executable.to_string_lossy()),
            );
            config(
                &mut c,
                "mcp_servers.xxassxx_butler.args",
                toml::Value::Array(vec![
                    string("--db"),
                    string(store.path.to_string_lossy()),
                    string("butler"),
                    string("mcp-serve"),
                ]),
            );
            config(
                &mut c,
                "mcp_servers.xxassxx_butler.cwd",
                string(options.workdir.to_string_lossy()),
            );
            config(
                &mut c,
                "mcp_servers.xxassxx_butler.env_vars",
                toml::Value::Array(vec![string(member.credential_env)]),
            );
            config(&mut c, "mcp_servers.xxassxx_butler.required", true.into());
            config(
                &mut c,
                "mcp_servers.xxassxx_butler.default_tools_approval_mode",
                string("approve"),
            );
        } else {
            c.env_remove(&member.credential_env);
        }
        if let Ok(model) = crate::model::ModelConfig::parse(&member.model) {
            if !model.api_key_env.is_empty() {
                c.env_remove(&model.api_key_env);
            }
        }
    }
    if let Some(model) = &options.model {
        c.arg("--model").arg(model);
    }
    c.env("XXASSXX_RUN_TOKEN", &lease.token)
        .arg("-")
        .stdin(Stdio::piped());
    Ok(c)
}

// Linux bwrap cannot materialize a denied child inside an already masked parent.
// The parent already protects that child, unless an intervening read grant reopens
// a subtree. Preserve those necessary child denials and all existing read bounds.
fn remove_redundant_denials(fs: &mut toml::map::Map<String, toml::Value>) {
    let redundant: Vec<_> = fs
        .iter()
        .filter_map(|(child, access)| {
            if access.as_str() != Some("deny") || !Path::new(child).is_absolute() {
                return None;
            }
            let covered = fs.iter().any(|(parent, rule)| {
                parent != child
                    && rule.as_str() == Some("deny")
                    && Path::new(parent).is_absolute()
                    && Path::new(child).starts_with(parent)
                    && !fs.iter().any(|(grant, rule)| {
                        matches!(rule.as_str(), Some("read" | "write"))
                            && Path::new(grant).starts_with(parent)
                            && Path::new(child).starts_with(grant)
                    })
            });
            covered.then(|| child.clone())
        })
        .collect();
    for path in redundant {
        fs.remove(&path);
    }
}

async fn execute(
    store: &mut Store,
    lease: &Lease,
    options: &Options,
    deadline: Instant,
) -> std::result::Result<Option<i32>, Failure> {
    let (ok, version) = probe(&options.codex, &options.workdir, &["--version"], deadline).await?;
    if !ok {
        return Err(Failure::new("process_failed", "codex --version failed"));
    }
    let version = version
        .lines()
        .find(|l| l.starts_with("codex-cli "))
        .unwrap_or(version.trim());
    store
        .version(&lease.run_id, version)
        .map_err(|e| Failure::new("storage_error", e.to_string()))?;
    let (logged_in, status) = probe(
        &options.codex,
        &options.workdir,
        &["login", "status"],
        deadline,
    )
    .await?;
    if !logged_in {
        let code = classify(&status);
        return Err(Failure::new(
            code,
            if code == "not_logged_in" {
                "Official Codex login is unavailable; run codex login"
            } else {
                "codex login status failed; inspect the official CLI"
            },
        ));
    }
    let mut child = build_command(store, lease, options)?
        .spawn()
        .map_err(|e| Failure::new("process_failed", e.to_string()))?;
    let _group = Group(child.id().unwrap() as i32);
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    // Task content is deliberately absent: the model must fetch it through MCP.
    let prompt = format!(
        "xXASSXx execution event. Assigned task_id: {}. Current run_id: {}.\nCall the xxassxx MCP get_task tool with that task_id, follow its input as the task data, then call submit_task_result with that task_id, the current run_id returned by get_task, a stable idempotency_key (use the current run_id), and a nonempty result string. On retries use exactly the same key and result. This applies also after resuming: call get_task again and submit for the NEW current run_id. Do not use shell, files, or other tools to access the task database. Do not claim completion without an accepted MCP submission. Finish your turn after submission.\n",
        lease.task_id, lease.run_id
    );
    let prompt = if crate::task_coordinator::check_execution(&store.conn, &lease.task_id)
        .map_err(|e| Failure::new("business_task", e.to_string()))?
        .is_some()
    {
        format!(
            "xXASSXx business task execution. Assigned task_id: {}. Current run_id: {}. Call get_task for the current confirmed revision and physical scope. Follow execution_policy strictly. Use Codex native tools within authorized_roots. Follow business_task.phase: discover returns candidate paths for Rust to freeze; analyze reads only immutable materials; execute may write and run only in execution_access.directory, with sources/dependencies read-only. If necessary information is missing, use submit_task_question with body, reason, known and these task/run IDs and idempotency_key equal to run_id. Otherwise use submit_task_result following the result_contract. End this turn after one accepted submission. A question, MCP acceptance, exit 0, and turn.completed do not complete the business task. On resume read get_task again for answers. Never access the task database. Project programs may run only in phase execute under its explicit local grant.\n",
            lease.task_id, lease.run_id
        )
    } else if crate::workflow::for_task(&store.conn, &lease.task_id)
        .map_err(|e| Failure::new("storage_error", e.to_string()))?
        .is_some()
    {
        format!(
            "xXASSXx collaboration execution event. Assigned task_id: {}. Current run_id: {}. Call get_task, read its task-scoped context, use read_task_file for the authorized actual file content, and submit exactly one submit_collaboration_turn with these task/run IDs and idempotency_key equal to this run ID. Follow the structured action guidance in get_task. This is one turn, not necessarily the whole collaboration; after asking or answering a clarification, end the turn and let the persisted scheduler resume you when appropriate. On resume call get_task again for the new event and run ID. No shell, direct filesystem access, unrelated contacts or database access. After an accepted submission end your turn.\n",
            lease.task_id, lease.run_id
        )
    } else {
        prompt
    };
    let operation = async {
        stdin.write_all(prompt.as_bytes()).await?;
        stdin.shutdown().await?;
        drop(stdin);
        let events = async {
            let mut reader = BufReader::new(stdout);
            let mut failure = None;
            let mut terminal_failure = false;
            let mut count = 0;
            let mut bytes = 0;
            loop {
                let mut line = String::new();
                let n = (&mut reader)
                    .take(MAX_FRAME + 1)
                    .read_line(&mut line)
                    .await?;
                if n == 0 {
                    break;
                }
                ensure!(n as u64 <= MAX_FRAME, "Codex event exceeds size limit");
                count += 1;
                ensure!(count <= 10000, "too many Codex events");
                bytes += n;
                ensure!(
                    bytes <= 16 * 1024 * 1024,
                    "Codex event stream exceeds 16 MiB"
                );
                let event: Value =
                    serde_json::from_str(&line).context("malformed Codex JSON event")?;
                ensure!(event["type"].is_string(), "Codex event lacks a type");
                // The run capability must not enter persistent event/error logs.
                let event: Value =
                    serde_json::from_str(&event.to_string().replace(&lease.token, "[redacted]"))?;
                store.event(lease, &event)?;
                if event["type"] == "turn.failed" {
                    failure = Some(event.to_string());
                    terminal_failure = true;
                }
                // `error` can be a transient reconnect; a later completed turn may recover.
                if event["type"] == "error" && failure.is_none() {
                    failure = Some(event.to_string());
                }
                if event["type"] == "turn.completed" && !terminal_failure {
                    failure = None;
                }
            }
            Ok::<_, anyhow::Error>(failure)
        };
        let errors = async {
            let mut reader = BufReader::new(stderr);
            let mut tail = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = reader.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                tail.extend_from_slice(&buf[..n]);
                if tail.len() > 32768 {
                    tail.drain(..tail.len() - 32768);
                }
            }
            Ok::<_, anyhow::Error>(String::from_utf8_lossy(&tail).into_owned())
        };
        let (events, stderr, status) = tokio::try_join!(events, errors, async {
            Ok::<_, anyhow::Error>(child.wait().await?)
        })?;
        Ok::<_, anyhow::Error>((events, stderr, status.code()))
    };
    match timeout_at(deadline, operation).await {
        Err(_) => {
            let _ = child.kill().await;
            Err(Failure::new(
                "timeout",
                format!(
                    "Codex exceeded {} seconds; execution capability revoked",
                    options.timeout_secs
                ),
            ))
        }
        Ok(Err(e)) => {
            let _ = child.kill().await;
            let code = if e.to_string().contains("resumed a different session ID") {
                "session_mismatch"
            } else {
                "protocol_error"
            };
            Err(Failure::new(code, e.to_string()))
        }
        Ok(Ok((event_error, stderr, exit))) => {
            if event_error.is_some() || exit != Some(0) {
                let message = format!("{}\n{}", event_error.unwrap_or_default(), stderr)
                    .replace(&lease.token, "[redacted]");
                let mut failure = Failure::new(classify(&message), message);
                failure.exit_code = exit;
                Err(failure)
            } else {
                Ok(exit)
            }
        }
    }
}

async fn watch_authorization(store: &Store, task: &str) -> Failure {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if let Err(e) = crate::native_tasks::context(&store.conn, task) {
            return Failure::new("authorization_changed", e.to_string());
        }
    }
}

/// Run inside the internal worker, whose stdin is the invoking CLI's liveness pipe.
pub async fn run(store: &mut Store, task: &str, mut options: Options) -> Result<()> {
    ensure!(
        cfg!(unix),
        "phase 1 process cleanup currently supports macOS/Linux only"
    );
    if let Some(path) = crate::native_tasks::working_directory(&store.conn, task)? {
        options.workdir = path;
    }
    options.workdir = options
        .workdir
        .canonicalize()
        .context("workdir does not exist")?;
    ensure!(options.workdir.is_dir(), "workdir must be a directory");
    options.mcp_executable = options
        .mcp_executable
        .canonicalize()
        .context("MCP executable not found")?;
    if options.codex.components().count() > 1 {
        options.codex = std::path::absolute(&options.codex)?;
    }
    let _task_lock = crate::supervisor::TaskLock::acquire(&store.path, task)?;
    let parent_closed = crate::supervisor::parent_closed()?;
    let session = store.task(task)?.session_id;
    let _session_lock = session
        .as_deref()
        .map(|id| {
            crate::supervisor::TaskLock::acquire(
                &store.path,
                &crate::team::stable_id("codex-session", id),
            )
        })
        .transpose()?;
    let lease = store.claim(task, options.resume, &options.workdir, options.timeout_secs)?;
    let deadline = Instant::now() + Duration::from_secs(options.timeout_secs);
    eprintln!(
        "task={} run={} mode={}",
        task,
        lease.run_id,
        if options.resume {
            "resume-by-id"
        } else {
            "new-session"
        }
    );
    let watcher = Store::open(&store.path)?;
    let result = tokio::select! {
        biased;
        _=parent_closed=>Err(Failure::new("interrupted","runner disconnected; stopping execution")),
        _=tokio::signal::ctrl_c()=>Err(Failure::new("interrupted","runner interrupted")),
        failure=watch_authorization(&watcher,task)=>Err(failure),
        result=execute(store,&lease,&options,deadline)=>result,
    };
    match result {
        Ok(exit) => store.finish(&lease, exit, None),
        Err(failure) => store.finish(
            &lease,
            failure.exit_code,
            Some((failure.code, &failure.message)),
        ),
    }
}

#[cfg(test)]
mod permission_tests {
    use super::*;

    #[test]
    fn nested_denials_preserve_private_bounds_and_reopened_subtrees() {
        let mut fs: toml::map::Map<String, toml::Value> = [
            (":minimal", "read"),
            ("/work", "read"),
            ("/work/private", "deny"),
            ("/work/private/secrets.env", "deny"),
            ("/work/private/materials", "read"),
            ("/work/private/materials/credential", "deny"),
            ("/work/private-other/token", "deny"),
        ]
        .into_iter()
        .map(|(p, a)| (p.into(), string(a)))
        .collect();
        remove_redundant_denials(&mut fs);
        assert!(!fs.contains_key("/work/private/secrets.env"));
        assert_eq!(fs["/work/private"].as_str(), Some("deny"));
        assert_eq!(
            fs["/work/private/materials/credential"].as_str(),
            Some("deny")
        );
        assert_eq!(fs["/work/private-other/token"].as_str(), Some("deny"));
        assert_eq!(fs["/work"].as_str(), Some("read"));
        assert_eq!(fs["/work/private/materials"].as_str(), Some("read"));
    }
}
