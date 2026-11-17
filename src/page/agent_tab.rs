//! The agent's own tab in a browser obu attached to but did not launch.
//!
//! An attached browser (`rb mode attach`, `--host/--port`) is the user's, and
//! its first page target is the tab they are looking at. Adopting it — what
//! every attach did before — navigated that tab under them. A session there
//! starts on a tab obu created instead: its own window, opened in the
//! background, so the user's tab keeps both its content and its visibility.
//! Its tabs are remembered per endpoint under the data dir, so one-shot
//! commands land on the same tab instead of opening a window each.

use super::*;
use crate::cdp::{list_page_targets, TargetInfo};
use std::path::{Path, PathBuf};

/// The page target a fresh session starts on, and the process-wide record of
/// which kind of browser it is. A browser obu launched: its first tab.
/// One it only attached to: the agent's own tab — never one the user has open.
pub async fn session_target(base: &str, attached: bool) -> Result<TargetInfo> {
    crate::browser::set_attached_browser(attached);
    if !attached {
        return crate::cdp::first_page_target(base).await;
    }
    agent_target(base, &record_path()).await
}

/// `<data-dir>/agent-tab.json`: per endpoint (`host:port`), the tab the
/// session is on and the tabs obu itself opened there:
/// `{"127.0.0.1:9222": {"current": "<targetId>", "own": ["<targetId>", …]}}`.
pub(super) fn record_path() -> PathBuf {
    crate::platform::obu_dir().join("agent-tab.json")
}

/// One endpoint's entry. `own` is newest-first and only ever holds tabs
/// obu created: it is what a session may fall back to — a tab reached
/// by an explicit `switch-tab` can be the user's, and is `current` only.
#[derive(Default, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Entry {
    #[serde(default)]
    current: Option<String>,
    #[serde(default)]
    own: Vec<String>,
}

/// Tabs remembered per endpoint; closed ones are pruned on the next resume.
const OWN_CAP: usize = 16;

type Record = std::collections::BTreeMap<String, Entry>;

/// A missing or malformed record is empty: the worst case is one new tab.
fn read_record(record: &Path) -> Record {
    std::fs::read_to_string(record)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_record(record: &Path, map: &Record) -> Result<()> {
    if let Some(parent) = record.parent() {
        crate::platform::secure_create_dir_all(parent)
            .map_err(|e| ObuError::Other(format!("cannot create data dir: {e}")))?;
    }
    let body = serde_json::to_string(map)
        .map_err(|e| ObuError::Other(format!("cannot serialize the agent-tab record: {e}")))?;
    crate::platform::secure_write_file(record, body.as_bytes())
        .map_err(|e| ObuError::Other(format!("cannot write {}: {e}", record.display())))
}

/// Record the tab the session is on — what the next session (a later
/// one-shot command, a restarted daemon) resumes on. `created` marks a tab
/// obu opened itself.
pub(super) fn remember(record: &Path, base: &str, target_id: &str, created: bool) -> Result<()> {
    note(record, base, target_id, created, true)
}

/// A tab obu opened that the session has not moved to (and may never:
/// the download helper tab is opened, used and closed). It joins `own`;
/// `current` stays where the session is — recorded as current, a helper that
/// was then closed sent the next command to another tab of the agent's
/// instead of the one it was working in.
pub(super) fn remember_opened(record: &Path, base: &str, target_id: &str) -> Result<()> {
    note(record, base, target_id, true, false)
}

fn note(record: &Path, base: &str, target_id: &str, created: bool, current: bool) -> Result<()> {
    let mut map = read_record(record);
    let entry = map.entry(base.to_string()).or_default();
    if current {
        entry.current = Some(target_id.to_string());
    }
    if created || entry.own.iter().any(|id| id == target_id) {
        entry.own.retain(|id| id != target_id);
        entry.own.insert(0, target_id.to_string());
        entry.own.truncate(OWN_CAP);
    }
    write_record(record, &map)
}

/// The tab to resume on among the `live` page targets: the one the session
/// was on, else the newest tab obu opened that still exists. Never a
/// tab it neither opened nor was switched to.
fn resume_on(entry: &Entry, live: &[TargetInfo]) -> Option<String> {
    let alive = |id: &String| {
        live.iter()
            .any(|t| t.id == *id && t.web_socket_debugger_url.is_some())
    };
    entry
        .current
        .iter()
        .chain(entry.own.iter())
        .find(|id| alive(id))
        .cloned()
}

/// A browser-level connection for `base` (`Target.*` belongs to the browser
/// target, not to a page).
pub(super) async fn browser_client(base: &str) -> Result<CdpClient> {
    let ws = crate::cdp::version(base)
        .await?
        .web_socket_debugger_url
        .ok_or_else(|| ObuError::Other("browser has no webSocketDebuggerUrl".into()))?;
    CdpClient::connect(&ws).await
}

/// `Target.createTarget`. `quiet` (an attached browser): the tab gets its own
/// window, created in the background. Measured on Brave 157: a default
/// create — and `Page.bringToFront` — turns the user's active tab `hidden`;
/// a background tab in their window is itself `hidden` (throttled); a
/// background window is `visible` and leaves theirs `visible`.
pub(super) async fn create_tab(
    client: &CdpClient,
    url: &str,
    context: Option<&str>,
    quiet: bool,
) -> Result<String> {
    let mut params = json!({ "url": url });
    if let Some(id) = context {
        params["browserContextId"] = json!(id);
    }
    if quiet {
        let mut window = params.clone();
        window["newWindow"] = json!(true);
        window["background"] = json!(true);
        match client.send("Target.createTarget", Some(window)).await {
            Ok(res) => return created_id(&res),
            // A browser without window control (headless shell, Android)
            // rejects the pair: a background tab still steals nothing.
            Err(ObuError::Cdp { .. }) => params["background"] = json!(true),
            Err(e) => return Err(e),
        }
    }
    created_id(&client.send("Target.createTarget", Some(params)).await?)
}

fn created_id(res: &Value) -> Result<String> {
    res.get("targetId")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| ObuError::Other("no targetId from Target.createTarget".into()))
}

/// The agent's tab on `base`: the one it was on (or another it opened)
/// while the browser still has it, else a fresh one — first use, or the user
/// closed them.
pub(super) async fn agent_target(base: &str, record: &Path) -> Result<TargetInfo> {
    let find = |targets: Vec<TargetInfo>, id: &str| {
        targets
            .into_iter()
            .find(|t| t.id == id && t.web_socket_debugger_url.is_some())
    };
    let live = list_page_targets(base).await?;
    let mut map = read_record(record);
    if let Some(entry) = map.get_mut(base) {
        // Keep the record honest: forget the tabs that were closed, and
        // point it at the one this session resumes on.
        let resume = resume_on(entry, &live);
        let before = (entry.current.clone(), entry.own.len());
        entry.own.retain(|id| live.iter().any(|t| t.id == *id));
        entry.current = resume.clone();
        if before != (entry.current.clone(), entry.own.len()) {
            // Housekeeping only: the tab to resume on is already known.
            if let Err(e) = write_record(record, &map) {
                eprintln!("[obu] {e} — the agent-tab record was not updated");
            }
        }
        if let Some(t) = resume.and_then(|id| find(live, &id)) {
            return Ok(t);
        }
    }
    let client = browser_client(base).await?;
    let id = create_tab(&client, "about:blank", None, true).await?;
    // Recorded before anything else can fail: an unrecorded tab would be
    // orphaned and the next command would open another window. If it cannot
    // be recorded (data dir read-only, disk full) it is closed again, so a
    // failing setup leaves nothing behind in the user's browser.
    if let Err(e) = remember(record, base, &id, true) {
        let _ = client
            .send("Target.closeTarget", Some(json!({"targetId": id})))
            .await;
        return Err(ObuError::Other(format!(
            "cannot record the agent's own tab in {} ({e}) — without that record every \
             command would open another window in your browser, so the tab was closed \
             again. Make the data dir writable (OBU_HOME) and retry.",
            record.display()
        )));
    }
    eprintln!(
        "[obu] opened the agent's own tab {id} (background window) — your open tabs are \
         left alone"
    );
    // HTTP discovery can lag a just-created target.
    for _ in 0..20 {
        if let Some(t) = find(list_page_targets(base).await?, &id) {
            return Ok(t);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(ObuError::Other(format!(
        "the agent tab {id} was created but never appeared in the browser's target list"
    )))
}

impl Page {
    /// Move the session to the agent's own tab, opening a fresh one when the
    /// remembered one is gone. Returns its target id.
    pub async fn switch_to_agent_tab(&mut self) -> Result<String> {
        let target = agent_target(&self.base, &record_path()).await?;
        self.switch_tab(&target.id).await?;
        Ok(target.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;

    /// What the fake browser holds: its page targets, and every
    /// `Target.createTarget` params object it was sent.
    #[derive(Default)]
    struct Fake {
        pages: Vec<String>,
        creates: Vec<Value>,
        /// Every target id `Target.closeTarget` was sent for.
        closes: Vec<String>,
        /// Reject `newWindow` like a browser without window control.
        no_windows: bool,
    }

    /// A browser-shaped endpoint: `/json` + `/json/version` over HTTP and a
    /// browser-level WebSocket that answers `Target.createTarget`. Starts
    /// with one page target, `USER` — the tab the user is looking at.
    async fn fake_browser(no_windows: bool) -> (String, Arc<Mutex<Fake>>) {
        let state = Arc::new(Mutex::new(Fake {
            pages: vec!["USER".into()],
            no_windows,
            ..Default::default()
        }));
        let ws = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ws_addr = ws.local_addr().unwrap();
        let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = http.local_addr().unwrap().to_string();

        let st = state.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = ws.accept().await {
                let st = st.clone();
                tokio::spawn(async move {
                    let mut sock = tokio_tungstenite::accept_async(stream).await.unwrap();
                    while let Some(Ok(Message::Text(text))) = sock.next().await {
                        let req: Value = serde_json::from_str(text.as_str()).unwrap();
                        let reply = if req["method"] == "Target.createTarget" {
                            let mut b = st.lock().unwrap();
                            b.creates.push(req["params"].clone());
                            if b.no_windows && req["params"].get("newWindow").is_some() {
                                json!({"id": req["id"], "error": {"code": -32000, "message": "not supported"}})
                            } else {
                                let id = format!("AGENT{}", b.creates.len());
                                b.pages.push(id.clone());
                                json!({"id": req["id"], "result": {"targetId": id}})
                            }
                        } else if req["method"] == "Target.closeTarget" {
                            let id = req["params"]["targetId"].as_str().unwrap_or_default();
                            let mut b = st.lock().unwrap();
                            b.pages.retain(|p| p != id);
                            b.closes.push(id.to_string());
                            json!({"id": req["id"], "result": {"success": true}})
                        } else {
                            json!({"id": req["id"], "result": {}})
                        };
                        sock.send(Message::Text(reply.to_string().into()))
                            .await
                            .unwrap();
                    }
                });
            }
        });

        let st = state.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = http.accept().await {
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/");
                let body = if path.starts_with("/json/version") {
                    json!({
                        "Browser": "Fake/1.0",
                        "webSocketDebuggerUrl": format!("ws://{ws_addr}/devtools/browser/x"),
                    })
                } else {
                    let pages = st.lock().unwrap().pages.clone();
                    Value::Array(
                        pages
                            .iter()
                            .map(|id| {
                                json!({
                                    "id": id, "type": "page", "url": "about:blank",
                                    "webSocketDebuggerUrl": format!("ws://{ws_addr}/devtools/page/{id}"),
                                })
                            })
                            .collect(),
                    )
                }
                .to_string();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(format!("{head}{body}").as_bytes()).await;
            }
        });
        (base, state)
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("obu-agent-tab-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("agent-tab.json")
    }

    #[tokio::test]
    async fn an_attached_session_gets_its_own_background_window_never_the_users_tab() {
        // Regression: every attach took `first_page_target` — the tab the
        // user was looking at — and `nav` then navigated it under them.
        let (base, browser) = fake_browser(false).await;
        let record = scratch("own");

        let first = agent_target(&base, &record).await.unwrap();
        assert_eq!(
            first.id, "AGENT1",
            "the session must not adopt the user's tab"
        );
        let creates = browser.lock().unwrap().creates.clone();
        assert_eq!(
            creates,
            vec![json!({"url": "about:blank", "newWindow": true, "background": true})],
            "own window, opened in the background"
        );

        // The next command (a new process, same data dir) resumes on it.
        let again = agent_target(&base, &record).await.unwrap();
        assert_eq!(again.id, "AGENT1");
        assert_eq!(browser.lock().unwrap().creates.len(), 1, "no second window");

        // The user closed the agent's window: a fresh one, still not theirs.
        browser.lock().unwrap().pages.retain(|p| p != "AGENT1");
        let fresh = agent_target(&base, &record).await.unwrap();
        assert_eq!(fresh.id, "AGENT2");
        assert_eq!(
            read_record(&record)[&base],
            Entry {
                current: Some("AGENT2".into()),
                own: vec!["AGENT2".into()],
            },
            "the closed tab is pruned from the record"
        );

        let _ = std::fs::remove_dir_all(record.parent().unwrap());
    }

    #[tokio::test]
    async fn a_browser_without_window_control_gets_a_background_tab() {
        let (base, browser) = fake_browser(true).await;
        let record = scratch("nowin");
        let t = agent_target(&base, &record).await.unwrap();
        assert_eq!(t.id, "AGENT2", "the refused window is retried as a tab");
        let creates = browser.lock().unwrap().creates.clone();
        assert_eq!(
            creates[1],
            json!({"url": "about:blank", "background": true}),
            "the fallback must still not take the foreground"
        );
        let _ = std::fs::remove_dir_all(record.parent().unwrap());
    }

    #[tokio::test]
    async fn an_owned_browser_opens_plain_tabs() {
        // A browser obu launched has no user to disturb: tabs open the
        // way they always did (same window, foreground).
        let (base, browser) = fake_browser(false).await;
        let client = browser_client(&base).await.unwrap();
        let id = create_tab(&client, "https://a.test/", Some("CTX"), false)
            .await
            .unwrap();
        assert_eq!(id, "AGENT1");
        assert_eq!(
            browser.lock().unwrap().creates,
            vec![json!({"url": "https://a.test/", "browserContextId": "CTX"})]
        );
    }

    #[tokio::test]
    async fn a_closed_current_tab_falls_back_to_a_tab_the_agent_opened_not_the_users() {
        // Live repro: `open-tab` then `close-tab` on it opened a third
        // window while the agent's first tab was still there.
        let (base, browser) = fake_browser(false).await;
        let record = scratch("fallback");
        assert_eq!(agent_target(&base, &record).await.unwrap().id, "AGENT1");

        // `open-tab`: a second tab of its own, now current.
        browser.lock().unwrap().pages.push("AGENT-B".into());
        remember(&record, &base, "AGENT-B", true).unwrap();
        // `switch-tab` onto the user's tab: current, but never "own".
        remember(&record, &base, "USER", false).unwrap();
        assert_eq!(agent_target(&base, &record).await.unwrap().id, "USER");

        // The user closes that tab: back to the newest tab the agent opened.
        browser.lock().unwrap().pages.retain(|p| p != "USER");
        browser.lock().unwrap().pages.insert(0, "USER-2".into());
        assert_eq!(agent_target(&base, &record).await.unwrap().id, "AGENT-B");
        browser.lock().unwrap().pages.retain(|p| p != "AGENT-B");
        assert_eq!(agent_target(&base, &record).await.unwrap().id, "AGENT1");
        assert_eq!(
            browser.lock().unwrap().creates.len(),
            1,
            "no new window while a tab of its own is still open"
        );
        let _ = std::fs::remove_dir_all(record.parent().unwrap());
    }

    /// The download helper tab is opened, used and closed without the
    /// session ever moving to it. Recorded as `current`, its closing sent the
    /// next command to another tab of the agent's, not the one it worked in.
    #[tokio::test]
    async fn a_helper_tab_never_becomes_the_tab_to_resume_on() {
        let (base, browser) = fake_browser(false).await;
        let record = scratch("helper");
        assert_eq!(agent_target(&base, &record).await.unwrap().id, "AGENT1");
        // `open-tab` + switch: the agent now works in AGENT-B.
        browser.lock().unwrap().pages.push("AGENT-B".into());
        remember(&record, &base, "AGENT-B", true).unwrap();

        // A helper tab comes…
        browser.lock().unwrap().pages.push("HELPER".into());
        remember_opened(&record, &base, "HELPER").unwrap();
        assert_eq!(
            read_record(&record)[&base].current.as_deref(),
            Some("AGENT-B"),
            "the session did not move"
        );
        assert_eq!(agent_target(&base, &record).await.unwrap().id, "AGENT-B");
        // …and goes: still the tab the agent was working in.
        browser.lock().unwrap().pages.retain(|p| p != "HELPER");
        assert_eq!(agent_target(&base, &record).await.unwrap().id, "AGENT-B");
        // It is one of the agent's own all the same (never the user's).
        remember_opened(&record, &base, "HELPER-2").unwrap();
        assert!(read_record(&record)[&base]
            .own
            .contains(&"HELPER-2".to_string()));
        let _ = std::fs::remove_dir_all(record.parent().unwrap());
    }

    /// A data dir that cannot be written: every command would open one more
    /// window in the user's browser and forget it. The window is closed and
    /// the command says why.
    #[tokio::test]
    async fn a_tab_that_cannot_be_recorded_is_not_left_in_the_users_browser() {
        let (base, browser) = fake_browser(false).await;
        let record = scratch("unwritable");
        // The record's directory is a file: nothing can be written under it.
        let dir = record.parent().unwrap();
        std::fs::write(dir, b"not a directory").unwrap();

        let err = agent_target(&base, &record).await.unwrap_err().to_string();
        assert!(err.contains("cannot record the agent's own tab"), "{err}");
        assert!(err.contains("OBU_HOME"), "names the way out: {err}");
        let b = browser.lock().unwrap();
        assert_eq!(b.closes, ["AGENT1"], "the window it opened is closed again");
        assert_eq!(b.pages, ["USER"], "the user's tab is all that is left");
        drop(b);
        let _ = std::fs::remove_file(dir);
    }

    #[test]
    fn the_record_keeps_each_endpoint_apart_and_survives_corruption() {
        let record = scratch("record");
        assert!(read_record(&record).is_empty());
        remember(&record, "127.0.0.1:9222", "A", true).unwrap();
        remember(&record, "127.0.0.1:9333", "B", true).unwrap();
        remember(&record, "127.0.0.1:9222", "C", true).unwrap();
        remember(&record, "127.0.0.1:9222", "A", false).unwrap();
        let map = read_record(&record);
        assert_eq!(
            map["127.0.0.1:9222"],
            Entry {
                current: Some("A".into()),
                own: vec!["A".into(), "C".into()],
            },
            "re-selecting an own tab moves it to the front"
        );
        assert_eq!(map["127.0.0.1:9333"].current.as_deref(), Some("B"));
        // A corrupted record reads as empty, never an error.
        std::fs::write(&record, b"{ not json").unwrap();
        assert!(read_record(&record).is_empty());
        let _ = std::fs::remove_dir_all(record.parent().unwrap());
    }
}
