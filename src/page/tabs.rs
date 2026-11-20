//! Tab lifecycle: listing, opening, switching, aliveness — plus the
//! isolated-world eval used by internal probes.

use super::*;
use crate::cdp::list_page_targets;

impl Page {
    /// Input is delivered to this target, including after a foreground popup.
    /// Never in an attached browser: `Page.bringToFront` hides whatever tab
    /// shares the window and raises that window over the user's work — and
    /// the agent's tabs there each sit in their own window, already active.
    pub async fn activate(&self) -> Result<()> {
        if crate::browser::attached_browser() {
            return Ok(());
        }
        self.cdp
            .send_with_timeout("Page.bringToFront", None, Duration::from_secs(5))
            .await?;
        Ok(())
    }

    /// Is this target still on screen as its window's active tab? A
    /// background tab reports `document.visibilityState == "hidden"`; one
    /// that is merely behind another application's window still reads
    /// "visible". An answer we cannot read counts as inactive — the caller
    /// then activates, which is the pre-#89 behavior.
    pub async fn is_active_tab(&self) -> bool {
        active_from_visibility(
            self.cdp
                .send_with_timeout(
                    "Runtime.evaluate",
                    Some(serde_json::json!({
                        "expression": "document.visibilityState",
                        "returnByValue": true,
                    })),
                    Duration::from_secs(2),
                )
                .await
                .ok()
                .as_ref(),
        )
    }

    /// Activate only when input could miss this target. `Page.bringToFront`
    /// activates the browser window at the OS level on macOS — every click
    /// and navigation used to raise it above whatever the person was working
    /// in (issue #89). A page with focus emulation takes input in a
    /// background tab (measured, Chrome 158), and reads "visible" whatever
    /// its tab is doing, so it is left alone; a popup an action opens still
    /// hands the foreground back to its opener (`Page::act`). Without
    /// emulation a background tab loses input (#49) and is activated.
    /// `OBU_NO_ACTIVATE=1` never raises the window, accepting that trade.
    /// An attached browser is never raised at all (see [`Page::activate`]).
    pub async fn activate_if_needed(&self) -> Result<()> {
        if self.shown
            || crate::browser::attached_browser()
            || std::env::var("OBU_NO_ACTIVATE")
                .map(|v| v == "1")
                .unwrap_or(false)
        {
            return Ok(());
        }
        if self.is_active_tab().await {
            return Ok(());
        }
        self.activate().await
    }

    /// Attached browser: refuse input while the session's window is minimized
    /// AND the page could not be given focus emulation (see `Page::attach`).
    /// With emulation a minimized window renders and takes input like any
    /// other (measured, Brave 157: clicks, typing, select, scroll and vision
    /// all land, window still minimized). Without it a minimized page is not
    /// rendered: text targets resolved to the wrong element and focus never
    /// reached the field — and this lane never raises a window to fix it. An
    /// honest refusal with the way out beats a click on the wrong thing.
    pub async fn refuse_minimized_window(&self) -> Result<()> {
        if self.shown || !crate::browser::attached_browser() {
            return Ok(());
        }
        let window = self
            .cdp
            .send_with_timeout("Browser.getWindowForTarget", None, Duration::from_secs(2))
            .await
            .ok();
        if !minimized_from_window(window.as_ref()) {
            return Ok(());
        }
        Err(ObuError::Other(
            "the agent's browser window is minimized — a minimized page is not rendered, so \
             clicks and keys cannot be delivered reliably, and obu never raises a window \
             in a browser it attached to. Continue in a fresh background window (`state \
             open-tab <url>`), or have the window restored (it may stay behind other windows)."
                .into(),
        ))
    }

    /// Evaluate a JS expression in an isolated world. The isolated world has
    /// DOM access but its own JavaScript context — anti-bot scripts in the
    /// main world cannot observe our queries via patched DOM methods or
    /// Error.stack frames. Lazily creates the world; recreates on navigation.
    /// Falls back to regular Runtime.evaluate if the isolated world fails.
    pub async fn eval_isolated(&self, expr: &str) -> Result<Value> {
        self.cdp
            .eval_isolated_with_timeout(expr, true, true, Duration::from_secs(30))
            .await
    }

    /// List the browser's page targets — over the pipe's browser-level
    /// connection in pipe mode, over the HTTP debug endpoint in WS mode.
    pub(super) async fn list_page_targets(&self) -> Vec<crate::cdp::TargetInfo> {
        if let Some(bc) = &self.browser_client {
            // Short timeout: Target.getTargets is browser-level and should
            // return in <10ms. If Chrome is busy (page navigating, pipe
            // congested), don't block the click flow for 30s — skip tab
            // detection instead.
            match bc
                .send_with_timeout("Target.getTargets", None, Duration::from_secs(3))
                .await
            {
                Ok(res) => res
                    .get("targetInfos")
                    .and_then(|t| t.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
                            .filter_map(|t| {
                                Some(crate::cdp::TargetInfo {
                                    id: t.get("targetId")?.as_str()?.to_string(),
                                    kind: "page".to_string(),
                                    title: t.get("title")?.as_str()?.to_string(),
                                    url: t.get("url")?.as_str()?.to_string(),
                                    attached: t
                                        .get("attached")
                                        .and_then(|a| a.as_bool())
                                        .unwrap_or(false),
                                    web_socket_debugger_url: None,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                Err(_) => Vec::new(),
            }
        } else {
            list_page_targets(&self.base).await.unwrap_or_default()
        }
    }

    /// List all page targets (tabs). Public wrapper for the MCP
    /// server's close-tab auto-switch.
    pub async fn tab_targets(&self) -> Vec<crate::cdp::TargetInfo> {
        self.list_page_targets().await
    }

    /// Create a new tab (browser-level Target.createTarget) and return its
    /// target id. Uses the browser-level pipe client in pipe mode; in WS
    /// mode connects to the browser's own debugger WebSocket (discovered
    /// via /json/version) — Target.createTarget belongs to the browser
    /// target, and the page session may be dead exactly when we need this
    /// (dead-tab recovery).
    pub async fn open_tab_target(&self, url: &str) -> Result<String> {
        // Bare hosts must work here exactly like `nav`: Target.createTarget
        // with a scheme-less URL ("localhost:3000/x") never loads — Chrome
        // reads "localhost:" as the scheme — leaving a stuck about:blank tab
        // whose screenshot then burns its full CDP timeout (reproduced live).
        let url = with_scheme(url);
        let client = match &self.browser_client {
            Some(bc) => bc.clone(),
            // WS mode: browser-level connection on demand.
            None => super::agent_tab::browser_client(&self.base).await?,
        };
        // An attached browser is the user's: the tab opens in a background
        // window of its own instead of taking over the one they are in.
        let attached = crate::browser::attached_browser();
        let id =
            super::agent_tab::create_tab(&client, &url, self.cdp.browser_context_id(), attached)
                .await?;
        if attached {
            // A tab of the agent's own: one it may fall back to later. Not
            // the current one until the session actually switches to it.
            if let Err(e) =
                super::agent_tab::remember_opened(&super::agent_tab::record_path(), &self.base, &id)
            {
                eprintln!("[obu] {e} — the new tab is not remembered as the agent's own");
            }
        }
        Ok(id)
    }

    /// Attached browser: record the tab the session is on (see `agent_tab`).
    /// Best-effort — a failed write costs continuity, never the command.
    fn remember_tab(&self, target_id: &str, created: bool) {
        if let Err(e) = super::agent_tab::remember(
            &super::agent_tab::record_path(),
            &self.base,
            target_id,
            created,
        ) {
            eprintln!("[obu] {e} — the next command may resume on another tab");
        }
    }

    /// Attach an auxiliary tab without detaching or replacing the active page.
    pub(crate) async fn attach_tab_target(&self, target_id: &str) -> Result<Page> {
        let session = if let Some(client) = &self.browser_client {
            // Pipe mode: flat-session attach via the browser-level client.
            let res = client
                .send(
                    "Target.attachToTarget",
                    Some(serde_json::json!({ "targetId": target_id, "flatten": true })),
                )
                .await?;
            let sid = res
                .get("sessionId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ObuError::Other("Target.attachToTarget returned no sessionId".into())
                })?;
            CdpSession::child(client.clone(), sid)
        } else {
            // WS mode: connect to the target's own WebSocket URL.
            // Retry briefly: a just-created target can lag in HTTP
            // discovery ("tab not found" on a fresh open-tab was a race).
            let mut t = None;
            for _ in 0..5 {
                let targets = list_page_targets(&self.base).await?;
                t = targets.into_iter().find(|t| t.id == target_id);
                if t.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let t = t.ok_or_else(|| ObuError::Other(format!("tab not found: {target_id}")))?;
            let client = CdpClient::connect(t.ws_url()?).await?;
            CdpSession::root(client)
        };
        Page::attach(session, &self.base, self.browser_client.clone()).await
    }

    /// Switch the session to a different tab (target id from
    /// `state tabs`). The whole page state is rebuilt against the
    /// new tab: domains re-enabled, stealth re-injected, fresh
    /// capture. The old tab stays OPEN — this is focus switching,
    /// not closing. `*self = new_page` drops the old Page, whose
    /// Drop aborts its background tasks (dialogs/network/hum).
    pub async fn switch_tab(&mut self, target_id: &str) -> Result<()> {
        // Attach to the NEW target FIRST. The old code detached
        // the current session first — if the attach then failed,
        // the session was detached from everything (bricked).
        let new_page = self.attach_tab_target(target_id).await?;
        new_page.activate().await?;
        // New session attached — now detach the OLD one (pipe
        // mode) so only one session stays attached. Failure
        // here is harmless (two sessions attached briefly).
        if let Some(client) = &self.browser_client {
            if let Some(sid) = self.cdp.session_id() {
                let _ = client
                    .send(
                        "Target.detachFromTarget",
                        Some(serde_json::json!({ "sessionId": sid })),
                    )
                    .await;
            }
        }
        *self = new_page;
        // Attached browser: the tab the session is on outlives this process —
        // record it so the next one-shot command resumes here, not on the
        // tab this one left.
        if crate::browser::attached_browser() {
            self.remember_tab(target_id, false);
        }
        Ok(())
    }

    /// Is the current tab still alive? A cheap probe used after
    /// close-tab to detect that the agent closed the tab the
    /// session was attached to.
    pub async fn current_tab_alive(&self) -> bool {
        self.cdp
            .send_with_timeout(
                "Runtime.evaluate",
                Some(serde_json::json!({ "expression": "1", "returnByValue": true })),
                Duration::from_secs(3),
            )
            .await
            .is_ok()
    }
}

/// The activation decision from a `Runtime.evaluate` reply to
/// `document.visibilityState`: only an explicit "visible" means the target is
/// already its window's active tab, so `Page.bringToFront` is skipped. A
/// hidden tab, an error or an unreadable reply activates — the pre-#89
/// behavior, because input to a background tab was the #49 bug.
fn active_from_visibility(reply: Option<&Value>) -> bool {
    reply
        .and_then(|r| r.get("result"))
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_str())
        == Some("visible")
}

/// Is the window in a `Browser.getWindowForTarget` reply minimized? An
/// unreadable reply is "not minimized": the check exists to refuse a known
/// bad state, never to block input on a browser that cannot answer it.
fn minimized_from_window(reply: Option<&Value>) -> bool {
    reply
        .and_then(|r| r.get("bounds"))
        .and_then(|b| b.get("windowState"))
        .and_then(|s| s.as_str())
        == Some("minimized")
}

#[cfg(test)]
mod activation_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_visible_page_skips_activation() {
        for (reply, active) in [
            (
                Some(json!({"result": {"type": "string", "value": "visible"}})),
                true,
            ),
            (
                Some(json!({"result": {"type": "string", "value": "hidden"}})),
                false,
            ),
            (Some(json!({"result": {}})), false),
            (Some(json!({"error": {"message": "target gone"}})), false),
            (None, false),
        ] {
            assert_eq!(active_from_visibility(reply.as_ref()), active, "{reply:?}");
        }
    }

    #[test]
    fn only_a_minimized_window_refuses_attached_input() {
        for (reply, minimized) in [
            (
                Some(json!({"windowId": 7, "bounds": {"windowState": "minimized"}})),
                true,
            ),
            (
                Some(json!({"windowId": 7, "bounds": {"windowState": "normal", "left": 0}})),
                false,
            ),
            (
                Some(json!({"windowId": 7, "bounds": {"windowState": "fullscreen"}})),
                false,
            ),
            // Browsers without window control answer with an error or
            // nothing: input must not be blocked on that.
            (Some(json!({"error": {"message": "not supported"}})), false),
            (None, false),
        ] {
            assert_eq!(
                minimized_from_window(reply.as_ref()),
                minimized,
                "{reply:?}"
            );
        }
    }
}
