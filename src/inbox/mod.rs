//! Persistent member correspondence. Reading never authorizes work or marks a person read.
pub mod http;
pub mod local;
use crate::{
    store::now,
    team::{Message, stable_id},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const RELAY_SCHEMA: u32 = 2;
const MAX_BODY: usize = 16_384;
const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Actor {
    pub member: String,
    pub origin: String,
    pub can_send: bool,
    pub human_view: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendMessage {
    pub recipient: String,
    pub subject: String,
    pub body: String,
    pub idempotency_key: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub reply_to: String,
    pub body: String,
    pub idempotency_key: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Viewed {
    pub through_seq: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateFacts {
    pub revision: i64,
    pub sequence: i64,
    pub state: String,
    pub needs_action: bool,
    pub next_owner: Option<String>,
    pub action: String,
}

pub struct InboxService<'a> {
    pub conn: &'a Connection,
    pub actor: &'a Actor,
}

fn valid_text(s: &str, max: usize) -> Result<()> {
    ensure!(
        !s.trim().is_empty() && s.len() <= max && !s.contains('\0'),
        "invalid_text"
    );
    Ok(())
}
fn member(c: &Connection, id: &str) -> Result<()> {
    ensure!(
        c.query_row(
            "SELECT EXISTS(SELECT 1 FROM relay_members WHERE id=?1)",
            [id],
            |r| r.get::<_, bool>(0)
        )?,
        "unknown_member"
    );
    Ok(())
}
fn change(c: &Connection, who: &str, thread: &str, entry: Option<&str>) -> Result<()> {
    c.execute(
        "INSERT INTO inbox_changes(member,thread_id,entry_id,created_at) VALUES(?1,?2,?3,?4)",
        params![who, thread, entry, now()],
    )?;
    Ok(())
}

pub fn migrate(c: &mut Connection) -> Result<()> {
    let version: u32 = c.pragma_query_value(None, "user_version", |r| r.get(0))?;
    ensure!(
        version <= RELAY_SCHEMA,
        "unsupported mailbox schema {version}"
    );
    if version == RELAY_SCHEMA {
        return Ok(());
    }
    let tx = c.transaction()?;
    tx.execute_batch(include_str!("schema.sql"))?;
    let messages = {
        let mut q = tx.prepare("SELECT payload FROM relay_messages ORDER BY rowid")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for raw in messages {
        project(&tx, &serde_json::from_str(&raw)?, true)?;
    }
    if tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='relay_file_transfers')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        let rows = {
            let mut q = tx.prepare(
                "SELECT manifest,state FROM relay_file_transfers ORDER BY created_at,id",
            )?;
            q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (raw, state) in rows {
            project_transfer(&tx, &serde_json::from_str(&raw)?, &state, true)?;
        }
    }
    tx.pragma_update(None, "user_version", RELAY_SCHEMA)?;
    tx.commit()?;
    Ok(())
}

fn entry(c: &Connection, id: &str, who: &str) -> Result<Value> {
    let raw: String = c.query_row("SELECT json_object('id',id,'thread_id',thread_id,'seq',seq,'sender',sender,'recipient',recipient,'origin',origin,'kind',kind,'body',body,'reply_to',reply_to,'revision',revision,'source_seq',source_seq,'facts',json(facts),'created_at',created_at,'historical',historical) FROM inbox_entries WHERE id=?1 AND (sender=?2 OR recipient=?2)", params![id,who], |r| r.get(0)).optional()?.context("thread_not_visible")?;
    let mut value: Value = serde_json::from_str(&raw)?;
    let synced: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM inbox_changes ch JOIN inbox_sync_receipts r ON r.member=ch.member AND r.cursor>=ch.cursor WHERE ch.entry_id=?1 AND ch.member=?2)",params![id,value["recipient"].as_str()],|r|r.get(0))?;
    value["recipient_background_synced"] = json!(synced);
    if value["kind"] == "attachment" {
        if let Some(transfer) = value["facts"]["manifest"]["id"].as_str() {
            let current:Option<(String,i64,bool)>=c.query_row("SELECT state,expires_at,purged FROM relay_file_transfers WHERE id=?1 AND (sender=?2 OR recipient=?2)",params![transfer,who],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            if let Some((state, expires, purged)) = current {
                let state =
                    if expires <= now() && matches!(state.as_str(), "uploading" | "available") {
                        "expired".to_owned()
                    } else {
                        state
                    };
                value["facts"]["available"] = json!(
                    !purged
                        && expires > now()
                        && matches!(state.as_str(), "available" | "received")
                );
                value["facts"]["state"] = json!(state);
                value["facts"]["expires_at"] = json!(expires);
            }
        }
    }
    Ok(value)
}

fn append(c: &Connection, thread: &str, value: &Value, historical: bool) -> Result<Value> {
    let id = value["id"].as_str().context("entry id")?;
    let seq: i64 = c.query_row(
        "SELECT COALESCE(MAX(seq),0)+1 FROM inbox_entries WHERE thread_id=?1",
        [thread],
        |r| r.get(0),
    )?;
    c.execute("INSERT INTO inbox_entries(id,source_id,thread_id,seq,sender,recipient,origin,kind,body,reply_to,revision,source_seq,facts,created_at,historical) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",params![id,value["source_id"].as_str(),thread,seq,value["sender"].as_str(),value["recipient"].as_str(),value["origin"].as_str(),value["kind"].as_str(),value["body"].as_str(),value["reply_to"].as_str(),value["revision"].as_i64(),value["source_seq"].as_i64(),value["facts"].to_string(),value["created_at"].as_i64().unwrap_or_else(now),historical])?;
    for who in ["sender", "recipient"] {
        let who = value[who].as_str().context("member")?;
        c.execute("INSERT INTO inbox_member_state(thread_id,member,baseline_seq) VALUES(?1,?2,?3) ON CONFLICT(thread_id,member) DO UPDATE SET baseline_seq=MAX(baseline_seq,excluded.baseline_seq)",params![thread,who,if historical {seq} else {0}])?;
        change(c, who, thread, Some(id))?;
    }
    entry(c, id, value["sender"].as_str().unwrap())
}

/// Called inside the relay message transaction. Visibility stays pairwise even for shared tasks.
pub fn project(c: &Connection, message: &Message, historical: bool) -> Result<()> {
    let source = format!("relay:{}", message.message_id);
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM inbox_entries WHERE source_id=?1)",
        [&source],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let wire = if message.operation == "task_v2" {
        Some(crate::task_coordinator::Wire::parse(message)?)
    } else {
        None
    };
    if wire.as_ref().is_some_and(|w| w.event == "meeting_report") {
        return Ok(());
    }
    let thread = stable_id(
        "member-inbox",
        wire.as_ref()
            .map_or(message.conversation_id.as_str(), |w| &w.task_id),
    );
    let subject = wire
        .as_ref()
        .and_then(|w| w.data["title"].as_str())
        .unwrap_or("协作记录")
        .chars()
        .take(160)
        .collect::<String>();
    c.execute(
        "INSERT OR IGNORE INTO inbox_threads VALUES(?1,?2,?3,?4)",
        params![
            thread,
            if wire.is_some() {
                "task"
            } else {
                "correspondence"
            },
            subject,
            wire.as_ref().map(|w| &w.task_id)
        ],
    )?;
    let body = wire
        .as_ref()
        .map(|w| {
            w.data["body"]
                .as_str()
                .or_else(|| w.data["original"].as_str())
                .unwrap_or("")
        })
        .unwrap_or(&message.body);
    let id = stable_id("member-inbox-entry", &source);
    let reply = message
        .reply_to
        .as_ref()
        .map(|r| stable_id("member-inbox-entry", &format!("relay:{r}")));
    append(
        c,
        &thread,
        &json!({"id":id,"source_id":source,"sender":message.sender,"recipient":message.recipient,"origin":"local_agent","kind":wire.as_ref().map_or(message.kind.as_str(),|w|&w.event),"body":body,"reply_to":reply,"revision":wire.as_ref().map(|w|w.revision),"source_seq":wire.as_ref().map(|w|w.sequence),"facts":wire.as_ref().map(|w|&w.data),"created_at":message.created_at}),
        historical,
    )?;
    Ok(())
}

/// Attachment events contain the existing public manifest, never file bytes or local paths.
pub fn project_transfer(
    c: &Connection,
    m: &crate::transfers::Manifest,
    state: &str,
    historical: bool,
) -> Result<()> {
    let source = format!("attachment:{}:{state}", m.id);
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM inbox_entries WHERE source_id=?1)",
        [&source],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let thread = stable_id(
        "member-inbox",
        m.task.as_ref().map_or(m.id.as_str(), |t| &t.id),
    );
    c.execute(
        "INSERT OR IGNORE INTO inbox_threads VALUES(?1,?2,'附件交付',?3)",
        params![
            thread,
            if m.task.is_some() {
                "task"
            } else {
                "attachment"
            },
            m.task.as_ref().map(|t| &t.id)
        ],
    )?;
    let label = match state {
        "uploading" => "正在上传，尚不可领取",
        "available" => "已准备，可在终端领取",
        "received" => "接收端已校验并保存",
        "revoked" => "已撤销；已有本地副本保留",
        "purged" => "服务器文件已清理，清单记录保留",
        _ => "已过期，清单记录保留",
    };
    let body = format!(
        "附件{label}\n{}\n{}",
        m.note,
        m.files
            .iter()
            .map(|f| format!("{} · {} 字节 · SHA256 {}", f.name, f.bytes, f.sha256))
            .collect::<Vec<_>>()
            .join("\n")
    );
    append(
        c,
        &thread,
        &json!({"id":stable_id("member-inbox-entry",&source),"source_id":source,"sender":m.sender,"recipient":m.recipient,"origin":"file_transfer","kind":"attachment","body":body,"revision":m.task.as_ref().map(|t|t.revision),"facts":{"manifest":m,"state":state},"created_at":now()}),
        historical,
    )?;
    Ok(())
}

impl InboxService<'_> {
    fn check(&self) -> Result<()> {
        member(self.conn, &self.actor.member)
    }
    fn visible(&self, thread: &str) -> Result<()> {
        self.check()?;
        ensure!(
            self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM inbox_member_state WHERE thread_id=?1 AND member=?2)",
                params![thread, self.actor.member],
                |r| r.get::<_, bool>(0)
            )?,
            "thread_not_visible"
        );
        Ok(())
    }
    pub fn identity(&self) -> Result<Value> {
        self.check()?;
        let team: String = self
            .conn
            .query_row("SELECT id FROM relay_team", [], |r| r.get(0))?;
        let name: String = self.conn.query_row(
            "SELECT name FROM relay_members WHERE id=?1",
            [&self.actor.member],
            |r| r.get(0),
        )?;
        Ok(
            json!({"team_id":team,"member_id":self.actor.member,"display_name":name,"can_read":true,"can_send":self.actor.can_send,"origin":self.actor.origin}),
        )
    }
    pub fn contacts(&self) -> Result<Value> {
        self.check()?;
        let mut q = self
            .conn
            .prepare("SELECT id,name FROM relay_members WHERE id!=?1 ORDER BY id")?;
        Ok(json!(
            q.query_map([&self.actor.member], |r| Ok(
                json!({"member_id":r.get::<_,String>(0)?,"display_name":r.get::<_,String>(1)?})
            ))?
            .collect::<rusqlite::Result<Vec<_>>>()?
        ))
    }
    pub fn summary(&self, thread: &str) -> Result<Value> {
        self.visible(thread)?;
        let who = &self.actor.member;
        // No shared subject, ordering, counts or task facts from an invisible entry.
        let latest: String = self.conn.query_row("SELECT id FROM inbox_entries WHERE thread_id=?1 AND (sender=?2 OR recipient=?2) ORDER BY seq DESC LIMIT 1",params![thread,who],|r|r.get(0))?;
        let latest = entry(self.conn, &latest, who)?;
        let (kind, subject, task): (String, String, Option<String>) = self.conn.query_row(
            "SELECT kind,subject,task_id FROM inbox_threads WHERE id=?1",
            [thread],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let (seen, baseline): (i64, i64) = self.conn.query_row(
            "SELECT seen_seq,baseline_seq FROM inbox_member_state WHERE thread_id=?1 AND member=?2",
            params![thread, who],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let unread: bool = self.conn.query_row("SELECT EXISTS(SELECT 1 FROM inbox_entries WHERE thread_id=?1 AND recipient=?2 AND seq>MAX(?3,?4) AND historical=0)",params![thread,who,seen,baseline],|r|r.get(0))?;
        let last_inbound_seq:i64=self.conn.query_row("SELECT COALESCE(MAX(seq),0) FROM inbox_entries WHERE thread_id=?1 AND recipient=?2 AND historical=0",params![thread,who],|r|r.get(0))?;
        let activity:i64=self.conn.query_row("SELECT COALESCE(MAX(cursor),0) FROM inbox_changes WHERE thread_id=?1 AND member=?2 AND entry_id IS NOT NULL",params![thread,who],|r|r.get(0))?;
        let revision: Option<i64> = self.conn.query_row("SELECT MAX(revision) FROM inbox_entries WHERE thread_id=?1 AND (sender=?2 OR recipient=?2)",params![thread,who],|r|r.get(0))?;
        let personal: Option<String> = self.conn.query_row("SELECT facts FROM inbox_private_facts WHERE thread_id=?1 AND member=?2 AND revision>=COALESCE(?3,0)",params![thread,who,revision],|r|r.get(0)).optional()?;
        let facts: Value = personal
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or(json!({"state":"状态待同步","needs_action":false}));
        let subject = if kind == "task" {
            let title: Option<String> = self.conn.query_row("SELECT json_extract(facts,'$.title') FROM inbox_entries WHERE thread_id=?1 AND (sender=?2 OR recipient=?2) AND kind='proposal' ORDER BY revision DESC,seq DESC LIMIT 1",params![thread,who],|r|r.get(0)).optional()?.flatten();
            title.unwrap_or_else(|| "协作任务".into())
        } else {
            subject
        };
        Ok(
            json!({"id":thread,"kind":kind,"subject":subject,"task_id":task,"revision":revision,"activity_cursor":activity,"last_entry":latest,"last_inbound_seq":last_inbound_seq,"unread":unread,"seen_seq":seen,"baseline_seq":baseline,"history_read_unknown":baseline>0&&seen<baseline,"needs_action":facts["needs_action"]==true,"task_facts":facts}),
        )
    }
    pub fn list(&self, filter: &str, before: i64, limit: usize) -> Result<Value> {
        self.check()?;
        ensure!(
            matches!(filter, "all" | "unread" | "action")
                && (1..=100).contains(&limit)
                && before >= 0,
            "invalid_page"
        );
        // Snapshot of visible activity IDs, stable when newer activity arrives during paging.
        let mut q=self.conn.prepare("SELECT e.thread_id,MAX(ch.cursor) FROM inbox_changes ch JOIN inbox_entries e ON e.id=ch.entry_id WHERE ch.member=?1 AND ch.cursor<=?2 GROUP BY e.thread_id ORDER BY MAX(ch.cursor) DESC")?;
        let ceiling = if before == 0 { i64::MAX } else { before - 1 };
        let rows = q
            .query_map(params![self.actor.member, ceiling], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut items = Vec::new();
        let mut cursor = 0;
        let mut bytes = 0;
        let mut more = false;
        for (id, activity) in rows {
            // A thread moved above this page must not reappear via an older entry.
            let max:i64=self.conn.query_row("SELECT MAX(cursor) FROM inbox_changes WHERE member=?1 AND thread_id=?2 AND entry_id IS NOT NULL",params![self.actor.member,id],|r|r.get(0))?;
            if max > ceiling {
                continue;
            }
            let value = self.summary(&id)?;
            if filter == "all"
                || (filter == "unread" && value["unread"] == true)
                || (filter == "action" && value["needs_action"] == true)
            {
                let size = value.to_string().len();
                if !items.is_empty() && bytes + size > MAX_PAGE_BYTES {
                    more = true;
                    break;
                }
                bytes += size;
                items.push(value);
                cursor = activity;
                if items.len() == limit {
                    more = true;
                    break;
                }
            }
        }
        Ok(
            json!({"threads":items,"next_before":if more {Some(cursor)}else{None},"observed_at":now()}),
        )
    }
    pub fn read(&self, thread: &str, after: i64, limit: usize) -> Result<Value> {
        self.visible(thread)?;
        ensure!(after >= 0 && (1..=100).contains(&limit), "invalid_page");
        let mut q=self.conn.prepare("SELECT id FROM inbox_entries WHERE thread_id=?1 AND (sender=?2 OR recipient=?2) AND seq>?3 ORDER BY seq LIMIT ?4")?;
        let ids = q
            .query_map(
                params![thread, self.actor.member, after, limit as i64],
                |r| r.get::<_, String>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut entries = Vec::new();
        let mut bytes = 0;
        for id in &ids {
            let value = entry(self.conn, id, &self.actor.member)?;
            let size = value.to_string().len();
            if !entries.is_empty() && bytes + size > MAX_PAGE_BYTES {
                break;
            }
            bytes += size;
            entries.push(value);
        }
        let through = entries
            .last()
            .and_then(|v| v["seq"].as_i64())
            .unwrap_or(after);
        let more:bool=self.conn.query_row("SELECT EXISTS(SELECT 1 FROM inbox_entries WHERE thread_id=?1 AND (sender=?2 OR recipient=?2) AND seq>?3)",params![thread,self.actor.member,through],|r|r.get(0))?;
        Ok(
            json!({"thread":self.summary(thread)?,"entries":entries,"next_after":through,"has_more":more,"observed_at":now()}),
        )
    }
    fn write(
        &self,
        key: &str,
        request: Value,
        parent: Option<(&str, &str)>,
        recipient: &str,
        subject: &str,
        body: &str,
    ) -> Result<Value> {
        self.check()?;
        ensure!(self.actor.can_send, "send_forbidden");
        valid_text(key, 128)?;
        valid_text(body, MAX_BODY)?;
        valid_text(subject, 320)?;
        ensure!(recipient != self.actor.member, "invalid_recipient");
        member(self.conn, recipient)?;
        let tx = rusqlite::Transaction::new_unchecked(
            self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let request = request.to_string();
        let old: Option<(String, String)> = tx
            .query_row(
                "SELECT request,result FROM inbox_writes WHERE member=?1 AND key=?2",
                params![self.actor.member, key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((old, result)) = old {
            ensure!(old == request, "idempotency_conflict");
            return Ok(serde_json::from_str(&result)?);
        }
        let recent:i64=tx.query_row("SELECT COUNT(*) FROM inbox_entries WHERE sender=?1 AND kind='message' AND created_at>?2",params![self.actor.member,now()-60],|r|r.get(0))?;
        ensure!(recent < 60, "rate_limited");
        let new_thread = uuid::Uuid::new_v4().to_string();
        let (thread, reply) = parent.map_or((new_thread.as_str(), None), |(t, r)| (t, Some(r)));
        if reply.is_none() {
            tx.execute(
                "INSERT INTO inbox_threads VALUES(?1,'correspondence',?2,NULL)",
                params![thread, subject],
            )?;
        }
        let id = uuid::Uuid::new_v4().to_string();
        let result = append(
            &tx,
            thread,
            &json!({"id":id,"source_id":format!("message:{id}"),"sender":self.actor.member,"recipient":recipient,"origin":self.actor.origin,"kind":"message","body":body,"reply_to":reply,"facts":{},"created_at":now()}),
            false,
        )?;
        let revision:Option<i64>=tx.query_row("SELECT MAX(revision) FROM inbox_entries WHERE thread_id=?1 AND (sender=?2 OR recipient=?2)",params![thread,self.actor.member],|r|r.get(0))?;
        let result = json!({"thread_id":thread,"entry":result,"current_revision":revision,"persisted":true,"delivery":"已进入对方收件箱","recipient_background_synced":false,"viewing_requirement":"对方需要新版终端或 dot 查看；后台同步与本人查看分别记录"});
        tx.execute(
            "INSERT INTO inbox_writes VALUES(?1,?2,?3,?4)",
            params![self.actor.member, key, request, result.to_string()],
        )?;
        tx.commit()?;
        Ok(result)
    }
    pub fn send(&self, m: &SendMessage) -> Result<Value> {
        self.write(
            &m.idempotency_key,
            json!(m),
            None,
            &m.recipient,
            &m.subject,
            &m.body,
        )
    }
    pub fn reply(&self, thread: &str, m: &Reply) -> Result<Value> {
        self.visible(thread)?;
        let target = entry(self.conn, &m.reply_to, &self.actor.member)?;
        ensure!(target["thread_id"] == thread, "invalid_reply_target");
        let recipient = if target["sender"] == self.actor.member {
            target["recipient"].as_str()
        } else {
            target["sender"].as_str()
        }
        .context("invalid_reply_target")?;
        self.write(
            &m.idempotency_key,
            json!({"thread":thread,"reply":m}),
            Some((thread, &m.reply_to)),
            recipient,
            "回复",
            &m.body,
        )
    }
    pub fn viewed(&self, thread: &str, through: i64) -> Result<Value> {
        self.visible(thread)?;
        ensure!(self.actor.human_view, "human_view_forbidden");
        ensure!(through>=0 && (through==0 || self.conn.query_row("SELECT EXISTS(SELECT 1 FROM inbox_entries WHERE thread_id=?1 AND seq=?2 AND (sender=?3 OR recipient=?3))",params![thread,through,self.actor.member],|r|r.get::<_,bool>(0))?),"invalid_view_cursor");
        let tx = rusqlite::Transaction::new_unchecked(
            self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let n=tx.execute("UPDATE inbox_member_state SET seen_seq=?3 WHERE thread_id=?1 AND member=?2 AND seen_seq<?3",params![thread,self.actor.member,through])?;
        if n > 0 {
            change(&tx, &self.actor.member, thread, None)?;
        }
        tx.commit()?;
        self.summary(thread)
    }
    pub fn changes(&self, after: i64, limit: usize) -> Result<Value> {
        self.check()?;
        ensure!(after >= 0 && (1..=100).contains(&limit), "invalid_page");
        let mut q=self.conn.prepare("SELECT cursor,thread_id,entry_id FROM inbox_changes WHERE member=?1 AND cursor>?2 ORDER BY cursor LIMIT ?3")?;
        let rows = q
            .query_map(params![self.actor.member, after, limit as i64], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut result = Vec::new();
        let mut cursor = after;
        let mut bytes = 0;
        for (seq, thread, id) in rows {
            let value = json!({"cursor":seq,"thread":self.summary(&thread)?,"entry":id.map(|id|entry(self.conn,&id,&self.actor.member)).transpose()?});
            let size = value.to_string().len();
            if !result.is_empty() && bytes + size > MAX_PAGE_BYTES {
                break;
            }
            bytes += size;
            result.push(value);
            cursor = seq;
        }
        let more: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM inbox_changes WHERE member=?1 AND cursor>?2)",
            params![self.actor.member, cursor],
            |r| r.get(0),
        )?;
        Ok(json!({"changes":result,"next_cursor":cursor,"has_more":more,"observed_at":now()}))
    }

    pub fn private_facts(&self, thread: &str, facts: &PrivateFacts) -> Result<Value> {
        self.visible(thread)?;
        ensure!(self.actor.human_view, "human_view_forbidden");
        ensure!(
            facts.revision > 0
                && facts.sequence > 0
                && facts.action.len() <= 2000
                && facts.state.len() <= 80,
            "invalid_text"
        );
        let task: Option<String> = self.conn.query_row(
            "SELECT task_id FROM inbox_threads WHERE id=?1",
            [thread],
            |r| r.get(0),
        )?;
        ensure!(task.is_some(), "invalid_reply_target");
        let tx = rusqlite::Transaction::new_unchecked(
            self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let n=tx.execute("INSERT INTO inbox_private_facts VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(thread_id,member) DO UPDATE SET revision=excluded.revision,sequence=excluded.sequence,facts=excluded.facts,updated_at=excluded.updated_at WHERE excluded.revision>inbox_private_facts.revision OR (excluded.revision=inbox_private_facts.revision AND excluded.sequence>inbox_private_facts.sequence)",params![thread,self.actor.member,facts.revision,facts.sequence,serde_json::to_string(facts)?,now()])?;
        if n > 0 {
            change(&tx, &self.actor.member, thread, None)?;
        }
        tx.commit()?;
        self.summary(thread)
    }

    pub fn synced(&self, cursor: i64) -> Result<Value> {
        self.check()?;
        ensure!(self.actor.human_view, "human_view_forbidden");
        let max: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(cursor),0) FROM inbox_changes WHERE member=?1",
            [&self.actor.member],
            |r| r.get(0),
        )?;
        ensure!(cursor >= 0 && cursor <= max, "invalid_view_cursor");
        let tx = rusqlite::Transaction::new_unchecked(
            self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let previous: i64 = tx
            .query_row(
                "SELECT cursor FROM inbox_sync_receipts WHERE member=?1",
                [&self.actor.member],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        tx.execute("INSERT INTO inbox_sync_receipts VALUES(?1,?2,?3) ON CONFLICT(member) DO UPDATE SET cursor=MAX(cursor,excluded.cursor),updated_at=excluded.updated_at",params![self.actor.member,cursor,now()])?;
        // Notify original senders only for newly cached inbound entries. Receipt-only
        // changes have no entry_id and cannot cause an acknowledgement feedback loop.
        let entries = {
            let mut q=tx.prepare("SELECT DISTINCT e.sender,e.thread_id FROM inbox_changes ch JOIN inbox_entries e ON e.id=ch.entry_id WHERE ch.member=?1 AND ch.cursor>?2 AND ch.cursor<=?3 AND e.recipient=?1")?;
            q.query_map(params![self.actor.member, previous, cursor], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (sender, thread) in entries {
            change(&tx, &sender, &thread, None)?;
        }
        tx.commit()?;
        Ok(json!({"persisted":true}))
    }

    pub fn actor_read(&self, thread: &str, through: i64) -> Result<()> {
        self.visible(thread)?;
        ensure!(through>=0 && (through==0 || self.conn.query_row("SELECT EXISTS(SELECT 1 FROM inbox_entries WHERE thread_id=?1 AND seq=?2 AND (sender=?3 OR recipient=?3))",params![thread,through,self.actor.member],|r|r.get::<_,bool>(0))?),"invalid_view_cursor");
        self.conn.execute("INSERT INTO inbox_actor_receipts VALUES(?1,?2,?3,?4,?5) ON CONFLICT(thread_id,member,actor) DO UPDATE SET through_seq=MAX(through_seq,excluded.through_seq),updated_at=excluded.updated_at",params![thread,self.actor.member,self.actor.origin,through,now()])?;
        Ok(())
    }
}
