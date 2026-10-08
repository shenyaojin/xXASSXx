//! Immutable owner-authorized attachments. Models propose; Rust grants and transports.
mod local;
mod network;
pub mod relay;
use anyhow::{Result, ensure};
pub use local::*;
pub use network::sync;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

pub const CHUNK: u64 = 8 * 1024 * 1024;
pub const GIB: u64 = 1024 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileSpec {
    pub id: String,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TaskRef {
    pub id: String,
    pub revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub protocol: u32,
    pub id: String,
    pub sender: String,
    pub recipient: String,
    pub note: String,
    pub files: Vec<FileSpec>,
    pub task: Option<TaskRef>,
    pub replaces: Option<String>,
}
impl Manifest {
    pub fn validate(&self, max_file: u64, max_batch: u64) -> Result<()> {
        uuid::Uuid::parse_str(&self.id)?;
        ensure!(
            self.protocol == 1 && self.sender != self.recipient,
            "invalid transfer identity"
        );
        ensure!(
            !self.sender.is_empty()
                && self.sender.len() <= 64
                && !self.recipient.is_empty()
                && self.recipient.len() <= 64,
            "invalid members"
        );
        ensure!(
            self.note.len() <= 4096 && (1..=40).contains(&self.files.len()),
            "invalid attachment manifest size"
        );
        if let Some(t) = &self.task {
            uuid::Uuid::parse_str(&t.id)?;
            ensure!(t.revision > 0, "invalid revision");
        }
        if let Some(id) = &self.replaces {
            uuid::Uuid::parse_str(id)?;
            ensure!(id != &self.id, "invalid replacement");
        }
        let mut ids = std::collections::HashSet::new();
        let mut total = 0u64;
        for f in &self.files {
            uuid::Uuid::parse_str(&f.id)?;
            ensure!(ids.insert(&f.id), "duplicate file id");
            ensure!(
                !f.name.is_empty()
                    && f.name.len() <= 255
                    && f.name != "."
                    && f.name != ".."
                    && !f
                        .name
                        .chars()
                        .any(|c| c.is_control() || c == '/' || c == '\\'),
                "invalid attachment filename"
            );
            ensure!(f.bytes <= max_file, "单文件超过允许大小");
            ensure!(
                f.sha256.len() == 64
                    && f.sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "invalid checksum"
            );
            total = total
                .checked_add(f.bytes)
                .ok_or_else(|| anyhow::anyhow!("size overflow"))?;
        }
        ensure!(total <= max_batch, "附件总量超过允许大小");
        Ok(())
    }
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn checksum(path: &Path) -> Result<(u64, String)> {
    let mut file = crate::transfers::local::plain_open(path)?;
    let mut h = Sha256::new();
    let mut n = 0u64;
    let mut buf = [0u8; 65536];
    loop {
        let count = file.read(&mut buf)?;
        if count == 0 {
            break;
        }
        h.update(&buf[..count]);
        n += count as u64;
    }
    Ok((n, format!("{:x}", h.finalize())))
}
pub fn durable_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
pub fn space_available(path: &Path, required: u64) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut s = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    ensure!(
        unsafe { libc::statvfs(path.as_ptr(), s.as_mut_ptr()) } == 0,
        "无法检查磁盘空间"
    );
    let s = unsafe { s.assume_init() };
    // libc uses different integer widths for statvfs across macOS and Linux.
    #[allow(clippy::unnecessary_cast)]
    let free = (s.f_bavail as u64).saturating_mul(s.f_frsize as u64);
    ensure!(
        free >= required.saturating_add(64 * 1024 * 1024),
        "磁盘可用空间不足；已保留传输进度"
    );
    Ok(())
}
