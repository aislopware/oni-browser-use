//! The real-browser lane's fixed attach port (`rb port`).
//!
//! A browser started with `--remote-debugging-port=<N>` writes no
//! `DevToolsActivePort` (Brave never writes one), so `rb mode attach` could
//! not find it and every command needed `--host/--port` — which bypasses the
//! daemon. These tests lock the config surface and prove the attach path
//! consults the configured port, with no browser/profile discovery, through
//! real CLI subprocesses on an isolated `BLADE_HOME`.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

struct Home(PathBuf);

impl Home {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "blade-rb-port-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("blade")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        Self(root)
    }

    /// Run the CLI on this home. HOME is empty, so nothing here can resolve
    /// a real browser profile: an attach that still went through discovery
    /// would fail on the selection, not on the port.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bladebro"))
            .args(args)
            .env("BLADE_HOME", self.0.join("blade"))
            .env("HOME", self.0.join("home"))
            .env("XDG_CONFIG_HOME", self.0.join("home/.config"))
            // Windows resolves profiles from these, not from HOME.
            .env("USERPROFILE", self.0.join("home"))
            .env("LOCALAPPDATA", self.0.join("home/AppData/Local"))
            .env("APPDATA", self.0.join("home/AppData/Roaming"))
            .env("BLADE_NO_UPDATE_CHECK", "1")
            .env("BLADE_NO_WARMING", "1")
            .env_remove("BLADE_LANE")
            .output()
            .unwrap()
    }

    fn rb(&self, args: &[&str]) -> Value {
        let mut full = vec!["--json", "rb"];
        full.extend_from_slice(args);
        let out = self.run(&full);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "{args:?}: invalid JSON ({e}): {}",
                String::from_utf8_lossy(&out.stdout)
            )
        })
    }

    fn write_config(&self, cfg: Value) {
        std::fs::write(self.0.join("blade/realbrowser.json"), cfg.to_string()).unwrap();
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A DevTools-shaped HTTP endpoint with one tab — the user's — and no
/// browser-level WebSocket. Returns its port and the request paths it
/// served, in order.
fn mock_devtools() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let Some(path) = req.split_whitespace().nth(1).map(str::to_string) else {
                // The liveness probe: a bare connect with no request.
                continue;
            };
            let body = if path.starts_with("/json/version") {
                json!({"Browser": "MockBrowser/1.0", "Protocol-Version": "1.3"}).to_string()
            } else {
                json!([{
                    "id": "USER", "type": "page", "url": "https://user.example/",
                    "webSocketDebuggerUrl": format!("ws://127.0.0.1:{port}/devtools/page/USER"),
                }])
                .to_string()
            };
            log.lock().unwrap().push(path);
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    (port, seen)
}

#[test]
fn rb_port_sets_shows_and_clears_the_fixed_attach_port() {
    let home = Home::new("cfg");
    assert_eq!(home.rb(&["port"])["port"], Value::Null, "unset by default");

    let (port, _seen) = mock_devtools();
    let set = home.rb(&["port", &port.to_string()]);
    assert_eq!(set["port"], port);
    assert_eq!(set["live"], true, "something is listening there: {set}");

    // Persisted, and reported as the port an attach would use.
    home.rb(&["mode", "attach"]);
    let status = home.rb(&["status"]);
    assert_eq!(status["port"], port, "{status}");
    assert_eq!(status["attach_port"], port, "{status}");
    assert_eq!(status["effective_mode"], "attach", "{status}");

    let cleared = home.rb(&["port", "auto"]);
    assert_eq!(cleared["port"], Value::Null);
    assert_eq!(home.rb(&["status"])["port"], Value::Null);

    // Not a port → a usage error, and the config is left alone.
    for bad in ["0", "abc", "70000"] {
        let out = home.run(&["--json", "rb", "port", bad]);
        assert!(!out.status.success(), "`rb port {bad}` must fail");
        assert!(text(&out).contains("rb port"), "{}", text(&out));
    }
    assert_eq!(home.rb(&["port"])["port"], Value::Null);
}

#[test]
fn attach_uses_the_fixed_port_without_browser_discovery() {
    let home = Home::new("attach");
    let (port, seen) = mock_devtools();
    home.write_config(json!({"enabled": true, "mode": "attach", "port": port}));

    let out = home.run(&["nav", "https://example.com", "--no-daemon"]);
    let text = text(&out);
    assert!(
        !out.status.success(),
        "the mock cannot open a tab for the agent: {text}"
    );
    let seen = seen.lock().unwrap().clone();
    assert!(
        seen.iter().any(|p| p.starts_with("/json/version")),
        "the fixed port must be probed, saw {seen:?}: {text}"
    );
    // Linux proves a browser of this user holds the port and refuses the
    // mock; elsewhere that proof is unavailable and the attach proceeds.
    #[cfg(target_os = "linux")]
    assert!(
        text.contains(&format!("127.0.0.1:{port}"))
            && text.contains("not served by any live browser"),
        "{text}"
    );
    #[cfg(not(target_os = "linux"))]
    {
        assert!(
            text.contains(&format!(
                "attaching to MockBrowser/1.0 on fixed port {port}"
            )),
            "{text}"
        );
        assert!(
            seen.iter().any(|p| p == "/json"),
            "tabs must be listed on the fixed port, saw {seen:?}"
        );
        // The one tab there is the user's. The command must fail on opening
        // a tab of its own (this mock has no browser socket) — it used to
        // adopt the first tab and navigate it.
        assert!(
            text.contains("browser has no webSocketDebuggerUrl"),
            "the user's tab must never be adopted: {text}"
        );
    }
}

#[test]
fn a_dead_fixed_port_is_an_error_that_names_the_port_and_the_way_out() {
    let home = Home::new("dead");
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    home.write_config(json!({"enabled": true, "mode": "attach", "port": port}));

    let out = home.run(&["nav", "https://example.com", "--no-daemon"]);
    let text = text(&out);
    assert!(!out.status.success(), "{text}");
    assert!(
        text.contains(&format!(
            "no browser debug endpoint answers on 127.0.0.1:{port}"
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!("--remote-debugging-port={port}")) && text.contains("rb port auto"),
        "the error must name the recovery: {text}"
    );

    // `auto` with a dead fixed port never attaches to it.
    home.write_config(json!({"enabled": true, "mode": "auto", "port": port}));
    let status = home.rb(&["status"]);
    assert_ne!(status["effective_mode"], "attach", "{status}");
    assert_eq!(status["port"], port);
}
