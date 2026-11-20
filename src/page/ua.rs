//! The headless User-Agent override.
//!
//! A headless browser names itself `HeadlessChrome` in its UA, so the page
//! session gets `Chrome` instead. An override without `userAgentMetadata`
//! empties `navigator.userAgentData` and drops the `Sec-CH-UA` headers, so
//! the override carries the browser's own client hints (brands with their
//! GREASE entry, full versions, platform, architecture, …). They are read
//! once per browser from `getHighEntropyValues` on a hidden `chrome://version`
//! page in a throwaway context: a page on `about:blank` is not a secure
//! context and has no `navigator.userAgentData`, and CDP has no getter.
//! Headless Chrome reports the same hints as headful (measured: Chrome for
//! Testing 158 and Brave 157 on macOS).

use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::cdp::{CdpClient, CdpSession};

const HINTS_DEADLINE: Duration = Duration::from_secs(5);

const HINTS_JS: &str = "navigator.userAgentData.getHighEntropyValues([\
    'architecture','bitness','brands','formFactors','fullVersionList','mobile',\
    'model','platform','platformVersion','uaFullVersion','wow64'])";

/// The hints of each browser asked, keyed by its endpoint and UA. A failed
/// read is kept as `None`: every page of a browser gets the same override,
/// and none waits on a read that fails again.
static HINTS: Mutex<Vec<(String, Option<Value>)>> = Mutex::new(Vec::new());

/// Rewrite a headless browser's `HeadlessChrome` UA on this page session.
pub(super) async fn override_headless_ua(
    cdp: &CdpSession,
    base: &str,
    browser_client: Option<&CdpClient>,
    real_ua: &str,
    platform: &str,
) {
    let mut params = json!({
        "userAgent": real_ua.replace("HeadlessChrome", "Chrome"),
        "platform": platform,
    });
    if let Some(metadata) = client_hints(base, browser_client, real_ua).await {
        params["userAgentMetadata"] = metadata;
    }
    if let Err(e) = cdp.send("Network.setUserAgentOverride", Some(params)).await {
        crate::browser::set_ua_override_failed();
        eprintln!("[obu] WARNING: UA override failed: {e}");
    }
}

async fn client_hints(base: &str, browser_client: Option<&CdpClient>, ua: &str) -> Option<Value> {
    let key = format!("{base} {ua}");
    if let Some((_, hints)) = HINTS.lock().ok()?.iter().find(|(k, _)| *k == key) {
        return hints.clone();
    }
    let hints = read_client_hints(base, browser_client).await;
    if hints.is_none() {
        eprintln!(
            "[obu] WARNING: could not read the browser's UA client hints; \
             navigator.userAgentData stays empty under the headless UA override"
        );
    }
    if let Ok(mut cache) = HINTS.lock() {
        cache.push((key, hints.clone()));
    }
    hints
}

async fn read_client_hints(base: &str, browser_client: Option<&CdpClient>) -> Option<Value> {
    let browser = match browser_client {
        Some(client) => client.clone(),
        None => {
            let version = crate::cdp::version(base).await.ok()?;
            CdpClient::connect(version.web_socket_debugger_url.as_deref()?)
                .await
                .ok()?
        }
    };
    let context = browser
        .send(
            "Target.createBrowserContext",
            Some(json!({"disposeOnDetach": true})),
        )
        .await
        .ok()?["browserContextId"]
        .as_str()?
        .to_string();
    let hints = tokio::time::timeout(HINTS_DEADLINE, read_hints(&browser, &context))
        .await
        .ok()
        .flatten();
    let _ = browser
        .send(
            "Target.disposeBrowserContext",
            Some(json!({"browserContextId": context})),
        )
        .await;
    ua_metadata(&hints?)
}

async fn read_hints(browser: &CdpClient, context: &str) -> Option<Value> {
    let target = browser
        .send(
            "Target.createTarget",
            Some(json!({
                "url": "about:blank",
                "browserContextId": context,
                "background": true,
                "hidden": true,
            })),
        )
        .await
        .ok()?;
    let attached = browser
        .send(
            "Target.attachToTarget",
            Some(json!({"targetId": target["targetId"].as_str()?, "flatten": true})),
        )
        .await
        .ok()?;
    let page = CdpSession::child(browser.clone(), attached["sessionId"].as_str()?);
    page.enable("Page").await.ok()?;
    let loaded = page.wait_for("Page.loadEventFired", HINTS_DEADLINE);
    page.send("Page.navigate", Some(json!({"url": "chrome://version/"})))
        .await
        .ok()?;
    loaded.await.ok()?;
    let hints = page
        .send(
            "Runtime.evaluate",
            Some(json!({"expression": HINTS_JS, "awaitPromise": true, "returnByValue": true})),
        )
        .await
        .ok()?;
    Some(hints["result"]["value"].clone())
}

/// `getHighEntropyValues` output as CDP's `UserAgentMetadata`.
fn ua_metadata(hints: &Value) -> Option<Value> {
    let mut metadata = Map::new();
    for (from, to) in [
        ("brands", "brands"),
        ("fullVersionList", "fullVersionList"),
        ("uaFullVersion", "fullVersion"),
        ("platform", "platform"),
        ("platformVersion", "platformVersion"),
        ("architecture", "architecture"),
        ("model", "model"),
        ("mobile", "mobile"),
        ("bitness", "bitness"),
        ("wow64", "wow64"),
        ("formFactors", "formFactors"),
    ] {
        if let Some(value) = hints.get(from) {
            metadata.insert(to.into(), value.clone());
        }
    }
    let required = [
        "brands",
        "platform",
        "platformVersion",
        "architecture",
        "model",
        "mobile",
    ];
    required
        .iter()
        .all(|key| metadata.contains_key(*key))
        .then_some(Value::Object(metadata))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use std::sync::{Arc, Mutex};
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::tungstenite::{Message, protocol::Role};

    type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

    const HEADLESS_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/158.0.0.0 Safari/537.36";

    /// What Chrome for Testing 158 on an M4 Mac reports from
    /// `getHighEntropyValues`, headless or not.
    fn real_hints() -> Value {
        json!({
            "architecture": "arm",
            "bitness": "64",
            "brands": [{"brand": "Not.A/Brand", "version": "24"}, {"brand": "Chromium", "version": "158"}],
            "formFactors": ["Desktop"],
            "fullVersionList": [
                {"brand": "Not.A/Brand", "version": "24.0.0.0"},
                {"brand": "Chromium", "version": "158.0.8127.36"}
            ],
            "mobile": false,
            "model": "",
            "platform": "macOS",
            "platformVersion": "27.2.1",
            "uaFullVersion": "158.0.8127.36",
            "wow64": false
        })
    }

    /// A browser on the far end of an in-memory WebSocket that answers what
    /// the client-hints read asks and logs (sessionId, method, params).
    /// `fail` names a method answered with a protocol error.
    async fn fake_browser(fail: Option<&'static str>) -> (CdpClient, Seen) {
        let (near, far) = tokio::io::duplex(1 << 20);
        let client_ws = WebSocketStream::from_raw_socket(near, Role::Client, None).await;
        let mut browser = WebSocketStream::from_raw_socket(far, Role::Server, None).await;
        let client = CdpClient::from_socket(client_ws).unwrap();
        let seen: Seen = Arc::default();
        let log = seen.clone();
        tokio::spawn(async move {
            while let Some(Ok(Message::Text(text))) = browser.next().await {
                let req: Value = serde_json::from_str(text.as_str()).unwrap();
                let sid = req["sessionId"].as_str().unwrap_or("").to_string();
                let method = req["method"].as_str().unwrap_or("").to_string();
                log.lock()
                    .unwrap()
                    .push((sid.clone(), method.clone(), req["params"].clone()));
                let result = match method.as_str() {
                    _ if Some(method.as_str()) == fail => None,
                    "Target.createBrowserContext" => Some(json!({"browserContextId": "ctx"})),
                    "Target.createTarget" => Some(json!({"targetId": "hints"})),
                    "Target.attachToTarget" => Some(json!({"sessionId": "hints-session"})),
                    "Runtime.evaluate" => {
                        Some(json!({"result": {"type": "object", "value": real_hints()}}))
                    }
                    _ => Some(json!({})),
                };
                let reply = match result {
                    Some(result) => json!({"id": req["id"], "sessionId": sid, "result": result}),
                    None => json!({"id": req["id"], "sessionId": sid,
                        "error": {"code": -32000, "message": "refused"}}),
                };
                browser
                    .send(Message::Text(reply.to_string().into()))
                    .await
                    .unwrap();
                if method == "Page.navigate" {
                    let loaded = json!({"method": "Page.loadEventFired", "sessionId": sid,
                        "params": {"timestamp": 1.0}});
                    browser
                        .send(Message::Text(loaded.to_string().into()))
                        .await
                        .unwrap();
                }
            }
        });
        (client, seen)
    }

    fn sent(seen: &Seen, method: &str) -> Vec<(String, Value)> {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|(_, m, _)| m == method)
            .map(|(sid, _, params)| (sid.clone(), params.clone()))
            .collect()
    }

    #[tokio::test]
    async fn headless_override_carries_the_browsers_own_client_hints() {
        let (browser, seen) = fake_browser(None).await;
        let page = CdpSession::child(browser.clone(), "page");
        for _ in 0..2 {
            override_headless_ua(&page, "hints-test", Some(&browser), HEADLESS_UA, "MacIntel")
                .await;
        }

        let overrides = sent(&seen, "Network.setUserAgentOverride");
        assert_eq!(overrides.len(), 2);
        for (sid, params) in &overrides {
            assert_eq!(sid, "page");
            assert_eq!(
                params["userAgent"],
                HEADLESS_UA.replace("HeadlessChrome", "Chrome")
            );
            assert_eq!(params["platform"], "MacIntel");
            let hints = real_hints();
            assert_eq!(
                params["userAgentMetadata"],
                json!({
                    "brands": hints["brands"],
                    "fullVersionList": hints["fullVersionList"],
                    "fullVersion": "158.0.8127.36",
                    "platform": "macOS",
                    "platformVersion": "27.2.1",
                    "architecture": "arm",
                    "model": "",
                    "mobile": false,
                    "bitness": "64",
                    "wow64": false,
                    "formFactors": ["Desktop"],
                })
            );
        }
        // Read once, from a hidden page in a throwaway context that is gone
        // afterwards; the second page reuses it.
        let contexts = sent(&seen, "Target.createBrowserContext");
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].1["disposeOnDetach"], true);
        let targets = sent(&seen, "Target.createTarget");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].1["browserContextId"], "ctx");
        assert_eq!(targets[0].1["hidden"], true);
        let navigations = sent(&seen, "Page.navigate");
        assert_eq!(navigations[0].0, "hints-session");
        assert_eq!(navigations[0].1["url"], "chrome://version/");
        let disposed = sent(&seen, "Target.disposeBrowserContext");
        assert_eq!(disposed.len(), 1);
        assert_eq!(disposed[0].1["browserContextId"], "ctx");
    }

    #[tokio::test]
    async fn unreadable_hints_still_override_the_ua_and_drop_the_context() {
        let (browser, seen) = fake_browser(Some("Page.navigate")).await;
        let page = CdpSession::child(browser.clone(), "page");
        for _ in 0..2 {
            override_headless_ua(
                &page,
                "hints-fail-test",
                Some(&browser),
                HEADLESS_UA,
                "MacIntel",
            )
            .await;
        }

        let overrides = sent(&seen, "Network.setUserAgentOverride");
        assert_eq!(overrides.len(), 2);
        for (_, params) in &overrides {
            assert_eq!(
                *params,
                json!({
                    "userAgent": HEADLESS_UA.replace("HeadlessChrome", "Chrome"),
                    "platform": "MacIntel",
                })
            );
        }
        // The failed read is not repeated: every page of the browser gets the
        // same override, and none waits on a read that fails again.
        assert_eq!(sent(&seen, "Target.createBrowserContext").len(), 1);
        assert_eq!(sent(&seen, "Target.disposeBrowserContext").len(), 1);
    }
}
