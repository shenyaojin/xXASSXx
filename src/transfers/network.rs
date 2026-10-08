use super::local::*;
use super::*;
use crate::{
    app,
    store::{Store, now},
    team::stable_id,
};
use anyhow::{Context, bail};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    time::Duration,
};
async fn json_response(mut r: reqwest::Response) -> Result<Value> {
    let status = r.status();
    let mut b = vec![];
    while let Some(chunk) = r.chunk().await? {
        ensure!(b.len() + chunk.len() <= 4 * 1024 * 1024, "文件接口响应过大");
        b.extend_from_slice(&chunk);
    }
    let v: Value = serde_json::from_slice(&b).unwrap_or(Value::Null);
    ensure!(
        status.is_success(),
        "file_http_{}: {}",
        status.as_u16(),
        v["error"].as_str().unwrap_or("文件服务暂不可用")
    );
    Ok(v)
}
async fn blocking<T: Send + 'static>(
    db: PathBuf,
    f: impl FnOnce(&Store) -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(move || f(&Store::open(&db)?)).await?
}
fn related_task(store: &Store, m: &Manifest) -> Option<Value> {
    let reference = m.task.as_ref()?;
    let task = crate::task_coordinator::get(store, &reference.id).ok()?;
    (task["revision"] == reference.revision
        && task["participants"]
            .as_array()?
            .iter()
            .any(|v| v == &m.sender))
    .then_some(task)
}
fn session_for(store: &Store, m: &Manifest) -> Result<String> {
    if m.task.is_some() {
        if let Some(current) = related_task(store, m) {
            return Ok(current["session_id"]
                .as_str()
                .context("missing task session")?
                .into());
        }
    }
    let work: Option<String> = store
        .conn
        .query_row(
            "SELECT value FROM app_preferences WHERE key='working_directory'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let work = work
        .map(PathBuf::from)
        .unwrap_or(store.member_config()?.executor.workdir);
    let p = app::project(store, &work)?;
    app::session(store, p["id"].as_str().unwrap(), &store.owner()?)
}
fn ingest(store: &Store, v: &Value) -> Result<()> {
    let m: Manifest = serde_json::from_value(v["manifest"].clone())?;
    let p = preferences(store)?;
    m.validate(
        p["max_file"].as_u64().unwrap(),
        p["max_batch"].as_u64().unwrap(),
    )?;
    ensure!(m.recipient == store.owner()?, "文件接收身份不匹配");
    let old: Option<String> = store
        .conn
        .query_row(
            "SELECT manifest FROM file_transfers WHERE id=?1",
            [&m.id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(raw) = old {
        ensure!(
            serde_json::from_str::<Manifest>(&raw)? == m,
            "附件清单发生变化"
        );
        if get(store, &m.id)?["state"] != "received"
            && matches!(v["state"].as_str(), Some("revoked" | "expired"))
        {
            state(store, &m.id, v["state"].as_str().unwrap(), None)?;
        }
        return Ok(());
    }
    let session = session_for(store, &m)?;
    let task = m
        .task
        .as_ref()
        .filter(|_| related_task(store, &m).is_some());
    let requested = m.bytes() <= p["auto_receive"].as_u64().unwrap();
    let status = match v["state"].as_str() {
        Some("expired") => "expired",
        Some("revoked") => "revoked",
        _ => "offered",
    };
    store.conn.execute("INSERT INTO file_transfers(id,direction,session_id,task_id,revision,manifest,state,requested,created_at,updated_at) VALUES(?1,'in',?2,?3,?4,?5,?6,?7,?8,?8)",params![m.id,session,task.map(|t|&t.id),task.map(|t|t.revision),serde_json::to_string(&m)?,status,requested,now()])?;
    state(store, &m.id, status, None)?;
    Ok(())
}
fn still_authorized(store: &Store, id: &str) -> Result<bool> {
    let v = get(store, id)?;
    if v["authorized"] != true
        || matches!(
            v["state"].as_str(),
            Some("revoking" | "revoked" | "expired")
        )
    {
        return Ok(false);
    }
    let revoked: bool = store.conn.query_row(
        "SELECT revoked FROM transfer_grants WHERE id=?1",
        [id],
        |r| r.get(0),
    )?;
    if revoked {
        return Ok(false);
    }
    if let Some(t) = manifest(store, id)?.task {
        let current = crate::task_coordinator::get(store, &t.id)?;
        if current["revision"] != t.revision
            || current["state"] == "cancelled"
            || (current["draft"]["mode"] == "execute"
                && !crate::task_workspace::has_grant(&store.conn, &t.id, t.revision)?)
        {
            revoke(store, id)?;
            return Ok(false);
        }
    }
    Ok(true)
}
async fn outgoing(
    store: &Store,
    c: &reqwest::Client,
    url: &str,
    token: &str,
    id: &str,
) -> Result<()> {
    let m = manifest(store, id)?;
    let current = get(store, id)?;
    if current["state"] == "revoking" {
        let r = c
            .post(format!("{url}/v1/files/{id}/revoke"))
            .bearer_auth(token)
            .send()
            .await?;
        if r.status() != reqwest::StatusCode::NOT_FOUND {
            json_response(r).await?;
        }
        state(store, id, "revoked", None)?;
        return Ok(());
    }
    if !still_authorized(store, id)? {
        return Ok(());
    }
    let remote = json_response(
        c.post(format!("{url}/v1/files"))
            .bearer_auth(token)
            .json(&m)
            .send()
            .await?,
    )
    .await?;
    match remote["state"].as_str() {
        Some("received") => {
            state(store, id, "delivered", None)?;
            return Ok(());
        }
        Some("revoked" | "expired") => {
            state(store, id, remote["state"].as_str().unwrap(), None)?;
            return Ok(());
        }
        Some("available") => {
            state(store, id, "available", None)?;
            return Ok(());
        }
        Some("uploading") => {}
        _ => bail!("无效的文件服务状态"),
    }
    state(store, id, "uploading", None)?;
    let chunks = remote["chunks"].as_array().context("missing chunk state")?;
    let mut sent = 0;
    for f in &m.files {
        let p: PathBuf = store
            .conn
            .query_row(
                "SELECT path FROM transfer_files WHERE transfer_id=?1 AND file_id=?2",
                params![id, f.id],
                |r| r.get::<_, String>(0),
            )?
            .into();
        let present: u64 = chunks
            .iter()
            .filter(|x| x["file_id"] == f.id)
            .filter_map(|x| x["bytes"].as_u64())
            .sum();
        store.conn.execute(
            "UPDATE transfer_files SET offset=?3 WHERE transfer_id=?1 AND file_id=?2",
            params![id, f.id, present],
        )?;
        for n in 0..f.bytes.div_ceil(CHUNK) {
            if chunks
                .iter()
                .any(|x| x["file_id"] == f.id && x["chunk"] == n)
            {
                continue;
            }
            if !still_authorized(store, id)? {
                return Ok(());
            }
            let length = (f.bytes - n * CHUNK).min(CHUNK);
            let path = p.clone();
            let bytes =
                tokio::task::spawn_blocking(move || read_chunk(&path, n * CHUNK, length)).await??;
            json_response(
                c.put(format!("{url}/v1/files/{id}/{}/chunks/{n}", f.id))
                    .bearer_auth(token)
                    .header("x-content-sha256", digest(&bytes))
                    .body(bytes)
                    .send()
                    .await?,
            )
            .await?;
            store.conn.execute("UPDATE transfer_files SET offset=MIN(?3,offset+?4) WHERE transfer_id=?1 AND file_id=?2",params![id,f.id,f.bytes,length])?;
            sent += 1;
            if sent >= 4 {
                return Ok(());
            }
        }
    }
    if still_authorized(store, id)? {
        let r = json_response(
            c.post(format!("{url}/v1/files/{id}/complete"))
                .bearer_auth(token)
                .send()
                .await?,
        )
        .await?;
        ensure!(
            r["state"] == "available" || r["state"] == "received",
            "附件尚未发布"
        );
        state(
            store,
            id,
            if r["state"] == "received" {
                "delivered"
            } else {
                "available"
            },
            None,
        )?;
    }
    Ok(())
}
fn receive_offset(store: &Store, m: &Manifest, f: &FileSpec) -> Result<(PathBuf, u64)> {
    let dir = base(store).join("incoming").join(&m.id).join(&f.id);
    private_dir(&dir)?;
    let dest = dir.join(&f.name);
    let part = dir.join(".download.part");
    let known: Option<(String, u64)> = store
        .conn
        .query_row(
            "SELECT path,offset FROM transfer_files WHERE transfer_id=?1 AND file_id=?2",
            params![m.id, f.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if dest.exists() && dest != part {
        ensure!(
            checksum(&dest)? == (f.bytes, f.sha256.clone()),
            "本地附件目标已改变；不会覆盖"
        );
        store.conn.execute("INSERT INTO transfer_files(transfer_id,file_id,path,offset) VALUES(?1,?2,?3,?4) ON CONFLICT(transfer_id,file_id) DO UPDATE SET path=excluded.path,offset=excluded.offset",params![m.id,f.id,dest.to_str(),f.bytes])?;
        return Ok((dest, f.bytes));
    }
    space_available(&dir, f.bytes)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&part)?;
    let actual = file.metadata()?.len();
    let mut offset = known.map_or(0, |(_, n)| n);
    if actual < offset || offset > f.bytes {
        offset = 0;
    }
    file.set_len(offset)?;
    store.conn.execute("INSERT INTO transfer_files(transfer_id,file_id,path,offset) VALUES(?1,?2,?3,?4) ON CONFLICT(transfer_id,file_id) DO UPDATE SET path=excluded.path,offset=excluded.offset",params![m.id,f.id,part.to_str(),offset])?;
    Ok((part, offset))
}
fn append(
    store: &Store,
    id: &str,
    file: &str,
    path: &Path,
    offset: u64,
    bytes: &[u8],
) -> Result<()> {
    let mut output = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    ensure!(output.metadata()?.len() == offset, "下载进度发生冲突");
    output.seek(SeekFrom::Start(offset))?;
    output.write_all(bytes)?;
    output.sync_all()?;
    store.conn.execute(
        "UPDATE transfer_files SET offset=?3 WHERE transfer_id=?1 AND file_id=?2",
        params![id, file, offset + bytes.len() as u64],
    )?;
    Ok(())
}
fn finalize_file(store: &Store, m: &Manifest, f: &FileSpec, path: &Path) -> Result<()> {
    ensure!(
        checksum(path)? == (f.bytes, f.sha256.clone()),
        "下载文件校验失败"
    );
    let dir = path.parent().context("missing attachment directory")?;
    let dest = dir.join(&f.name);
    if dest != path {
        ensure!(!dest.exists(), "附件目标文件已存在");
        fs::rename(path, &dest)?;
        durable_dir(dir)?;
    }
    store.conn.execute(
        "UPDATE transfer_files SET path=?3,offset=?4 WHERE transfer_id=?1 AND file_id=?2",
        params![m.id, f.id, dest.to_str(), f.bytes],
    )?;
    Ok(())
}
async fn incoming(
    store: &Store,
    c: &reqwest::Client,
    url: &str,
    token: &str,
    id: &str,
) -> Result<()> {
    let v = get(store, id)?;
    let m = manifest(store, id)?;
    if v["state"] == "received" {
        let r = c
            .post(format!("{url}/v1/files/{id}/receipt"))
            .bearer_auth(token)
            .json(&m)
            .send()
            .await?;
        if r.status().is_success() {
            json_response(r).await?;
            store.conn.execute(
                "UPDATE file_transfers SET requested=0,error=NULL WHERE id=?1",
                [id],
            )?;
        }
        return Ok(());
    }
    state(store, id, "downloading", None)?;
    let mut count = 0;
    for f in &m.files {
        let db = store.path.clone();
        let mf = m.clone();
        let ff = f.clone();
        let (path, mut offset) = blocking(db, move |s| receive_offset(s, &mf, &ff)).await?;
        while offset < f.bytes {
            let n = offset / CHUNK;
            ensure!(offset % CHUNK == 0, "下载进度不在分块边界");
            let mut r = c
                .get(format!("{url}/v1/files/{id}/{}/chunks/{n}", f.id))
                .bearer_auth(token)
                .send()
                .await?;
            if !r.status().is_success() {
                json_response(r).await?;
                bail!("下载失败");
            }
            let expected = r
                .headers()
                .get("x-content-sha256")
                .and_then(|h| h.to_str().ok())
                .context("missing chunk checksum")?
                .to_string();
            let mut bytes = vec![];
            let len = (f.bytes - offset).min(CHUNK);
            while let Some(chunk) = r.chunk().await? {
                ensure!(bytes.len() + chunk.len() <= len as usize, "分块长度超限");
                bytes.extend_from_slice(&chunk);
            }
            ensure!(
                bytes.len() as u64 == len && digest(&bytes) == expected,
                "下载分块校验失败"
            );
            let db = store.path.clone();
            let transfer = id.to_owned();
            let file = f.id.clone();
            let dest = path.clone();
            blocking(db, move |s| {
                append(s, &transfer, &file, &dest, offset, &bytes)
            })
            .await?;
            offset += len;
            count += 1;
            if count >= 4 && offset < f.bytes {
                return Ok(());
            }
        }
        let db = store.path.clone();
        let mf = m.clone();
        let ff = f.clone();
        blocking(db, move |s| finalize_file(s, &mf, &ff, &path)).await?;
    }
    state(store, id, "received", None)?;
    json_response(
        c.post(format!("{url}/v1/files/{id}/receipt"))
            .bearer_auth(token)
            .json(&m)
            .send()
            .await?,
    )
    .await?;
    store
        .conn
        .execute("UPDATE file_transfers SET requested=0 WHERE id=?1", [id])?;
    Ok(())
}
pub async fn sync(store: &Store) -> Result<Value> {
    let _lock = crate::supervisor::TaskLock::acquire(
        &store.path,
        &stable_id(&store.owner()?, "file-transfer-worker"),
    )?;
    let cfg = store.member_config()?;
    let token = cfg.credential()?;
    let url = cfg.mailbox_url.trim_end_matches('/');
    let c = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let who = json_response(
        c.get(format!("{url}/v1/whoami"))
            .bearer_auth(&token)
            .send()
            .await?,
    )
    .await?;
    ensure!(
        who["team_id"] == store.identity()?["team_id"] && who["member_id"] == store.owner()?,
        "信箱身份不匹配"
    );
    if who["file_transfer_protocol"] != 1 {
        for v in list(store)?
            .into_iter()
            .filter(|v| v["direction"] == "out" && v["state"] == "queued")
        {
            state(
                store,
                v["id"].as_str().unwrap(),
                "unsupported",
                Some("信箱需要升级"),
            )?;
        }
        return Ok(json!({"supported":false}));
    }
    json_response(
        c.post(format!("{url}/v1/files/capabilities"))
            .bearer_auth(&token)
            .send()
            .await?,
    )
    .await?;
    let mut errors = vec![];
    let mut cursor = 0;
    loop {
        let page = json_response(
            c.get(format!("{url}/v1/files?after={cursor}"))
                .bearer_auth(&token)
                .send()
                .await?,
        )
        .await?;
        for offer in page["items"]
            .as_array()
            .context("missing attachment inbox")?
        {
            if let Err(e) = ingest(store, offer) {
                errors.push(e.to_string());
            }
        }
        let Some(next) = page["next"].as_i64() else {
            break;
        };
        ensure!(next > cursor, "invalid attachment cursor");
        cursor = next;
    }
    // Completed history must never hide an older in-flight transfer.
    let ids = {
        let mut q = store.conn.prepare("SELECT id FROM file_transfers WHERE (direction='out' AND state IN ('queued','uploading','available','revoking')) OR (direction='in' AND requested=1 AND state IN ('offered','downloading','received')) ORDER BY rowid")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for id in ids {
        let v = get(store, &id)?;
        let id = v["id"].as_str().unwrap();
        let result = if v["direction"] == "out"
            && matches!(
                v["state"].as_str(),
                Some("queued" | "uploading" | "available" | "revoking")
            ) {
            outgoing(store, &c, url, &token, id).await
        } else if v["direction"] == "in"
            && v["requested"] == true
            && matches!(
                v["state"].as_str(),
                Some("offered" | "downloading" | "received")
            )
        {
            incoming(store, &c, url, &token, id).await
        } else {
            continue;
        };
        if let Err(e) = result {
            let error = e.to_string();
            let status = if error.contains("unsupported") {
                Some("unsupported")
            } else if error.contains("file_http_400")
                || error.contains("file_http_413")
                || error.contains("校验失败")
                || error.contains("磁盘")
            {
                Some("failed")
            } else {
                None
            };
            if let Some(status) =
                status.filter(|_| get(store, id).is_ok_and(|v| v["state"] != "received"))
            {
                state(store, id, status, Some(&error))?;
            } else {
                store.conn.execute(
                    "UPDATE file_transfers SET error=?2 WHERE id=?1",
                    params![id, error],
                )?;
            }
            errors.push(error);
        }
    }
    Ok(json!({"supported":true,"errors":errors}))
}
