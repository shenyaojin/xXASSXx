//! Portable task descriptions. Physical paths remain in the member's local bindings.
use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

fn reference_boundary(prefix: &str) -> bool {
    let boundary = prefix
        .chars()
        .next_back()
        .is_none_or(|c| c.is_whitespace() || "(（[{\"'`=:,;，；：".contains(c));
    // A URL query such as https://host/?path=/tmp is not a local reference.
    boundary
        && !prefix
            .rsplit(|c: char| c.is_whitespace() || "(（[{\"'`,;，；".contains(c))
            .next()
            .unwrap_or("")
            .contains("://")
}

// A path outside the sender's bindings can describe a failed search, or a path
// quoted from another machine. Preserve it as an unbound reference, never infer
// ownership or grant access. Transport must not fail just because an executor
// explains that /tmp (for example) contained no matching file.
fn unbound_paths(text: &str) -> String {
    let text = text.replace("file://", "unbound://file/");
    let mut out = String::new();
    for (at, ch) in text.char_indices() {
        let boundary = reference_boundary(&text[..at]);
        let rest = &text[at..];
        if boundary
            && ch == '/'
            && rest.chars().nth(1).is_some_and(|c| {
                !c.is_whitespace() && c != '/' && !")]},;:。；，）\"'`".contains(c)
            })
        {
            out.push_str("unbound://absolute");
        } else if boundary && ch.is_ascii_alphabetic() && rest.as_bytes().get(1..3) == Some(b":\\")
        {
            out.push_str("unbound://absolute/");
        }
        out.push(ch);
    }
    out
}

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
                text = text.replace("file:///", "/");
                for (path, alias) in paths {
                    if path == "/" || path.is_empty() {
                        continue;
                    }
                    let mut result = String::new();
                    let mut cursor = 0;
                    for (at, _) in text.match_indices(path) {
                        let end = at + path.len();
                        let next = text[end..].chars().next();
                        result.push_str(&text[cursor..at]);
                        if reference_boundary(&text[..at])
                            && next.is_none_or(|c| {
                                c == '/' || c.is_whitespace() || ")]},;:。；，）\"'`".contains(c)
                            })
                        {
                            result.push_str(alias);
                        } else {
                            result.push_str(path);
                        }
                        cursor = end;
                    }
                    result.push_str(&text[cursor..]);
                    text = result;
                }
                Value::String(unbound_paths(&text))
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
