//! Parse a single requested environment field as data; never execute an env file.
use anyhow::{Context, Result, ensure};
use std::{fs::OpenOptions, io::Read, path::Path};
pub fn read_field(path: &Path, name: &str) -> Result<String> {
    crate::team::validate_env_name(name)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|_| anyhow::anyhow!("cannot open configured secrets file"))?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.len() <= 65536,
        "invalid secrets file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            meta.mode() & 0o777 == 0o600 && meta.uid() == unsafe { libc::geteuid() },
            "secrets file must belong to this user with mode 600"
        );
    }
    let mut text = String::new();
    file.take(65537)
        .read_to_string(&mut text)
        .map_err(|_| anyhow::anyhow!("invalid secrets file encoding"))?;
    let mut key = None;
    for line in text.lines() {
        let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
        if let Some((field, value)) = line.split_once('=') {
            if field.trim() != name {
                continue;
            }
            ensure!(key.is_none(), "duplicate requested secret field");
            let raw = value.trim();
            let value = if raw.starts_with('\'') || raw.starts_with('"') {
                let quote = raw.as_bytes()[0] as char;
                ensure!(
                    raw.len() >= 2 && raw.ends_with(quote),
                    "invalid quoted secret field"
                );
                &raw[1..raw.len() - 1]
            } else {
                raw.split_whitespace().next().unwrap_or("")
            };
            ensure!(
                !value.is_empty() && value.len() <= 8192 && !value.chars().any(char::is_whitespace),
                "invalid secret field"
            );
            key = Some(value.to_owned());
        }
    }
    key.context("requested field missing from secrets file")
}
