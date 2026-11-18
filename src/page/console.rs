//! The console of the current document, read over CDP when it is asked for.
//!
//! Chrome keeps what a document logged (console calls, uncaught exceptions,
//! and its own messages such as a failed request) and replays all of it to a
//! session that enables `Runtime` and `Log` (measured, Chrome 158: the replay
//! precedes each reply, 2.4 ms for both). So nothing is installed in the page
//! to capture it, and the read works in every lane, an attached browser
//! included.
//!
//! `Runtime.enable` is page-observable while it is on (a logged Error then
//! runs the page's `Error.prepareStackTrace`), so both domains are on only
//! for the read and off again before it returns. `Runtime.disable` releases
//! the session's remote objects; the reader world finds its anchor again on
//! its next read.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::broadcast::error::TryRecvError;

use super::*;
use crate::cdp::protocol::CdpEvent;

/// Entries kept, newest last. Chrome itself keeps 1000 per page.
const MAX_ENTRIES: usize = 200;
/// Characters of one entry's message.
const MAX_MESSAGE: usize = 400;
/// Logged objects whose properties are read. A replayed argument carries
/// only "Object"; its content costs one command each.
const MAX_EXPANDED: usize = 40;
/// Properties shown of one logged object.
const MAX_PROPERTIES: usize = 12;
/// Stack frames kept of an exception or a logged Error.
const MAX_FRAMES: usize = 3;
const COMMAND: Duration = Duration::from_secs(5);

impl Page {
    /// What the current document logged, oldest first: an array of
    /// `{l: level, m: message, t: epoch ms}`.
    pub async fn console_log(&self) -> Result<Value> {
        let cdp = self.cdp.main_world();
        let mut sub = cdp.subscribe();
        let read = async {
            cdp.send_with_timeout("Runtime.enable", None, COMMAND)
                .await?;
            // A target without the Log domain still has its console.
            let _ = cdp.send_with_timeout("Log.enable", None, COMMAND).await;
            let mut events = Vec::new();
            loop {
                match sub.try_recv() {
                    Ok(ev) => events.push(ev),
                    Err(TryRecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            let mut objects = HashMap::new();
            for id in objects_to_expand(&events) {
                let props = cdp
                    .send_with_timeout(
                        "Runtime.getProperties",
                        Some(json!({"objectId": id, "ownProperties": true})),
                        COMMAND,
                    )
                    .await;
                if let Ok(props) = props {
                    objects.insert(id, props);
                }
            }
            Ok(entries(&events, &objects))
        };
        let out: Result<Vec<Value>> = read.await;
        let _ = cdp.send_with_timeout("Log.disable", None, COMMAND).await;
        let _ = cdp
            .send_with_timeout("Runtime.disable", None, COMMAND)
            .await;
        out.map(Value::Array)
    }

    pub async fn clear_console_log(&self) -> Result<()> {
        let cdp = self.cdp.main_world();
        cdp.send_with_timeout("Runtime.discardConsoleEntries", None, COMMAND)
            .await?;
        let _ = cdp.send_with_timeout("Log.clear", None, COMMAND).await;
        Ok(())
    }
}

fn is_plain_object(arg: &Value) -> bool {
    arg.get("type").and_then(Value::as_str) == Some("object")
        && matches!(
            arg.get("subtype").and_then(Value::as_str),
            None | Some("array")
        )
}

/// Object ids of the plain objects and arrays logged by the newest entries.
fn objects_to_expand(events: &[CdpEvent]) -> Vec<String> {
    let mut ids: Vec<String> = events
        .iter()
        .rev()
        .filter(|ev| ev.method == "Runtime.consoleAPICalled")
        .take(MAX_ENTRIES)
        .flat_map(|ev| ev.params.get("args").and_then(Value::as_array))
        .flatten()
        .filter(|arg| is_plain_object(arg))
        .filter_map(|arg| arg.get("objectId").and_then(Value::as_str))
        .map(str::to_string)
        .take(MAX_EXPANDED)
        .collect();
    ids.dedup();
    ids
}

fn cut(mut text: String) -> String {
    if text.chars().count() > MAX_MESSAGE {
        text = text.chars().take(MAX_MESSAGE).collect::<String>() + "…";
    }
    text
}

/// "Error: boom\n    at f (app.js:2:5)\n    at …" as one line:
/// `Error: boom | at f (app.js:2:5)`, at most [`MAX_FRAMES`] frames.
fn one_line_stack(description: &str) -> String {
    let mut lines = description.lines().map(str::trim).filter(|l| !l.is_empty());
    let mut out = lines.next().unwrap_or_default().to_string();
    let mut frames = 0;
    for line in lines {
        if line.starts_with("at ") {
            if frames == MAX_FRAMES {
                break;
            }
            frames += 1;
        }
        out.push_str(" | ");
        out.push_str(line);
    }
    out
}

/// A value inside a logged object: primitives as JSON writes them, anything
/// else by its description ("Object", "Array(3)").
fn nested(value: &Value) -> Option<String> {
    match value.get("type").and_then(Value::as_str)? {
        "undefined" | "function" | "symbol" => None,
        "string" => Some(serde_json::to_string(value.get("value")?).ok()?),
        _ => Some(match value.get("value") {
            Some(v) if !v.is_null() => v.to_string(),
            _ if value.get("subtype").and_then(Value::as_str) == Some("null") => "null".into(),
            _ => value
                .get("unserializableValue")
                .or(value.get("description"))
                .and_then(Value::as_str)
                .unwrap_or("…")
                .to_string(),
        }),
    }
}

/// A logged object from its own properties, as `{"a":1,"b":"x"}` or `[1,2]`.
/// Accessors read as `"(...)"`: `Runtime.getProperties` does not call them.
fn object_text(arg: &Value, props: &Value) -> String {
    let array = arg.get("subtype").and_then(Value::as_str) == Some("array");
    let mut parts = Vec::new();
    let mut more = false;
    for p in props
        .get("result")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = p.get("name").and_then(Value::as_str).unwrap_or_default();
        if p.get("enumerable").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let text = match p.get("value") {
            Some(v) => match nested(v) {
                Some(text) => text,
                None => continue,
            },
            None => "\"(...)\"".to_string(),
        };
        if parts.len() == MAX_PROPERTIES {
            more = true;
            break;
        }
        parts.push(if array {
            text
        } else {
            format!("{}:{text}", Value::String(name.to_string()))
        });
    }
    if more {
        parts.push("…".into());
    }
    if array {
        format!("[{}]", parts.join(","))
    } else {
        format!("{{{}}}", parts.join(","))
    }
}

/// A logged object from the preview Chrome attaches to a live console event,
/// for when its handle is gone (a host that shares the page's debugger
/// releases console objects as they arrive). Chrome previews a few properties
/// and says when it left some out.
fn preview_text(preview: &Value) -> String {
    let array = preview.get("subtype").and_then(Value::as_str) == Some("array");
    let mut parts = Vec::new();
    for p in preview
        .get("properties")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_PROPERTIES)
    {
        let value = p.get("value").and_then(Value::as_str).unwrap_or_default();
        let text = match p.get("type").and_then(Value::as_str).unwrap_or_default() {
            "undefined" | "function" | "symbol" => continue,
            "string" => Value::String(value.to_string()).to_string(),
            "accessor" => "\"(...)\"".to_string(),
            _ => value.to_string(),
        };
        let name = p.get("name").and_then(Value::as_str).unwrap_or_default();
        parts.push(if array {
            text
        } else {
            format!("{}:{text}", Value::String(name.to_string()))
        });
    }
    if preview.get("overflow").and_then(Value::as_bool) == Some(true) {
        parts.push("…".into());
    }
    if array {
        format!("[{}]", parts.join(","))
    } else {
        format!("{{{}}}", parts.join(","))
    }
}

fn argument_text(arg: &Value, objects: &HashMap<String, Value>) -> String {
    if arg.get("type").and_then(Value::as_str) == Some("string") {
        return arg
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
    }
    if let Some(v) = arg.get("value").filter(|v| !v.is_null()) {
        return v.to_string();
    }
    if arg.get("subtype").and_then(Value::as_str) == Some("null") {
        return "null".into();
    }
    if is_plain_object(arg) {
        let props = arg
            .get("objectId")
            .and_then(Value::as_str)
            .and_then(|id| objects.get(id));
        if let Some(props) = props {
            return object_text(arg, props);
        }
        if let Some(preview) = arg.get("preview") {
            return preview_text(preview);
        }
    }
    let description = arg
        .get("unserializableValue")
        .or(arg.get("description"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| arg.get("type").and_then(Value::as_str).unwrap_or("?"));
    if arg.get("subtype").and_then(Value::as_str) == Some("error") {
        one_line_stack(description)
    } else {
        description.to_string()
    }
}

/// `url:line:col` of a call frame or an exception, 1-based as editors and
/// stack traces count.
fn place(at: &Value) -> Option<String> {
    let url = at
        .get("url")
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())?;
    let line = at.get("lineNumber").and_then(Value::as_i64)? + 1;
    let column = at.get("columnNumber").and_then(Value::as_i64).unwrap_or(0) + 1;
    Some(format!("{url}:{line}:{column}"))
}

/// The entries of one replay, oldest first.
fn entries(events: &[CdpEvent], objects: &HashMap<String, Value>) -> Vec<Value> {
    let mut out: Vec<(f64, Value)> = Vec::new();
    let mut push = |level: &str, message: String, at: f64| {
        out.push((at, json!({"l": level, "m": cut(message), "t": at as u64})));
    };
    // The driver reads in an isolated world of its own, and extensions run
    // theirs: what those log or throw is not the page's console.
    let apart: Vec<i64> = events
        .iter()
        .filter(|ev| ev.method == "Runtime.executionContextCreated")
        .filter_map(|ev| ev.params.get("context"))
        .filter(|c| c.pointer("/auxData/type").and_then(Value::as_str) == Some("isolated"))
        .filter_map(|c| c.get("id").and_then(Value::as_i64))
        .collect();
    let from_apart = |context: Option<&Value>| {
        context
            .and_then(Value::as_i64)
            .is_some_and(|id| apart.contains(&id))
    };
    for ev in events {
        let p = &ev.params;
        match ev.method.as_str() {
            "Runtime.consoleAPICalled" if from_apart(p.get("executionContextId")) => {}
            "Runtime.exceptionThrown"
                if from_apart(p.pointer("/exceptionDetails/executionContextId")) => {}
            "Runtime.consoleAPICalled" => {
                let level = match p.get("type").and_then(Value::as_str).unwrap_or("log") {
                    "warning" => "warn",
                    // A group heading is a line of the log like any other.
                    "startGroup" | "startGroupCollapsed" => "log",
                    "endGroup" | "clear" => continue,
                    other => other,
                };
                let args = p.get("args").and_then(Value::as_array);
                let mut message = args
                    .into_iter()
                    .flatten()
                    .map(|a| argument_text(a, objects))
                    .collect::<Vec<_>>()
                    .join(" ");
                // Where an error or a warning was logged from is half of
                // finding it; a plain log line stays as short as it was.
                if matches!(level, "error" | "warn" | "assert" | "trace") {
                    let from = p.pointer("/stackTrace/callFrames/0").and_then(place);
                    if let Some(from) = from.filter(|from| !message.contains(from.as_str())) {
                        message.push_str(&format!(" @{from}"));
                    }
                }
                let at = p.get("timestamp").and_then(Value::as_f64).unwrap_or(0.0);
                push(level, message, at);
            }
            "Runtime.exceptionThrown" => {
                let Some(d) = p.get("exceptionDetails") else {
                    continue;
                };
                let text = d.get("text").and_then(Value::as_str).unwrap_or("Uncaught");
                let thrown = d.get("exception");
                let description = thrown
                    .and_then(|e| e.get("description").or(e.get("value")))
                    .map(|v| match v.as_str() {
                        Some(s) => one_line_stack(s),
                        None => v.to_string(),
                    });
                let mut message = match description {
                    Some(description) => format!("{text} {description}"),
                    None => text.to_string(),
                };
                if let Some(from) = place(d).filter(|from| !message.contains(from.as_str())) {
                    message.push_str(&format!(" @{from}"));
                }
                let at = p.get("timestamp").and_then(Value::as_f64).unwrap_or(0.0);
                push("exception", message, at);
            }
            "Log.entryAdded" => {
                let Some(e) = p.get("entry") else { continue };
                let level = match e.get("level").and_then(Value::as_str).unwrap_or("info") {
                    "warning" => "warn",
                    "verbose" => "debug",
                    other => other,
                };
                let mut message = e
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let url = e.get("url").and_then(Value::as_str).unwrap_or_default();
                if !url.is_empty() && !message.contains(url) {
                    message.push_str(&format!(" {url}"));
                }
                let at = e.get("timestamp").and_then(Value::as_f64).unwrap_or(0.0);
                push(level, message, at);
            }
            _ => {}
        }
    }
    // Three streams, each in order; one log, in the order things happened.
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    let skip = out.len().saturating_sub(MAX_ENTRIES);
    out.into_iter().skip(skip).map(|(_, entry)| entry).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(method: &str, params: Value) -> CdpEvent {
        CdpEvent {
            method: method.into(),
            params,
            session_id: None,
        }
    }

    fn messages(events: &[CdpEvent], objects: &HashMap<String, Value>) -> Vec<String> {
        entries(events, objects)
            .iter()
            .map(|e| format!("{}: {}", e["l"].as_str().unwrap(), e["m"].as_str().unwrap()))
            .collect()
    }

    // The shapes below are what Chrome 158 replayed for a page that logged,
    // warned, threw and failed a request.
    #[test]
    fn a_replay_becomes_one_log_in_the_order_things_happened() {
        let events = [
            event(
                "Runtime.consoleAPICalled",
                json!({"type": "log", "timestamp": 10.0, "args": [
                    {"type": "string", "value": "page start"},
                    {"type": "object", "className": "Object", "description": "Object", "objectId": "1.2.3"},
                ], "stackTrace": {"callFrames": [{"url": "http://h/", "lineNumber": 4, "columnNumber": 8}]}}),
            ),
            event(
                "Runtime.exceptionThrown",
                json!({"timestamp": 30.0, "exceptionDetails": {
                    "text": "Uncaught", "url": "http://h/", "lineNumber": 8, "columnNumber": 19,
                    "exception": {"type": "object", "subtype": "error", "description":
                        "ReferenceError: f is not defined\n    at http://h/:9:20\n    at a (http://h/a.js:1:1)\n    at b (http://h/a.js:2:1)\n    at c (http://h/a.js:3:1)"},
                }}),
            ),
            event(
                "Runtime.consoleAPICalled",
                json!({"type": "warning", "timestamp": 20.0, "args": [
                    {"type": "string", "value": "slow"},
                    {"type": "number", "value": 404},
                ], "stackTrace": {"callFrames": [{"url": "http://h/app.js", "lineNumber": 0, "columnNumber": 41}]}}),
            ),
            event(
                "Log.entryAdded",
                json!({"entry": {"source": "network", "level": "error", "timestamp": 25.0,
                    "text": "Failed to load resource: the server responded with a status of 404 (File not found)",
                    "url": "http://h/missing.json"}}),
            ),
            event(
                "Runtime.executionContextCreated",
                json!({"context": {"id": 2, "auxData": {"isDefault": true, "type": "default"}}}),
            ),
            // The driver's own world: a script of its that did not compile,
            // and a log from it, are not the page's.
            event(
                "Runtime.executionContextCreated",
                json!({"context": {"id": 5, "auxData": {"isDefault": false, "type": "isolated"}}}),
            ),
            event(
                "Runtime.exceptionThrown",
                json!({"timestamp": 26.0, "exceptionDetails": {"text": "Uncaught", "executionContextId": 5,
                    "exception": {"type": "object", "subtype": "error", "description": "SyntaxError: Unexpected token 'catch'"}}}),
            ),
            event(
                "Runtime.consoleAPICalled",
                json!({"type": "log", "timestamp": 27.0, "executionContextId": 5,
                    "args": [{"type": "string", "value": "from the driver"}]}),
            ),
        ];
        let objects = HashMap::from([(
            "1.2.3".to_string(),
            json!({"result": [
                {"name": "a", "enumerable": true, "value": {"type": "number", "value": 1}},
                {"name": "s", "enumerable": true, "value": {"type": "string", "value": "x\"y"}},
                {"name": "o", "enumerable": true, "value": {"type": "object", "description": "Array(3)"}},
                {"name": "f", "enumerable": true, "value": {"type": "function", "description": "f(){}"}},
                {"name": "g", "enumerable": true, "get": {"type": "function"}},
                {"name": "hidden", "enumerable": false, "value": {"type": "number", "value": 2}},
            ]}),
        )]);
        assert_eq!(
            messages(&events, &objects),
            [
                r#"log: page start {"a":1,"s":"x\"y","o":Array(3),"g":"(...)"}"#,
                "warn: slow 404 @http://h/app.js:1:42",
                "error: Failed to load resource: the server responded with a status of 404 (File not found) http://h/missing.json",
                "exception: Uncaught ReferenceError: f is not defined | at http://h/:9:20 | at a (http://h/a.js:1:1) | at b (http://h/a.js:2:1)",
            ]
        );
    }

    // The argument shapes are what Chrome 158 sent with a live console event.
    #[test]
    fn an_object_whose_handle_is_gone_is_read_from_its_preview() {
        let events = [event(
            "Runtime.consoleAPICalled",
            json!({"type": "log", "timestamp": 1.0, "args": [
                {"type": "object", "description": "Object", "objectId": "1.1",
                 "preview": {"type": "object", "description": "Object", "overflow": true, "properties": [
                    {"name": "a", "type": "number", "value": "1"},
                    {"name": "s", "type": "string", "value": "x"},
                    {"name": "n", "type": "object", "value": "null", "subtype": "null"},
                    {"name": "o", "type": "object", "value": "Object"},
                    {"name": "f", "type": "function", "value": ""}]}},
                {"type": "object", "subtype": "array", "description": "Array(3)", "objectId": "1.2",
                 "preview": {"type": "object", "subtype": "array", "description": "Array(3)", "overflow": false, "properties": [
                    {"name": "0", "type": "number", "value": "1"},
                    {"name": "1", "type": "string", "value": "two"},
                    {"name": "2", "type": "object", "value": "Object"}]}},
            ]}),
        )];
        assert_eq!(
            messages(&events, &HashMap::new()),
            [r#"log: {"a":1,"s":"x","n":null,"o":Object,…} [1,"two",Object]"#]
        );
    }

    #[test]
    fn an_object_nobody_read_keeps_its_description_and_an_error_its_stack() {
        let events = [event(
            "Runtime.consoleAPICalled",
            json!({"type": "error", "timestamp": 1.0, "args": [
                {"type": "object", "description": "Object", "objectId": "9"},
                {"type": "object", "subtype": "error", "description": "Error: inner\n    at http://h/:7:27"},
                {"type": "object", "subtype": "null", "value": null},
                {"type": "undefined"},
                {"type": "number", "unserializableValue": "NaN", "description": "NaN"},
            ]}),
        )];
        assert_eq!(
            messages(&events, &HashMap::new()),
            ["error: Object Error: inner | at http://h/:7:27 null undefined NaN"]
        );
    }

    #[test]
    fn only_the_newest_entries_are_kept_and_expanded() {
        let events: Vec<CdpEvent> = (0..MAX_ENTRIES + 50)
            .map(|i| {
                event(
                    "Runtime.consoleAPICalled",
                    json!({"type": "log", "timestamp": i as f64, "args": [
                        {"type": "object", "description": "Object", "objectId": format!("o{i}")},
                    ]}),
                )
            })
            .collect();
        let kept = entries(&events, &HashMap::new());
        assert_eq!(kept.len(), MAX_ENTRIES);
        assert_eq!(kept[0]["t"], 50);
        let ids = objects_to_expand(&events);
        assert_eq!(ids.len(), MAX_EXPANDED);
        assert_eq!(ids[0], format!("o{}", MAX_ENTRIES + 49));
    }

    #[test]
    fn a_long_message_is_cut() {
        let events = [event(
            "Runtime.consoleAPICalled",
            json!({"type": "log", "timestamp": 1.0, "args": [{"type": "string", "value": "x".repeat(2000)}]}),
        )];
        let m = entries(&events, &HashMap::new())[0]["m"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(m.chars().count(), MAX_MESSAGE + 1);
        assert!(m.ends_with('…'));
    }
}
