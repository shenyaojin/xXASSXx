//! Portable task descriptions. Physical paths remain in the member's local bindings.
use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

pub fn portable(c: &Connection, member: &str, task: &str, value: Value) -> Result<Value> {
    let work: String = c.query_row(
        "SELECT p.path FROM app_tasks t JOIN app_projects p ON p.id=t.project_id WHERE t.id=?1",
        [task],
        |r| r.get(0),
    )?;
    let mut paths = vec![(work, format!("member://{member}/work"))];
    let mut q = c.prepare("SELECT path FROM file_roots ORDER BY path")?;
    for (index, path) in q.query_map([], |r| r.get::<_, String>(0))?.enumerate() {
        paths.push((path?, format!("member://{member}/root-{index}")));
    }
    if let Some(home) = std::env::var_os("HOME") {
        paths.push((
            home.to_string_lossy().into(),
            format!("member://{member}/home"),
        ));
    }
    #[cfg(target_os = "macos")]
    for (path, alias) in paths.clone() {
        if path.starts_with("/private/var/") || path.starts_with("/private/tmp/") {
            paths.push((path.trim_start_matches("/private").into(), alias));
        }
    }
    // Prefer the most specific directory; stable sorting keeps work before roots.
    paths.sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));
    fn visit(value: Value, paths: &[(String, String)]) -> Value {
        match value {
            Value::String(mut text) => {
                for (path, alias) in paths {
                    if path == "/" || path.is_empty() {
                        continue;
                    }
                    let mut result = String::new();
                    let mut remaining = text.as_str();
                    while let Some(at) = remaining.find(path) {
                        let end = at + path.len();
                        let next = remaining[end..].chars().next();
                        result.push_str(&remaining[..at]);
                        if next.is_none_or(|c| {
                            c == '/' || c.is_whitespace() || ")]},;:。；，）\"'`".contains(c)
                        }) {
                            result.push_str(alias);
                        } else {
                            result.push_str(path);
                        }
                        remaining = &remaining[end..];
                    }
                    result.push_str(remaining);
                    text = result;
                }
                Value::String(text)
            }
            Value::Array(items) => {
                Value::Array(items.into_iter().map(|v| visit(v, paths)).collect())
            }
            Value::Object(items) => Value::Object(
                items
                    .into_iter()
                    .map(|(k, v)| (k, visit(v, paths)))
                    .collect(),
            ),
            other => other,
        }
    }
    Ok(visit(value, &paths))
}
