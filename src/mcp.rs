//! Minimal MCP stdio server. Stdout is exclusively newline-delimited JSON-RPC.
use crate::store::Store;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

pub const MAX_FRAME: u64 = 2 * 1024 * 1024;

pub struct Binding {
    pub task: String,
    pub run: String,
    pub token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    task_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitArgs {
    task_id: String,
    run_id: String,
    idempotency_key: String,
    result: String,
}

pub fn tools() -> Value {
    json!({"tools":[
        {"name":"get_task","description":"Read the assigned task by its ID. Must be called in every execution before submitting a result. Returns the current run_id.",
         "inputSchema":{"type":"object","properties":{"task_id":{"type":"string"}},"required":["task_id"],"additionalProperties":false},
         "annotations":{"readOnlyHint":true,"destructiveHint":false,"openWorldHint":false}},
        {"name":"submit_task_result","description":"Persist a result for the current task and run. Retry with exactly the same idempotency_key and result. Acceptance alone does not mark the execution complete.",
         "inputSchema":{"type":"object","properties":{"task_id":{"type":"string"},"run_id":{"type":"string"},"idempotency_key":{"type":"string"},"result":{"type":"string","minLength":1}},"required":["task_id","run_id","idempotency_key","result"],"additionalProperties":false},
         "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}}
    ]})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CollaborationArgs {
    task_id: String,
    run_id: String,
    idempotency_key: String,
    action: String,
    body: String,
    #[serde(default)]
    reply_to: Option<String>,
    #[serde(default)]
    sources: Vec<crate::workflow::Source>,
}
fn bound_tools(store: &Store, binding: &Binding) -> Result<Value> {
    if crate::workflow::for_task(&store.conn, &binding.task)?.is_none() {
        return Ok(tools());
    }
    Ok(json!({"tools":[tools()["tools"][0].clone(),
        {"name":"list_task_files","description":"List only this task's exact owner-authorized immutable version/path grants and read limits. No contents.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
        {"name":"read_task_file","description":"Read actual UTF-8 file content in this task's fixed grants. Call get_task first. Use next_offset for more chunks; max_bytes must be 4..8192 by default. Never accepts absolute paths.","inputSchema":{"type":"object","properties":{"version_id":{"type":"string"},"path":{"type":"string"},"offset":{"type":"integer","minimum":0},"max_bytes":{"type":"integer","minimum":4}},"required":["version_id","path","max_bytes"],"additionalProperties":false}},
        {"name":"submit_collaboration_turn","description":"Stage a bounded turn outcome; accepted only for the bound task/run. Rust applies it after verified Codex exit. ask sends a question and waits; reply answers an incoming clarification and continues waiting; complete submits your final assignment. Use exact source references returned by read_task_file or received messages. For retries preserve the same key and complete arguments.","inputSchema":{"type":"object","properties":{"task_id":{"type":"string"},"run_id":{"type":"string"},"idempotency_key":{"type":"string"},"action":{"type":"string","enum":["ask","reply","complete"]},"body":{"type":"string"},"reply_to":{"type":"string"},"sources":{"type":"array","items":{"type":"object","properties":{"member":{"type":"string"},"object_id":{"type":"string"},"version_id":{"type":"string"},"path":{"type":"string"},"sha256":{"type":"string"}},"required":["member","object_id","version_id","path","sha256"],"additionalProperties":false}}},"required":["task_id","run_id","idempotency_key","action","body","sources"],"additionalProperties":false}}
    ]}))
}

fn call(store: &mut Store, binding: &Binding, params: &Value) -> Result<Value> {
    match params["name"].as_str().context("missing tool name")? {
        "get_task" => {
            let args: ReadArgs = serde_json::from_value(params["arguments"].clone())?;
            ensure!(args.task_id == binding.task, "task identity mismatch");
            store.read_task(&args.task_id, &binding.run, &binding.token)
        }
        "submit_task_result" => {
            let args: SubmitArgs = serde_json::from_value(params["arguments"].clone())?;
            ensure!(
                args.task_id == binding.task && args.run_id == binding.run,
                "task/run identity mismatch"
            );
            store.submit_result(
                &args.task_id,
                &args.run_id,
                &binding.token,
                &args.idempotency_key,
                &args.result,
            )
        }
        "list_task_files" => {
            ensure!(
                params["arguments"] == json!({}),
                "list_task_files takes no arguments"
            );
            crate::task_files::list(store, binding)
        }
        "read_task_file" => crate::task_files::read(
            store,
            binding,
            serde_json::from_value(params["arguments"].clone())?,
        ),
        "submit_collaboration_turn" => {
            let a: CollaborationArgs = serde_json::from_value(params["arguments"].clone())?;
            ensure!(
                a.task_id == binding.task && a.run_id == binding.run,
                "task/run identity mismatch"
            );
            let result = serde_json::to_string(&crate::workflow::Turn {
                action: a.action,
                body: a.body,
                reply_to: a.reply_to,
                sources: a.sources,
            })?;
            store.submit_result(
                &a.task_id,
                &a.run_id,
                &binding.token,
                &a.idempotency_key,
                &result,
            )
        }
        _ => anyhow::bail!("unknown tool"),
    }
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

pub async fn serve(store: Store, binding: Binding) -> Result<()> {
    serve_scope(store, Some(binding)).await
}
pub async fn serve_butler(store: Store) -> Result<()> {
    store.owner()?;
    serve_scope(store, None).await
}
async fn serve_scope(mut store: Store, binding: Option<Binding>) -> Result<()> {
    let mut reader = BufReader::new(tokio::io::stdin());
    let mut out = tokio::io::stdout();
    let mut negotiated = false;
    let mut initialized = false;
    loop {
        let mut line = String::new();
        let n = (&mut reader)
            .take(MAX_FRAME + 1)
            .read_line(&mut line)
            .await?;
        if n == 0 {
            return Ok(());
        }
        ensure!(n as u64 <= MAX_FRAME, "MCP frame too large");
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                out.write_all(
                    format!("{}\n", error(Value::Null, -32700, "parse error")).as_bytes(),
                )
                .await?;
                out.flush().await?;
                continue;
            }
        };
        let id = request.get("id").cloned();
        let method = request["method"].as_str().unwrap_or("");
        let valid = request.is_object()
            && request["jsonrpc"] == "2.0"
            && !method.is_empty()
            && id.as_ref().is_none_or(|v| v.is_string() || v.is_number())
            && request.get("params").is_none_or(Value::is_object);
        let response = if !valid {
            Some(error(id.unwrap_or(Value::Null), -32600, "invalid request"))
        } else if id.is_none() {
            if method == "notifications/initialized" && negotiated {
                if let Some(binding) = &binding {
                    store.initialized(&binding.task, &binding.run, &binding.token)?;
                }
                initialized = true;
            }
            None
        } else {
            let id = id.unwrap();
            let result = match method {
                "initialize" if !negotiated => {
                    let requested = request["params"]["protocolVersion"].as_str().unwrap_or("");
                    let protocol = match requested {
                        "2024-11-05" | "2025-03-26" | "2025-06-18" => requested,
                        _ => "2025-06-18",
                    };
                    negotiated = true;
                    let mut info = json!({"protocolVersion":protocol,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"xxassxx","version":env!("CARGO_PKG_VERSION")}});
                    if binding.is_none() {
                        info["instructions"] = json!(
                            "This is the user's local xXASSXx butler. Use butler_context to discover identity, configured contacts and the import allowlist. Use collaboration tools for user-requested communication. Operate on the user's chosen project through the host's normal file tools; opening a project does not import or share it. Remote file analysis requires explicit task grants. Read replies/status instead of claiming success from delivery alone."
                        );
                    }
                    Ok(info)
                }
                "ping" => Ok(json!({})),
                _ if !initialized => Err((-32000, "MCP is not initialized")),
                "tools/list" => Ok(if let Some(binding) = &binding {
                    bound_tools(&store, binding)?
                } else {
                    crate::butler_mcp::tools()
                }),
                "tools/call" => match match &binding {
                    Some(binding) => call(&mut store, binding, &request["params"]),
                    None => crate::butler_mcp::call(&mut store, &request["params"]).await,
                } {
                    Ok(data) => Ok(
                        json!({"content":[{"type":"text","text":data.to_string()}],"structuredContent":data,"isError":false}),
                    ),
                    Err(e) => {
                        Ok(json!({"content":[{"type":"text","text":e.to_string()}],"isError":true}))
                    }
                },
                _ => Err((-32601, "method not found")),
            };
            Some(match result {
                Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                Err((code, msg)) => error(id, code, msg),
            })
        };
        if let Some(response) = response {
            out.write_all(format!("{response}\n").as_bytes()).await?;
            out.flush().await?;
        }
    }
}
