//! The reader world: where the driver's own page reads run.
//!
//! Every perception step is a `Runtime.evaluate` — the capture, the element
//! lookups, the settle waits, the verdict probes. Run in the page's world they
//! go through the page's own DOM prototypes: a page that wraps
//! `Element.prototype.getAttribute` (any bot-defence script can) counted
//! 160–290 calls per command with no page script on the stack (measured,
//! Brave 157: `see` 164, `click` 201, `type` 292). Run in an isolated world
//! of the same frame they see the same DOM through prototypes the page cannot
//! reach — the same instrumented page counts 0.
//!
//! A session with a reader ([`CdpSession::reading_apart`]) sends its plain
//! `Runtime.evaluate` calls there. What must see the page's JavaScript — the
//! agent's own `eval`, site adapters calling the page's API, the stealth
//! self-checks — asks for [`CdpSession::main_world`] instead.
//!
//! The world is addressed by the object id of its `globalThis`, not by its
//! execution-context id: context ids restart in every renderer process, so
//! after a cross-process navigation a remembered id can name an unrelated
//! context of the new document (measured: `3` in both processes). An object
//! id carries the isolate it belongs to and simply stops resolving.
//!
//! No `Runtime.enable` anywhere here: it is the one CDP side effect pages can
//! detect, and this driver never sends it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::session::CdpSession;
use crate::error::{BladeError, Result};

/// `Runtime.evaluate` parameters with the same meaning on
/// `Runtime.callFunctionOn`. A call carrying anything else (a context id, the
/// command-line API, …) asked for something specific and is sent as written.
const SHARED_PARAMS: &[&str] = &[
    "expression",
    "returnByValue",
    "awaitPromise",
    "userGesture",
    "silent",
    "generatePreview",
    "objectGroup",
];

/// In the reader world only: the set of calls that started running and have
/// not been asked about. One mark per call, not "the last one" — the idle hum
/// and an action share a reader, and a single slot let one overwrite the
/// other's mark between a throw and the question.
const RAN_SLOT: &str = "Symbol.for('r')";

#[derive(Default)]
pub(super) struct Reader {
    /// Object id of the reader world's `globalThis` in the current document.
    anchor: Mutex<Option<String>>,
    /// The target has no frame to make a world in (a worker): stop asking.
    unavailable: AtomicBool,
    calls: AtomicU64,
}

impl Reader {
    /// Reads go to the reader world (the target has frames to make one in).
    pub(super) fn apart(&self) -> bool {
        !self.unavailable.load(Ordering::Relaxed)
    }
}

/// `BLADE_MAIN_WORLD=1` keeps every read in the page's world — the behavior
/// before the reader world, for telling a page-side difference apart.
pub(super) fn enabled() -> bool {
    std::env::var("BLADE_MAIN_WORLD").as_deref() != Ok("1")
}

/// The caller's deadline, shared by every command a read takes.
#[derive(Clone, Copy)]
struct Budget {
    until: Instant,
    total: Duration,
}

impl Budget {
    fn new(total: Duration) -> Self {
        Self {
            until: Instant::now() + total,
            total,
        }
    }

    fn left(self) -> Result<Duration> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            // Spent before the script could run: the page is not answering.
            return Err(BladeError::Timeout(self.total));
        }
        Ok(left)
    }
}

/// The anchor no longer resolves: its document is gone (navigation, a
/// restore from the back/forward cache). Nothing ran.
fn anchor_gone(e: &BladeError) -> bool {
    matches!(e, BladeError::Cdp { message, .. }
        if message.contains("Cannot find context")
            || message.contains("Could not find object")
            || message.contains("Invalid remote object id"))
}

/// The expression as a function body. `Runtime.evaluate` takes a program,
/// `callFunctionOn` a function: an expression (every driver script is one
/// IIFE) fits `return (…)`; a statement list does not compile there and is
/// sent to the page's world as before (see [`evaluate`]).
fn as_function(expression: &str, call: u64) -> String {
    let body = expression.trim().trim_end_matches(';').trim_end();
    // No local names here: the expression must resolve every identifier
    // exactly as it would at the top level. Old marks are dropped in bulk;
    // the newline keeps a trailing `//` comment from eating the parenthesis.
    format!(
        "function(){{(this[{RAN_SLOT}]||(this[{RAN_SLOT}]=new Set())).add({call});\
         this[{RAN_SLOT}].size>256&&this[{RAN_SLOT}].forEach(function(v,_,s){{v<{call}-128&&s.delete(v)}});\
         return (\n{body}\n);}}"
    )
}

fn translatable(params: &Value) -> Option<&str> {
    let obj = params.as_object()?;
    if obj.keys().any(|k| !SHARED_PARAMS.contains(&k.as_str())) {
        return None;
    }
    obj.get("expression")?.as_str()
}

/// Make (or find) the reader world of the main frame's current document and
/// return the object id of its global.
async fn anchor(cdp: &CdpSession, budget: Budget) -> Result<String> {
    let frame = |tree: &Value| -> Option<(String, String)> {
        let f = tree.get("frameTree")?.get("frame")?;
        Some((
            f.get("id")?.as_str()?.to_string(),
            f.get("loaderId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ))
    };
    let no_frame = || BladeError::Other("reader world: the target has no frame".into());
    for _ in 0..3 {
        let before = cdp.raw("Page.getFrameTree", None, budget.left()?).await?;
        let (frame_id, loader) = frame(&before).ok_or_else(no_frame)?;
        let world = cdp
            .raw(
                "Page.createIsolatedWorld",
                Some(json!({"frameId": frame_id, "worldName": ""})),
                budget.left()?,
            )
            .await?;
        let Some(context) = world.get("executionContextId").and_then(Value::as_i64) else {
            return Err(no_frame());
        };
        let global = cdp
            .raw(
                "Runtime.evaluate",
                Some(json!({"expression": "globalThis", "contextId": context})),
                budget.left()?,
            )
            .await;
        let object = match global {
            Ok(v) => v
                .pointer("/result/objectId")
                .and_then(Value::as_str)
                .map(str::to_string),
            // The document went away between the two commands.
            Err(e) if anchor_gone(&e) => None,
            Err(e) => return Err(e),
        };
        // The context id was only valid in the document it was made for. A
        // navigation committing in between could have made it name another
        // context (see the module note); the loader id says whether one did.
        let after = cdp.raw("Page.getFrameTree", None, budget.left()?).await?;
        if let (Some(object), Some((_, now))) = (object, frame(&after)) {
            if now == loader {
                return Ok(object);
            }
        }
    }
    Err(BladeError::Other(
        "reader world: the document kept changing".into(),
    ))
}

/// A `SyntaxError` with no stack frame: what a function body that does not
/// compile answers with. One thrown by running code (`JSON.parse`) carries
/// the frames it was thrown through. Necessary, not sufficient — [`ran`]
/// settles it.
fn never_compiled(reply: &Value) -> bool {
    let Some(exception) = reply.pointer("/exceptionDetails/exception") else {
        return false;
    };
    exception.get("className").and_then(Value::as_str) == Some("SyntaxError")
        && !exception
            .get("description")
            .and_then(Value::as_str)
            .is_some_and(|d| d.contains("\n    at "))
}

/// Did the function of call `call` start running? Told apart from a compile
/// failure of the wrapper, where nothing ran and the page's world may still
/// take the script.
async fn ran(cdp: &CdpSession, anchor: &str, call: u64, budget: Budget) -> bool {
    let Ok(left) = budget.left() else {
        return true;
    };
    cdp.raw(
        "Runtime.callFunctionOn",
        Some(json!({
            "objectId": anchor,
            "functionDeclaration":
                format!("function(){{return !!(this[{RAN_SLOT}]&&this[{RAN_SLOT}].delete({call}));}}"),
            "returnByValue": true,
        })),
        left,
    )
    .await
    .ok()
    .and_then(|v| v.pointer("/result/value").and_then(Value::as_bool))
    // Unknown is treated as "ran": never run a script twice.
    .unwrap_or(true)
}

/// `Runtime.evaluate`, in the reader world when there is one.
pub(super) async fn evaluate(
    cdp: &CdpSession,
    reader: &Reader,
    params: Option<Value>,
    timeout: Duration,
) -> Result<Value> {
    let in_page = |params: Option<Value>, left: Duration| async move {
        cdp.raw("Runtime.evaluate", params, left).await
    };
    let Some(expression) = params.as_ref().and_then(translatable) else {
        return in_page(params, timeout).await;
    };
    if reader.unavailable.load(Ordering::Relaxed) {
        return in_page(params, timeout).await;
    }
    let budget = Budget::new(timeout);
    let call = reader.calls.fetch_add(1, Ordering::Relaxed) + 1;
    let mut function = params.clone().unwrap_or_default();
    if let Some(obj) = function.as_object_mut() {
        obj.remove("expression");
        obj.insert(
            "functionDeclaration".into(),
            Value::String(as_function(expression, call)),
        );
    }

    // Two rounds: the remembered anchor, then a fresh one if that document
    // is gone. A gone anchor means the function never started.
    for _ in 0..2 {
        let known = reader
            .anchor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let object = match known {
            Some(object) => object,
            None => match anchor(cdp, budget).await {
                Ok(object) => {
                    *reader.anchor.lock().unwrap_or_else(|e| e.into_inner()) = Some(object.clone());
                    object
                }
                Err(e @ (BladeError::Closed | BladeError::Timeout(_))) => return Err(e),
                Err(e) => {
                    // No Page domain on this target (a worker): there is one
                    // world, and asking again would cost two commands a read.
                    if matches!(&e, BladeError::Cdp { code: -32601, .. }) {
                        reader.unavailable.store(true, Ordering::Relaxed);
                    }
                    return in_page(params, budget.left()?).await;
                }
            },
        };
        if let Some(obj) = function.as_object_mut() {
            obj.insert("objectId".into(), Value::String(object.clone()));
        }
        match cdp
            .raw(
                "Runtime.callFunctionOn",
                Some(function.clone()),
                budget.left()?,
            )
            .await
        {
            Err(e) if anchor_gone(&e) => {
                let mut slot = reader.anchor.lock().unwrap_or_else(|e| e.into_inner());
                if slot.as_deref() == Some(object.as_str()) {
                    *slot = None;
                }
            }
            Ok(v) if never_compiled(&v) && !ran(cdp, &object, call, budget).await => {
                // Not an expression: it never compiled as a function body.
                return in_page(params, budget.left()?).await;
            }
            other => return other,
        }
    }
    // The document changed under both rounds; the page's world answers for
    // whatever is there now, as it always did.
    in_page(params, budget.left()?).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_evaluations_move_to_the_reader_world() {
        let plain = json!({"expression": "1", "returnByValue": true, "awaitPromise": true});
        assert_eq!(translatable(&plain), Some("1"));
        // An explicit context, or a parameter callFunctionOn does not have,
        // is a request for exactly that.
        for specific in [
            json!({"expression": "1", "contextId": 4}),
            json!({"expression": "1", "includeCommandLineAPI": true}),
            json!({"expression": "1", "replMode": true}),
            json!({"returnByValue": true}),
        ] {
            assert_eq!(translatable(&specific), None, "{specific}");
        }
    }

    #[test]
    fn an_expression_becomes_a_function_body_that_cannot_swallow_its_own_end() {
        let f = as_function(" (()=>1)(); \n", 7);
        assert!(f.contains(".add(7);"), "{f}");
        assert!(f.ends_with("return (\n(()=>1)()\n);}"), "{f}");
        // A trailing line comment must not comment out the closing paren.
        let f = as_function("document.title // the title", 1);
        assert!(f.ends_with("// the title\n);}"), "{f}");
    }

    #[test]
    fn only_a_syntax_error_without_frames_can_be_a_body_that_never_compiled() {
        let reply = |description: &str| {
            json!({"exceptionDetails": {"exception": {
                "className": "SyntaxError", "description": description,
            }}})
        };
        assert!(never_compiled(&reply(
            "SyntaxError: Unexpected identifier 'statements'"
        )));
        // Thrown by code that ran: it has the frames it came through.
        assert!(!never_compiled(&reply(
            "SyntaxError: Unexpected end of JSON input\n    at JSON.parse (<anonymous>)\n    at <anonymous>:3:6"
        )));
        assert!(!never_compiled(&json!({"result": {"value": 1}})));
        assert!(!never_compiled(&json!({"exceptionDetails": {"exception": {
            "className": "TypeError", "description": "TypeError: x",
        }}})));
    }

    #[test]
    fn only_a_missing_anchor_is_retried() {
        let gone = |m: &str| BladeError::Cdp {
            code: -32000,
            message: m.into(),
        };
        assert!(anchor_gone(&gone("Cannot find context with specified id")));
        assert!(anchor_gone(&gone("Could not find object with given id")));
        // The script started and its document went away: the caller's news,
        // never a silent second run.
        assert!(!anchor_gone(&gone("Execution context was destroyed.")));
        assert!(!anchor_gone(&BladeError::Timeout(Duration::from_secs(1))));
        assert!(!anchor_gone(&BladeError::Closed));
    }
}
