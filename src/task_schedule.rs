//! Relay-owned facts, durable meetings and participant-scoped daily summaries.
//! UTC storage and injected clocks; system events never impersonate members.
use crate::{
    store::{Store, now},
    task_coordinator::Wire,
    team::{Message, stable_id},
};
use anyhow::{Context, Result, ensure};
use chrono::{Duration as Days, NaiveDate, NaiveTime, TimeZone};
use chrono_tz::Tz;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub meeting_interval_secs: i64,
    pub meeting_wait_secs: i64,
    pub meeting_max_rounds: u32,
    pub timezone: String,
    pub daily_time: String,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            meeting_interval_secs: 7200,
            meeting_wait_secs: 300,
            meeting_max_rounds: 2,
            timezone: "America/Denver".into(),
            daily_time: "18:00".into(),
        }
    }
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.meeting_interval_secs >= 1
                && self.meeting_wait_secs >= 1
                && (1..=2).contains(&self.meeting_max_rounds),
            "invalid meeting schedule"
        );
        self.timezone
            .parse::<Tz>()
            .context("invalid IANA timezone")?;
        NaiveTime::parse_from_str(&self.daily_time, "%H:%M")?;
        Ok(())
    }
}
pub fn migrate(c: &Connection, cfg: &Config) -> Result<()> {
    cfg.validate()?;
    c.execute_batch("BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS relay_task_config(singleton INTEGER PRIMARY KEY,config TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS relay_tasks(id TEXT PRIMARY KEY,revision INTEGER NOT NULL,initiator TEXT NOT NULL,participants TEXT NOT NULL,facts TEXT NOT NULL,state TEXT NOT NULL,sequence INTEGER NOT NULL,next_meeting INTEGER NOT NULL,updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS relay_task_facts(task_id TEXT NOT NULL,revision INTEGER NOT NULL,member TEXT NOT NULL,sequence INTEGER NOT NULL,facts TEXT NOT NULL,updated_at INTEGER NOT NULL,PRIMARY KEY(task_id,revision,member));
CREATE TABLE IF NOT EXISTS relay_meetings(id TEXT PRIMARY KEY,task_id TEXT NOT NULL,revision INTEGER NOT NULL,due_at INTEGER NOT NULL,deadline INTEGER NOT NULL,participants TEXT NOT NULL,state TEXT NOT NULL,round INTEGER NOT NULL DEFAULT 1,merged_count INTEGER NOT NULL DEFAULT 0,result TEXT);
CREATE TABLE IF NOT EXISTS relay_reports(meeting_id TEXT NOT NULL,member TEXT NOT NULL,payload TEXT NOT NULL,reported_at INTEGER NOT NULL,PRIMARY KEY(meeting_id,member));
CREATE TABLE IF NOT EXISTS relay_system_events(id TEXT PRIMARY KEY,recipient TEXT NOT NULL,kind TEXT NOT NULL,payload TEXT NOT NULL,created_at INTEGER NOT NULL,received INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS relay_daily(member TEXT NOT NULL,team_date TEXT NOT NULL,cutoff INTEGER NOT NULL,PRIMARY KEY(member,team_date));
CREATE TABLE IF NOT EXISTS relay_local_facts(id TEXT PRIMARY KEY,member TEXT NOT NULL,payload TEXT NOT NULL);
PRAGMA user_version=1;COMMIT;")?;
    c.execute("INSERT INTO relay_task_config VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET config=excluded.config",[serde_json::to_string(cfg)?])?;
    Ok(())
}
/// Called in the same transaction as accepting the authenticated member message.
pub fn accept(c: &Connection, m: &Message, at: i64) -> Result<()> {
    if m.operation != "task_v2" {
        return Ok(());
    }
    let w = Wire::parse(m)?;
    let existing: Option<(i64, String, String, i64, String)> = c
        .query_row(
            "SELECT revision,initiator,participants,sequence,state FROM relay_tasks WHERE id=?1",
            [&w.task_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    if w.event == "proposal" {
        ensure!(
            w.data["initiator"] == m.sender && w.data["confirmed"] == w.revision,
            "invalid task confirmation attestation"
        );
        let people: Vec<String> = serde_json::from_value(w.data["participants"].clone())?;
        ensure!(
            people.contains(&m.sender) && people.contains(&m.recipient) && people.len() < 10,
            "invalid participants"
        );
        for member in &people {
            ensure!(
                c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM relay_members WHERE id=?1)",
                    [member],
                    |r| r.get::<_, bool>(0)
                )?,
                "unknown participant"
            );
        }
        if let Some((rev, initiator, _, _, _)) = &existing {
            ensure!(initiator == &m.sender, "task initiator mismatch");
            if w.revision < *rev {
                return Ok(());
            }
        }
        let cfg: Config = serde_json::from_str(&c.query_row(
            "SELECT config FROM relay_task_config",
            [],
            |r| r.get::<_, String>(0),
        )?)?;
        c.execute("INSERT INTO relay_tasks VALUES(?1,?2,?3,?4,?5,'queued',?6,?7,?8) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,participants=excluded.participants,facts=excluded.facts,state='queued',sequence=excluded.sequence,next_meeting=excluded.next_meeting,updated_at=excluded.updated_at WHERE excluded.revision>relay_tasks.revision",params![w.task_id,w.revision,m.sender,w.data["participants"].to_string(),w.data.to_string(),w.sequence,at+cfg.meeting_interval_secs,at])?;
    } else {
        let (rev, initiator, people, seq, state) =
            existing.context("task proposal not received")?;
        let people: Vec<String> = serde_json::from_str(&people)?;
        ensure!(
            people.contains(&m.sender) && people.contains(&m.recipient),
            "task participant mismatch"
        );
        if w.revision < rev {
            return Ok(());
        }
        ensure!(w.revision == rev, "future task revision");
        if matches!(state.as_str(), "completed" | "cancelled") {
            return Ok(());
        }
        if matches!(w.event.as_str(), "complete" | "cancel" | "suspend") {
            ensure!(initiator == m.sender, "only A may finish or suspend");
            if w.sequence > seq {
                c.execute("UPDATE relay_tasks SET state=?2,sequence=?3,facts=?4,updated_at=?5 WHERE id=?1",params![w.task_id,match w.event.as_str(){"complete"=>"completed","cancel"=>"cancelled",_=>"draft"},w.sequence,w.data.to_string(),at])?;
            }
        } else if m.sender == initiator && w.sequence > seq {
            let reported = if w.event == "answer" {
                Some("processing")
            } else {
                w.data["state"].as_str().filter(|s| {
                    matches!(*s, "queued" | "processing" | "waiting" | "needs_attention")
                })
            };
            c.execute("UPDATE relay_tasks SET sequence=?2,updated_at=?3,state=COALESCE(?4,state) WHERE id=?1",params![w.task_id,w.sequence,at,reported])?;
        }
    }
    c.execute("INSERT INTO relay_task_facts VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(task_id,revision,member) DO UPDATE SET sequence=excluded.sequence,facts=excluded.facts,updated_at=excluded.updated_at WHERE excluded.sequence>relay_task_facts.sequence",params![w.task_id,w.revision,m.sender,w.sequence,json!({"event":w.event,"data":w.data}).to_string(),at])?;
    Ok(())
}
fn event(c: &Connection, key: &str, to: &str, kind: &str, payload: &Value, at: i64) -> Result<()> {
    c.execute("INSERT OR IGNORE INTO relay_system_events(id,recipient,kind,payload,created_at) VALUES(?1,?2,?3,?4,?5)",params![stable_id(key,to),to,kind,payload.to_string(),at])?;
    Ok(())
}
pub fn scheduled_cutoff(cfg: &Config, at: i64) -> Result<(String, i64)> {
    let zone: Tz = cfg.timezone.parse()?;
    let local = zone
        .timestamp_opt(at, 0)
        .single()
        .context("invalid clock")?;
    let time = NaiveTime::parse_from_str(&cfg.daily_time, "%H:%M")?;
    fn resolve(zone: Tz, date: NaiveDate, time: NaiveTime) -> Result<i64> {
        let dt = date.and_time(time);
        // At a fall-back overlap use the first instant; at a spring-forward gap
        // use the first valid local minute after the configured time.
        for n in 0..=180 {
            if let Some(v) = zone
                .from_local_datetime(&(dt + Days::minutes(n)))
                .earliest()
            {
                return Ok(v.timestamp());
            }
        }
        anyhow::bail!("cannot resolve configured daily time")
    }
    let mut date = local.date_naive();
    let mut cutoff = resolve(zone, date, time)?;
    if cutoff > at {
        date = date.pred_opt().context("date underflow")?;
        cutoff = resolve(zone, date, time)?;
    }
    Ok((date.to_string(), cutoff))
}
pub fn tick(c: &Connection, at: i64) -> Result<()> {
    let cfg: Config = serde_json::from_str(&c.query_row(
        "SELECT config FROM relay_task_config",
        [],
        |r| r.get::<_, String>(0),
    )?)?;
    let tx = c.unchecked_transaction()?;
    let tasks = {
        let mut q=tx.prepare("SELECT id,revision,initiator,participants,facts,state,next_meeting,updated_at FROM relay_tasks ORDER BY id")?;
        q.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"revision":r.get::<_,i64>(1)?,"initiator":r.get::<_,String>(2)?,"participants":r.get::<_,String>(3)?,"facts":r.get::<_,String>(4)?,"state":r.get::<_,String>(5)?,"due":r.get::<_,i64>(6)?,"updated_at":r.get::<_,i64>(7)?})))?.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for task in &tasks {
        let id = task["id"].as_str().unwrap();
        let rev = task["revision"].as_i64().unwrap();
        let people: Vec<String> = serde_json::from_str(task["participants"].as_str().unwrap())?;
        if matches!(
            task["state"].as_str(),
            Some("draft" | "completed" | "cancelled")
        ) {
            tx.execute(
                "UPDATE relay_meetings SET state='closed' WHERE task_id=?1 AND state='open'",
                [id],
            )?;
            continue;
        }
        if task["due"].as_i64().unwrap() <= at {
            let due = task["due"].as_i64().unwrap();
            let skipped = (at - due) / cfg.meeting_interval_secs;
            let effective = due + skipped * cfg.meeting_interval_secs;
            let meeting = stable_id(id, &format!("meeting:{rev}:{effective}"));
            tx.execute("UPDATE relay_meetings SET state='merged',result=?2 WHERE task_id=?1 AND state='open'",params![id,json!({"merged_into":meeting}).to_string()])?;
            tx.execute("UPDATE relay_system_events SET received=1 WHERE kind='meeting_invitation' AND json_extract(payload,'$.task_id')=?1 AND received=0",[id])?;
            tx.execute("INSERT OR IGNORE INTO relay_meetings(id,task_id,revision,due_at,deadline,participants,state,merged_count) VALUES(?1,?2,?3,?4,?5,?6,'open',?7)",params![meeting,id,rev,effective,at+cfg.meeting_wait_secs,serde_json::to_string(&people)?,skipped])?;
            for member in &people {
                event(
                    &tx,
                    &meeting,
                    member,
                    "meeting_invitation",
                    &json!({"source":"system","meeting_id":meeting,"task_id":id,"revision":rev,"due_at":effective,"deadline":at+cfg.meeting_wait_secs,"participants":people,"round":1,"max_rounds":cfg.meeting_max_rounds,"merged_periods":skipped}),
                    at,
                )?;
            }
            tx.execute(
                "UPDATE relay_tasks SET next_meeting=?2 WHERE id=?1",
                params![id, effective + cfg.meeting_interval_secs],
            )?;
        }
    }
    let meetings = {
        let mut q=tx.prepare("SELECT id,task_id,revision,participants,deadline FROM relay_meetings WHERE state='open'")?;
        q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, task, rev, raw, deadline) in meetings {
        let people: Vec<String> = serde_json::from_str(&raw)?;
        let reports = reports(&tx, &id)?;
        if deadline <= at || reports.len() == people.len() {
            let missing = people
                .iter()
                .filter(|m| !reports.iter().any(|r| r["member"] == **m))
                .collect::<Vec<_>>();
            let result = json!({"source":"system","meeting_id":id,"task_id":task,"revision":rev,"cutoff":at,"reports":reports,"absent":missing,"coordination_owner":tasks.iter().find(|t|t["id"]==task).map(|t|t["initiator"].clone())});
            tx.execute(
                "UPDATE relay_meetings SET state='finished',result=?2 WHERE id=?1",
                params![id, result.to_string()],
            )?;
            tx.execute("UPDATE relay_system_events SET received=1 WHERE kind='meeting_summary' AND json_extract(payload,'$.task_id')=?1 AND received=0",[&task])?;
            for member in &people {
                event(
                    &tx,
                    &format!("{id}:summary"),
                    member,
                    "meeting_summary",
                    &result,
                    at,
                )?;
            }
        }
    }
    let (date, cutoff) = scheduled_cutoff(&cfg, at)?;
    let people = {
        let mut q = tx.prepare("SELECT id FROM relay_members ORDER BY id")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for member in people {
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM relay_daily WHERE member=?1 AND team_date=?2)",
            params![member, date],
            |r| r.get::<_, bool>(0),
        )? {
            continue;
        }
        let previous: Option<i64> = tx.query_row(
            "SELECT MAX(cutoff) FROM relay_daily WHERE member=?1",
            [&member],
            |r| r.get(0),
        )?;
        let zone: Tz = cfg.timezone.parse()?;
        if previous.is_none()
            && zone
                .timestamp_opt(at, 0)
                .single()
                .context("invalid clock")?
                .date_naive()
                .to_string()
                != date
        {
            continue;
        }
        let oldest_unread:Option<i64>=tx.query_row("SELECT MIN(json_extract(payload,'$.from_cutoff')) FROM relay_system_events WHERE recipient=?1 AND kind='daily_summary' AND received=0",[&member],|r|r.get(0))?;
        let mut entries = Vec::new();
        for t in &tasks {
            let participants: Vec<String> =
                serde_json::from_str(t["participants"].as_str().unwrap())?;
            if !participants.contains(&member) {
                continue;
            }
            let updated = t["updated_at"].as_i64().unwrap();
            // Facts newer than cutoff are explicitly observed at generation, never
            // attributed to the preceding day.
            if matches!(t["state"].as_str(), Some("completed" | "cancelled"))
                && previous.is_some_and(|p| updated <= p)
            {
                continue;
            }
            let mut q=tx.prepare("SELECT member,facts,updated_at FROM relay_task_facts WHERE task_id=?1 AND revision=?2")?;
            let facts=q.query_map(params![t["id"].as_str(),t["revision"].as_i64()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().map(|(m,f,time)|json!({"member":m,"last_observed_at":time,"facts":serde_json::from_str::<Value>(&f).unwrap_or(Value::Null)})).collect::<Vec<_>>();
            entries.push(json!({"task_id":t["id"],"short_id":format!("T-{}",&t["id"].as_str().unwrap()[..8]),"revision":t["revision"],"state":t["state"],"initiator":t["initiator"],"updated_at":updated,"last_known_progress":facts}));
        }
        if entries.is_empty() {
            continue;
        }
        let payload = json!({"source":"system","team_date":date,"timezone":cfg.timezone,"scheduled_cutoff":cutoff,"cutoff":at,"from_cutoff":oldest_unread.or(previous),"tasks":entries,"note":"未收到新汇报的成员显示最后已知进展；补报已合并"});
        tx.execute("UPDATE relay_system_events SET received=1 WHERE recipient=?1 AND kind='daily_summary' AND received=0",[&member])?;
        event(
            &tx,
            &format!("daily:{date}"),
            &member,
            "daily_summary",
            &payload,
            at,
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO relay_daily VALUES(?1,?2,?3)",
            params![member, date, at],
        )?;
    }
    tx.commit()?;
    Ok(())
}
fn reports(c: &Connection, id: &str) -> Result<Vec<Value>> {
    let mut q = c.prepare(
        "SELECT member,payload,reported_at FROM relay_reports WHERE meeting_id=?1 ORDER BY member",
    )?;
    let rs = q
        .query_map([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rs.into_iter().map(|(m,p,at)|json!({"member":m,"report":serde_json::from_str::<Value>(&p).unwrap_or(Value::Null),"reported_at":at})).collect())
}
pub fn report(c: &Connection, member: &str, p: &Value, at: i64) -> Result<()> {
    ensure!(p.to_string().len() < 24000, "report too large");
    let id = p["meeting_id"].as_str().context("missing meeting")?;
    let (task, rev, raw, state): (String, i64, String, String) = c.query_row(
        "SELECT task_id,revision,participants,state FROM relay_meetings WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let people: Vec<String> = serde_json::from_str(&raw)?;
    ensure!(
        people.iter().any(|m| m == member) && p["task_id"] == task && p["revision"] == rev,
        "meeting participant or revision mismatch"
    );
    let tx = c.unchecked_transaction()?;
    let changed = tx.execute(
        "INSERT OR IGNORE INTO relay_reports VALUES(?1,?2,?3,?4)",
        params![id, member, p.to_string(), at],
    )?;
    if changed > 0 && state == "finished" {
        for to in &people {
            event(
                &tx,
                &format!("late:{id}:{member}"),
                to,
                "meeting_supplement",
                &json!({"source":"system","meeting_id":id,"task_id":task,"revision":rev,"member":member,"report":p,"reported_at":at}),
                at,
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}
pub fn pending(c: &Connection, member: &str) -> Result<Vec<Value>> {
    let mut q=c.prepare("SELECT id,kind,payload,created_at FROM relay_system_events WHERE recipient=?1 AND received=0 ORDER BY rowid LIMIT 50")?;
    let rows = q
        .query_map([member], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().map(|(id,k,p,at)|json!({"id":id,"kind":k,"payload":serde_json::from_str::<Value>(&p).unwrap_or(Value::Null),"created_at":at})).collect())
}
pub fn local_event(store: &Store, v: &Value) -> Result<Option<Value>> {
    let id = v["id"].as_str().context("missing system event ID")?;
    let kind = v["kind"].as_str().context("missing system event kind")?;
    let p = &v["payload"];
    ensure!(
        p["source"] == "system"
            && matches!(
                kind,
                "meeting_invitation" | "meeting_summary" | "meeting_supplement" | "daily_summary"
            ),
        "unsupported system event"
    );
    let tx = store.conn.unchecked_transaction()?;
    let fresh = tx.execute(
        "INSERT OR IGNORE INTO task_system_events VALUES(?1,?2,?3,?4)",
        params![
            id,
            kind,
            p.to_string(),
            v["created_at"].as_i64().unwrap_or_else(now)
        ],
    )? > 0;
    let mut report = None;
    if kind == "daily_summary" {
        if fresh {
            let project = app_project(store)?;
            let session = crate::app::session(store, &project, &store.owner()?)?;
            let entries = p["tasks"].as_array().context("invalid daily summary")?;
            let body = format!(
                "任务日报 {}（{}）\n汇总截止 {}\n{}",
                p["team_date"].as_str().unwrap_or(""),
                p["timezone"].as_str().unwrap_or(""),
                local_time(p["cutoff"].as_i64()),
                entries
                    .iter()
                    .map(|t| format!(
                        "{} · {} · 协调人 @{} · 最后更新 {}\n{}",
                        t["short_id"].as_str().unwrap_or(""),
                        t["state"].as_str().unwrap_or(""),
                        t["initiator"].as_str().unwrap_or(""),
                        local_time(t["updated_at"].as_i64()),
                        t["last_known_progress"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|p| format!(
                                "  @{}：{} · 下一步 @{} · 最后观测 {}",
                                p["member"].as_str().unwrap_or(""),
                                p["facts"]["data"]["body"]
                                    .as_str()
                                    .unwrap_or("已确认任务，等待进一步汇报"),
                                p["facts"]["data"]["next_owner"].as_str().unwrap_or("待定"),
                                local_time(p["last_observed_at"].as_i64())
                            ))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            crate::app::add_message(
                &tx,
                id,
                &session,
                None,
                "system",
                &store.owner()?,
                kind,
                &body,
                None,
                None,
            )?;
        }
    } else if let Some(task) = p["task_id"].as_str() {
        if crate::task_coordinator::is_task(&tx, task)? {
            let t = crate::task_coordinator::get(store, task)?;
            if t["revision"] == p["revision"] {
                if kind == "meeting_invitation" {
                    report = Some(
                        json!({"meeting_id":p["meeting_id"],"task_id":task,"revision":t["revision"],"state":t["state"],"completed":t["result"],"evidence":t["executions"],"blocking":t["waiting_for"],"questions":t["questions"],"next_owner":t["next_owner"],"next_action":t["waiting_for"],"updated_at":t["updated_at"]}),
                    );
                } else if fresh {
                    let body = format!(
                        "{} · {}\n{}",
                        t["short_id"].as_str().unwrap_or(""),
                        if kind == "meeting_supplement" {
                            "离线成员补报"
                        } else {
                            "任务例会汇总"
                        },
                        render_meeting(p)
                    );
                    crate::app::add_message(
                        &tx,
                        id,
                        t["session_id"].as_str().unwrap(),
                        Some(task),
                        "system",
                        &store.owner()?,
                        kind,
                        &body,
                        None,
                        None,
                    )?;
                }
            }
        }
    }
    tx.commit()?;
    Ok(report)
}
fn app_project(store: &Store) -> Result<String> {
    crate::app::bridge::default_project(store)
}
fn render_meeting(p: &Value) -> String {
    let mut lines = Vec::new();
    for r in p["reports"].as_array().into_iter().flatten() {
        let report = &r["report"];
        lines.push(format!(
            "@{}：{}；等待 {}；下一步 @{}",
            r["member"].as_str().unwrap_or(""),
            report["state"].as_str().unwrap_or("未知"),
            report["blocking"].as_str().unwrap_or("无"),
            report["next_owner"].as_str().unwrap_or("待定")
        ));
    }
    if let Some(a) = p["absent"].as_array() {
        if !a.is_empty() {
            lines.push(format!(
                "尚未汇报：{}；上线后补报",
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
    }
    if p["report"].is_object() {
        lines.push(format!(
            "@{}：{}",
            p["member"].as_str().unwrap_or(""),
            p["report"]["state"].as_str().unwrap_or("未知")
        ));
    }
    lines.join("\n")
}

fn local_time(timestamp: Option<i64>) -> String {
    timestamp
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S %Z")
                .to_string()
        })
        .unwrap_or_else(|| "未知".into())
}
