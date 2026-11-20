//! Session-scoped CDP access (S1: pipe transport multiplexing).
//!
//! One connection can carry many targets in flat mode: the browser-level
//! session plus one sub-session per attached target. [`CdpSession`] wraps a
//! shared [`CdpClient`] with an optional session id so the rest of the driver
//! (Page, actions, state, perception) speaks to exactly one session without
//! knowing which transport or mode is underneath.
//!
//! - WS mode (direct page WebSocket): session id is `None`; every call hits
//!   the page's own connection, events arrive unfiltered.
//! - Pipe mode (`--remote-debugging-pipe`): the root session drives browser
//!   domains (Target.*), child sessions drive page domains; events are
//!   filtered to the child's session id.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::cdp::client::CdpClient;
use crate::cdp::protocol::CdpEvent;
use crate::cdp::reader::{self, Reader};
use crate::error::{ObuError, Result};

/// A handle to one CDP session on a shared connection. Cheap to clone.
#[derive(Clone)]
pub struct CdpSession {
    client: CdpClient,
    session_id: Option<String>,
    cookie_context: Option<(CdpClient, String)>,
    /// When set, plain `Runtime.evaluate` calls on this handle run in the
    /// reader world (see [`reader`](crate::cdp::reader)). Shared by clones.
    reader: Option<Arc<Reader>>,
}

impl CdpSession {
    /// The root session of a connection (browser-level on pipe, or the whole
    /// connection on a direct page WebSocket).
    pub fn root(client: CdpClient) -> Self {
        Self {
            client,
            session_id: None,
            cookie_context: None,
            reader: None,
        }
    }

    /// A child session (flat-mode `Target.attachToTarget` result).
    pub fn child(client: CdpClient, session_id: impl Into<String>) -> Self {
        Self {
            client,
            session_id: Some(session_id.into()),
            cookie_context: None,
            reader: None,
        }
    }

    /// The underlying connection (for browser-level commands from a child).
    pub fn client(&self) -> &CdpClient {
        &self.client
    }

    /// Has the transport been closed?
    pub fn is_closed(&self) -> bool {
        self.client.is_closed()
    }

    /// This session's id, if it is a child session.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub(crate) fn set_cookie_context(&mut self, browser: CdpClient, context: String) {
        self.cookie_context = Some((browser, context));
    }

    pub(crate) fn browser_context_id(&self) -> Option<&str> {
        self.cookie_context.as_ref().map(|(_, id)| id.as_str())
    }

    /// Storage's default is the regular browser profile, even on an OTR page.
    /// Only a browser-level command accepts browserContextId.
    pub(crate) async fn cookie_store(&self) -> Result<Value> {
        if let Some((browser, context)) = &self.cookie_context {
            browser
                .send(
                    "Storage.getCookies",
                    Some(serde_json::json!({"browserContextId":context})),
                )
                .await
        } else {
            self.send("Storage.getCookies", None).await
        }
    }

    /// This session, with its own page reads moved out of the page's world:
    /// every plain `Runtime.evaluate` sent through the returned handle (and
    /// its clones) runs in an isolated world of the main frame, where the
    /// page's scripts cannot observe it. For page sessions.
    pub fn reading_apart(mut self) -> Self {
        if reader::enabled() {
            self.reader = Some(Arc::new(Reader::default()));
        }
        self
    }

    /// Whether this handle's reads run outside the page's world — where the
    /// page's own JavaScript (globals, handlers it assigned) is not visible.
    pub fn reads_apart(&self) -> bool {
        self.reader.as_ref().is_some_and(|r| r.apart())
    }

    /// A handle on the same session whose `Runtime.evaluate` runs in the
    /// page's own world — for what must see the page's JavaScript: the
    /// agent's `eval`, a site adapter calling the page's API, a stealth
    /// self-check reading what the page reads.
    pub fn main_world(&self) -> Self {
        let mut page = self.clone();
        page.reader = None;
        page
    }

    /// A driver read: on a page session it runs in the reader world (see
    /// [`reader`](crate::cdp::reader)), elsewhere as written. Upstream's
    /// isolated-world call sites land here, so both ask for one world.
    pub(crate) async fn eval_isolated_with_timeout(
        &self,
        expression: &str,
        return_by_value: bool,
        await_promise: bool,
        deadline: Duration,
    ) -> Result<Value> {
        self.send_with_timeout(
            "Runtime.evaluate",
            Some(json!({
                "expression": expression,
                "returnByValue": return_by_value,
                "awaitPromise": await_promise,
            })),
            deadline,
        )
        .await
    }

    /// A driver-side watcher whose result does not matter (the mutation
    /// counter): only a closed connection is an error.
    pub(crate) async fn eval_isolated_strict(
        &self,
        expression: &str,
        deadline: Duration,
    ) -> Result<()> {
        match self
            .send_with_timeout(
                "Runtime.evaluate",
                Some(json!({ "expression": expression })),
                deadline,
            )
            .await
        {
            Err(ObuError::Closed) => Err(ObuError::Closed),
            _ => Ok(()),
        }
    }

    /// Send a command with the default timeout.
    pub async fn send(&self, method: &str, params: Option<Value>) -> Result<Value> {
        self.send_with_timeout(method, params, crate::cdp::client::DEFAULT_TIMEOUT)
            .await
    }

    /// Send a command with an explicit timeout.
    pub async fn send_with_timeout(
        &self,
        method: &str,
        params: Option<Value>,
        deadline: Duration,
    ) -> Result<Value> {
        if method == "Runtime.evaluate" {
            if let Some(reader) = &self.reader {
                return reader::evaluate(self, reader, params, deadline).await;
            }
        }
        self.raw(method, params, deadline).await
    }

    /// The command as written, on this session.
    pub(super) async fn raw(
        &self,
        method: &str,
        params: Option<Value>,
        deadline: Duration,
    ) -> Result<Value> {
        self.client
            .send_session_with_timeout(self.session_id.as_deref(), method, params, deadline)
            .await
    }

    /// Enable a CDP domain on this session.
    pub async fn enable(&self, domain: &str) -> Result<Value> {
        self.send(&format!("{domain}.enable"), None).await
    }

    /// Disable a CDP domain on this session.
    pub async fn disable(&self, domain: &str) -> Result<Value> {
        self.send(&format!("{domain}.disable"), None).await
    }

    /// Subscribe to this session's event stream. On a child session, events
    /// belonging to other sessions are filtered out transparently.
    pub fn subscribe(&self) -> SessionSubscription {
        SessionSubscription {
            rx: self.client.subscribe(),
            session_id: self.session_id.clone(),
            closed: false,
        }
    }

    /// Wait for the next event on this session whose method equals `method`.
    pub fn wait_for(
        &self,
        method: &str,
        deadline: Duration,
    ) -> impl std::future::Future<Output = Result<CdpEvent>> {
        let mut sub = self.subscribe();
        let method = method.to_string();
        async move {
            tokio::time::timeout(deadline, async {
                loop {
                    match sub.recv().await {
                        Ok(ev) if ev.method == method => return Ok(ev),
                        Ok(_) => continue,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(crate::error::ObuError::Closed);
                        }
                    }
                }
            })
            .await
            .map_err(|_| crate::error::ObuError::Timeout(deadline))?
        }
    }
}

/// A filtered view of the connection's event bus for one session. Mirrors
/// `broadcast::Receiver`'s recv semantics so existing loops compile unchanged.
pub struct SessionSubscription {
    rx: broadcast::Receiver<CdpEvent>,
    session_id: Option<String>,
    closed: bool,
}

impl SessionSubscription {
    /// Receive the next event belonging to this session. Root sessions
    /// (no id) accept every event; child sessions end with `Closed` once
    /// Chrome detaches their target (`Target.detachedFromTarget`).
    pub async fn recv(&mut self) -> std::result::Result<CdpEvent, broadcast::error::RecvError> {
        if self.closed {
            return Err(broadcast::error::RecvError::Closed);
        }
        loop {
            let ev = self.rx.recv().await?;
            match (&self.session_id, &ev.session_id) {
                (None, _) => return Ok(ev),
                (Some(mine), Some(theirs)) if mine == theirs => return Ok(ev),
                (Some(mine), None)
                    if ev.method == "Target.detachedFromTarget"
                        && ev.params.get("sessionId").and_then(Value::as_str)
                            == Some(mine.as_str()) =>
                {
                    self.closed = true;
                    return Err(broadcast::error::RecvError::Closed);
                }
                _ => continue,
            }
        }
    }
}

impl SessionSubscription {
    /// The next event of this session already on the bus, without waiting.
    pub fn try_recv(&mut self) -> std::result::Result<CdpEvent, broadcast::error::TryRecvError> {
        loop {
            let ev = self.rx.try_recv()?;
            match (&self.session_id, &ev.session_id) {
                (None, _) => return Ok(ev),
                (Some(mine), Some(theirs)) if mine == theirs => return Ok(ev),
                _ => continue,
            }
        }
    }
}

impl std::fmt::Debug for CdpSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdpSession")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn child_subscription_closes_on_matching_target_detach() {
        let (tx, rx) = broadcast::channel(16);
        let mut sub = SessionSubscription {
            rx,
            session_id: Some("sess-a".into()),
            closed: false,
        };
        tx.send(CdpEvent {
            method: "Target.detachedFromTarget".into(),
            params: json!({"sessionId": "sess-a", "targetId": "t-a"}),
            session_id: None,
        })
        .unwrap();
        tx.send(CdpEvent {
            method: "Page.loadEventFired".into(),
            params: json!({}),
            session_id: Some("sess-a".into()),
        })
        .unwrap();

        assert!(matches!(
            sub.recv().await,
            Err(broadcast::error::RecvError::Closed)
        ));
        assert!(matches!(
            sub.recv().await,
            Err(broadcast::error::RecvError::Closed)
        ));
    }

    #[tokio::test]
    async fn child_subscription_ignores_unrelated_detach_and_root_receives_all() {
        let (tx, rx_child) = broadcast::channel(16);
        let rx_root = tx.subscribe();
        let mut child = SessionSubscription {
            rx: rx_child,
            session_id: Some("sess-a".into()),
            closed: false,
        };
        let mut root = SessionSubscription {
            rx: rx_root,
            session_id: None,
            closed: false,
        };

        tx.send(CdpEvent {
            method: "Target.detachedFromTarget".into(),
            params: json!({"sessionId": "sess-other", "targetId": "t-other"}),
            session_id: None,
        })
        .unwrap();
        tx.send(CdpEvent {
            method: "Page.loadEventFired".into(),
            params: json!({"ts": 1}),
            session_id: Some("sess-a".into()),
        })
        .unwrap();

        let root_ev = root.recv().await.unwrap();
        assert_eq!(root_ev.method, "Target.detachedFromTarget");

        let child_ev = child.recv().await.unwrap();
        assert_eq!(child_ev.method, "Page.loadEventFired");
    }
}
