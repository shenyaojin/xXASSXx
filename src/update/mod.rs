//! Local application maintenance. No model calls or business-task permissions.
pub mod install;
pub mod package;
pub mod release;

use anyhow::{Context, Result, ensure};
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use crate::store::SCHEMA_VERSION as SCHEMA;
pub fn target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        _ => anyhow::bail!("没有适用于此平台的发布包"),
    }
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Stable,
    #[default]
    Preview,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    Off,
    #[default]
    Notify,
    Idle,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub channel: Channel,
    pub policy: Policy,
    /// HTTPS directory containing update.json, its signature and the archives.
    pub source: Option<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            channel: if env!("CARGO_PKG_VERSION").contains('-') {
                Channel::Preview
            } else {
                Channel::Stable
            },
            policy: Policy::Notify,
            source: None,
        }
    }
}
impl Settings {
    pub fn load(exe: &Path) -> Result<Self> {
        let path = install::sidecar(exe, "settings.json")?;
        if !path.exists() {
            return Ok(Self::default());
        }
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }
}
#[derive(Args, Default)]
pub struct Command {
    /// Only check; does not open or migrate a personal database.
    #[arg(long)]
    pub check: bool,
    /// Signed offline release directory.
    #[arg(long, conflicts_with = "source")]
    pub from: Option<PathBuf>,
    /// HTTPS release directory, or 'github' to use GitHub Releases.
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long, value_enum)]
    pub channel: Option<Channel>,
    /// Save update settings and exit. Only explicitly supplied fields change.
    #[arg(long, conflicts_with_all = ["check", "from"])]
    pub configure: bool,
    #[arg(long, value_enum, requires = "configure")]
    pub policy: Option<Policy>,
    /// Personal client data directory whose background service should restart.
    #[arg(long)]
    pub directory: Option<PathBuf>,
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=300))]
    pub wait_secs: u64,
    #[arg(long, hide = true)]
    pub probe: bool,
    #[arg(long, hide = true)]
    pub reopen: Option<PathBuf>,
    #[arg(long, hide = true)]
    pub session: Option<String>,
    #[arg(long, hide = true)]
    pub automatic: bool,
}

pub async fn execute(args: Command) -> Result<()> {
    if args.probe {
        println!(
            "{}",
            serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"schema":SCHEMA,"target":target()?})
        );
        return Ok(());
    }
    let exe = std::env::current_exe()?.canonicalize()?;
    let mut settings = Settings::load(&exe)?;
    if let Some(channel) = args.channel {
        settings.channel = channel;
    }
    if let Some(policy) = args.policy {
        settings.policy = policy;
    }
    if let Some(source) = &args.source {
        ensure!(
            source == "github" || source.starts_with("https://"),
            "更新源必须使用 HTTPS 或 github"
        );
        settings.source = (source != "github").then(|| source.trim_end_matches('/').into());
    }
    if args.configure {
        install::managed(&exe)?;
        install::write_json(&install::sidecar(&exe, "settings.json")?, &settings)?;
        // A new policy/source takes effect without waiting for the old cache.
        let _ = std::fs::remove_file(install::sidecar(&exe, "cache.json")?);
        println!("更新设置已保存：{}", serde_json::to_string(&settings)?);
        return Ok(());
    }
    let result = run(&exe, &settings, &args).await;
    if let Some(project) = args.reopen {
        if let Err(ref error) = result {
            eprintln!("更新未完成：{error:#}");
        }
        // The UI's SQLite connection and terminal modes were closed before exec.
        // Reopen even after a download failure, keeping the exact conversation.
        use std::os::unix::process::CommandExt;
        let mut command = std::process::Command::new(&exe);
        command.arg("open").arg(project);
        if let Some(directory) = args.directory {
            command.arg("--directory").arg(directory);
        }
        if let Some(session) = args.session {
            command.arg("--resume-session").arg(session);
        }
        command.env(
            "XXASSXX_UPDATE_NOTICE",
            match &result {
                Ok(()) => "更新检查完成；已回到原会话。".to_owned(),
                Err(e) => format!("更新未完成：{e}；原会话已保留，可用 /update 重试。"),
            },
        );
        return Err(command.exec().into());
    }
    result
}
async fn run(exe: &Path, settings: &Settings, args: &Command) -> Result<()> {
    let _update = if args.check {
        None
    } else {
        install::managed(exe)?;
        Some(install::lock(exe, "update.lock", true)?)
    };
    println!("正在检查更新…");
    let Some(release) = release::check(settings, args.from.as_deref()).await? else {
        println!("当前 {}，此渠道没有更新版本。", env!("CARGO_PKG_VERSION"));
        return Ok(());
    };
    println!(
        "发现 {} → {}\n{}",
        env!("CARGO_PKG_VERSION"),
        release.manifest.version,
        crate::tui::editor::safe(&release.manifest.notes)
    );
    if args.check {
        return Ok(());
    }
    // Version 1 deliberately excludes incompatible database migrations. Opening
    // Store here would migrate before quiescence and make binary rollback unsafe.
    ensure!(
        release.manifest.schema == SCHEMA,
        "新版需要数据库迁移；请按版本说明手动升级，自动更新未修改程序或数据"
    );
    let stage = tempfile::Builder::new()
        .prefix(".xxassxx-stage-")
        .tempdir_in(exe.parent().context("missing install directory")?)?;
    println!("正在下载安装包并验证签名和校验值…");
    package::prepare(&release, args.from.as_deref(), stage.path()).await?;
    let root = crate::setup::directory(args.directory.clone(), "client")?;
    let db = root.join("member.sqlite3");
    install::apply(
        exe,
        &stage.path().join("xxassxx"),
        &release.manifest,
        &db,
        args.wait_secs,
        args.automatic,
    )
    .await?;
    let _ = std::fs::remove_file(install::sidecar(exe, "cache.json")?);
    println!(
        "已更新到 {}。身份、配置、历史和授权均保留。",
        release.manifest.version
    );
    Ok(())
}

#[derive(Deserialize, Serialize)]
struct Cache {
    checked_at: i64,
    version: String,
    settings: String,
    notice: Option<String>,
    available: bool,
}
pub struct Notice {
    pub text: Option<String>,
    pub automatic: bool,
}
pub async fn background_check(exe: PathBuf) -> Result<Notice> {
    install::managed(&exe)?;
    let settings = Settings::load(&exe)?;
    if settings.policy == Policy::Off {
        return Ok(Notice {
            text: None,
            automatic: false,
        });
    }
    let key = serde_json::to_string(&settings)?;
    let path = install::sidecar(&exe, "cache.json")?;
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(c) = serde_json::from_slice::<Cache>(&bytes) {
            let age = crate::store::now() - c.checked_at;
            if (0..86400).contains(&age)
                && c.version == env!("CARGO_PKG_VERSION")
                && c.settings == key
            {
                return Ok(Notice {
                    text: c.notice,
                    automatic: c.available && settings.policy == Policy::Idle,
                });
            }
        }
    }
    let result = release::check(&settings, None).await;
    let available = result
        .as_ref()
        .is_ok_and(|r| r.as_ref().is_some_and(|r| r.manifest.schema == SCHEMA));
    let text = match result {
        Ok(Some(r)) => Some(format!(
            "发现新版 {} · {}",
            r.manifest.version,
            if r.manifest.schema != SCHEMA {
                "此版需要数据库迁移，请按版本说明手动升级"
            } else {
                "输入 /update 自动下载安装"
            }
        )),
        Ok(None) => None,
        Err(_) => Some("暂时无法检查更新；可继续使用，输入 /update 重试。".into()),
    };
    install::write_json(
        &path,
        &Cache {
            checked_at: crate::store::now(),
            version: env!("CARGO_PKG_VERSION").into(),
            settings: key,
            notice: text.clone(),
            available,
        },
    )?;
    Ok(Notice {
        text,
        automatic: available && settings.policy == Policy::Idle,
    })
}
