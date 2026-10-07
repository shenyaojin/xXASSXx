//! Collect intent and let official Codex execute with its own tools. No file index/search engine.
use crate::{
    app, file_roots,
    store::{Store, now},
    team::stable_id,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};

pub fn roots(conn: &Connection, task: &str) -> Result<Option<Vec<PathBuf>>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT roots FROM native_tasks WHERE task_id=?1",
            [task],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|raw| Ok(serde_json::from_str(&raw)?)).transpose()
}

pub fn available_roots(store: &Store) -> Result<Vec<PathBuf>> {
    let roots = store
        .file_roots()?
        .into_iter()
        .filter(|p| file_roots::directory(p).is_ok_and(|c| c == *p))
        .collect::<Vec<_>>();
    ensure!(
        !roots.is_empty(),
        "尚无可用白名单目录。请先在本机用 Ctrl+R 添加目录，或运行 xxassxx client roots add PATH，然后重新提问。"
    );
    Ok(roots)
}

fn check_roots(conn: &Connection, roots: &[PathBuf]) -> Result<()> {
    ensure!(!roots.is_empty(), "no authorized roots");
    for root in roots {
        let allowed: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM file_roots WHERE path=?1)",
            [root.to_str()],
            |r| r.get(0),
        )?;
        ensure!(
            allowed && file_roots::directory(root).is_ok_and(|p| p == *root),
            "白名单已改变或目录不可用；请重新发起任务"
        );
    }
    Ok(())
}

pub fn context(conn: &Connection, task: &str) -> Result<Option<Value>> {
    let Some(roots) = roots(conn, task)? else {
        return Ok(None);
    };
    check_roots(conn, &roots)?;
    let original: String =
        conn.query_row("SELECT input FROM tasks WHERE id=?1", [task], |r| r.get(0))?;
    Ok(Some(json!({
        "intent": serde_json::from_str::<Value>(&original).unwrap_or(json!(original)),
        "authorized_roots": roots.iter().enumerate().map(|(n,p)| json!({"root":n,"path":p})).collect::<Vec<_>>(),
        "execution_policy": "Use your native Codex shell/read/search tools to fulfill the original user intent. This is a read-only task: search, inspect and explain actual files within authorized_roots. Do not edit files, run project programs, install software, or contact other members. Do not access credentials or the xXASSXx database. Use non-login shell commands. For file questions inspect the actual filesystem; do not require an object ID or a pre-imported snapshot. Preserve the user's language. If unsupported or insufficient, state the specific limitation. Do not invent matches or claim you searched without using tools. Return relevant file references and brief reasons, not bulk file contents. Root numbers below refer to authorized_roots, not arbitrary paths.",
        "result_contract": {"tool":"submit_task_result", "result":"A JSON-encoded STRING: {\"body\":\"answer in the user's language\",\"files\":[{\"root\":0,\"path\":\"relative/path\",\"reason\":\"why relevant\"}]}. At most 40 files; body <=12000 bytes; each reason <=1000 bytes. files may be empty for non-file answers or no matches. Clearly state partial searches or failures. Never include local absolute paths in body/path/reason."}
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    body: String,
    files: Vec<Reference>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    root: usize,
    path: String,
    reason: String,
}

pub fn validate(conn: &Connection, task: &str, result: &str) -> Result<()> {
    let Some(roots) = roots(conn, task)? else {
        return Ok(());
    };
    check_roots(conn, &roots)?;
    ensure!(result.len() <= 24000, "native result exceeds reply limit");
    let out: Output = serde_json::from_str(result)
        .context("result must be a JSON string containing body and files")?;
    ensure!(
        !out.body.trim().is_empty() && out.body.len() <= 12000 && out.files.len() <= 40,
        "invalid result size"
    );
    for root in &roots {
        ensure!(
            !result.contains(root.to_str().unwrap()),
            "return relative references, not local absolute paths"
        );
    }
    for f in &out.files {
        let root = roots.get(f.root).context("unknown authorized root")?;
        let path = Path::new(&f.path);
        ensure!(
            !f.path.is_empty()
                && f.path.len() <= 2048
                && !f.reason.is_empty()
                && f.reason.len() <= 1000
                && path.components().all(|c| matches!(c, Component::Normal(_))),
            "invalid relative file reference"
        );
        let actual = root
            .join(path)
            .canonicalize()
            .context("returned file does not exist")?;
        ensure!(
            actual.starts_with(root) && actual.is_file(),
            "returned reference is outside its root or is not a file"
        );
    }
    Ok(())
}

pub fn render(conn: &Connection, task: &str, result: &str) -> Result<String> {
    validate(conn, task, result)?;
    let roots = roots(conn, task)?.context("not a native task")?;
    let out: Output = serde_json::from_str(result)?;
    let mut text = out.body;
    for file in out.files {
        let label = roots[file.root]
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        text.push_str(&format!("\n\n• [{label}] {}\n  {}", file.path, file.reason));
    }
    Ok(text)
}

pub fn create_local(store: &mut Store, i: &app::Instruction) -> Result<Value> {
    ensure!(
        i.recipient == store.owner()?,
        "only the owner can delegate a local intent"
    );
    let task = stable_id(&i.request_id, "native-codex");
    if roots(&store.conn, &task)?.is_some() {
        return Ok(json!({"task_id":task,"state":store.task(&task)?.state}));
    }
    let roots = available_roots(store)?;
    let mut history = app::history(store, &i.session_id)?;
    if history.len() > 12 {
        history.drain(..history.len() - 12);
    }
    let history = history.into_iter().map(|v| json!({"sender":v["sender"],"body":v["body"].as_str().unwrap_or("").chars().take(4000).collect::<String>()})).collect::<Vec<_>>();
    let input = json!({"request":i.body,"context":history}).to_string();
    let tx = store.conn.transaction()?;
    tx.execute(
        "INSERT INTO tasks(id,input,created_at) VALUES(?1,?2,?3)",
        params![task, input, now()],
    )?;
    tx.execute(
        "INSERT INTO native_tasks(task_id,roots,command_id) VALUES(?1,?2,?3)",
        params![task, serde_json::to_string(&roots)?, i.request_id],
    )?;
    tx.commit()?;
    Ok(json!({"task_id":task,"state":"pending"}))
}

/// Same supervised CLI path as peer delegation, with durable task/run records.
pub async fn execute(store: &Store, task: &str, exe: &Path) -> Result<()> {
    let cfg = store.member_config()?.executor;
    let t = store.task(task)?;
    if t.state == "succeeded" {
        return Ok(());
    }
    let mut command = tokio::process::Command::new(exe);
    command
        .arg("--db")
        .arg(&store.path)
        .arg("run-once")
        .arg(task)
        .arg("--workdir")
        .arg(&cfg.workdir)
        .arg("--codex")
        .arg(&cfg.codex)
        .arg("--timeout-secs")
        .arg(cfg.timeout_secs.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if t.session_id.is_some() {
        command.arg("--resume");
    }
    if let Some(model) = cfg.model {
        command.arg("--model").arg(model);
    }
    let status = command.spawn()?.wait().await?;
    ensure!(
        status.success() && store.task(task)?.state == "succeeded",
        "Codex 未完成任务，请检查登录状态或执行记录"
    );
    Ok(())
}

pub async fn run_local(store: &mut Store, exe: &Path) -> Result<()> {
    if store.member_config()?.executor.mode != "auto" {
        return Ok(());
    }
    let ids = {
        let mut q = store.conn.prepare("SELECT n.task_id,c.payload FROM native_tasks n JOIN app_commands c ON c.id=n.command_id WHERE n.reported=0 ORDER BY c.created_at LIMIT 1")?;
        q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (task, raw) in ids {
        let _lock = crate::supervisor::TaskLock::acquire(
            &store.path,
            &stable_id(&task, "native-dispatch"),
        )?;
        let i: app::Instruction = serde_json::from_str(&raw)?;
        app::respond(store, &i, "native_running", "Codex 正在处理这项只读任务。")?;
        let result = execute(store, &task, exe).await.and_then(|_| {
            render(
                &store.conn,
                &task,
                store
                    .task(&task)?
                    .result
                    .as_deref()
                    .context("missing result")?,
            )
        });
        let (kind, body) = match result {
            Ok(body) => ("native_result", body),
            Err(e) => (
                "native_failure",
                format!("本次任务未完成：{e}。修复后可重新提问。"),
            ),
        };
        let tx = store.conn.transaction()?;
        app::add_message(
            &tx,
            &stable_id(&task, "native-result"),
            &i.session_id,
            None,
            "butler",
            &i.recipient,
            kind,
            &body,
            Some(&i.request_id),
            None,
        )?;
        tx.execute(
            "UPDATE native_tasks SET reported=1 WHERE task_id=?1",
            [&task],
        )?;
        tx.commit()?;
    }
    Ok(())
}
