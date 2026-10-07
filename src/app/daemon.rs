//! Independent receive, owner-dialogue and execution futures on separate SQLite connections.
//! Network/model awaits never hold a SQLite transaction or block terminal input.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
pub async fn run(
    db: &Path,
    exe: &Path,
    poll: u64,
    max_failures: u32,
    reconnect: bool,
) -> Result<()> {
    ensure!(
        (1..=3600).contains(&poll) && (1..=10).contains(&max_failures),
        "invalid service polling limits"
    );
    let mut wire = Store::open(db)?;
    let mut owner = Store::open(db)?;
    let mut worker = Store::open(db)?;
    let mut runtime = crate::service::Runtime::acquire(&wire)?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    // A crash during a model call is uncertain: never replay it silently.
    let interrupted = {
        let mut q = wire
            .conn
            .prepare("SELECT payload FROM app_commands WHERE state='processing'")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for raw in interrupted {
        let i: Instruction = serde_json::from_str(&raw)?;
        let Ok(_command_lock) = crate::supervisor::TaskLock::acquire(
            &wire.path,
            &stable_id(&i.request_id, "owner-command"),
        ) else {
            continue;
        };
        wire.conn.execute("UPDATE app_model_runs SET state='interrupted',error='service_interrupted' WHERE command_id=?1 AND state='running'",[&i.request_id])?;
        wire.conn.execute("UPDATE app_commands SET state='needs_attention',error='服务中断；请检查已有操作记录，不会自动重放不确定指令' WHERE id=?1",[&i.request_id])?;
        respond(
            &wire,
            &i,
            "failure",
            "服务中断：此指令的执行结果需要检查。已有任务和消息仍保留，没有自动重新启动任务。",
        )?;
    }
    let stopping = Arc::new(AtomicBool::new(false));
    let transport = async {
        let mut failures = 0u32;
        while !stopping.load(Ordering::Relaxed) {
            let tick = async {
                crate::team::sync(&mut wire).await?;
                bridge::reconcile(&mut wire)?;
                crate::presence::network(&wire, "connected", None)?;
                if let Err(e) = crate::presence::heartbeat(&mut wire).await {
                    eprintln!("presence: {e}");
                }
                Ok::<_, anyhow::Error>(())
            }
            .await;
            match tick {
                Ok(()) => {
                    failures = 0;
                    runtime.update("running", None)?;
                }
                Err(e) => {
                    let error = e.to_string();
                    failures = failures.saturating_add(1);
                    crate::presence::network(&wire, "unreachable", Some(&error))?;
                    let permanent = error.contains("http_status_401")
                        || error.contains("http_status_403")
                        || error.contains("credential")
                        || error.contains("identity/team");
                    if permanent
                        || (failures >= max_failures
                            && !(reconnect && crate::team::transient_transport_error(&error)))
                    {
                        runtime.update("blocked", Some(error))?;
                        stopping.store(true, Ordering::Relaxed);
                        break;
                    }
                    runtime.update("backoff", Some(error))?;
                }
            }
            let wait = poll.saturating_mul(1u64 << failures.min(5)).min(60) * 10;
            for _ in 0..wait {
                if runtime.stopping() {
                    stopping.store(true, Ordering::Relaxed);
                }
                if stopping.load(Ordering::Relaxed) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        Ok::<_, anyhow::Error>(())
    };
    let dialogue = async {
        while !stopping.load(Ordering::Relaxed) {
            if let Err(e) = model::process_one(&mut owner).await {
                eprintln!("owner command: {e}");
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Ok::<_, anyhow::Error>(())
    };
    let execution = async {
        while !stopping.load(Ordering::Relaxed) {
            bridge::reconcile(&mut worker)?;
            for r in worker.messages(None)?.into_iter().filter(|r| {
                r.direction == "in"
                    && r.message.kind == "request"
                    && r.message.workflow.is_none()
                    && bridge::envelope(&r.message).is_none()
                    && matches!(r.state.as_str(), "waiting" | "delegated")
            }) {
                if stopping.load(Ordering::Relaxed) {
                    break;
                }
                if !worker.conversation_open(&r.message.conversation_id)? {
                    continue;
                }
                if let Err(e) =
                    crate::butler::process_and_dispatch(&mut worker, &r.message.message_id, exe)
                        .await
                {
                    eprintln!("peer request: {e}");
                }
            }
            worker.ingest_workflows()?;
            if let Err(e) = crate::native_tasks::run_local(&mut worker, exe).await {
                eprintln!("local Codex intent: {e}");
            }
            for id in worker.workflow_ids()? {
                if stopping.load(Ordering::Relaxed) {
                    break;
                }
                if let Err(e) = crate::workflow_scheduler::run_one(&mut worker, &id, exe).await {
                    eprintln!("workflow: {e}");
                }
            }
            bridge::reconcile(&mut worker)?;
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Ok::<_, anyhow::Error>(())
    };
    let joined = async {
        let guarded_transport = async {
            let r = transport.await;
            if r.is_err() {
                stopping.store(true, Ordering::Relaxed);
            }
            r
        };
        let guarded_execution = async {
            let r = execution.await;
            if r.is_err() {
                stopping.store(true, Ordering::Relaxed);
            }
            r
        };
        let (a, b, c) = tokio::join!(guarded_transport, dialogue, guarded_execution);
        a?;
        b?;
        c?;
        Ok::<_, anyhow::Error>(())
    };
    tokio::pin!(joined);
    tokio::select! {result=&mut joined=>result, _=interrupt.recv()=>{stopping.store(true,Ordering::Relaxed);joined.await},_=terminate.recv()=>{stopping.store(true,Ordering::Relaxed);joined.await}}
}
