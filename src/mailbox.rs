//! A small single-team relay; authenticates sender and never opens member databases.
use crate::{store::now, team::Message};
use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path as FsPath, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayMember {
    pub member_id: String,
    pub display_name: String,
    pub credential_env: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayConfig {
    pub team_id: String,
    pub members: Vec<RelayMember>,
    #[serde(default)]
    pub secrets_file: Option<PathBuf>,
}
#[derive(Clone)]
struct Relay {
    db: PathBuf,
    team: String,
}
type ApiError = (StatusCode, Json<Value>);
fn problem(status: StatusCode, text: &str) -> ApiError {
    (status, Json(json!({"error":text})))
}
fn connection(db: &FsPath) -> Result<Connection> {
    let c = Connection::open(db)?;
    c.busy_timeout(Duration::from_secs(5))?;
    Ok(c)
}
fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
fn auth(c: &Connection, headers: &HeaderMap) -> std::result::Result<String, ApiError> {
    let value = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| problem(StatusCode::UNAUTHORIZED, "member credential required"))?;
    c.query_row(
        "SELECT id FROM relay_members WHERE token_hash=?1",
        [digest(value)],
        |r| r.get(0),
    )
    .optional()
    .map_err(|_| problem(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable"))?
    .ok_or_else(|| problem(StatusCode::UNAUTHORIZED, "unknown member credential"))
}
pub fn router(db: &FsPath, cfg: &RelayConfig) -> Result<Router> {
    ensure!(
        (2..10).contains(&cfg.members.len()),
        "configure between two and nine members"
    );
    if let Some(p) = db.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut c = connection(db)?;
    c.pragma_update(None, "journal_mode", "WAL")?;
    c.execute_batch("CREATE TABLE IF NOT EXISTS relay_team(id TEXT PRIMARY KEY); CREATE TABLE IF NOT EXISTS relay_members(id TEXT PRIMARY KEY,name TEXT NOT NULL,token_hash TEXT NOT NULL UNIQUE); CREATE TABLE IF NOT EXISTS relay_messages(id TEXT PRIMARY KEY,sender TEXT NOT NULL,recipient TEXT NOT NULL,payload TEXT NOT NULL,received INTEGER NOT NULL DEFAULT 0,created_at INTEGER NOT NULL);")?;
    c.execute_batch("CREATE TABLE IF NOT EXISTS relay_presence(member_id TEXT PRIMARY KEY,busy INTEGER NOT NULL,seen_at INTEGER NOT NULL)")?;
    let tx = c.transaction()?;
    let team: Option<String> = tx
        .query_row("SELECT id FROM relay_team", [], |r| r.get(0))
        .optional()?;
    if let Some(team) = team {
        ensure!(
            team == cfg.team_id,
            "mailbox already belongs to another team"
        );
    } else {
        tx.execute("INSERT INTO relay_team(id) VALUES(?1)", [&cfg.team_id])?;
    }
    tx.execute("DELETE FROM relay_members", [])?;
    for member in &cfg.members {
        crate::team::validate_env_name(&member.credential_env)?;
        let token = if let Some(path) = &cfg.secrets_file {
            crate::secrets::read_field(path, &member.credential_env)?
        } else {
            std::env::var(&member.credential_env).map_err(|_| {
                anyhow::anyhow!("missing or invalid relay member credential environment variable")
            })?
        };
        ensure!(
            token.len() >= 16,
            "member credential must have at least 16 bytes"
        );
        tx.execute(
            "INSERT INTO relay_members(id,name,token_hash) VALUES(?1,?2,?3)",
            params![member.member_id, member.display_name, digest(&token)],
        )?;
    }
    tx.commit()?;
    Ok(Router::new()
        .route("/v1/whoami", get(whoami))
        .route("/v1/presence", get(presence_get).post(presence_post))
        .route("/v1/messages", post(send))
        .route("/v1/inbox", get(inbox))
        .route("/v1/ack/{id}", post(ack))
        .route("/v1/status/{id}", get(status))
        .layer(DefaultBodyLimit::max(65536))
        .with_state(Arc::new(Relay {
            db: db.canonicalize()?,
            team: cfg.team_id.clone(),
        })))
}
async fn send(
    State(relay): State<Arc<Relay>>,
    headers: HeaderMap,
    Json(msg): Json<Message>,
) -> std::result::Result<Json<Value>, ApiError> {
    let mut c = connection(&relay.db)
        .map_err(|_| problem(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable"))?;
    let sender = auth(&c, &headers)?;
    if msg.sender != sender {
        return Err(problem(
            StatusCode::FORBIDDEN,
            "sender does not match credential",
        ));
    }
    msg.validate()
        .map_err(|_| problem(StatusCode::BAD_REQUEST, "invalid message"))?;
    let tx = c
        .transaction()
        .map_err(|_| problem(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable"))?;
    let recipient: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM relay_members WHERE id=?1)",
            [&msg.recipient],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !recipient {
        return Err(problem(StatusCode::BAD_REQUEST, "unknown recipient"));
    }
    let payload = serde_json::to_string(&msg).unwrap();
    let previous: Option<String> = tx
        .query_row(
            "SELECT payload FROM relay_messages WHERE id=?1",
            [&msg.message_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| problem(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable"))?;
    if let Some(old) = previous {
        if old != payload {
            return Err(problem(StatusCode::CONFLICT, "message_id conflict"));
        }
    } else {
        tx.execute("INSERT INTO relay_messages(id,sender,recipient,payload,created_at) VALUES(?1,?2,?3,?4,?5)",params![msg.message_id,sender,msg.recipient,payload,now()]).map_err(|_|problem(StatusCode::INTERNAL_SERVER_ERROR,"storage unavailable"))?;
    }
    tx.commit()
        .map_err(|_| problem(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable"))?;
    Ok(Json(json!({"persisted":true,"message_id":msg.message_id})))
}
async fn inbox(
    State(relay): State<Arc<Relay>>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    let c = connection(&relay.db)
        .map_err(|_| problem(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable"))?;
    let me = auth(&c, &headers)?;
    let query = || -> Result<Vec<Value>> {
        let mut q=c.prepare("SELECT payload FROM relay_messages WHERE recipient=?1 AND received=0 ORDER BY rowid LIMIT 50")?;
        q.query_map([me], |r| r.get::<_, String>(0))?
            .map(|v| Ok(serde_json::from_str(&v?)?))
            .collect()
    };
    Ok(Json(
        json!({"team_id":relay.team,"messages":query().map_err(|_|problem(StatusCode::INTERNAL_SERVER_ERROR,"storage unavailable"))?}),
    ))
}
async fn ack(
    State(relay): State<Arc<Relay>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> std::result::Result<Json<Value>, ApiError> {
    let c = connection(&relay.db)
        .map_err(|_| problem(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable"))?;
    let me = auth(&c, &headers)?;
    let n = c
        .execute(
            "UPDATE relay_messages SET received=1 WHERE id=?1 AND recipient=?2",
            params![id, me],
        )
        .map_err(|_| problem(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable"))?;
    if n != 1 {
        return Err(problem(StatusCode::NOT_FOUND, "message not found"));
    }
    Ok(Json(json!({"received":true})))
}
async fn status(
    State(relay): State<Arc<Relay>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> std::result::Result<Json<Value>, ApiError> {
    let c = connection(&relay.db)
        .map_err(|_| problem(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable"))?;
    let me = auth(&c, &headers)?;
    let received: Option<bool> = c
        .query_row(
            "SELECT received FROM relay_messages WHERE id=?1 AND sender=?2",
            params![id, me],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| problem(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable"))?;
    Ok(Json(
        json!({"received":received.ok_or_else(||problem(StatusCode::NOT_FOUND,"message not found"))?}),
    ))
}

async fn whoami(
    State(relay): State<Arc<Relay>>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    let c = connection(&relay.db)
        .map_err(|_| problem(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable"))?;
    let me = auth(&c, &headers)?;
    Ok(Json(json!({"team_id":relay.team,"member_id":me})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PresenceInput {
    activity: String,
}
async fn presence_post(
    State(relay): State<Arc<Relay>>,
    headers: HeaderMap,
    Json(input): Json<PresenceInput>,
) -> std::result::Result<Json<Value>, ApiError> {
    let c = connection(&relay.db)
        .map_err(|_| problem(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable"))?;
    let me = auth(&c, &headers)?;
    if !matches!(input.activity.as_str(), "idle" | "busy") {
        return Err(problem(StatusCode::BAD_REQUEST, "invalid activity"));
    }
    c.execute("INSERT INTO relay_presence(member_id,busy,seen_at) VALUES(?1,?2,?3) ON CONFLICT(member_id) DO UPDATE SET busy=excluded.busy,seen_at=excluded.seen_at",params![me,input.activity=="busy",now()]).map_err(|_|problem(StatusCode::INTERNAL_SERVER_ERROR,"storage unavailable"))?;
    Ok(Json(json!({"accepted":true,"ttl_secs":30})))
}
async fn presence_get(
    State(relay): State<Arc<Relay>>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    let c = connection(&relay.db)
        .map_err(|_| problem(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable"))?;
    auth(&c, &headers)?;
    let query = || -> Result<Vec<Value>> {
        let mut q=c.prepare("SELECT m.id,p.busy,p.seen_at FROM relay_members m LEFT JOIN relay_presence p ON p.member_id=m.id ORDER BY m.id")?;
        Ok(q.query_map([],|r|{let busy:Option<bool>=r.get(1)?;Ok(json!({"member_id":r.get::<_,String>(0)?,"activity":busy.map(|b|if b{"busy"}else{"idle"}),"seen_at":r.get::<_,Option<i64>>(2)?,"ttl_secs":30}))})?.collect::<rusqlite::Result<Vec<_>>>()?)
    };
    Ok(Json(
        json!({"server_time":now(),"members":query().map_err(|_|problem(StatusCode::INTERNAL_SERVER_ERROR,"storage unavailable"))?}),
    ))
}
