//! Owner-local MCP capability: metadata and collaboration only. No task-result
//! mutation; the separate task/run-bound MCP continues to enforce execution IDs.
use crate::{butler::metadata_page, store::Store};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    object_id: String,
    #[serde(default)]
    version_id: Option<String>,
    #[serde(default)]
    offset: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Send {
    to: String,
    body: String,
    message_id: String,
    #[serde(default = "auto")]
    operation: String,
    #[serde(default)]
    object_id: Option<String>,
    #[serde(default)]
    version_id: Option<String>,
    #[serde(default)]
    reply_to: Option<String>,
}
fn auto() -> String {
    "auto".into()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Conversation {
    conversation_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Status {
    message_id: String,
}
pub fn tools() -> Value {
    let defs = [
        (
            "butler_context",
            "Read your xXASSXx identity, configured contacts, import allowlist and local service state. Use this first when the user asks to contact another butler. Contacts are not live presence.",
            json!({}),
            vec![],
            true,
        ),
        (
            "knowledge_objects",
            "List the local owner's object metadata; no file contents.",
            json!({}),
            vec![],
            true,
        ),
        (
            "knowledge_metadata",
            "Read local object/version metadata and a page of its manifest; no file contents.",
            json!({"object_id":{"type":"string"},"version_id":{"type":"string"},"offset":{"type":"integer","minimum":0}}),
            vec!["object_id"],
            true,
        ),
        (
            "collaboration_send_request",
            "Persist a request to a configured contact. Supply a stable UUID message_id for safe retries. Delivery occurs on collaboration_sync or the running butler.",
            json!({"to":{"type":"string"},"body":{"type":"string"},"message_id":{"type":"string"},"operation":{"type":"string","enum":["auto","metadata","analysis"]},"object_id":{"type":"string"},"version_id":{"type":"string"},"reply_to":{"type":"string"}}),
            vec!["to", "body", "message_id"],
            false,
        ),
        (
            "collaboration_read_conversation",
            "Read original requests and replies in a local conversation.",
            json!({"conversation_id":{"type":"string"}}),
            vec!["conversation_id"],
            true,
        ),
        (
            "collaboration_read_replies",
            "Read replies linked to a particular request, retaining exact source/version IDs.",
            json!({"message_id":{"type":"string"}}),
            vec!["message_id"],
            true,
        ),
        (
            "collaboration_status",
            "Read delivery, processing and delegation state. Received does not mean Codex completed.",
            json!({"message_id":{"type":"string"}}),
            vec!["message_id"],
            true,
        ),
        (
            "collaboration_sync",
            "Deliver the durable outbox and receive/ack the local member's inbox over the configured HTTP mailbox. Does not invoke a model.",
            json!({}),
            vec![],
            false,
        ),
    ];
    json!({"tools":defs.into_iter().map(|(name,description,properties,required,read)|json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":read,"destructiveHint":false,"openWorldHint":false}})).collect::<Vec<_>>()})
}
pub async fn call(store: &mut Store, params: &Value) -> Result<Value> {
    store.owner()?;
    let args = params["arguments"].clone();
    match params["name"].as_str().context("missing tool name")? {
        "butler_context" => {
            let _: Empty = serde_json::from_value(args)?;
            crate::interactive::context(store)
        }
        "knowledge_objects" => {
            let _: Empty = serde_json::from_value(args)?;
            Ok(json!(store.objects(false)?))
        }
        "knowledge_metadata" => {
            let a: Metadata = serde_json::from_value(args)?;
            metadata_page(
                store.resolve_metadata(&a.object_id, a.version_id.as_deref(), false)?,
                a.offset,
            )
        }
        "collaboration_send_request" => {
            let a: Send = serde_json::from_value(args)?;
            Ok(json!(store.new_request(
                &a.to,
                &a.body,
                &a.operation,
                (a.object_id, a.version_id),
                a.reply_to.as_deref(),
                &a.message_id
            )?))
        }
        "collaboration_read_conversation" => {
            let a: Conversation = serde_json::from_value(args)?;
            let v = store.conversation(&a.conversation_id)?;
            ensure!(
                v.to_string().len() < 1024 * 1024,
                "conversation too large; read individual request replies"
            );
            Ok(v)
        }
        "collaboration_read_replies" => {
            let a: Status = serde_json::from_value(args)?;
            let original = store.message(&a.message_id)?;
            Ok(
                json!({"request":original,"replies":store.messages(Some(&original.message.conversation_id))?.into_iter().filter(|r|r.message.kind=="reply" && r.message.reply_to.as_deref()==Some(&a.message_id)).collect::<Vec<_>>()}),
            )
        }
        "collaboration_status" => {
            let a: Status = serde_json::from_value(args)?;
            let d = store.delegation(&a.message_id)?;
            Ok(
                json!({"message":store.message(&a.message_id)?,"delegation":{"state":d["state"],"task_id":d["task"]["id"],"task_state":d["task"]["state"],"session_id":d["task"]["session_id"]}}),
            )
        }
        "collaboration_sync" => {
            let _: Empty = serde_json::from_value(args)?;
            crate::team::sync(store).await
        }
        _ => anyhow::bail!("unknown or disallowed butler tool"),
    }
}
