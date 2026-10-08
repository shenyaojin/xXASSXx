use crate::{knowledge::Content, store::Store};
use anyhow::Result;
use clap::Subcommand;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum Commands {
    /// Immutable file attachments, local grants and resumable delivery.
    Files {
        #[command(subcommand)]
        action: crate::transfers::Command,
    },
    /// Shared durable owner commands, chats, tasks and events (also a test adapter).
    App {
        #[command(subcommand)]
        action: crate::app_cli::Command,
    },
    /// Personal installation, model checks, local butler and collaboration status.
    Client {
        #[arg(long, global = true)]
        directory: Option<PathBuf>,
        #[command(subcommand)]
        action: crate::setup::ClientCommand,
    },
    /// Team mailbox setup and hosting; no LLM or Codex login required.
    Server {
        #[arg(long, global = true)]
        directory: Option<PathBuf>,
        #[command(subcommand)]
        action: crate::setup::ServerCommand,
    },
    /// Start, stop, inspect, and read logs for a user-owned butler process.
    Service {
        #[command(subcommand)]
        action: crate::service::Command,
    },
    /// Bounded persistent collaboration and owner-authorized file access.
    Collaboration {
        #[command(subcommand)]
        action: crate::workflow_cli::Command,
    },
    Delegation {
        #[command(subcommand)]
        action: DelegationCommand,
    },
    Mailbox {
        #[command(subcommand)]
        action: MailboxCommand,
    },
    Message {
        #[command(subcommand)]
        action: MessageCommand,
    },
    Conversation {
        #[command(subcommand)]
        action: ConversationCommand,
    },
    Butler {
        #[command(subcommand)]
        action: ButlerCommand,
    },
    Member {
        #[command(subcommand)]
        action: MemberCommand,
    },
    Object {
        #[command(subcommand)]
        action: ObjectCommand,
    },
    Version {
        #[command(subcommand)]
        action: VersionCommand,
    },
    /// Manage this database's allowed local import directories.
    Roots {
        #[command(subcommand)]
        action: crate::file_roots::Command,
    },
}
#[derive(Subcommand)]
pub enum MemberCommand {
    Init {
        #[arg(long)]
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        team: String,
    },
    Configure {
        config: PathBuf,
    },
    Show,
    Contacts,
}
#[derive(Subcommand)]
pub enum MailboxCommand {
    Serve {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, default_value = "127.0.0.1:7788")]
        listen: std::net::SocketAddr,
    },
}
#[derive(Subcommand)]
pub enum MessageCommand {
    Send {
        #[arg(long)]
        to: String,
        #[arg(long)]
        body: String,
        #[arg(long, default_value = "auto")]
        operation: String,
        #[arg(long)]
        object_id: Option<String>,
        #[arg(long)]
        version_id: Option<String>,
        #[arg(long)]
        reply_to: Option<String>,
        #[arg(long)]
        message_id: Option<String>,
    },
    Reply {
        request: String,
        #[arg(long)]
        body: String,
        #[arg(long)]
        version_id: Option<String>,
    },
    Inbox,
    Show {
        id: String,
    },
}
#[derive(Subcommand)]
pub enum ConversationCommand {
    Show { id: String },
    Close { id: String },
}
#[derive(Subcommand)]
pub enum ButlerCommand {
    Sync,
    Process {
        id: String,
    },
    Tick,
    McpServe,
    ModelHistory {
        id: String,
    },
    Run {
        #[arg(long, default_value_t = 2)]
        poll_secs: u64,
        #[arg(long, default_value_t = 3)]
        max_failures: u32,
        /// Keep retrying transient transport failures, with capped backoff and no inference.
        #[arg(long)]
        reconnect: bool,
    },
}
#[derive(Subcommand)]
pub enum DelegationCommand {
    Status { message_id: String },
    Run { message_id: String },
}
#[derive(Subcommand)]
pub enum ObjectCommand {
    Create {
        #[arg(long)]
        title: String,
        #[arg(long)]
        kind: String,
        #[arg(long)]
        shared: bool,
    },
    Show {
        id: String,
    },
    List,
    Share {
        id: String,
        #[arg(long)]
        private: bool,
    },
    Publish {
        id: String,
        #[arg(long)]
        expected: String,
        #[arg(long)]
        request_id: String,
        #[arg(long, default_value = "")]
        note: String,
        #[arg(long, conflicts_with = "text", required_unless_present = "text")]
        source: Option<PathBuf>,
        #[arg(long, conflicts_with = "source", required_unless_present = "source")]
        text: Option<String>,
    },
    History {
        id: String,
    },
}
#[derive(Subcommand)]
pub enum VersionCommand {
    Show { id: String },
    Text { id: String },
    Verify { id: String },
    Diff { left: String, right: String },
    Export { id: String, target: PathBuf },
}
pub async fn execute(db: &Path, command: Commands) -> Result<()> {
    match command {
        Commands::Client { directory, action } => {
            return crate::setup::client(&crate::setup::directory(directory, "client")?, action)
                .await;
        }
        Commands::Server { directory, action } => {
            return crate::setup::server(&crate::setup::directory(directory, "server")?, action)
                .await;
        }
        _ => {}
    }
    if let Commands::Mailbox {
        action: MailboxCommand::Serve { config, listen },
    } = command
    {
        let config = toml::from_str(&std::fs::read_to_string(config)?)?;
        let router = crate::mailbox::router(db, &config)?;
        let listener = tokio::net::TcpListener::bind(listen).await?;
        println!(
            "{}",
            json!({"listening":listener.local_addr()?.to_string()})
        );
        axum::serve(listener, router)
            .with_graceful_shutdown(crate::setup::shutdown_signal())
            .await?;
        return Ok(());
    }
    let mut store = if matches!(
        command,
        Commands::Member {
            action: MemberCommand::Init { .. }
        }
    ) {
        Store::init(db)?
    } else {
        Store::open(db)?
    };
    let value: Value = match command {
        Commands::Files { action } => crate::transfers::execute(&store, action).await?,
        Commands::Client { .. } | Commands::Server { .. } => unreachable!(),
        Commands::App { action } => crate::app_cli::execute(&mut store, action).await?,
        Commands::Roots { action } => crate::file_roots::execute(&store, action)?,
        Commands::Service { action } => crate::service::execute(&store, action).await?,
        Commands::Collaboration { action } => {
            crate::workflow_cli::execute(&mut store, action).await?
        }
        Commands::Delegation {
            action: DelegationCommand::Status { message_id },
        } => store.delegation(&message_id)?,
        Commands::Delegation {
            action: DelegationCommand::Run { message_id },
        } => {
            crate::butler::run_delegation(&mut store, &message_id, &std::env::current_exe()?)
                .await?
        }
        Commands::Mailbox { .. } => unreachable!(),
        Commands::Member {
            action: MemberCommand::Init { id, name, team },
        } => {
            store.set_identity(&id, &name, &team)?;
            json!({"member_id":id,"display_name":name,"team_id":team})
        }
        Commands::Member {
            action: MemberCommand::Configure { config },
        } => {
            let mut cfg: crate::team::MemberConfig =
                toml::from_str(&std::fs::read_to_string(&config)?)?;
            let parent = config.canonicalize()?.parent().unwrap().to_path_buf();
            if cfg.executor.workdir.is_relative() {
                cfg.executor.workdir = parent.join(&cfg.executor.workdir).canonicalize()?;
            }
            if cfg.executor.codex.components().count() > 1 && cfg.executor.codex.is_relative() {
                cfg.executor.codex = parent.join(&cfg.executor.codex).canonicalize()?;
            }
            if let Some(path) = &cfg.secrets_file {
                cfg.secrets_file = Some(parent.join(path).canonicalize()?);
            }
            store.configure_member(&cfg)?;
            json!({"configured":true})
        }
        Commands::Member {
            action: MemberCommand::Show,
        } => store.identity()?,
        Commands::Member {
            action: MemberCommand::Contacts,
        } => serde_json::to_value(store.contacts()?)?,
        Commands::Message { action } => match action {
            MessageCommand::Send {
                to,
                body,
                operation,
                object_id,
                version_id,
                reply_to,
                message_id,
            } => serde_json::to_value(store.new_request(
                &to,
                &body,
                &operation,
                (object_id, version_id),
                reply_to.as_deref(),
                &message_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            )?)?,
            MessageCommand::Reply {
                request,
                body,
                version_id,
            } => serde_json::to_value(store.reply(&request, &body, version_id)?)?,
            MessageCommand::Inbox => serde_json::to_value(
                store
                    .messages(None)?
                    .into_iter()
                    .filter(|m| m.direction == "in")
                    .collect::<Vec<_>>(),
            )?,
            MessageCommand::Show { id } => serde_json::to_value(store.message(&id)?)?,
        },
        Commands::Conversation { action } => match action {
            ConversationCommand::Show { id } => store.conversation(&id)?,
            ConversationCommand::Close { id } => {
                store.close_conversation(&id)?;
                json!({"closed":true})
            }
        },
        Commands::Butler {
            action: ButlerCommand::Sync,
        } => crate::team::sync(&mut store).await?,
        Commands::Butler {
            action: ButlerCommand::Process { id },
        } => {
            crate::butler::process_and_dispatch(&mut store, &id, &std::env::current_exe()?).await?
        }
        Commands::Butler {
            action: ButlerCommand::Tick,
        } => crate::butler::tick(&mut store, &std::env::current_exe()?).await?,
        Commands::Butler {
            action: ButlerCommand::McpServe,
        } => return crate::mcp::serve_butler(store).await,
        Commands::Butler {
            action: ButlerCommand::ModelHistory { id },
        } => store.model_history(&id)?,
        Commands::Butler {
            action:
                ButlerCommand::Run {
                    poll_secs,
                    max_failures,
                    reconnect,
                },
        } => {
            return crate::app::daemon::run(
                &store.path,
                &std::env::current_exe()?,
                poll_secs,
                max_failures,
                reconnect,
            )
            .await;
        }

        Commands::Object { action } => match action {
            ObjectCommand::Create {
                title,
                kind,
                shared,
            } => serde_json::to_value(store.create_object(&title, &kind, shared)?)?,
            ObjectCommand::Show { id } => serde_json::to_value(store.object(&id)?)?,
            ObjectCommand::List => serde_json::to_value(store.objects(false)?)?,
            ObjectCommand::Share { id, private } => {
                store.share_object(&id, !private)?;
                json!({"shared":!private})
            }
            ObjectCommand::History { id } => serde_json::to_value(store.history(&id)?)?,
            ObjectCommand::Publish {
                id,
                expected,
                request_id,
                note,
                source,
                text,
            } => {
                let content = match (&source, &text) {
                    (Some(p), _) => Content::Path(p),
                    (_, Some(t)) => Content::Text(t),
                    _ => unreachable!(),
                };
                serde_json::to_value(store.publish(
                    &id,
                    if expected == "none" {
                        None
                    } else {
                        Some(&expected)
                    },
                    &request_id,
                    &note,
                    content,
                )?)?
            }
        },
        Commands::Version { action } => match action {
            VersionCommand::Show { id } => serde_json::to_value(store.get_version(&id)?)?,
            VersionCommand::Text { id } => json!({"version_id":id,"text":store.text_version(&id)?}),
            VersionCommand::Verify { id } => {
                store.verify_version(&id)?;
                json!({"version_id":id,"verified":true})
            }
            VersionCommand::Export { id, target } => {
                store.export_version(&id, &target)?;
                json!({"version_id":id,"exported":true})
            }
            VersionCommand::Diff { left, right } => store.diff_versions(&left, &right)?,
        },
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
