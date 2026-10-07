//! A thin second adapter over exactly the same application API used by the TUI.
use crate::{app, store::Store};
use anyhow::{Context, Result};
use clap::Subcommand;
use serde_json::{Value, json};
use std::{io::Read, path::PathBuf};
#[derive(Subcommand)]
pub enum Command {
    /// Open/reuse a persistent project conversation; no model calls.
    Session {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        recipient: Option<String>,
    },
    /// Create an independent local conversation in a working directory.
    NewSession {
        #[arg(long)]
        project: PathBuf,
    },
    /// Advance a queued business task job/execution once; normally handled by the daemon.
    TaskTick,
    /// Explicitly release one inspected pre-upgrade execution from its migration hold.
    RecoverLegacy {
        kind: String,
        id: String,
    },
    /// Inspect a business task, revisions, questions, sources and execution evidence.
    Task {
        id: String,
    },
    /// View or set bounded coordinator/material limits from a JSON file.
    TaskLimits {
        #[arg(long)]
        input: Option<PathBuf>,
    },
    /// Submit a structured instruction from stdin or a UTF-8 JSON file. Local identity is authoritative.
    Submit {
        #[arg(long, default_value = "-")]
        input: String,
    },
    Snapshot {
        session: String,
    },
    Events {
        session: String,
        #[arg(long, default_value_t = 0)]
        after: i64,
    },
    Read {
        session: String,
        through: i64,
    },
    Command {
        id: String,
    },
    /// Process one queued owner command; normally the daemon does this.
    Process,
}
pub async fn execute(store: &mut Store, command: Command) -> Result<Value> {
    match command {
        Command::RecoverLegacy { kind, id } => {
            anyhow::ensure!(
                matches!(kind.as_str(), "native" | "workflow"),
                "kind must be native or workflow"
            );
            if kind == "workflow" {
                store.workflow_retry(&id)?;
                store.conn.execute("DELETE FROM legacy_holds WHERE kind='app' AND id IN (SELECT id FROM app_tasks WHERE workflow_id=?1)",[&id])?;
            }
            let changed = store.conn.execute(
                "DELETE FROM legacy_holds WHERE kind=?1 AND id=?2",
                rusqlite::params![kind, id],
            )?;
            if kind == "native" && changed == 1 {
                store.conn.execute("UPDATE messages SET state='delegated',error=NULL WHERE id IN (SELECT message_id FROM delegations WHERE task_id=?1) AND state='needs_attention'",[&id])?;
            }
            Ok(json!({"released":changed==1,"id":id,"kind":kind}))
        }
        Command::NewSession { project } => {
            let p = app::project(store, &project)?;
            Ok(
                json!({"session_id":app::new_session(store,p["id"].as_str().unwrap())?,"working_directory":p}),
            )
        }
        Command::TaskTick => {
            crate::app::bridge::reconcile(store)?;
            crate::task_coordinator::tick(store, &std::env::current_exe()?).await?;
            Ok(json!({"processed":true}))
        }
        Command::Task { id } => crate::app::tasks::get(store, &id),
        Command::TaskLimits { input } => crate::task_coordinator::limits(store, input.as_deref()),
        Command::Session { project, recipient } => {
            let p = app::project(store, &project)?;
            let to = recipient.unwrap_or(store.owner()?);
            let id = app::session(store, p["id"].as_str().unwrap(), &to)?;
            Ok(json!({"session_id":id,"project":p,"recipient":to}))
        }
        Command::Submit { input } => {
            let raw = if input == "-" {
                let mut s = String::new();
                std::io::stdin().take(65537).read_to_string(&mut s)?;
                s
            } else {
                std::fs::read_to_string(input)?
            };
            let i: app::Instruction =
                serde_json::from_str(&raw).context("invalid instruction JSON")?;
            app::submit(store, &app::Actor::local(store)?, &i)
        }
        Command::Snapshot { session } => app::snapshot(store, &session),
        Command::Events { session, after } => Ok(json!(app::events(store, &session, after)?)),
        Command::Read { session, through } => {
            app::mark_read(store, &session, through)?;
            Ok(json!({"read_through":through}))
        }
        Command::Command { id } => app::command(store, &id),
        Command::Process => {
            let processed = app::model::process_one(store).await?;
            Ok(json!({"processed":processed}))
        }
    }
}
