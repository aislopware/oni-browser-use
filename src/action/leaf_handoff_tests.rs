//! A handler the page assigned (`row.onclick = open`) lives in the page's
//! world; the find script runs in the driver's and reads `null`. With nothing
//! else vouching for the row it forwarded the click to a control inside it
//! (the row's Delete button). `find_by_sig` now asks the page's world about
//! that one property before the click moves. A fake page speaking just
//! enough CDP locks what is asked, where, and what follows from the answer.

use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use super::find::find_by_sig;
use crate::cdp::{CdpClient, CdpSession};

const SIG: &str = "|button|Open thread|1";
const ROW: [f64; 4] = [0.0, 0.0, 400.0, 60.0];
const DELETE: [f64; 4] = [330.0, 18.0, 60.0, 24.0];

#[derive(Default)]
struct FakePage {
    /// What the page's world answers for `typeof row.onclick === 'function'`.
    handler: bool,
    /// Every command received: (method, params).
    seen: Vec<(String, Value)>,
}

impl FakePage {
    fn answer(&mut self, method: &str, params: &Value) -> Value {
        self.seen.push((method.to_string(), params.clone()));
        let found = |mode: &str| {
            let own = mode == "box!";
            json!({"result": {"type": "object", "value": {
                "ok": true, "box": if own { ROW } else { DELETE },
                "leaf": !own, "leafOpen": !own, "hit_tgt": "button [Delete]",
            }}})
        };
        match method {
            "Page.getFrameTree" => {
                json!({"frameTree": {"frame": {"id": "MAIN", "loaderId": "L"}}})
            }
            "Page.createIsolatedWorld" => json!({"executionContextId": 7}),
            "Runtime.evaluate" if params.get("contextId").is_some() => {
                json!({"result": {"type": "object", "objectId": "APART"}})
            }
            // The find script in the page's own world (a session not reading
            // apart): there `n.onclick` is visible, the script decides alone.
            "Runtime.evaluate" => found(if self.handler { "box!" } else { "box" }),
            "Runtime.callFunctionOn" if params["objectId"] == "APART" => {
                let body = params["functionDeclaration"].as_str().unwrap_or_default();
                if body.contains(r#","node","#) || body.contains(r#","host","#) {
                    json!({"result": {"type": "object", "objectId": "ROW-APART"}})
                } else if body.contains(r#","check","#) {
                    // A custom element: its text is a mask of what it holds.
                    json!({"result": {"type": "object", "value": {
                        "ok": true, "text": "*****", "hostKind": "x-edit",
                        "hostIsTgt": true, "hostCE": true,
                    }}})
                } else if body.contains(r#","box!","#) {
                    found("box!")
                } else {
                    found("box")
                }
            }
            "DOM.describeNode" if params["objectId"] == "ROW-APART" => {
                json!({"node": {"backendNodeId": 41}})
            }
            "DOM.resolveNode" if params["backendNodeId"] == 41 => {
                json!({"object": {"type": "object", "objectId": "ROW-PAGE"}})
            }
            "Runtime.callFunctionOn" if params["objectId"] == "ROW-PAGE" => {
                let body = params["functionDeclaration"].as_str().unwrap_or_default();
                if body.contains("this.value") {
                    json!({"result": {"type": "string", "value": "hello"}})
                } else {
                    json!({"result": {"type": "boolean", "value": self.handler}})
                }
            }
            _ => json!({}),
        }
    }
}

async fn fake_page(handler: bool) -> (CdpSession, Arc<Mutex<FakePage>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let page = Arc::new(Mutex::new(FakePage {
        handler,
        ..FakePage::default()
    }));
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
            let result = state.lock().unwrap().answer(method, &request["params"]);
            let reply = json!({"id": request["id"], "result": result});
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

/// The commands that ran code in the page's own world.
fn in_the_pages_world(page: &Arc<Mutex<FakePage>>) -> Vec<String> {
    page.lock()
        .unwrap()
        .seen
        .iter()
        .filter(|(m, p)| match m.as_str() {
            "Runtime.evaluate" => p.get("contextId").is_none(),
            "Runtime.callFunctionOn" => p["objectId"] == "ROW-PAGE",
            _ => false,
        })
        .map(|(m, p)| format!("{m} {}", p["functionDeclaration"].as_str().unwrap_or("")))
        .collect()
}

#[tokio::test]
async fn a_row_with_a_page_assigned_handler_keeps_its_click() {
    let (cdp, page) = fake_page(true).await;
    let cdp = cdp.reading_apart();
    let found = find_by_sig(&cdp, SIG, &[], "box", None).await.unwrap();
    assert_eq!(found.box_, Some(ROW), "the row is clicked where it is");
    assert_ne!(found.leaf, Some(true));
    // The whole lookup stays in the driver's world; the page's is asked for
    // one property of the one node, nothing else.
    assert_eq!(
        in_the_pages_world(&page),
        ["Runtime.callFunctionOn function(){return typeof this.onclick==='function'}"]
    );
    // The handles taken for the question are given back.
    let seen = &page.lock().unwrap().seen;
    assert!(seen.iter().any(|(m, _)| m == "Runtime.releaseObjectGroup"));
}

#[tokio::test]
async fn a_row_without_one_still_forwards_and_says_so() {
    let (cdp, page) = fake_page(false).await;
    let cdp = cdp.reading_apart();
    let found = find_by_sig(&cdp, SIG, &[], "box", None).await.unwrap();
    assert_eq!(found.box_, Some(DELETE));
    assert_eq!(found.leaf, Some(true), "the verdict names the redirect");
    assert_eq!(in_the_pages_world(&page).len(), 1);
}

#[tokio::test]
async fn nothing_is_asked_when_the_script_already_ran_in_the_pages_world() {
    // BLADE_MAIN_WORLD=1, or a target with no frames: the script saw
    // `n.onclick` itself, and its answer stands.
    let (cdp, page) = fake_page(false).await;
    let found = find_by_sig(&cdp, SIG, &[], "box", None).await.unwrap();
    assert_eq!(found.box_, Some(DELETE));
    let seen: Vec<String> = page
        .lock()
        .unwrap()
        .seen
        .iter()
        .map(|s| s.0.clone())
        .collect();
    assert_eq!(seen, ["Runtime.evaluate"]);
}

/// The same trade for a readback: a custom element's `value` is a page-world
/// accessor. Read apart, the script could only offer the element's text, and
/// a correct `type` was reported as a mismatch and typed a second time.
#[tokio::test]
async fn a_custom_elements_value_is_read_where_the_page_defined_it() {
    let (cdp, page) = fake_page(false).await;
    let cdp = cdp.reading_apart();
    let found = find_by_sig(&cdp, SIG, &[], "check", None).await.unwrap();
    assert_eq!(found.text.as_deref(), Some("hello"));
    assert_eq!(found.host_kind.as_deref(), Some("x-edit"));
    assert_eq!(
        in_the_pages_world(&page),
        ["Runtime.callFunctionOn function(){return typeof this.value==='string'?this.value:null}"]
    );
}
