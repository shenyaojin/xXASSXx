use crate::{
    store::Store,
    workflow::{self, Limits},
};
use anyhow::{Result, ensure};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::{io::Read, path::PathBuf};
#[derive(Args)]
pub struct Input {
    /// Task purpose; use '-' to read UTF-8 text from stdin.
    #[arg(long)]
    goal: String,
    /// Exact immutable VERSION_ID:relative/path; repeat for each permitted file.
    #[arg(long, required = true)]
    grant: Vec<String>,
    /// Optional JSON object overriding the bounded collaboration limits.
    #[arg(long)]
    limits: Option<PathBuf>,
}
impl Input {
    fn load(self) -> Result<(String, Vec<String>, Limits)> {
        let mut goal = self.goal;
        if goal == "-" {
            goal.clear();
            std::io::stdin().take(16385).read_to_string(&mut goal)?;
        }
        let limits = self
            .limits
            .map(|p| -> Result<Limits> { Ok(serde_json::from_slice(&std::fs::read(p)?)?) })
            .transpose()?
            .unwrap_or_default();
        Ok((goal, self.grant, limits))
    }
}
#[derive(Subcommand)]
pub enum Command {
    /// Preauthorize one incoming task from this peer and these exact local files.
    Prepare {
        #[arg(long)]
        peer: String,
        #[command(flatten)]
        input: Input,
    },
    /// Start an owner task using the peer's preauthorized workflow ID.
    Start {
        #[arg(long)]
        peer: String,
        #[arg(long)]
        remote_workflow: String,
        #[command(flatten)]
        input: Input,
    },
    /// Run a bounded local file task without any peer communication capability.
    Local {
        #[command(flatten)]
        input: Input,
    },
    /// Show state, queue, original messages, exact file reads, model and Codex runs.
    Show {
        id: String,
    },
    List,
    /// Dispatch at most one pending event. Normal automation uses butler run.
    Run {
        id: String,
    },
    /// Explicitly retry a failed/uncertain event; keeps the same saved session ID.
    Retry {
        id: String,
    },
    /// Stop future wakes and reject in-flight tool writes; retain history.
    Stop {
        id: String,
    },
    /// Materialize and locate the verified final JSON result and source references.
    Result {
        id: String,
    },
}
pub async fn execute(store: &mut Store, command: Command) -> Result<Value> {
    match command {
        Command::Prepare { peer, input } => {
            let (goal, files, limits) = input.load()?;
            Ok(json!(store.create_workflow(
                "peer",
                Some(&peer),
                None,
                &goal,
                &files,
                limits
            )?))
        }
        Command::Start {
            peer,
            remote_workflow,
            input,
        } => {
            let (goal, files, limits) = input.load()?;
            Ok(json!(store.create_workflow(
                "owner",
                Some(&peer),
                Some(&remote_workflow),
                &goal,
                &files,
                limits
            )?))
        }
        Command::Local { input } => {
            let (goal, files, limits) = input.load()?;
            Ok(json!(store.create_workflow(
                "local", None, None, &goal, &files, limits
            )?))
        }
        Command::Show { id } => store.workflow_status(&id),
        Command::List => Ok(json!(
            store
                .workflow_ids()?
                .iter()
                .map(|id| workflow::get(&store.conn, id))
                .collect::<Result<Vec<_>>>()?
        )),
        Command::Run { id } => {
            store.ingest_workflows()?;
            crate::workflow_scheduler::run_one(store, &id, &std::env::current_exe()?).await
        }
        Command::Retry { id } => {
            store.workflow_retry(&id)?;
            Ok(json!({"workflow_id":id,"state":"ready"}))
        }
        Command::Stop { id } => {
            store.workflow_stop(&id)?;
            Ok(json!({"workflow_id":id,"state":"stopped"}))
        }
        Command::Result { id } => {
            let w = workflow::get(&store.conn, &id)?;
            ensure!(
                w.state == "completed",
                "workflow has no verified final result"
            );
            store.materialize_workflow(&id)?;
            Ok(json!({"path":store.workflow_artifact_path(&id),"result":w.result}))
        }
    }
}
