//! Download into a private staging directory; never unpack arbitrary paths.
use super::{
    release::{self, Release},
    target,
};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::Path,
    time::Duration,
};

pub async fn prepare(release: &Release, from: Option<&Path>, stage: &Path) -> Result<()> {
    let asset = release
        .manifest
        .assets
        .get(target()?)
        .context("missing platform")?;
    let archive = stage.join("package.tar.gz");
    let mut out = std::fs::File::create(&archive)?;
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let mut write = |bytes: &[u8]| -> Result<()> {
        size += bytes.len() as u64;
        ensure!(size <= asset.size, "安装包超过声明大小");
        out.write_all(bytes)?;
        hash.update(bytes);
        Ok(())
    };
    if let Some(dir) = from {
        let mut input = std::fs::File::open(dir.join(&asset.name))?;
        let mut buf = [0; 65536];
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            write(&buf[..n])?;
        }
    } else {
        let mut response = release::client()?
            .get(format!("{}/{}", release.base, asset.name))
            .send()
            .await?
            .error_for_status()?;
        while let Some(chunk) = response.chunk().await? {
            write(&chunk)?;
        }
    }
    ensure!(
        size == asset.size && release::hex(&hash.finalize()) == asset.sha256,
        "安装包校验失败，原程序保持不变"
    );
    out.sync_all()?;
    drop(out);
    extract(&archive, &stage.join("xxassxx"))?;
    let mut command = tokio::process::Command::new(stage.join("xxassxx"));
    command.args(["update", "--probe"]).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(10), command.output()).await??;
    ensure!(output.status.success(), "新版程序无法启动");
    let info: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    ensure!(
        info["version"] == release.manifest.version.to_string()
            && info["target"] == target()?
            && info["schema"] == release.manifest.schema,
        "新版程序与签名清单不一致"
    );
    Ok(())
}

pub fn extract(archive: &Path, dest: &Path) -> Result<()> {
    let input = std::fs::File::open(archive)?;
    let decoder = flate2::read::GzDecoder::new(input);
    // Bounds decompression even when a signed archive is accidentally malformed.
    let mut archive = tar::Archive::new(decoder.take(256 * 1024 * 1024));
    let mut found = false;
    for entry in archive.entries()? {
        let mut entry = entry?;
        ensure!(
            !found
                && entry.path()?.as_ref() == Path::new("xxassxx")
                && entry.header().entry_type().is_file(),
            "安装包只允许包含普通文件 xxassxx"
        );
        ensure!(entry.size() <= 128 * 1024 * 1024, "解压文件过大");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dest)?;
        std::io::copy(&mut entry, &mut file)?;
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o755))?;
        file.sync_all()?;
        found = true;
    }
    ensure!(found, "安装包没有 xxassxx");
    Ok(())
}
