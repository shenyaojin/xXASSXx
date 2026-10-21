//! Small, persisted recovery contract. Counters are cumulative; explicit grants are additive.
use crate::{store::now, team::stable_id};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    TransientModel,
    ModelConfiguration,
    UserInput,
    LocalPermission,
    ExecutionUncertain,
    BudgetExhausted,
    ExecutionError,
    MaterialInput,
}
impl Reason {
    pub fn action(self) -> &'static str {
        match self {
            Self::TransientModel => "retry_model",
            Self::ModelConfiguration => "fix_configuration",
            Self::UserInput => "answer_question",
            Self::LocalPermission => "approve_locally",
            Self::ExecutionUncertain => "inspect_then_resume",
            Self::BudgetExhausted => "grant_limited_continuation",
            Self::ExecutionError => "inspect_then_resume",
            Self::MaterialInput => "correct_materials",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::TransientModel => "模型或网络暂时不可用",
            Self::ModelConfiguration => "需要检查模型认证、额度或配置",
            Self::UserInput => "等待事实或决定",
            Self::LocalPermission => "等待执行电脑的本机授权",
            Self::ExecutionUncertain => "执行中断，已有成果需要核对",
            Self::BudgetExhausted => "本次自动尝试额度已用完",
            Self::ExecutionError => "执行已暂停，需要核对原因或调整后继续",
            Self::MaterialInput => "材料清单需要调整，可在原任务继续",
        }
    }
}
#[derive(Debug)]
pub struct Failure {
    pub reason: Reason,
    pub detail: String,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail)
    }
}
impl std::error::Error for Failure {}
pub fn failure(reason: Reason, detail: impl Into<String>) -> anyhow::Error {
    Failure {
        reason,
        detail: detail.into(),
    }
    .into()
}
pub fn classify(error: &anyhow::Error) -> Reason {
    if error
        .downcast_ref::<crate::task_materials::MaterialRejection>()
        .is_some()
    {
        return Reason::MaterialInput;
    }
    error
        .downcast_ref::<Failure>()
        .map_or(Reason::ExecutionError, |e| e.reason)
}
#[allow(clippy::too_many_arguments)]
pub fn record(
    c: &Connection,
    t: &Value,
    key: &str,
    reason: Reason,
    kind: &str,
    step: &str,
    detail: &str,
    owner: &str,
) -> Result<Value> {
    let id = stable_id(key, "block");
    c.execute("INSERT OR IGNORE INTO task_blocks(id,task_id,revision,reason,step_kind,step_id,action,detail,owner,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![id,t["id"].as_str(),t["revision"].as_i64(),serde_json::to_value(reason)?.as_str(),kind,step,reason.action(),detail,owner,now()])?;
    Ok(
        json!({"id":id,"reason":reason,"step_kind":kind,"step_id":step,"action":reason.action(),"detail":detail,"owner":owner}),
    )
}
pub fn current(c: &Connection, t: &Value) -> Result<Value> {
    let raw:Option<String>=c.query_row("SELECT json_object('id',id,'reason',reason,'step_kind',step_kind,'step_id',step_id,'action',action,'detail',detail,'owner',owner) FROM task_blocks WHERE task_id=?1 AND revision=?2 AND state='open' ORDER BY rowid DESC LIMIT 1",params![t["id"].as_str(),t["revision"].as_i64()],|r|r.get(0)).optional()?;
    Ok(raw
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or(Value::Null))
}
pub fn extra(c: &Connection, t: &Value, model: bool) -> Result<i64> {
    Ok(c.query_row(if model {"SELECT COALESCE(SUM(model_rounds),0) FROM task_recovery_grants WHERE task_id=?1 AND revision=?2"}else{"SELECT COALESCE(SUM(execution_rounds),0) FROM task_recovery_grants WHERE task_id=?1 AND revision=?2"},params![t["id"].as_str(),t["revision"].as_i64()],|r|r.get(0))?)
}
pub fn grant(c: &Connection, t: &Value, key: &str) -> Result<()> {
    let block = current(c, t)?;
    c.execute("INSERT OR IGNORE INTO task_recovery_grants(id,task_id,revision,block_id,execution_rounds,model_rounds,created_at) SELECT ?1,?2,?3,?4,max_rounds,max_rounds,?5 FROM task_settings WHERE singleton=1",params![key,t["id"].as_str(),t["revision"].as_i64(),block["id"].as_str(),now()])?;
    Ok(())
}
pub fn resolve(c: &Connection, t: &Value) -> Result<()> {
    c.execute(
        "UPDATE task_blocks SET state='resolved' WHERE task_id=?1 AND revision=?2 AND state='open'",
        params![t["id"].as_str(), t["revision"].as_i64()],
    )?;
    Ok(())
}
