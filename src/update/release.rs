//! Signed release metadata. Hosting is separate from installation policy.
use super::{Channel, Settings, target};
use anyhow::{Context, Result, ensure};
use ed25519_dalek::{Signature, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, time::Duration};

pub const PUBLIC_KEY: &str = include_str!("public-key.hex");
const MAX_METADATA: usize = 1024 * 1024;
const REPOSITORY: &str = "shenyaojin/xXASSXx";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Asset {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub format: u32,
    pub version: Version,
    pub schema: u32,
    pub notes: String,
    pub assets: BTreeMap<String, Asset>,
}
#[derive(Clone, Debug)]
pub struct Release {
    pub manifest: Manifest,
    pub base: String,
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn unhex<const N: usize>(s: &str) -> Result<[u8; N]> {
    let s = s.trim();
    ensure!(s.len() == N * 2 && s.is_ascii(), "invalid hex length");
    let mut bytes = [0; N];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)?;
    }
    Ok(bytes)
}
pub fn verify(bytes: &[u8], signature: &str, key: &str) -> Result<Manifest> {
    ensure!(bytes.len() <= MAX_METADATA, "release metadata too large");
    VerifyingKey::from_bytes(&unhex(key)?)?
        .verify_strict(bytes, &Signature::from_bytes(&unhex(signature)?))
        .context("更新签名无效，未安装任何文件")?;
    let m: Manifest = serde_json::from_slice(bytes)?;
    ensure!(m.format == 1 && m.schema > 0, "unsupported update format");
    ensure!(m.notes.len() <= 16384, "release notes too long");
    for (platform, asset) in &m.assets {
        ensure!(
            asset.name == format!("xxassxx-{platform}.tar.gz")
                && !asset.name.contains('/')
                && !asset.name.contains('\\')
                && asset.size > 0
                && asset.size <= 256 * 1024 * 1024,
            "invalid release asset"
        );
        unhex::<32>(&asset.sha256)?;
    }
    Ok(m)
}
pub fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("xxassxx/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(180))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?)
}
pub async fn metadata(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let mut response = client
        .get(url)
        .timeout(Duration::from_secs(20))
        .send()
        .await?
        .error_for_status()?;
    let mut result = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            result.len() + chunk.len() <= MAX_METADATA,
            "release metadata too large"
        );
        result.extend_from_slice(&chunk);
    }
    Ok(result)
}
pub async fn load(base: &str, from: Option<&Path>) -> Result<Release> {
    let (bytes, signature) = if let Some(dir) = from {
        let bytes = std::fs::read(dir.join("update.json"))?;
        let signature = std::fs::read_to_string(dir.join("update.json.sig"))?;
        (bytes, signature)
    } else {
        let client = client()?;
        let bytes = metadata(&client, &format!("{base}/update.json")).await?;
        let signature = metadata(&client, &format!("{base}/update.json.sig")).await?;
        (bytes, String::from_utf8(signature)?)
    };
    Ok(Release {
        manifest: verify(&bytes, &signature, PUBLIC_KEY)?,
        base: base.into(),
    })
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}
pub fn eligible(version: &Version, current: &Version, channel: Channel) -> bool {
    version > current && (channel == Channel::Preview || version.pre.is_empty())
}
pub async fn check(settings: &Settings, from: Option<&Path>) -> Result<Option<Release>> {
    let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
    let release = if let Some(dir) = from {
        load("", Some(dir)).await?
    } else if let Some(base) = settings.source.as_deref() {
        ensure!(base.starts_with("https://"), "更新源必须使用 HTTPS");
        load(base.trim_end_matches('/'), None).await?
    } else {
        let client = client()?;
        let mut versions = Vec::new();
        // Bound network work, but do not rely on API ordering or /latest (excludes Alpha).
        for page in 1..=10 {
            let bytes = metadata(
                &client,
                &format!(
                    "https://api.github.com/repos/{REPOSITORY}/releases?per_page=100&page={page}"
                ),
            )
            .await?;
            let releases: Vec<GithubRelease> = serde_json::from_slice(&bytes)?;
            let done = releases.len() < 100;
            for r in releases {
                if r.draft || (r.prerelease && settings.channel == Channel::Stable) {
                    continue;
                }
                if let Some(raw) = r.tag_name.strip_prefix('v') {
                    if let Ok(v) = Version::parse(raw) {
                        if eligible(&v, &current, settings.channel) {
                            versions.push(v);
                        }
                    }
                }
            }
            if done {
                break;
            }
        }
        let Some(version) = versions.into_iter().max() else {
            return Ok(None);
        };
        let base = format!("https://github.com/{REPOSITORY}/releases/download/v{version}");
        let r = load(&base, None).await?;
        ensure!(
            r.manifest.version == version,
            "release tag and signed version disagree"
        );
        r
    };
    if !eligible(&release.manifest.version, &current, settings.channel) {
        return Ok(None);
    }
    ensure!(
        release.manifest.assets.contains_key(target()?),
        "新版缺少本机平台的安装包"
    );
    Ok(Some(release))
}
