//! Separately versioned binary channel. Existing message parsers never see attachments.
use super::*;
use crate::{
    mailbox::{ApiError, auth, connection, problem},
    store::now,
};
use anyhow::Context;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{fs, io::Write, path::PathBuf, sync::Arc};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_file: u64,
    pub max_batch: u64,
    pub member_quota: u64,
    pub team_quota: u64,
    pub retention_secs: i64,
    pub receipt_retention_secs: i64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_file: GIB,
            max_batch: 2 * GIB,
            member_quota: 5 * GIB,
            team_quota: 20 * GIB,
            retention_secs: 7 * 86400,
            receipt_retention_secs: 86400,
        }
    }
}
struct Relay {
    db: PathBuf,
    root: PathBuf,
    limits: Limits,
}
fn migrate(c: &rusqlite::Connection) -> Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS relay_file_capabilities(member TEXT PRIMARY KEY,protocol INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS relay_file_transfers(id TEXT PRIMARY KEY,sender TEXT NOT NULL,recipient TEXT NOT NULL,manifest TEXT NOT NULL,state TEXT NOT NULL,bytes INTEGER NOT NULL,expires_at INTEGER NOT NULL,purge_at INTEGER NOT NULL,purged INTEGER NOT NULL DEFAULT 0,created_at INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS relay_file_chunks(transfer_id TEXT NOT NULL,file_id TEXT NOT NULL,chunk INTEGER NOT NULL,bytes INTEGER NOT NULL,sha256 TEXT NOT NULL,PRIMARY KEY(transfer_id,file_id,chunk));")?;
    Ok(())
}
pub fn router(db: &Path, limits: &Limits) -> Result<Router> {
    ensure!(
        limits.max_file > 0
            && limits.max_file <= 64 * GIB
            && limits.max_batch >= limits.max_file
            && limits.max_batch <= 128 * GIB
            && limits.member_quota >= limits.max_batch
            && limits.team_quota >= limits.member_quota
            && limits.retention_secs > 0
            && limits.receipt_retention_secs >= 0,
        "invalid transfer limits"
    );
    let db = db.canonicalize()?;
    let root = db.with_extension("attachments");
    super::local::private_dir(&root)?;
    migrate(&connection(&db)?)?;
    let s = Arc::new(Relay {
        db,
        root,
        limits: limits.clone(),
    });
    let weak = Arc::downgrade(&s);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let Some(s) = weak.upgrade() else {
                break;
            };
            let _ = tokio::task::spawn_blocking(move || cleanup(&s)).await;
        }
    });
    Ok(Router::new()
        .route(
            "/v1/files/capabilities",
            get(peer_capabilities).post(capabilities),
        )
        .route("/v1/files", get(inbox).post(create))
        .route("/v1/files/{id}", get(status))
        .route(
            "/v1/files/{id}/{file}/chunks/{chunk}",
            put(upload).get(download),
        )
        .route("/v1/files/{id}/complete", post(complete))
        .route("/v1/files/{id}/receipt", post(receipt))
        .route("/v1/files/{id}/revoke", post(revoke))
        .layer(DefaultBodyLimit::max(CHUNK as usize))
        .with_state(s))
}
async fn run<T: Send + 'static>(
    s: Arc<Relay>,
    headers: HeaderMap,
    f: impl FnOnce(&Relay, &mut rusqlite::Connection, &str) -> Result<T> + Send + 'static,
) -> std::result::Result<T, ApiError> {
    tokio::task::spawn_blocking(move || {
        let mut c = connection(&s.db)
            .map_err(|_| problem(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable"))?;
        let me = auth(&c, &headers)?;
        f(&s, &mut c, &me).map_err(|e| {
            let msg = e.to_string();
            let code = if msg == "not found" {
                StatusCode::NOT_FOUND
            } else if msg.contains("conflict") || msg.contains("unsupported") {
                StatusCode::CONFLICT
            } else if msg.contains("quota") {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                StatusCode::BAD_REQUEST
            };
            problem(code, &msg)
        })
    })
    .await
    .map_err(|_| {
        problem(
            StatusCode::INTERNAL_SERVER_ERROR,
            "transfer worker unavailable",
        )
    })?
}
fn load(c: &rusqlite::Connection, id: &str, me: &str) -> Result<(Manifest, String)> {
    uuid::Uuid::parse_str(id)?;
    let row:Option<(String,String)>=c.query_row("SELECT manifest,state FROM relay_file_transfers WHERE id=?1 AND (sender=?2 OR recipient=?2)",params![id,me],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (raw, state) = row.context("not found")?;
    Ok((serde_json::from_str(&raw)?, state))
}
fn info(c: &rusqlite::Connection, id: &str, me: &str) -> Result<Value> {
    c.execute("UPDATE relay_file_transfers SET state='expired' WHERE id=?1 AND expires_at<=?2 AND state IN ('uploading','available')",params![id,now()])?;
    let (m, state) = load(c, id, me)?;
    let expires: i64 = c.query_row(
        "SELECT expires_at FROM relay_file_transfers WHERE id=?1",
        [id],
        |r| r.get(0),
    )?;
    let mut q=c.prepare("SELECT file_id,chunk,bytes,sha256 FROM relay_file_chunks WHERE transfer_id=?1 ORDER BY file_id,chunk")?;
    let chunks=q.query_map([id],|r|Ok(json!({"file_id":r.get::<_,String>(0)?,"chunk":r.get::<_,u64>(1)?,"bytes":r.get::<_,u64>(2)?,"sha256":r.get::<_,String>(3)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({"manifest":m,"state":state,"expires_at":expires,"chunks":chunks}))
}
fn active(c: &rusqlite::Connection, id: &str) -> Result<()> {
    let expires: i64 = c.query_row(
        "SELECT expires_at FROM relay_file_transfers WHERE id=?1",
        [id],
        |r| r.get(0),
    )?;
    ensure!(expires > now(), "transfer expired");
    Ok(())
}
async fn peer_capabilities(
    State(s): State<Arc<Relay>>,
    h: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    run(s, h, |_, c, _| {
        let mut q = c.prepare(
            "SELECT member FROM relay_file_capabilities WHERE protocol=1 ORDER BY member",
        )?;
        let members = q
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Json(json!({"protocol":1,"members":members})))
    })
    .await
}
async fn capabilities(
    State(s): State<Arc<Relay>>,
    h: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    run(s,h,|s,c,me|{c.execute("INSERT INTO relay_file_capabilities VALUES(?1,1) ON CONFLICT(member) DO UPDATE SET protocol=1",[me])?;Ok(Json(json!({"protocol":1,"limits":s.limits,"chunk_bytes":CHUNK}))) }).await
}
#[derive(Deserialize, Default)]
struct Page {
    #[serde(default)]
    after: i64,
}
async fn inbox(
    State(s): State<Arc<Relay>>,
    h: HeaderMap,
    Query(page): Query<Page>,
) -> std::result::Result<Json<Value>, ApiError> {
    run(s,h,move |_s,c,me| {
        ensure!(page.after >= 0, "invalid cursor");
        let mut q=c.prepare("SELECT rowid,id FROM relay_file_transfers WHERE recipient=?1 AND state!='uploading' AND rowid>?2 ORDER BY rowid LIMIT 50")?;
        let rows=q.query_map(params![me,page.after],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let next=if rows.len()==50 { rows.last().map(|r|r.0) } else { None };
        Ok(Json(json!({"items":rows.iter().map(|(_,id)|info(c,id,me)).collect::<Result<Vec<_>>>()?,"next":next})))
    }).await
}
async fn status(
    State(s): State<Arc<Relay>>,
    h: HeaderMap,
    RoutePath(id): RoutePath<String>,
) -> std::result::Result<Json<Value>, ApiError> {
    run(s, h, move |_, c, me| Ok(Json(info(c, &id, me)?))).await
}
async fn create(
    State(s): State<Arc<Relay>>,
    h: HeaderMap,
    Json(m): Json<Manifest>,
) -> std::result::Result<Json<Value>, ApiError> {
    run(s,h,move|s,c,me|{
    m.validate(s.limits.max_file,s.limits.max_batch)?;ensure!(m.sender==me,"sender mismatch");
    let tx=c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM relay_members WHERE id=?1)",[&m.recipient],|r|r.get(0))?;ensure!(valid,"unknown recipient");
    let raw=serde_json::to_string(&m)?;
    let previous:Option<String>=tx.query_row("SELECT manifest FROM relay_file_transfers WHERE id=?1",[&m.id],|r|r.get(0)).optional()?;
    if let Some(old)=previous {ensure!(old==raw,"manifest conflict");tx.commit()?;return Ok(Json(info(c,&m.id,me)?));}
    let ready:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM relay_file_capabilities WHERE member=?1 AND protocol=1)",[&m.recipient],|r|r.get(0))?;ensure!(ready,"recipient unsupported; 对方需启动支持附件的新版客户端");
    if let Some(id)=&m.replaces {let(old,_)=load(&tx,id,me)?;ensure!(old.sender==me&&old.recipient==m.recipient,"replacement conflict");}
    let(team,member):(u64,u64)=tx.query_row("SELECT COALESCE(SUM(bytes),0),COALESCE(SUM(CASE WHEN sender=?1 THEN bytes ELSE 0 END),0) FROM relay_file_transfers WHERE purged=0",[me],|r|Ok((r.get(0)?,r.get(1)?)))?;
    ensure!(m.bytes()<=s.limits.team_quota.saturating_sub(team)&&m.bytes()<=s.limits.member_quota.saturating_sub(member),"relay quota exhausted");
    space_available(&s.root,m.bytes().saturating_mul(2))?;
    tx.execute("INSERT INTO relay_file_transfers(id,sender,recipient,manifest,state,bytes,expires_at,purge_at,created_at) VALUES(?1,?2,?3,?4,'uploading',?5,?6,?6,?7)",params![m.id,m.sender,m.recipient,raw,m.bytes(),now()+s.limits.retention_secs,now()])?;tx.commit()?;
    Ok(Json(info(c,&m.id,me)?))
}).await
}
fn chunk_path(s: &Relay, id: &str, file: &str, chunk: u64) -> PathBuf {
    s.root.join(id).join(file).join(format!("{chunk}.chunk"))
}
fn spec<'a>(m: &'a Manifest, file: &str, chunk: u64) -> Result<(&'a FileSpec, u64)> {
    let f = m.files.iter().find(|f| f.id == file).context("not found")?;
    let offset = chunk.checked_mul(CHUNK).context("invalid chunk")?;
    ensure!(offset < f.bytes, "invalid chunk");
    Ok((f, (f.bytes - offset).min(CHUNK)))
}
async fn upload(
    State(s): State<Arc<Relay>>,
    h: HeaderMap,
    RoutePath((id, file, n)): RoutePath<(String, String, u64)>,
    bytes: Bytes,
) -> std::result::Result<Json<Value>, ApiError> {
    let claimed = h
        .get("x-content-sha256")
        .and_then(|x| x.to_str().ok())
        .unwrap_or("")
        .to_string();
    run(s,h,move|s,c,me|{
        uuid::Uuid::parse_str(&id)?;let _lock=crate::supervisor::TaskLock::acquire(&s.db,&id)?;
        let(m,state)=load(c,&id,me)?;ensure!(m.sender==me,"not found");active(c,&id)?;ensure!(state=="uploading","transfer is not uploading");let(_,len)=spec(&m,&file,n)?;
        ensure!(bytes.len() as u64==len && digest(&bytes)==claimed,"chunk checksum mismatch");
        let prior:Option<(u64,String)>=c.query_row("SELECT bytes,sha256 FROM relay_file_chunks WHERE transfer_id=?1 AND file_id=?2 AND chunk=?3",params![id,file,n],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some(old)=prior {ensure!(old==(len,claimed),"chunk conflict");return Ok(Json(json!({"stored":true})));}
        let path=chunk_path(s,&id,&file,n);let parent=path.parent().unwrap();super::local::private_dir(parent)?;space_available(&s.root,len)?;
        let mut temp=tempfile::NamedTempFile::new_in(parent)?;temp.write_all(&bytes)?;temp.as_file().sync_all()?;temp.persist(&path)?;durable_dir(parent)?;
        c.execute("INSERT INTO relay_file_chunks VALUES(?1,?2,?3,?4,?5)",params![id,file,n,len,claimed])?;
        Ok(Json(json!({"stored":true})))
    }).await
}
async fn complete(
    State(s): State<Arc<Relay>>,
    h: HeaderMap,
    RoutePath(id): RoutePath<String>,
) -> std::result::Result<Json<Value>, ApiError> {
    run(s,h,move|s,c,me|{
    uuid::Uuid::parse_str(&id)?;let _lock=crate::supervisor::TaskLock::acquire(&s.db,&id)?;let(m,state)=load(c,&id,me)?;ensure!(m.sender==me,"not found");
    if matches!(state.as_str(),"available"|"received"){return Ok(Json(info(c,&id,me)?));}active(c,&id)?;ensure!(state=="uploading","transfer cannot be published");
    let dir=s.root.join(&id);super::local::private_dir(&dir)?;
    for f in &m.files {
        let destination=dir.join(format!("{}.data",f.id));
        if destination.exists() && checksum(&destination)?==(f.bytes,f.sha256.clone()){continue;}
        space_available(&s.root,f.bytes)?;let mut out=tempfile::NamedTempFile::new_in(&dir)?;let mut hash=Sha256::new();let mut written=0u64;
        for n in 0..f.bytes.div_ceil(CHUNK) {
            let expected:Option<(u64,String)>=c.query_row("SELECT bytes,sha256 FROM relay_file_chunks WHERE transfer_id=?1 AND file_id=?2 AND chunk=?3",params![id,f.id,n],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let (len,h)=expected.context("upload incomplete")?;let data=super::local::read_chunk(&chunk_path(s,&id,&f.id,n),0,len)?;ensure!(digest(&data)==h,"stored chunk checksum mismatch");out.write_all(&data)?;hash.update(&data);written+=len;
        }
        ensure!(written==f.bytes&&format!("{:x}",hash.finalize())==f.sha256,"file checksum mismatch");out.as_file().sync_all()?;out.persist(&destination)?;durable_dir(&dir)?;
    }
    c.execute("UPDATE relay_file_transfers SET state='available' WHERE id=?1 AND state='uploading'",[&id])?;
    for f in &m.files { let dir=s.root.join(&id).join(&f.id); if dir.exists(){fs::remove_dir_all(dir)?;} }
    Ok(Json(info(c,&id,me)?))
}).await
}
async fn download(
    State(s): State<Arc<Relay>>,
    h: HeaderMap,
    RoutePath((id, file, n)): RoutePath<(String, String, u64)>,
) -> std::result::Result<(HeaderMap, Vec<u8>), ApiError> {
    run(s, h, move |s, c, me| {
        uuid::Uuid::parse_str(&id)?;
        let _lock = crate::supervisor::TaskLock::acquire(&s.db, &id)?;
        let (m, state) = load(c, &id, me)?;
        ensure!(m.recipient == me, "not found");
        active(c, &id)?;
        ensure!(
            matches!(state.as_str(), "available" | "received"),
            "attachment unavailable"
        );
        let (_, len) = spec(&m, &file, n)?;
        let bytes = super::local::read_chunk(
            &s.root.join(&id).join(format!("{file}.data")),
            n * CHUNK,
            len,
        )?;
        let mut headers = HeaderMap::new();
        headers.insert("x-content-sha256", digest(&bytes).parse()?);
        headers.insert("content-type", "application/octet-stream".parse()?);
        Ok((headers, bytes))
    })
    .await
}
async fn receipt(
    State(s): State<Arc<Relay>>,
    h: HeaderMap,
    RoutePath(id): RoutePath<String>,
    Json(received): Json<Manifest>,
) -> std::result::Result<Json<Value>, ApiError> {
    run(s, h, move |s, c, me| {
        uuid::Uuid::parse_str(&id)?;
        let _lock = crate::supervisor::TaskLock::acquire(&s.db, &id)?;
        let (m, state) = load(c, &id, me)?;
        ensure!(m.recipient == me && received == m, "receipt mismatch");
        ensure!(
            matches!(state.as_str(), "available" | "received"),
            "attachment unavailable"
        );
        if state == "available" {
            c.execute(
                "UPDATE relay_file_transfers SET state='received',purge_at=?2 WHERE id=?1",
                params![id, now() + s.limits.receipt_retention_secs],
            )?;
        }
        Ok(Json(json!({"received":true})))
    })
    .await
}
async fn revoke(
    State(s): State<Arc<Relay>>,
    h: HeaderMap,
    RoutePath(id): RoutePath<String>,
) -> std::result::Result<Json<Value>, ApiError> {
    run(s, h, move |s, c, me| {
        uuid::Uuid::parse_str(&id)?;
        let _lock = crate::supervisor::TaskLock::acquire(&s.db, &id)?;
        let (m, _) = load(c, &id, me)?;
        ensure!(m.sender == me, "not found");
        c.execute(
            "UPDATE relay_file_transfers SET state='revoked',purge_at=?2 WHERE id=?1",
            params![id, now()],
        )?;
        Ok(Json(
            json!({"revoked":true,"downloaded_copies_preserved":true}),
        ))
    })
    .await
}
fn cleanup(s: &Relay) -> Result<()> {
    let c = connection(&s.db)?;
    let mut q=c.prepare("SELECT id FROM relay_file_transfers WHERE purged=0 AND (purge_at<=?1 OR (expires_at<=?1 AND state!='received'))")?;
    let ids = q
        .query_map([now()], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(q);
    for id in ids {
        let Ok(_lock) = crate::supervisor::TaskLock::acquire(&s.db, &id) else {
            continue;
        };
        let dir = s.root.join(&id);
        if dir.exists() {
            fs::remove_dir_all(dir)?;
        }
        c.execute("UPDATE relay_file_transfers SET purged=1,state=CASE WHEN state IN ('received','revoked') THEN state ELSE 'expired' END WHERE id=?1",[&id])?;
        c.execute("DELETE FROM relay_file_chunks WHERE transfer_id=?1", [&id])?;
    }
    Ok(())
}
