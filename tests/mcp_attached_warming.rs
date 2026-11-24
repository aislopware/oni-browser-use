//! `obu mcp --port` drives a browser it attached to: someone's own (a host
//! app's browser panel, the user's Brave). First-run profile warming seeds a
//! profile obu launched by visiting a few top sites; run against an attached
//! browser it opened google.com, github.com and wikipedia.org in that
//! browser's agent tab before the agent's first call ran (seen live, Brave
//! 157, a fresh `OBU_HOME`). The CLI already skipped it for `--port`; the MCP
//! server did not.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

type Seen = Arc<Mutex<Vec<String>>>;

/// A Chrome-style debug endpoint on one port: `/json/version` and `/json`
/// over HTTP, and one WebSocket for the browser and its page that answers
/// every command. Records the URL of every `Page.navigate`.
async fn browser(navigated: Seen) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve(stream, addr, navigated.clone()));
        }
    });
    addr
}

async fn serve(mut stream: TcpStream, addr: SocketAddr, navigated: Seen) {
    let mut head = [0u8; 2048];
    let Ok(n) = stream.peek(&mut head).await else {
        return;
    };
    let request = String::from_utf8_lossy(&head[..n]).to_ascii_lowercase();
    if request.contains("upgrade: websocket") {
        let Ok(mut ws) = accept_async(stream).await else {
            return;
        };
        while let Some(Ok(message)) = ws.next().await {
            let Ok(request) = serde_json::from_str::<Value>(message.to_text().unwrap_or("")) else {
                continue;
            };
            let params = &request["params"];
            let result = match request["method"].as_str().unwrap_or("") {
                "Target.createTarget" => json!({"targetId": "T1"}),
                "Target.getTargetInfo" => {
                    json!({"targetInfo": {"targetId": "T1", "type": "page", "url": "about:blank"}})
                }
                "Page.getFrameTree" => {
                    json!({"frameTree": {"frame": {"id": "F1", "url": "about:blank"}}})
                }
                "Page.navigate" => {
                    navigated
                        .lock()
                        .unwrap()
                        .push(params["url"].as_str().unwrap_or("").to_string());
                    json!({"frameId": "F1", "loaderId": "L1"})
                }
                "Runtime.evaluate" | "Runtime.callFunctionOn" => {
                    let capture = params["expression"]
                        .as_str()
                        .or(params["functionDeclaration"].as_str())
                        .is_some_and(|e| e.contains("cap(d,[],0,0)"));
                    let value = if capture {
                        json!({"url": "about:blank", "readyState": "complete", "elements": []})
                    } else {
                        Value::Null
                    };
                    json!({"result": {"type": "object", "value": value}})
                }
                _ => json!({}),
            };
            let mut reply = json!({"id": request["id"], "result": result});
            if let Some(session) = request.get("sessionId") {
                reply["sessionId"] = session.clone();
            }
            if ws
                .send(Message::Text(reply.to_string().into()))
                .await
                .is_err()
            {
                break;
            }
        }
        return;
    }
    let mut buf = vec![0u8; n];
    let _ = stream.read_exact(&mut buf).await;
    let ws = format!("ws://{addr}/devtools/page/T1");
    let body = if request.starts_with("get /json/version") {
        json!({"Browser": "Chrome/159.0.0.0", "Protocol-Version": "1.3",
               "User-Agent": "Mozilla/5.0 Chrome/159.0.0.0",
               "webSocketDebuggerUrl": format!("ws://{addr}/devtools/browser/B1")})
    } else {
        json!([{"id": "T1", "type": "page", "url": "about:blank", "title": "",
                "webSocketDebuggerUrl": ws}])
    }
    .to_string();
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes()).await;
}

fn fresh_home() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obu-attached-warming-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn an_attached_browser_is_never_warmed() {
    let navigated: Seen = Arc::default();
    let addr = browser(navigated.clone()).await;
    // A fresh data dir: everything that gates warming says "first run".
    let home = fresh_home();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_obu"))
        .args(["mcp", "--port", &addr.port().to_string()])
        .env("OBU_HOME", &home)
        .env("OBU_LANE", "agent")
        .env("OBU_NO_UPDATE_CHECK", "1")
        .env_remove("OBU_NO_WARMING")
        .env_remove("OBU_TRANSPORT")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("mcp must spawn");
    let mut stdin = child.stdin.take().unwrap();
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": {"name": "act", "arguments": {"action": "navigate", "url": "https://agent.test/"}}});
    stdin
        .write_all(format!("{call}\n").as_bytes())
        .await
        .unwrap();
    // The answer (success or not) ends the call; warming would run before it.
    let mut stdout = child.stdout.take().unwrap();
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    tokio::time::timeout(Duration::from_secs(60), async {
        while stdout.read(&mut byte).await.unwrap_or(0) == 1 && byte[0] != b'\n' {
            line.push(byte[0]);
        }
    })
    .await
    .expect("the call must be answered");
    drop(stdin);
    let _ = tokio::time::timeout(Duration::from_secs(20), child.wait()).await;

    let navigated = navigated.lock().unwrap().clone();
    assert!(
        navigated.iter().any(|u| u.contains("agent.test")),
        "the agent's own navigation must reach the browser: {navigated:?} / {}",
        String::from_utf8_lossy(&line)
    );
    assert_eq!(
        navigated
            .iter()
            .filter(|u| !u.contains("agent.test"))
            .collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "an attached browser must not be sent anywhere the agent did not ask for"
    );
    let _ = std::fs::remove_dir_all(&home);
}
