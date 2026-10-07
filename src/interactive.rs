//! Interactive owner session. The foreground terminal belongs to official Codex;
//! the existing per-member butler remains a separate background service.
use crate::{model::ModelConfig, service, setup, store::Store};
use anyhow::{Context, Result, ensure};
use clap::Args;
use serde_json::{Value, json};
use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Args, Default)]
pub struct OpenArgs {
    /// Project to work in. Defaults to the current directory.
    #[arg(default_value = ".")]
    pub project: PathBuf,
    /// Existing personal client data directory (not the project directory).
    #[arg(long)]
    pub directory: Option<PathBuf>,
    /// Also add this project to the import allowlist. Does not import or share files.
    #[arg(long, conflicts_with = "check")]
    pub allow_import: bool,
    /// Inspect launch settings without starting Codex, the service, or model calls.
    #[arg(long)]
    pub check: bool,
}

pub struct Launch {
    pub project: PathBuf,
    pub codex: PathBuf,
    pub args: Vec<String>,
    pub remove_env: Vec<String>,
}

fn executable(path: &Path) -> Result<PathBuf> {
    let found = if path.is_absolute() || path.components().count() > 1 {
        Some(path.to_path_buf())
    } else {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|p| p.join(path))
            .find(|p| is_executable(p))
    };
    let found =
        found.context("找不到 Codex CLI；请安装 Codex，或检查个人端的 executor.codex 配置")?;
    ensure!(is_executable(&found), "Codex 程序不存在或不可执行");
    // Keep the installed shim's directory for PATH (e.g. an NVM node executable).
    Ok(std::path::absolute(found)?)
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub fn plan(store: &Store, project: &Path, binary: &Path) -> Result<Launch> {
    let project = project.canonicalize().context("工作文件夹不存在")?;
    ensure!(project.is_dir(), "请选择文件夹，而不是单个文件");
    ensure!(
        project.to_str().is_some(),
        "non-UTF-8 project path unsupported"
    );
    let cfg = store.member_config()?;
    // Personal installations keep mailbox credentials in a file. Do not pass
    // an environment-only legacy token to an interactive Codex session.
    ensure!(
        cfg.secrets_file.is_some(),
        "交互入口需要个人端的凭据文件；请使用 client init 建立个人端"
    );
    let codex = executable(&cfg.executor.codex)?;
    let mut table = toml::map::Map::new();
    table.insert(
        "command".into(),
        binary.to_string_lossy().into_owned().into(),
    );
    table.insert(
        "args".into(),
        toml::Value::Array(
            [
                "--db".to_owned(),
                store.path.to_string_lossy().into_owned(),
                "butler".into(),
                "mcp-serve".into(),
            ]
            .into_iter()
            .map(toml::Value::String)
            .collect(),
        ),
    );
    table.insert("cwd".into(), project.to_string_lossy().into_owned().into());
    table.insert("env_vars".into(), toml::Value::Array(vec![]));
    table.insert("enabled".into(), true.into());
    table.insert("required".into(), true.into());
    table.insert("startup_timeout_sec".into(), 15.into());
    table.insert("tool_timeout_sec".into(), 30.into());
    let args = vec![
        "--cd".into(),
        project.to_string_lossy().into_owned(),
        "--config".into(),
        format!("mcp_servers.xxassxx_butler={}", toml::Value::Table(table)),
    ];
    let model = ModelConfig::parse(&cfg.model)?;
    let mut remove_env = vec![
        "OPENAI_API_KEY".into(),
        "CODEX_API_KEY".into(),
        "DEEPSEEK_API_KEY".into(),
        "DEEPSEEK_API".into(),
        "XXASSXX_RUN_TOKEN".into(),
        cfg.credential_env,
    ];
    if !model.api_key_env.is_empty() {
        remove_env.push(model.api_key_env);
    }
    Ok(Launch {
        project,
        codex,
        args,
        remove_env,
    })
}

impl Launch {
    pub fn command(&self) -> Result<Command> {
        let mut cmd = Command::new(&self.codex);
        cmd.args(&self.args)
            .current_dir(&self.project)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let inherited_path = std::env::var_os("PATH").unwrap_or_default();
        let paths = std::iter::once(
            self.codex
                .parent()
                .context("invalid Codex path")?
                .to_path_buf(),
        )
        .chain(std::env::split_paths(&inherited_path));
        cmd.env("PATH", std::env::join_paths(paths)?);
        for name in &self.remove_env {
            cmd.env_remove(name);
        }
        Ok(cmd)
    }
}

pub async fn open(args: OpenArgs) -> Result<()> {
    let project = if args.project.as_os_str().is_empty() {
        Path::new(".")
    } else {
        &args.project
    };
    let root = setup::directory(args.directory, "client")?;
    let db = root.join("member.sqlite3");
    ensure!(
        db.is_file(),
        "个人端尚未初始化；先运行 xxassxx client init --help，并使用你的邀请文件加入团队"
    );
    let store = Store::open(&db)?;
    let launch = plan(&store, project, &std::env::current_exe()?)?;
    let identity = store.identity()?;
    if args.check {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "identity":identity,"project":launch.project,"codex":launch.codex,
                "codex_args":launch.args,"database":store.path,"allowed_directories":store.file_roots()?,
                "launches_codex":false,"starts_service":false,
            }))?
        );
        return Ok(());
    }
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "请在交互终端运行 xxassxx 或 xxassxx open PATH；配置检查可用 xxassxx open PATH --check"
    );
    let mut command = launch.command()?;
    let state = service::execute(
        &store,
        service::Command::Start {
            poll_secs: 2,
            max_failures: 3,
            reconnect: true,
        },
    )
    .await?;
    ensure!(
        state["alive"] == true,
        "local agent 未能启动；请检查 client status"
    );
    if args.allow_import {
        store.allow_directory(&launch.project)?;
    }
    eprintln!(
        "xXASSXx · {} (@{})",
        identity["display_name"].as_str().unwrap_or(""),
        identity["member_id"].as_str().unwrap_or("")
    );
    eprintln!("工作文件夹：{}", launch.project.display());
    eprintln!(
        "local agent 已启动；正在打开 Codex，并接入你的 local agent 工具。退出 Codex 后 local agent 继续运行。"
    );
    if args.allow_import {
        eprintln!("已允许从此文件夹导入；尚未导入或共享任何文件。");
    }
    drop(store);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Replace the launcher so Ctrl-C, terminal job control and exit status
        // belong to Codex. The daemon already has its own process group.
        Err(command.exec()).context("无法打开 Codex；后台 local agent 仍在运行")
    }
    #[cfg(not(unix))]
    {
        let _ = command;
        anyhow::bail!("interactive launcher supports macOS/Linux")
    }
}

pub fn context(store: &Store) -> Result<Value> {
    Ok(json!({
        "identity":store.identity()?,"contacts":store.contacts()?,
        "allowed_directories":store.file_roots()?,"service":service::status(store)?,
        "database":store.path,"cli_executable":std::env::current_exe()?,
        "notes":[
            "Contacts are configured members, not live presence.",
            "Edit the user's chosen workspace with the host's normal file tools. The import allowlist does not sandbox Codex.",
            "Opening a project does not import or share it. Only explicitly authorized source directories may be imported.",
            "Send messages only at the user's request. Remote file analysis requires a prepared peer workflow and explicit version/file grants; an ordinary message does not grant file access.",
            "For grant-based workflows use the CLI with this database: collaboration --help. Do not modify SQLite directly."
        ]
    }))
}
