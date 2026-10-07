use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use serde_json::json;
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
};
use xxassxx::{
    codex::{self, Options},
    mcp::{self, Binding},
    store::Store,
};

#[derive(Parser)]
#[command(
    version,
    about = "Local team collaboration: local agents, versioned files and a shared mailbox"
)]
struct Cli {
    /// Database for low-level commands (default: .xxassxx/tasks.sqlite3).
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Open the xXASSXx terminal interface in this project.
    #[arg(short = 'C', long = "cd")]
    project: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Args)]
struct RunArgs {
    task_id: String,
    #[arg(long)]
    workdir: PathBuf,
    #[arg(long, default_value = "codex")]
    codex: PathBuf,
    #[arg(long, default_value_t = 180)]
    timeout_secs: u64,
    #[arg(long)]
    resume: bool,
    #[arg(long)]
    model: Option<String>,
}

#[derive(Subcommand)]
enum Action {
    /// Open the xXASSXx terminal interface in a project.
    Open(xxassxx::interactive::OpenArgs),
    /// Explicitly open official Codex with temporary local agent MCP access.
    Codex(xxassxx::interactive::OpenArgs),
    #[command(flatten)]
    Phase2(xxassxx::cli::Commands),
    /// Initialize the task database (safe to repeat).
    Init,
    /// Submit task text; omit text or pass '-' to read stdin.
    Submit { text: Option<String> },
    /// Show a task, attempts, and optional raw structured Codex events.
    Show {
        task_id: String,
        #[arg(long)]
        events: bool,
    },
    /// List tasks.
    List,
    /// Execute one task; subsequent attempts must resume the saved session ID.
    RunOnce(RunArgs),
    /// Internal worker; stdin is the invoking CLI's liveness pipe.
    #[command(name = "_run-once", hide = true)]
    SupervisedRunOnce(RunArgs),
    /// Start task-scoped MCP on stdio; capability is passed via environment.
    McpServe {
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        run_id: String,
        #[arg(long, env = "XXASSXX_RUN_TOKEN", hide_env_values = true, hide = true)]
        token: String,
    },
}

async fn supervise(database: &Path, args: RunArgs) -> Result<()> {
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .arg("--db")
        .arg(database)
        .arg("_run-once")
        .arg(args.task_id)
        .arg("--workdir")
        .arg(args.workdir)
        .arg("--codex")
        .arg(args.codex)
        .arg("--timeout-secs")
        .arg(args.timeout_secs.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(unix)]
    command.process_group(0);
    if args.resume {
        command.arg("--resume");
    }
    if let Some(model) = args.model {
        command.arg("--model").arg(model);
    }
    // The worker must survive the CLI's death long enough to stop its children.
    // In particular, do not enable kill_on_drop here.
    let mut worker = command
        .spawn()
        .context("cannot start supervised executor")?;
    let keepalive = worker
        .stdin
        .take()
        .context("missing worker liveness pipe")?;
    let status = tokio::select! {
        status = worker.wait() => status?,
        _ = tokio::signal::ctrl_c() => {
            drop(keepalive);
            worker.wait().await?
        }
    };
    anyhow::ensure!(status.success(), "supervised execution failed ({status})");
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(e) = execute(Cli::parse()).await {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}

async fn execute(cli: Cli) -> Result<()> {
    let action = match cli.command {
        None => {
            anyhow::ensure!(
                cli.db.is_none(),
                "交互入口使用个人端数据库；可用 xxassxx open PATH --directory CLIENT_DIRECTORY"
            );
            return xxassxx::tui::open(xxassxx::interactive::OpenArgs {
                project: cli.project.unwrap_or_else(|| PathBuf::from(".")),
                ..Default::default()
            })
            .await;
        }
        Some(action) => action,
    };
    anyhow::ensure!(
        cli.project.is_none(),
        "-C 用于直接打开项目；带子命令时请使用 xxassxx open PATH"
    );
    if let Action::Open(args) = action {
        anyhow::ensure!(
            cli.db.is_none(),
            "open 使用个人端数据库；请使用 --directory 指定个人端"
        );
        return xxassxx::tui::open(args).await;
    }
    if let Action::Codex(args) = action {
        anyhow::ensure!(
            cli.db.is_none(),
            "codex 使用个人端数据库；请使用 --directory"
        );
        return xxassxx::interactive::open(args).await;
    }
    let db = cli
        .db
        .unwrap_or_else(|| PathBuf::from(".xxassxx/tasks.sqlite3"));
    if let Action::Phase2(command) = action {
        return xxassxx::cli::execute(&db, command).await;
    }
    if matches!(action, Action::Init) {
        let store = Store::init(&db)?;
        println!("{}", json!({"database":store.path,"schema_version":6}));
        return Ok(());
    }
    let mut store = Store::open(&db)?;
    let output = match action {
        Action::Open(_) | Action::Codex(_) | Action::Phase2(_) => unreachable!(),
        Action::Init => unreachable!(),
        Action::Submit { text } => {
            let text = match text {
                Some(s) if s != "-" => s,
                _ => {
                    let mut s = String::new();
                    std::io::stdin().take(262145).read_to_string(&mut s)?;
                    s
                }
            };
            serde_json::to_value(store.submit(&text)?)?
        }
        Action::Show { task_id, events } => {
            let task = store.task(&task_id)?;
            let runs = store.runs(&task_id)?;
            let mut output = json!({"task":task,"runs":runs});
            if events {
                output["events"] = serde_json::to_value(
                    runs.iter()
                        .map(|r| Ok((r.id.clone(), store.events(&r.id)?)))
                        .collect::<Result<std::collections::BTreeMap<_, _>>>()?,
                )?;
            }
            output
        }
        Action::List => serde_json::to_value(store.list()?)?,
        Action::RunOnce(args) => return supervise(&store.path, args).await,
        Action::SupervisedRunOnce(RunArgs {
            task_id,
            workdir,
            codex,
            timeout_secs,
            resume,
            model,
        }) => {
            let result = codex::run(
                &mut store,
                &task_id,
                Options {
                    codex,
                    workdir,
                    timeout_secs,
                    resume,
                    model,
                    mcp_executable: std::env::current_exe()
                        .context("cannot locate own MCP executable")?,
                },
            )
            .await;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"task":store.task(&task_id)?,"runs":store.runs(&task_id)?})
                )?
            );
            return result;
        }
        Action::McpServe {
            task_id,
            run_id,
            token,
        } => {
            return mcp::serve(
                store,
                Binding {
                    task: task_id,
                    run: run_id,
                    token,
                },
            )
            .await;
        }
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
