//! Device HTTP adapter; remote member OAuth has a separate authentication boundary.
use super::{Actor, InboxService, PrivateFacts, Reply, SendMessage, Viewed};
use crate::mailbox::{ApiError, auth, connection, problem};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Page {
    after: i64,
    before: i64,
    limit: usize,
    filter: String,
}
impl Default for Page {
    fn default() -> Self {
        Self {
            after: 0,
            before: 0,
            limit: 50,
            filter: "all".into(),
        }
    }
}
pub fn router(db: PathBuf) -> Router {
    Router::new()
        .route("/v2/member-inbox", get(list))
        .route("/v2/member-inbox/changes", get(changes))
        .route("/v2/member-inbox/synced", post(synced))
        .route("/v2/member-inbox/messages", post(send))
        .route("/v2/member-inbox/threads/{id}", get(read))
        .route("/v2/member-inbox/threads/{id}/reply", post(reply))
        .route("/v2/member-inbox/threads/{id}/viewed", post(viewed))
        .route("/v2/member-inbox/threads/{id}/facts", post(facts))
        .layer(DefaultBodyLimit::max(65536))
        .with_state(db)
}
pub(crate) fn error(e: anyhow::Error) -> ApiError {
    let code = e.to_string();
    let status = match code.as_str() {
        "unknown_member" | "thread_not_visible" => StatusCode::NOT_FOUND,
        "send_forbidden" | "human_view_forbidden" => StatusCode::FORBIDDEN,
        "idempotency_conflict" => StatusCode::CONFLICT,
        "rate_limited" => StatusCode::TOO_MANY_REQUESTS,
        "invalid_text"
        | "invalid_page"
        | "invalid_recipient"
        | "invalid_reply_target"
        | "invalid_view_cursor" => StatusCode::BAD_REQUEST,
        _ => return problem(StatusCode::SERVICE_UNAVAILABLE, "inbox_unavailable"),
    };
    problem(status, &code)
}
fn call(
    db: &std::path::Path,
    headers: &HeaderMap,
    f: impl FnOnce(InboxService<'_>) -> anyhow::Result<Value>,
) -> Result<Json<Value>, ApiError> {
    let conn = connection(db).map_err(error)?;
    let actor = Actor {
        member: auth(&conn, headers)?,
        origin: "terminal".into(),
        can_send: true,
        human_view: true,
    };
    f(InboxService {
        conn: &conn,
        actor: &actor,
    })
    .map(Json)
    .map_err(error)
}
async fn list(
    State(db): State<PathBuf>,
    headers: HeaderMap,
    Query(p): Query<Page>,
) -> Result<Json<Value>, ApiError> {
    call(&db, &headers, |s| s.list(&p.filter, p.before, p.limit))
}
async fn changes(
    State(db): State<PathBuf>,
    headers: HeaderMap,
    Query(p): Query<Page>,
) -> Result<Json<Value>, ApiError> {
    call(&db, &headers, |s| s.changes(p.after, p.limit))
}
async fn read(
    State(db): State<PathBuf>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(p): Query<Page>,
) -> Result<Json<Value>, ApiError> {
    call(&db, &headers, |s| s.read(&id, p.after, p.limit))
}
async fn send(
    State(db): State<PathBuf>,
    headers: HeaderMap,
    Json(m): Json<SendMessage>,
) -> Result<Json<Value>, ApiError> {
    call(&db, &headers, |s| s.send(&m))
}
async fn reply(
    State(db): State<PathBuf>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(m): Json<Reply>,
) -> Result<Json<Value>, ApiError> {
    call(&db, &headers, |s| s.reply(&id, &m))
}
async fn viewed(
    State(db): State<PathBuf>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(m): Json<Viewed>,
) -> Result<Json<Value>, ApiError> {
    call(&db, &headers, |s| s.viewed(&id, m.through_seq))
}
async fn facts(
    State(db): State<PathBuf>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(m): Json<PrivateFacts>,
) -> Result<Json<Value>, ApiError> {
    call(&db, &headers, |s| s.private_facts(&id, &m))
}
async fn synced(
    State(db): State<PathBuf>,
    headers: HeaderMap,
    Json(m): Json<Viewed>,
) -> Result<Json<Value>, ApiError> {
    call(&db, &headers, |s| s.synced(m.through_seq))
}
