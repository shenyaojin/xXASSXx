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
