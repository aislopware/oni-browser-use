//! The driver's page reads run in an isolated world, not the page's.
//!
//! Every capture, lookup, wait and verdict probe is a `Runtime.evaluate`. In
//! the page's world they call the page's DOM prototypes: an instrumented page
//! counted 164 (`see`) to 292 (`type`) calls per command with no page script
//! on the stack — a one-hook detection of the driver (measured, Brave 157).
//! A page session now sends them to an isolated world of the main frame,
//! addressed by the object id of that world's global (context ids restart per
//! renderer process and can name another context after a navigation).
//!
//! A fake page speaking just enough CDP locks the routing: what is sent, in
//! which world, and what happens when the document changes.

use std::sync::{Arc, Mutex};

use bladebro::cdp::{CdpClient, CdpSession};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

#[derive(Default)]
struct FakePage {
    /// Bumped by a navigation: a new document, a new loader, new object ids.
    document: u64,
    /// Navigations still to commit, each right after the next world is made
    /// (between `createIsolatedWorld` and the read of its global).
    races: u32,
    /// A target with no Page domain (a worker).
    no_page_domain: bool,
    /// The last reader-world call left its "started" mark.
    started: bool,
    /// Every command received: (method, params).
    seen: Vec<(String, Value)>,
}

impl FakePage {
    fn anchor(&self) -> String {
        format!("ISOLATE{}.7.1", self.document)
    }

    fn answer(&mut self, method: &str, params: &Value) -> Result<Value, (i64, &'static str)> {
        self.seen.push((method.to_string(), params.clone()));
        match method {
            "Page.getFrameTree" | "Page.createIsolatedWorld" if self.no_page_domain => {
                Err((-32601, "'Page.getFrameTree' wasn't found"))
            }
            "Page.getFrameTree" => Ok(json!({"frameTree": {"frame": {
                "id": "MAIN", "loaderId": format!("L{}", self.document),
            }}})),
            "Page.createIsolatedWorld" => {
                let made = json!({"executionContextId": 7});
                if self.races > 0 {
                    self.races -= 1;
                    self.document += 1;
                }
                Ok(made)
            }
            // The read of the new world's global.
            "Runtime.evaluate" if params.get("contextId").is_some() => {
                Ok(json!({"result": {"type": "object", "objectId": self.anchor()}}))
            }
            "Runtime.evaluate" => Ok(json!({"result": {"type": "string", "value": "page"}})),
            "Runtime.callFunctionOn" => {
                if params["objectId"] != self.anchor() {
                    return Err((-32000, "Cannot find context with specified id"));
                }
                let body = params["functionDeclaration"].as_str().unwrap_or_default();
                let syntax_error = |description: &str| {
                    json!({
                        "result": {"type": "object", "className": "SyntaxError"},
                        "exceptionDetails": {"exception": {
                            "className": "SyntaxError", "description": description,
                        }},
                    })
                };
                if body.starts_with("function(){return !!(") {
                    // "Did that call start?"
                    let started = std::mem::take(&mut self.started);
                    Ok(json!({"result": {"type": "boolean", "value": started}}))
                } else if body.contains("var statements") {
                    // `return (var …)` does not compile; nothing ran.
                    Ok(syntax_error("SyntaxError: Unexpected token 'var'"))
                } else if body.contains("new Function(") {
                    // Ran, then threw from a frameless place: only the mark
                    // tells it from a body that never compiled.
                    self.started = true;
                    Ok(syntax_error("SyntaxError: Unexpected token '}'"))
                } else if body.contains("JSON.parse(") {
                    self.started = true;
                    Ok(syntax_error(
                        "SyntaxError: Unexpected end of JSON input\n    at JSON.parse (<anonymous>)",
                    ))
                } else {
                    Ok(json!({"result": {"type": "string", "value": "isolated"}}))
                }
            }
            _ => Ok(json!({})),
        }
    }
}

async fn fake_page() -> (CdpSession, Arc<Mutex<FakePage>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let page = Arc::new(Mutex::new(FakePage::default()));
    let state = page.clone();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        while let Some(Ok(message)) = ws.next().await {
            let Ok(text) = message.to_text() else {
                continue;
            };
            let Ok(request) = serde_json::from_str::<Value>(text) else {
                continue;
            };
            let method = request["method"].as_str().unwrap_or_default();
            let reply = match state.lock().unwrap().answer(method, &request["params"]) {
                Ok(result) => json!({"id": request["id"], "result": result}),
                Err((code, message)) => json!({
                    "id": request["id"],
                    "error": {"code": code, "message": message},
                }),
            };
            if ws
                .send(Message::Text(reply.to_string().into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    let client = CdpClient::connect(&format!("ws://{address}"))
        .await
        .unwrap();
    (CdpSession::root(client), page)
}

async fn read(cdp: &CdpSession, expression: &str) -> String {
    let res = cdp
        .send(
            "Runtime.evaluate",
            Some(json!({"expression": expression, "returnByValue": true})),
        )
        .await
        .unwrap();
    res["result"]["value"].as_str().unwrap_or("?").to_string()
}

fn methods(page: &Arc<Mutex<FakePage>>) -> Vec<String> {
    std::mem::take(&mut page.lock().unwrap().seen)
        .into_iter()
        .map(|(m, p)| {
            if m == "Runtime.evaluate" && p.get("contextId").is_none() {
                "Runtime.evaluate(page)".to_string()
            } else {
                m
            }
        })
        .collect()
}

#[tokio::test]
async fn a_page_session_reads_in_its_own_world_and_follows_the_document() {
    let (cdp, page) = fake_page().await;

    // What shipped: the read is one `Runtime.evaluate` in the page's world.
    assert_eq!(read(&cdp, "document.title").await, "page");
    assert_eq!(methods(&page), ["Runtime.evaluate(page)"]);

    // A page session: the same call never reaches the page's world.
    let cdp = cdp.reading_apart();
    assert_eq!(read(&cdp, "document.title").await, "isolated");
    assert_eq!(
        methods(&page),
        [
            "Page.getFrameTree",
            "Page.createIsolatedWorld",
            "Runtime.evaluate",
            "Page.getFrameTree",
            "Runtime.callFunctionOn",
        ],
        "the world is made once, verified against the loader, then used"
    );
    // The world is kept: the next read is one command.
    assert_eq!(read(&cdp, "location.href").await, "isolated");
    assert_eq!(methods(&page), ["Runtime.callFunctionOn"]);

    // A navigation: the old world's object stops resolving, the read moves
    // to the new document's world — and still never to the page's.
    page.lock().unwrap().document += 1;
    assert_eq!(read(&cdp, "location.href").await, "isolated");
    let after_nav = methods(&page);
    assert_eq!(after_nav.first().unwrap(), "Runtime.callFunctionOn");
    assert_eq!(after_nav.last().unwrap(), "Runtime.callFunctionOn");
    assert!(
        !after_nav.contains(&"Runtime.evaluate(page)".to_string()),
        "{after_nav:?}"
    );

    // A navigation committing while the world is being made: the context id
    // just returned may name anything in the new document, so the world is
    // made again rather than trusted.
    {
        let mut p = page.lock().unwrap();
        p.document += 1;
        p.races = 1;
    }
    assert_eq!(read(&cdp, "1").await, "isolated");
    let raced = methods(&page);
    assert_eq!(
        raced
            .iter()
            .filter(|m| *m == "Page.createIsolatedWorld")
            .count(),
        2,
        "{raced:?}"
    );
    assert!(!raced.contains(&"Runtime.evaluate(page)".to_string()));
}

#[tokio::test]
async fn the_agents_javascript_and_explicit_contexts_stay_where_they_were_asked() {
    let (cdp, page) = fake_page().await;
    let cdp = cdp.reading_apart();
    assert_eq!(read(&cdp, "1").await, "isolated");
    methods(&page);

    // `eval`, adapters, stealth self-checks: the page's own world.
    assert_eq!(read(&cdp.main_world(), "window.app").await, "page");
    assert_eq!(methods(&page), ["Runtime.evaluate(page)"]);
    // …without turning the session itself back.
    assert_eq!(read(&cdp, "1").await, "isolated");
    methods(&page);

    // A call that names its context is sent as written.
    cdp.send(
        "Runtime.evaluate",
        Some(json!({"expression": "1", "contextId": 3})),
    )
    .await
    .unwrap();
    let seen = std::mem::take(&mut page.lock().unwrap().seen);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "Runtime.evaluate");
    assert_eq!(seen[0].1["contextId"], 3);

    // A statement list is not a function body: it did not run in the reader
    // world, so the page's world takes it, exactly once.
    assert_eq!(read(&cdp, "var statements = 1; statements").await, "page");
    let seen = methods(&page);
    assert_eq!(
        seen.iter()
            .filter(|m| *m == "Runtime.evaluate(page)")
            .count(),
        1,
        "{seen:?}"
    );
}

#[tokio::test]
async fn a_target_without_frames_keeps_its_one_world() {
    let (cdp, page) = fake_page().await;
    page.lock().unwrap().no_page_domain = true;
    let cdp = cdp.reading_apart();
    assert_eq!(read(&cdp, "1").await, "page");
    assert_eq!(
        methods(&page),
        ["Page.getFrameTree", "Runtime.evaluate(page)"]
    );
    // Asked once, not on every read.
    assert_eq!(read(&cdp, "1").await, "page");
    assert_eq!(methods(&page), ["Runtime.evaluate(page)"]);
}

/// The fallback to the page's world exists for scripts that are not an
/// expression. A script that ran and *threw* a `SyntaxError` must come back
/// as that error: running it a second time, in the page's world, would repeat
/// its side effects where the page can watch.
#[tokio::test]
async fn a_script_that_ran_and_threw_is_never_run_again_in_the_pages_world() {
    let (cdp, page) = fake_page().await;
    let cdp = cdp.reading_apart();
    assert_eq!(read(&cdp, "1").await, "isolated");
    methods(&page);

    for script in ["JSON.parse('')", "(new Function('}'))()"] {
        let res = cdp
            .send(
                "Runtime.evaluate",
                Some(json!({"expression": script, "returnByValue": true})),
            )
            .await
            .unwrap();
        assert_eq!(
            res["exceptionDetails"]["exception"]["className"], "SyntaxError",
            "{script}: the error is the answer"
        );
        let seen = methods(&page);
        assert!(
            !seen.contains(&"Runtime.evaluate(page)".to_string()),
            "{script}: {seen:?}"
        );
    }
}
