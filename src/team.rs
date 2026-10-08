//! Persistent local inbox/outbox. No shared database between butlers.
use crate::store::{Store, now};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, time::Duration};
use uuid::Uuid;

pub fn stable_id(scope: &str, key: &str) -> String {
    let h = Sha256::digest(format!("{scope}\0{key}"));
    let mut b = [0; 16];
    b.copy_from_slice(&h[..16]);
    b[6] = (b[6] & 15) | 0x50;
    b[8] = (b[8] & 63) | 0x80;
    Uuid::from_bytes(b).to_string()
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contact {
    pub member_id: String,
    pub display_name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecutorConfig {
    pub mode: String,
    pub codex: PathBuf,
    pub workdir: PathBuf,
    pub timeout_secs: u64,
    pub model: Option<String>,
}
impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            mode: "queue".into(),
            codex: "codex".into(),
            workdir: ".".into(),
            timeout_secs: 600,
            model: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberConfig {
    pub mailbox_url: String,
    pub credential_env: String,
    #[serde(default)]
    pub secrets_file: Option<PathBuf>,
    #[serde(default)]
    pub allow_insecure_http: bool,
    #[serde(default)]
    pub contacts: Vec<Contact>,
    #[serde(default)]
    pub executor: ExecutorConfig,
    #[serde(default)]
    pub model: Value,
}
impl MemberConfig {
    pub fn credential(&self) -> Result<String> {
        let value = if let Some(path) = &self.secrets_file {
            crate::secrets::read_field(path, &self.credential_env)?
        } else {
            std::env::var(&self.credential_env)
                .map_err(|_| anyhow::anyhow!("missing mailbox credential environment variable"))?
        };
        ensure!(!value.is_empty(), "empty mailbox credential");
        Ok(value)
    }
}
pub fn validate_url(url: &str, allow_insecure: bool) -> Result<()> {
    let url = reqwest::Url::parse(url).context("invalid service URL")?;
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "URL must not contain credentials, query or fragment"
    );
    let loopback = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && (loopback || allow_insecure)),
        "use HTTPS outside loopback, or explicitly allow insecure HTTP on a trusted network"
    );
    Ok(())
}
pub fn validate_env_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 128
            && name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
        "invalid credential environment variable name"
    );
    Ok(())
}

pub fn transient_transport_error(error: &str) -> bool {
    matches!(
        error,
        "mailbox_unavailable; outbox retained"
            | "mailbox_unavailable; inbox unchanged"
            | "mailbox ack failed; local receipt retained"
            | "mailbox status unavailable"
            | "http_status_408"
            | "http_status_429"
            | "http_status_500"
            | "http_status_502"
            | "http_status_503"
            | "http_status_504"
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub message_id: String,
    pub conversation_id: String,
    pub sender: String,
    pub recipient: String,
    pub kind: String,
    pub body: String,
    pub created_at: i64,
    pub reply_to: Option<String>,
    pub object_id: Option<String>,
    pub version_id: Option<String>,
    #[serde(default = "auto")]
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<crate::workflow::Wire>,
}
fn auto() -> String {
    "auto".into()
}
impl Message {
    pub fn validate(&self) -> Result<()> {
        for id in [&self.message_id, &self.conversation_id] {
            Uuid::parse_str(id).context("message/conversation ID must be a UUID")?;
        }
        for id in [&self.reply_to, &self.object_id, &self.version_id]
            .into_iter()
            .flatten()
        {
            Uuid::parse_str(id).context("invalid referenced ID")?;
        }
        ensure!(
            self.sender != self.recipient && !self.sender.is_empty() && !self.recipient.is_empty(),
            "invalid sender/recipient"
        );
        ensure!(
            matches!(self.kind.as_str(), "request" | "reply"),
            "invalid message kind"
        );
        ensure!(
            matches!(
                self.operation.as_str(),
                "auto" | "metadata" | "analysis" | "workflow" | "task_v2"
            ),
            "invalid operation"
        );
        ensure!(
            self.kind != "reply" || self.reply_to.is_some(),
            "reply must reference its request"
        );
        ensure!(
            self.version_id.is_none() || self.object_id.is_some(),
            "version requires object_id"
        );
        ensure!(
            !self.body.trim().is_empty() && self.body.len() <= 32768,
            "message body must contain 1..32768 bytes"
        );
        ensure!(
            (self.operation == "workflow") == self.workflow.is_some(),
            "workflow envelope/operation mismatch"
        );
        if self.operation == "task_v2" {
            crate::task_coordinator::Wire::parse(self)?;
        }
        if let Some(w) = &self.workflow {
            w.validate()?;
        }
        // Wire records have no file attachment or local-path field. Reject obvious
        // accidental path disclosure in explicitly authored free text as well.
        ensure!(
            !self.body.contains("file://")
                && !self.body.split_whitespace().any(|s| {
                    // A standalone slash is common prose ("main / current
                    // version"), not a disclosed local filesystem path.
                    (s.starts_with('/') && s != "/")
                        || s.starts_with("C:\\")
                        || s.starts_with("D:\\")
                }),
            "local absolute paths are not allowed in messages"
        );
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct MessageRecord {
    pub message: Message,
    pub direction: String,
    pub state: String,
    pub resolved_version: Option<String>,
    pub error: Option<String>,
}

impl Store {
    pub fn configure_member(&mut self, config: &MemberConfig) -> Result<()> {
        let model = crate::model::ModelConfig::parse(&config.model)?;
        ensure!(
            model.provider == "off" || model.api_key_env != config.credential_env,
            "model and mailbox must use separate credential environment variables"
        );
        let me = self.owner()?;
        validate_url(&config.mailbox_url, config.allow_insecure_http)?;
        validate_env_name(&config.credential_env)?;
        ensure!(
            matches!(config.executor.mode.as_str(), "queue" | "auto"),
            "executor mode must be queue or auto"
        );
        ensure!(
            config.executor.workdir.is_absolute() && config.executor.workdir.is_dir(),
            "executor workdir must be an existing absolute directory"
        );
        ensure!(
            (1..=86400).contains(&config.executor.timeout_secs),
            "invalid executor timeout"
        );
        ensure!(
            config.contacts.len() < 10,
            "phase 2 supports teams smaller than ten"
        );
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE identity SET config=?1 WHERE singleton=1",
            [serde_json::to_string(config)?],
        )?;
        tx.execute("DELETE FROM contacts", [])?;
        for c in &config.contacts {
            ensure!(
                c.member_id != me
                    && !c.member_id.is_empty()
                    && c.member_id.len() <= 64
                    && !c.display_name.is_empty(),
                "invalid contact"
            );
            tx.execute(
                "INSERT INTO contacts(member_id,display_name) VALUES(?1,?2)",
                params![c.member_id, c.display_name],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn member_config(&self) -> Result<MemberConfig> {
        let config: String =
            self.conn
                .query_row("SELECT config FROM identity WHERE singleton=1", [], |r| {
                    r.get(0)
                })?;
        serde_json::from_str(&config).context("member connection is not configured")
    }
    pub fn identity(&self) -> Result<Value> {
        let (id, name, team): (String, String, String) = self.conn.query_row(
            "SELECT member_id,display_name,team_id FROM identity WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(json!({"member_id":id,"display_name":name,"team_id":team}))
    }
    pub fn contacts(&self) -> Result<Vec<Contact>> {
        let mut q = self
            .conn
            .prepare("SELECT member_id,display_name FROM contacts ORDER BY member_id")?;
        Ok(q.query_map([], |r| {
            Ok(Contact {
                member_id: r.get(0)?,
                display_name: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn message(&self, id: &str) -> Result<MessageRecord> {
        let (payload, direction, state, resolved_version, error): (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ) = self
            .conn
            .query_row(
                "SELECT payload,direction,state,resolved_version,error FROM messages WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?
            .context("message not found")?;
        Ok(MessageRecord {
            message: serde_json::from_str(&payload)?,
            direction,
            state,
            resolved_version,
            error,
        })
    }
    pub fn messages(&self, conversation: Option<&str>) -> Result<Vec<MessageRecord>> {
        let mut q = self.conn.prepare(
            "SELECT id FROM messages WHERE (?1 IS NULL OR conversation_id=?1) ORDER BY rowid",
        )?;
        q.query_map([conversation], |r| r.get::<_, String>(0))?
            .map(|id| self.message(&id?))
            .collect()
    }
    pub fn conversation(&self, id: &str) -> Result<Value> {
        let (peer, state): (String, String) = self
            .conn
            .query_row(
                "SELECT peer,state FROM conversations WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .context("conversation not found")?;
        Ok(
            json!({"conversation_id":id,"peer":peer,"state":state,"messages":self.messages(Some(id))?}),
        )
    }
    pub fn close_conversation(&self, id: &str) -> Result<()> {
        ensure!(
            self.conn
                .execute("UPDATE conversations SET state='closed' WHERE id=?1", [id])?
                == 1,
            "conversation not found"
        );
        Ok(())
    }
    pub fn conversation_open(&self, id: &str) -> Result<bool> {
        Ok(self.conversation(id)?["state"] == "open")
    }
    pub fn save_message(&mut self, msg: &Message, incoming: bool) -> Result<MessageRecord> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::persist_message(&tx, msg, incoming)?;
        tx.commit()?;
        self.message(&msg.message_id)
    }
    pub(crate) fn persist_message(
        conn: &rusqlite::Connection,
        msg: &Message,
        incoming: bool,
    ) -> Result<()> {
        msg.validate()?;
        let me: String = conn.query_row(
            "SELECT member_id FROM identity WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let peer = if incoming {
            &msg.sender
        } else {
            &msg.recipient
        };
        ensure!(
            if incoming {
                msg.recipient == me
            } else {
                msg.sender == me
            },
            "message identity mismatch"
        );
        ensure!(
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM contacts WHERE member_id=?1)",
                [peer],
                |r| r.get::<_, bool>(0)
            )?,
            "member is not an authorized contact"
        );
        let payload = serde_json::to_string(msg)?;
        let old: Option<String> = conn
            .query_row(
                "SELECT payload FROM messages WHERE id=?1",
                [&msg.message_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(old) = old {
            ensure!(old == payload, "message_id conflict");
            return Ok(());
        }
        let conv: Option<(String, String)> = conn
            .query_row(
                "SELECT peer,state FROM conversations WHERE id=?1",
                [&msg.conversation_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let open = match conv {
            Some((p, state)) => {
                ensure!(p == *peer, "conversation peer mismatch");
                state == "open"
            }
            None => {
                ensure!(
                    msg.reply_to.is_none() && msg.kind == "request",
                    "unknown conversation for reply"
                );
                conn.execute(
                    "INSERT INTO conversations(id,peer,created_at) VALUES(?1,?2,?3)",
                    params![msg.conversation_id, peer, now()],
                )?;
                true
            }
        };
        ensure!(incoming || open, "conversation is closed");
        if let Some(parent) = &msg.reply_to {
            let parent: String = conn
                .query_row(
                    "SELECT payload FROM messages WHERE id=?1 AND conversation_id=?2",
                    params![parent, msg.conversation_id],
                    |r| r.get(0),
                )
                .optional()?
                .context("reply_to does not belong to this conversation")?;
            let parent: Message = serde_json::from_str(&parent)?;
            ensure!(
                parent.sender == msg.recipient && parent.recipient == msg.sender,
                "reply direction mismatch"
            );
            if msg.kind == "reply" {
                ensure!(parent.kind == "request", "reply_to must be a request");
                ensure!(
                    msg.object_id == parent.object_id,
                    "reply object differs from original request"
                );
                ensure!(
                    parent.version_id.is_none() || msg.version_id == parent.version_id,
                    "reply version differs from requested version"
                );
            }
        }
        let state = if !incoming {
            "pending"
        } else if !open {
            "closed"
        } else if msg.kind == "reply" {
            "received"
        } else {
            "waiting"
        };
        conn.execute("INSERT INTO messages(id,conversation_id,payload,direction,state,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![msg.message_id,msg.conversation_id,payload,if incoming{"in"}else{"out"},state,msg.created_at])?;
        if msg.kind == "reply" {
            conn.execute(
                "UPDATE messages SET state='replied',error=NULL WHERE id=?1",
                [&msg.reply_to],
            )?;
        }
        Ok(())
    }
    pub fn new_request(
        &mut self,
        to: &str,
        body: &str,
        operation: &str,
        reference: (Option<String>, Option<String>),
        reply_to: Option<&str>,
        request_id: &str,
    ) -> Result<MessageRecord> {
        Uuid::parse_str(request_id).context("request/message ID must be a UUID")?;
        let (mut object, mut version) = reference;
        let conv = if let Some(parent) = reply_to {
            let parent = self.message(parent)?.message;
            object = object.or(parent.object_id.clone());
            if version.is_none() && object == parent.object_id {
                version = parent.version_id;
            }
            parent.conversation_id
        } else {
            stable_id(request_id, "conversation")
        };
        let created_at = self
            .message(request_id)
            .ok()
            .map(|r| r.message.created_at)
            .unwrap_or_else(now);
        self.save_message(
            &Message {
                message_id: request_id.into(),
                conversation_id: conv,
                sender: self.owner()?,
                recipient: to.into(),
                kind: "request".into(),
                body: body.into(),
                created_at,
                reply_to: reply_to.map(str::to_owned),
                object_id: object,
                version_id: version,
                operation: operation.into(),
                workflow: None,
            },
            false,
        )
    }
    pub fn reply(
        &mut self,
        request: &str,
        body: &str,
        version: Option<String>,
    ) -> Result<MessageRecord> {
        let original = self.message(request)?;
        ensure!(
            original.direction == "in" && original.message.kind == "request",
            "can only reply to an incoming request"
        );
        let msg = &original.message;
        let pinned = self.pin_message_version(request)?;
        ensure!(
            version.is_none() || pinned.is_none() || version == pinned,
            "reply version differs from the resolved request"
        );
        let id = stable_id(request, "reply");
        let created_at = self
            .message(&id)
            .ok()
            .map(|r| r.message.created_at)
            .unwrap_or_else(now);
        let version = version.or(pinned).or(msg.version_id.clone());
        if let Some(v) = &version {
            let published = self.get_version(v)?;
            ensure!(
                msg.object_id.as_deref() == Some(&published.object_id),
                "reply version/object mismatch"
            );
            ensure!(
                self.object(&published.object_id)?.shared,
                "object metadata is private"
            );
        }
        self.save_message(
            &Message {
                message_id: id,
                conversation_id: msg.conversation_id.clone(),
                sender: self.owner()?,
                recipient: msg.sender.clone(),
                kind: "reply".into(),
                body: body.into(),
                created_at,
                reply_to: Some(request.into()),
                object_id: msg.object_id.clone(),
                version_id: version,
                operation: "auto".into(),
                workflow: None,
            },
            false,
        )
    }
    pub fn resolve_metadata(
        &self,
        object: &str,
        version: Option<&str>,
        shared_only: bool,
    ) -> Result<Value> {
        let obj = self.object(object)?;
        ensure!(!shared_only || obj.shared, "object metadata is private");
        let id = version.map(str::to_owned).or(obj.main.clone());
        let ver = if let Some(id) = id {
            let v = self.get_version(&id)?;
            ensure!(v.object_id == object, "version/object mismatch");
            Some(v)
        } else {
            None
        };
        Ok(json!({"source_member":obj.owner,"object":obj,"version":ver}))
    }
    pub fn pin_message_version(&self, id: &str) -> Result<Option<String>> {
        let record = self.message(id)?;
        if record.resolved_version.is_some() {
            return Ok(record.resolved_version);
        }
        if let Some(object) = &record.message.object_id {
            let value =
                self.resolve_metadata(object, record.message.version_id.as_deref(), true)?;
            let version = value["version"]["id"].as_str().map(str::to_owned);
            self.conn.execute(
                "UPDATE messages SET resolved_version=?2 WHERE id=?1 AND resolved_version IS NULL",
                params![id, version],
            )?;
            Ok(self.message(id)?.resolved_version)
        } else {
            Ok(None)
        }
    }
    pub fn message_state(&self, id: &str, state: &str, error: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET state=?2,error=?3 WHERE id=?1",
            params![id, state, error],
        )?;
        Ok(())
    }
}

pub async fn response_json(mut response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("http_status_{}", status.as_u16());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("HTTP response read failed"))?
    {
        ensure!(
            bytes.len() + chunk.len() <= 4 * 1024 * 1024,
            "HTTP response exceeds 4 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).context("invalid JSON response")
}
pub async fn sync(store: &mut Store) -> Result<Value> {
    let cfg = store.member_config()?;
    let credential = cfg.credential()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let base = cfg.mailbox_url.trim_end_matches('/');
    let mut sent = 0;
    let mut received = 0;
    let identity = client
        .get(format!("{base}/v1/whoami"))
        .bearer_auth(&credential)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("mailbox_unavailable; outbox retained"))?;
    let identity = response_json(identity).await?;
    ensure!(
        identity["team_id"] == store.identity()?["team_id"]
            && identity["member_id"] == store.owner()?,
        "mailbox identity/team mismatch"
    );
    let records = store.messages(None)?;
    let local_facts = {
        let mut q = store
            .conn
            .prepare("SELECT id,payload FROM task_fact_outbox WHERE sent=0 ORDER BY rowid")?;
        q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    if records
        .iter()
        .any(|r| r.direction == "out" && r.state == "pending" && r.message.operation == "task_v2")
        || !local_facts.is_empty()
    {
        ensure!(
            identity["task_protocol"] == 2,
            "信箱不支持任务协议 2，请先升级信箱；未派发任务"
        );
    }
    for (id, raw) in local_facts {
        let event: Value = serde_json::from_str(&raw)?;
        let response = client
            .post(format!("{base}/v2/task-facts"))
            .bearer_auth(&credential)
            .json(&json!({"id":id,"event":event}))
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("mailbox_unavailable; task facts retained"))?;
        response_json(response).await?;
        store
            .conn
            .execute("UPDATE task_fact_outbox SET sent=1 WHERE id=?1", [id])?;
    }
    let mut attachment_peers: Option<Value> = None;
    for record in records
        .iter()
        .filter(|r| r.direction == "out" && r.state == "pending")
    {
        if record.message.operation == "task_v2" {
            let wire = crate::task_coordinator::Wire::parse(&record.message)?;
            let files: bool = store.conn.query_row("SELECT EXISTS(SELECT 1 FROM app_tasks WHERE id=?1 AND (json_extract(draft,'$.mode')='files' OR json_extract(draft,'$.deliver_files')=1))",[&wire.task_id],|r|r.get(0))?;
            if files {
                if attachment_peers.is_none() {
                    attachment_peers = Some(if identity["file_transfer_protocol"] == 1 {
                        response_json(
                            client
                                .get(format!("{base}/v1/files/capabilities"))
                                .bearer_auth(&credential)
                                .send()
                                .await?,
                        )
                        .await?
                    } else {
                        json!({"members":[]})
                    });
                }
                let ready = attachment_peers.as_ref().unwrap()["members"]
                    .as_array()
                    .is_some_and(|members| members.iter().any(|m| m == &record.message.recipient));
                if !ready {
                    let reason = "附件任务等待对方启动支持文件传输的新版客户端，并确认信箱已升级";
                    store.conn.execute(
                        "UPDATE messages SET error=?2 WHERE id=?1",
                        rusqlite::params![record.message.message_id, reason],
                    )?;
                    if wire.event == "proposal" {
                        store.conn.execute("UPDATE app_tasks SET state='waiting',waiting_reason=?2,next_owner=?3 WHERE id=?1 AND state IN ('queued','waiting')",rusqlite::params![wire.task_id,reason,record.message.recipient])?;
                    }
                    continue;
                }
            }
        }
        let response = client
            .post(format!("{base}/v1/messages"))
            .bearer_auth(&credential)
            .json(&record.message)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("mailbox_unavailable; outbox retained"))?;
        response_json(response).await?;
        store.conn.execute(
            "UPDATE messages SET state='sent',error=NULL WHERE id=?1 AND state='pending'",
            [&record.message.message_id],
        )?;
        sent += 1;
    }
    let response = client
        .get(format!("{base}/v1/inbox"))
        .bearer_auth(&credential)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("mailbox_unavailable; inbox unchanged"))?;
    let value = response_json(response).await?;
    ensure!(
        value["team_id"] == store.identity()?["team_id"],
        "mailbox team mismatch"
    );
    let messages: Vec<Message> = serde_json::from_value(value["messages"].clone())?;
    for msg in messages {
        let fresh = store.message(&msg.message_id).is_err();
        store.save_message(&msg, true)?; // Commit before ack. Lost ack means safe redelivery.
        if msg.workflow.is_some() {
            store.ingest_workflows()?;
        }
        let response = client
            .post(format!("{base}/v1/ack/{}", msg.message_id))
            .bearer_auth(&credential)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("mailbox ack failed; local receipt retained"))?;
        response_json(response).await?;
        if fresh {
            received += 1;
        }
    }
    for record in records
        .iter()
        .filter(|r| r.direction == "out" && r.state == "sent")
    {
        let response = client
            .get(format!("{base}/v1/status/{}", record.message.message_id))
            .bearer_auth(&credential)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("mailbox status unavailable"))?;
        if response_json(response).await?["received"] == true {
            store.conn.execute(
                "UPDATE messages SET state='received' WHERE id=?1 AND state='sent'",
                [&record.message.message_id],
            )?;
        }
    }
    if identity["task_protocol"] == 2 {
        crate::app::bridge::reconcile(store)?;
        let response = client
            .get(format!("{base}/v2/events"))
            .bearer_auth(&credential)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("mailbox_unavailable; inbox unchanged"))?;
        let value = response_json(response).await?;
        ensure!(
            value["team_id"] == store.identity()?["team_id"] && value["source"] == "system",
            "system source mismatch"
        );
        for event in value["events"]
            .as_array()
            .context("missing system events")?
        {
            if let Some(report) = crate::task_schedule::local_event(store, event)? {
                response_json(
                    client
                        .post(format!("{base}/v2/meeting-report"))
                        .bearer_auth(&credential)
                        .json(&report)
                        .send()
                        .await?,
                )
                .await?;
            }
            response_json(
                client
                    .post(format!(
                        "{base}/v2/events/{}/ack",
                        event["id"].as_str().context("missing event ID")?
                    ))
                    .bearer_auth(&credential)
                    .send()
                    .await?,
            )
            .await?;
        }
    }
    Ok(json!({"sent":sent,"received":received}))
}
