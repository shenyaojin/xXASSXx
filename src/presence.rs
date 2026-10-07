//! Model-free server-timed leases. Unknown is different from an expired heartbeat.
use crate::{
    store::{Store, now},
    team::response_json,
};
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
pub fn network(store: &Store, state: &str, error: Option<&str>) -> Result<()> {
    store.conn.execute("UPDATE app_network SET state=?1,error=?2,last_success=CASE WHEN ?1='connected' THEN ?3 ELSE last_success END WHERE singleton=1",params![state,error,now()])?;
    Ok(())
}
pub fn view(store: &Store) -> Result<Value> {
    let connection=store.conn.query_row("SELECT state,last_success,error,presence_supported FROM app_network WHERE singleton=1",[],|r|Ok(json!({"state":r.get::<_,String>(0)?,"last_success":r.get::<_,Option<i64>>(1)?,"error":r.get::<_,Option<String>>(2)?,"presence_supported":r.get::<_,bool>(3)?})))?;
    let mut members = Vec::new();
    for c in store.contacts()? {
        let row: Option<(bool, i64, i64, i64)> = store
            .conn
            .query_row(
                "SELECT busy,server_age,ttl,fetched_at FROM app_presence WHERE member_id=?1",
                [&c.member_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let data = if let Some((busy, age, ttl, at)) = row {
            let age = age + (now() - at).max(0);
            json!({"state":if age<=ttl{"recent"}else{"expired"},"activity":if busy{"busy"}else{"idle"},"age_secs":age,"updated_at":at,"ttl_secs":ttl})
        } else {
            json!({"state":"unknown","activity":"unknown","updated_at":null})
        };
        members
            .push(json!({"member_id":c.member_id,"display_name":c.display_name,"presence":data}));
    }
    Ok(json!({"connection":connection,"members":members}))
}
pub async fn heartbeat(store: &mut Store) -> Result<()> {
    let cfg = store.member_config()?;
    let key = cfg.credential()?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let busy:bool=store.conn.query_row("SELECT EXISTS(SELECT 1 FROM workflows WHERE state='running') OR EXISTS(SELECT 1 FROM app_commands WHERE state='processing')",[],|r|r.get(0))?;
    let url = format!("{}/v1/presence", cfg.mailbox_url.trim_end_matches('/'));
    let response = client
        .post(&url)
        .bearer_auth(&key)
        .json(&json!({"activity":if busy{"busy"}else{"idle"}}))
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("presence_unavailable"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        store.conn.execute(
            "UPDATE app_network SET presence_supported=0 WHERE singleton=1",
            [],
        )?;
        store.conn.execute("DELETE FROM app_presence", [])?;
        return Ok(());
    }
    response_json(response).await?;
    let response = client
        .get(url)
        .bearer_auth(key)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("presence_unavailable"))?;
    let v = response_json(response).await?;
    let server = v["server_time"]
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("invalid presence time"))?;
    let contacts = store.contacts()?;
    let entries = v["members"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("invalid presence members"))?;
    ensure!(entries.len() < 10, "invalid presence size");
    let tx = store.conn.transaction()?;
    tx.execute(
        "UPDATE app_network SET presence_supported=1 WHERE singleton=1",
        [],
    )?;
    for member in entries {
        let id = member["member_id"].as_str().unwrap_or("");
        if !contacts.iter().any(|c| c.member_id == id) {
            continue;
        }
        if let Some(seen) = member["seen_at"].as_i64() {
            let ttl = member["ttl_secs"].as_i64().unwrap_or(30);
            ensure!((1..=120).contains(&ttl), "invalid lease");
            tx.execute("INSERT INTO app_presence(member_id,busy,server_age,ttl,fetched_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(member_id) DO UPDATE SET busy=excluded.busy,server_age=excluded.server_age,ttl=excluded.ttl,fetched_at=excluded.fetched_at",params![id,member["activity"]=="busy",(server-seen).max(0),ttl,now()])?;
        } else {
            tx.execute("DELETE FROM app_presence WHERE member_id=?1", [id])?;
        }
    }
    tx.commit()?;
    Ok(())
}
