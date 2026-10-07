//! First-run setup for two roles sharing the same Rust binary.
use crate::{
    mailbox::{RelayConfig, RelayMember},
    model::ModelConfig,
    store::Store,
    team::{Contact, ExecutorConfig, MemberConfig},
};
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Subcommand)]
pub enum ServerCommand {
    /// Create a team and private per-member invitation files. Never prints tokens.
    Init {
        #[arg(long)]
        team: String,
        #[arg(long)]
        public_url: String,
        /// Repeat as --member alice=Alice --member bob=Bob (2–9 members).
        #[arg(long, required = true)]
        member: Vec<String>,
    },
    /// Run the team mailbox; put an HTTPS reverse proxy in front of loopback.
    Serve {
        #[arg(long, default_value = "127.0.0.1:7788")]
        listen: std::net::SocketAddr,
    },
}

#[derive(Subcommand)]
pub enum ClientCommand {
    /// Import your private team invitation and configure your own butler.
    Init {
        #[arg(long)]
        invite: PathBuf,
        #[arg(long, value_parser = ["ollama", "deepseek", "compatible", "off"], default_value = "ollama")]
        provider: String,
        /// Exact installed model name; required unless provider=off.
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        model_url: Option<String>,
        /// A private env file; DEEPSEEK_API_KEY or MODEL_API_KEY, depending on provider.
        #[arg(long)]
        model_secrets: Option<PathBuf>,
        #[arg(long, default_value = "codex")]
        codex: PathBuf,
        /// Repeat for each local source directory. Empty means no file imports.
        #[arg(long = "allow-dir")]
        allow_dirs: Vec<PathBuf>,
    },
    /// Check configuration, mailbox identity, and Codex login. No inference by default.
    Doctor {
        /// Explicitly run a two-call model tool-roundtrip check (may consume quota).
        #[arg(long)]
        probe_model: bool,
    },
    Start,
    Stop,
    Status,
    Contacts,
    /// Show your invitation-bound username, display name and team; no credentials.
    Whoami,
    /// Manage local source directories; does not import or share their files.
    Roots {
        #[command(subcommand)]
        action: crate::file_roots::Command,
    },
    /// Store a request in your durable local outbox; the running service delivers it.
    Send {
        #[arg(long)]
        to: String,
        #[arg(long)]
        body: String,
        #[arg(long, default_value = "auto")]
        operation: String,
        #[arg(long)]
        object_id: Option<String>,
    },
    Inbox,
    Tasks,
    Task {
        id: String,
    },
    Result {
        id: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Invitation {
    format: u32,
    team_id: String,
    member_id: String,
    display_name: String,
    mailbox_url: String,
    credential: String,
    contacts: Vec<Contact>,
}

pub fn directory(value: Option<PathBuf>, role: &str) -> Result<PathBuf> {
    if let Some(path) = value {
        return Ok(std::path::absolute(path)?);
    }
    let base = if let Some(path) = std::env::var_os("XDG_DATA_HOME").filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        ensure!(path.is_absolute(), "XDG_DATA_HOME must be an absolute path");
        path
    } else {
        PathBuf::from(std::env::var_os("HOME").context("HOME is not set; use --directory")?)
            .join(".local/share")
    };
    Ok(base.join("xxassxx").join(role))
}

fn identifier(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "team/member IDs must use 1–64 letters, digits, hyphens or underscores"
    );
    Ok(())
}

fn new_directory(path: &Path) -> Result<()> {
    ensure!(
        !path.try_exists()?,
        "directory already exists; use existing configuration or choose a new --directory"
    );
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn private_write(path: &Path, data: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    file.write_all(data)?;
    file.sync_all()?;
    Ok(())
}

pub fn init_server(root: &Path, team: &str, public_url: &str, members: &[String]) -> Result<Value> {
    identifier(team)?;
    crate::team::validate_url(public_url, false)?;
    ensure!(
        (2..10).contains(&members.len()),
        "configure two to nine members"
    );
    let mut names = HashSet::new();
    let contacts = members
        .iter()
        .map(|entry| -> Result<Contact> {
            let (id, name) = entry
                .split_once('=')
                .context("use --member id=DisplayName")?;
            identifier(id)?;
            ensure!(
                names.insert(id.to_owned()) && !name.trim().is_empty() && name.len() <= 256,
                "duplicate member ID or invalid display name"
            );
            Ok(Contact {
                member_id: id.into(),
                display_name: name.into(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    new_directory(root)?;
    new_directory(&root.join("invitations"))?;
    let mut secrets = String::new();
    let mut relay_members = Vec::new();
    let mut invitations = Vec::new();
    for (index, contact) in contacts.iter().enumerate() {
        let field = format!("XXASSXX_MEMBER_{index}_TOKEN");
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        secrets.push_str(&format!("{field}={token}\n"));
        relay_members.push(RelayMember {
            member_id: contact.member_id.clone(),
            display_name: contact.display_name.clone(),
            credential_env: field,
        });
        let invitation = Invitation {
            format: 1,
            team_id: team.into(),
            member_id: contact.member_id.clone(),
            display_name: contact.display_name.clone(),
            mailbox_url: public_url.into(),
            credential: token,
            contacts: contacts
                .iter()
                .filter(|c| c.member_id != contact.member_id)
                .cloned()
                .collect(),
        };
        let path = root
            .join("invitations")
            .join(format!("{}.json", contact.member_id));
        private_write(&path, &serde_json::to_vec_pretty(&invitation)?)?;
        invitations.push(json!({"member":contact.member_id,"path":path}));
    }
    let secrets_file = root.join("secrets.env");
    private_write(&secrets_file, secrets.as_bytes())?;
    let config = RelayConfig {
        team_id: team.into(),
        members: relay_members,
        secrets_file: Some(secrets_file),
    };
    private_write(
        &root.join("server.toml"),
        toml::to_string_pretty(&config)?.as_bytes(),
    )?;
    Ok(
        json!({"role":"server","directory":root,"config":root.join("server.toml"),
        "invitations":invitations,"note":"Each invitation contains that member's credential. Give each person only their own file."}),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn init_client(
    root: &Path,
    invite: &Path,
    provider: &str,
    model: Option<String>,
    model_url: Option<String>,
    model_secrets: Option<PathBuf>,
    codex: PathBuf,
    allow_dirs: &[PathBuf],
) -> Result<Value> {
    let allow_dirs = allow_dirs
        .iter()
        .map(|p| crate::file_roots::directory(p))
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    ensure!(allow_dirs.len() <= 64, "at most 64 allowed directories");
    let mut bytes = Vec::new();
    std::fs::File::open(invite)?
        .take(65537)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 65536, "invitation too large");
    let invitation: Invitation =
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("invalid invitation file"))?;
    ensure!(invitation.format == 1, "unsupported invitation version");
    identifier(&invitation.team_id)?;
    identifier(&invitation.member_id)?;
    crate::team::validate_url(&invitation.mailbox_url, false)?;
    ensure!(
        (16..=256).contains(&invitation.credential.len())
            && invitation
                .credential
                .bytes()
                .all(|b| b.is_ascii_alphanumeric()),
        "invalid invitation credential"
    );
    ensure!(
        !invitation.display_name.trim().is_empty() && invitation.display_name.len() <= 256,
        "invalid display name"
    );
    let mut names = HashSet::new();
    ensure!(
        (1..9).contains(&invitation.contacts.len()),
        "invalid contacts"
    );
    for contact in &invitation.contacts {
        identifier(&contact.member_id)?;
        ensure!(
            contact.member_id != invitation.member_id
                && names.insert(&contact.member_id)
                && !contact.display_name.trim().is_empty()
                && contact.display_name.len() <= 256,
            "invalid contacts"
        );
    }
    let mut model_cfg = json!({"provider":provider,"model":model.unwrap_or_default()});
    if let Some(url) = model_url {
        model_cfg["base_url"] = json!(url);
    }
    if provider == "compatible" {
        model_cfg["api_key_env"] = json!("MODEL_API_KEY");
    }
    if let Some(file) = model_secrets {
        ensure!(
            provider != "off",
            "provider off does not need model credentials"
        );
        model_cfg["secrets_file"] = json!(std::fs::canonicalize(file)?);
    }
    let model_cfg = ModelConfig::parse(&model_cfg)?;
    if model_cfg.provider != "off" {
        // Checks credentials and client configuration without making any model request.
        crate::model::HttpModel::new(model_cfg.clone())?;
    }
    let codex = if codex.components().count() > 1 {
        std::path::absolute(codex)?
    } else {
        codex
    };
    new_directory(root)?;
    let workdir = root.join("work");
    new_directory(&workdir)?;
    let secret = root.join("secrets.env");
    private_write(
        &secret,
        format!("XXASSXX_MEMBER_TOKEN={}\n", invitation.credential).as_bytes(),
    )?;
    let config = MemberConfig {
        mailbox_url: invitation.mailbox_url,
        credential_env: "XXASSXX_MEMBER_TOKEN".into(),
        secrets_file: Some(secret),
        allow_insecure_http: false,
        contacts: invitation.contacts,
        executor: ExecutorConfig {
            mode: "auto".into(),
            codex,
            workdir,
            ..ExecutorConfig::default()
        },
        model: serde_json::to_value(model_cfg)?,
    };
    let mut store = Store::init(&root.join("member.sqlite3"))?;
    store.set_identity(
        &invitation.member_id,
        &invitation.display_name,
        &invitation.team_id,
    )?;
    store.configure_member(&config)?;
    for path in &allow_dirs {
        store.allow_directory(path)?;
    }
    private_write(
        &root.join("member.toml"),
        toml::to_string_pretty(&config)?.as_bytes(),
    )?;
    Ok(
        json!({"role":"client","directory":root,"database":store.path,
        "config":root.join("member.toml"),"model_provider":provider,
        "identity":store.identity()?,"allowed_directories":store.file_roots()?,
        "next":"Run client doctor --probe-model, then client start. No files are shared or authorized by setup."}),
    )
}

async fn codex_status(path: &Path, arguments: &[&str]) -> bool {
    let mut command = crate::codex::command(path, &std::env::temp_dir());
    command
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    matches!(tokio::time::timeout(Duration::from_secs(10), command.status()).await, Ok(Ok(s)) if s.success())
}

pub async fn doctor(store: &Store, probe: bool) -> Result<Value> {
    let cfg = store.member_config()?;
    let model = ModelConfig::parse(&cfg.model)?;
    let identity = store.identity()?;
    let mailbox = async {
        let credential = cfg.credential()?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let response = client
            .get(format!(
                "{}/v1/whoami",
                cfg.mailbox_url.trim_end_matches('/')
            ))
            .bearer_auth(credential)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("mailbox_unavailable"))?;
        let value = crate::team::response_json(response).await?;
        ensure!(
            value["member_id"] == identity["member_id"] && value["team_id"] == identity["team_id"],
            "mailbox identity mismatch"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let version = codex_status(&cfg.executor.codex, &["--version"]).await;
    let login = version && codex_status(&cfg.executor.codex, &["login", "status"]).await;
    let model_result = if model.provider == "off" {
        Ok(json!({"provider":"off","tool_roundtrip":"not_requested"}))
    } else {
        async {
            let http = crate::model::HttpModel::new(model.clone())?;
            if probe {
                http.probe_tools().await?;
            }
            Ok::<_, anyhow::Error>(json!({"provider":model.provider,"model":model.model,
                "tool_roundtrip":if probe {"passed"} else {"not_requested"}}))
        }
        .await
    };
    Ok(
        json!({"ready":mailbox.is_ok() && version && login && model_result.is_ok(),
        "mailbox":{"reachable_and_identity_matches":mailbox.is_ok(),"error":mailbox.err().map(|e|e.to_string())},
        "codex":{"available":version,"logged_in":login},
        "model":match model_result {Ok(v)=>v,Err(e)=>json!({"error":e.to_string()})},
        "allowed_directories":store.file_roots()?,
        "note":"Without --probe-model, model inference and tool capability are not tested. Codex login status does not prove available inference quota."}),
    )
}

pub async fn server(root: &Path, command: ServerCommand) -> Result<()> {
    let value = match command {
        ServerCommand::Init {
            team,
            public_url,
            member,
        } => init_server(root, &team, &public_url, &member)?,
        ServerCommand::Serve { listen } => {
            let cfg = toml::from_str(&std::fs::read_to_string(root.join("server.toml"))?)?;
            let router = crate::mailbox::router(&root.join("mailbox.sqlite3"), &cfg)?;
            let listener = tokio::net::TcpListener::bind(listen).await?;
            println!(
                "{}",
                json!({"role":"server","listening":listener.local_addr()?})
            );
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown_signal())
                .await?;
            return Ok(());
        }
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = signal.recv() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

pub async fn client(root: &Path, command: ClientCommand) -> Result<()> {
    if let ClientCommand::Init {
        invite,
        provider,
        model,
        model_url,
        model_secrets,
        codex,
        allow_dirs,
    } = command
    {
        let value = init_client(
            root,
            &invite,
            &provider,
            model,
            model_url,
            model_secrets,
            codex,
            &allow_dirs,
        )?;
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    let mut store = Store::open(&root.join("member.sqlite3"))?;
    let value = match command {
        ClientCommand::Doctor { probe_model } => {
            let report = doctor(&store, probe_model).await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            ensure!(
                report["ready"] == true,
                "client checks failed; see report above"
            );
            return Ok(());
        }
        ClientCommand::Start => {
            crate::service::execute(
                &store,
                crate::service::Command::Start {
                    poll_secs: 2,
                    max_failures: 3,
                    reconnect: true,
                },
            )
            .await?
        }
        ClientCommand::Stop => {
            crate::service::execute(&store, crate::service::Command::Stop { wait_secs: 30 }).await?
        }
        ClientCommand::Status => crate::service::status(&store)?,
        ClientCommand::Contacts => json!(store.contacts()?),
        ClientCommand::Whoami => {
            let mut identity = store.identity()?;
            identity["username"] = identity["member_id"].clone();
            identity["mailbox_url"] = json!(store.member_config()?.mailbox_url);
            identity
        }
        ClientCommand::Roots { action } => crate::file_roots::execute(&store, action)?,
        ClientCommand::Send {
            to,
            body,
            operation,
            object_id,
        } => json!(store.new_request(
            &to,
            &body,
            &operation,
            (object_id, None),
            None,
            &uuid::Uuid::new_v4().to_string()
        )?),
        ClientCommand::Inbox => json!(
            store
                .messages(None)?
                .into_iter()
                .filter(|m| m.direction == "in")
                .collect::<Vec<_>>()
        ),
        ClientCommand::Tasks => json!(
            store
                .workflow_ids()?
                .iter()
                .map(|id| crate::workflow::get(&store.conn, id))
                .collect::<Result<Vec<_>>>()?
        ),
        ClientCommand::Task { id } => store.workflow_status(&id)?,
        ClientCommand::Result { id } => {
            crate::workflow_cli::execute(&mut store, crate::workflow_cli::Command::Result { id })
                .await?
        }
        ClientCommand::Init { .. } => unreachable!(),
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
