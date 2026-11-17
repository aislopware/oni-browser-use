//! Mouse input on a hidden page in an ATTACHED browser (the user's own).
//!
//! Chrome acks a `mouseMoved` with the next frame, and a hidden page (its
//! window minimized or fully covered) produces none — the move is queued and
//! reaches the page ahead of the next press. Every other lane raises a hidden
//! tab before input; an attached browser is never raised, so there the click
//! used to die on the first move: "CDP command timed out after 5s"
//! (reproduced live, Brave 157, agent window minimized).
//!
//! Its own test binary: the attached flag is process-wide (and the tests here
//! take turns on it).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bladebro::action::{perform, Action};
use bladebro::cdp::{CdpClient, CdpSession};
use bladebro::page::LivePageModel;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

/// A page that is hidden: every command is answered except `mouseMoved`,
/// whose ack never comes. Returns the mouse event types it was sent.
async fn hidden_page() -> (CdpSession, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let seen = log.clone();
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
            if request["method"] == "Input.dispatchMouseEvent" {
                let kind = request["params"]["type"].as_str().unwrap_or("").to_string();
                seen.lock().unwrap().push(kind.clone());
                if kind == "mouseMoved" {
                    continue;
                }
            }
            // An empty settled page for the post-click capture.
            let capture = request["params"]["expression"]
                .as_str()
                .is_some_and(|e| e.contains("cap(d,[],0,0)"));
            let value = if capture {
                json!({"url": "http://fixture.test/", "readyState": "complete", "elements": []})
            } else {
                Value::Null
            };
            let reply = json!({
                "id": request["id"],
                "result": {"result": {"type": "object", "value": value}},
            });
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
    (CdpSession::root(client), log)
}

async fn click(cdp: &CdpSession) -> bladebro::Result<(bladebro::page::PageDelta, String)> {
    perform(
        cdp,
        &mut LivePageModel::new(),
        &Action::ClickCoord { x: 40.0, y: 40.0 },
        &Arc::new(Mutex::new(None)),
    )
    .await
}

/// The attached flag is one per process: its tests run one at a time.
static FLAG: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn a_click_on_a_hidden_page_lands_only_where_the_window_is_never_raised() {
    let _turn = FLAG.lock().await;
    // A browser bladebro owns: it raises a hidden tab before input, so an
    // unacked move still means a stuck browser — an error, not a click.
    bladebro::browser::set_attached_browser(false);
    let (cdp, log) = hidden_page().await;
    let err = tokio::time::timeout(Duration::from_secs(8), click(&cdp))
        .await
        .expect("the 5s move ack bounds the wait")
        .expect_err("an owned browser must not click past an unacked move");
    assert!(err.to_string().contains("timed out"), "{err}");
    assert_eq!(*log.lock().unwrap(), ["mouseMoved"], "no press was sent");

    // The user's browser: the moves stay queued and the press goes through.
    bladebro::browser::set_attached_browser(true);
    let (cdp, log) = hidden_page().await;
    let started = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(8), click(&cdp))
        .await
        .expect("a hidden page must not stall the click")
        .expect("the click must land without the move acks");
    let events = log.lock().unwrap().clone();
    let presses: Vec<&str> = events
        .iter()
        .map(String::as_str)
        .filter(|e| *e != "mouseMoved")
        .collect();
    assert_eq!(presses, ["mousePressed", "mouseReleased"], "{events:?}");
    assert!(
        events.iter().filter(|e| *e == "mouseMoved").count() >= 3,
        "the humanized path must still be sent: {events:?}"
    );
    assert_eq!(
        events.last().map(String::as_str),
        Some("mouseReleased"),
        "moves first, then the press: {events:?}"
    );
    // One full ack wait, not one per move of the path.
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "took {:?}",
        started.elapsed()
    );
}

/// A page that answers every command with an empty result, and records the
/// methods it was sent.
async fn blank_page() -> (CdpSession, Arc<Mutex<Vec<(String, Value)>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let seen = log.clone();
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
            let method = request["method"].as_str().unwrap_or("").to_string();
            seen.lock()
                .unwrap()
                .push((method, request["params"].clone()));
            let reply = json!({"id": request["id"], "result": {}});
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
    (CdpSession::root(client), log)
}

/// The agent's window in the user's browser is never raised, so it is covered
/// by what the user is looking at and its page is `hidden`: no frames, timers
/// at 1 Hz, trusted clicks in a hidden document (measured on a Brave started
/// without the anti-backgrounding flags — every earlier check here ran on one
/// that had them, and saw "visible"). The page must be given focus emulation
/// before anything is read from it or sent to it — and only there: a browser
/// bladebro owns is raised instead.
#[tokio::test]
async fn an_attached_page_is_shown_to_itself_before_anything_else_happens() {
    let _turn = FLAG.lock().await;
    let emulation = |log: &Arc<Mutex<Vec<(String, Value)>>>| {
        let log = log.lock().unwrap();
        let at = log
            .iter()
            .position(|(m, _)| m == "Emulation.setFocusEmulationEnabled");
        let first_script = log
            .iter()
            .position(|(m, _)| m.starts_with("Runtime.") || m.starts_with("Input."));
        (at.map(|i| log[i].1.clone()), at, first_script)
    };

    bladebro::browser::set_attached_browser(true);
    let (cdp, log) = blank_page().await;
    let _page = bladebro::page::Page::attach(cdp, "127.0.0.1:1", None).await;
    let (params, at, first_script) = emulation(&log);
    assert_eq!(
        params,
        Some(json!({"enabled": true})),
        "an attached page must get focus emulation"
    );
    assert!(
        at < first_script,
        "before the first script or input: {at:?} vs {first_script:?}"
    );

    bladebro::browser::set_attached_browser(false);
    let (cdp, log) = blank_page().await;
    let _page = bladebro::page::Page::attach(cdp, "127.0.0.1:1", None).await;
    assert_eq!(
        emulation(&log).1,
        None,
        "an owned browser is raised, not emulated"
    );
}
