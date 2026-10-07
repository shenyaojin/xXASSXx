//! File contents are available only through immutable, owner-created task grants.
use crate::{
    mcp::Binding,
    store::{Store, now},
    workflow,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Read,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadFile {
    pub version_id: String,
    pub path: String,
    #[serde(default)]
    pub offset: u64,
    pub max_bytes: u64,
}
pub fn list(store: &Store, b: &Binding) -> Result<Value> {
    Store::authorize(&store.conn, &b.task, &b.run, &b.token, false)?;
    let w = workflow::active(&store.conn, &b.task, &b.run)?;
    Ok(
        json!({"workflow_id":w.id,"files":workflow::grants(&store.conn,&w.id)?,"limits":w.limits,"read_bytes":w.read_bytes}),
    )
}
pub fn read(store: &mut Store, b: &Binding, args: ReadFile) -> Result<Value> {
    crate::knowledge::valid_relative(&args.path)?;
    let root = store.content_root();
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    Store::authorize(&tx, &b.task, &b.run, &b.token, false)?;
    let w = workflow::active(&tx, &b.task, &b.run)?;
    let read_task: i64 =
        tx.query_row("SELECT reads FROM runs WHERE id=?1", [&b.run], |r| r.get(0))?;
    ensure!(read_task > 0, "get_task must precede file access");
    ensure!(
        (4..=w.limits.max_chunk_bytes).contains(&args.max_bytes),
        "file chunk exceeds authorized limit"
    );
    let (object,hash,size):(String,String,u64)=tx.query_row("SELECT object_id,sha256,bytes FROM workflow_grants WHERE workflow_id=?1 AND version_id=?2 AND path=?3",params![w.id,args.version_id,args.path],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?.context("file/version is not authorized for this task")?;
    ensure!(
        size <= 64 * 1024 * 1024,
        "phase 3 text file exceeds 64 MiB validation limit"
    );
    ensure!(args.offset <= size, "offset exceeds file size");
    let mut blob = root.clone();
    for part in ["blobs", &hash[..2], &hash] {
        ensure!(
            !fs::symlink_metadata(&blob)?.file_type().is_symlink(),
            "symbolic links are not allowed in the content store"
        );
        blob.push(part);
    }
    ensure!(
        !fs::symlink_metadata(&blob)?.file_type().is_symlink(),
        "symbolic links are not allowed in the content store"
    );
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(&blob)
        .context("authorized content is missing")?;
    ensure!(file.metadata()?.is_file(), "content is not a regular file");
    let mut hasher = Sha256::new();
    let mut read = 0u64;
    let mut captured = Vec::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        let begin = args.offset.max(read);
        let end = (args.offset + args.max_bytes).min(read + count as u64);
        if end > begin {
            captured.extend_from_slice(&buffer[(begin - read) as usize..(end - read) as usize]);
        }
        read += count as u64;
        ensure!(read <= size, "authorized content grew beyond pinned size");
    }
    ensure!(
        read == size && format!("{:x}", hasher.finalize()) == hash,
        "authorized content hash/length mismatch"
    );
    let text = match std::str::from_utf8(&captured) {
        Ok(text) => text,
        Err(e) if e.error_len().is_none() && e.valid_up_to() > 0 => {
            std::str::from_utf8(&captured[..e.valid_up_to()])?
        }
        Err(_) => anyhow::bail!("content is not UTF-8 or offset is not a character boundary"),
    };
    ensure!(
        !text.contains('\0'),
        "binary content is unsupported by the text tool"
    );
    let count = text.len() as u64;
    let previous: u64 = tx.query_row(
        "SELECT COALESCE(SUM(bytes),0) FROM workflow_file_reads WHERE run_id=?1",
        [&b.run],
        |r| r.get(0),
    )?;
    ensure!(
        w.read_bytes + count <= w.limits.max_read_bytes
            && previous + count <= w.limits.max_read_per_run,
        "task file read budget exhausted"
    );
    tx.execute(
        "UPDATE workflows SET read_bytes=read_bytes+?2 WHERE id=?1",
        params![w.id, count],
    )?;
    tx.execute("INSERT INTO workflow_file_reads(workflow_id,run_id,object_id,version_id,path,sha256,offset,bytes,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![w.id,b.run,object,args.version_id,args.path,hash,args.offset,count,now()])?;
    let member: String = tx.query_row(
        "SELECT member_id FROM identity WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let value = json!({"source":{"member":member,"object_id":object,"version_id":args.version_id,"path":args.path,"sha256":hash},"offset":args.offset,"next_offset":args.offset+count,"eof":args.offset+count==size,"bytes":count,"text":text});
    tx.commit()?;
    Ok(value)
}
