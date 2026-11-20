//! The CLI's self-teaching surfaces — `help`, `help --json`, `help <cmd>`,
//! `<cmd> --help`, and the typo suggester behind unknown-command errors.
//!
//! `help --json` is generated from the same `tools_to_json()` the MCP server
//! serves, so the two surfaces cannot drift.

use serde_json::{json, Value};

use crate::error::{ObuError, Result};

// ── Help ───────────────────────────────────────────────────────────────

/// The CLI's agent manual — served by `help --json`. This is the single
/// discovery surface: tool schemas (identical to MCP `tools/list`), the
/// command map, the output contract, and the workflow that gets the most
/// out of both.
const INSTRUCTIONS: &str = "\
Obu CLI — drive a real, stealthy browser from shell commands.
SETUP: none. The first command auto-starts a background daemon (one Chrome for all later commands). --no-daemon forces isolated one-shot runs; 'obu stop' cleans up.
DISCOVERY: 'obu help' (human) — 'obu help --json' (this document) — 'obu help <command>' for one command in depth.
OUTPUT CONTRACT: --json on any command gives ONE JSON object on stdout: {\"ok\":bool,\"is_error\":bool,\"text\":string} (+ \"image_path\" for vision — the PNG is saved to a file, never inlined base64). Exit codes: 0 = success; 1 = the command ran but failed (read 'text' — it carries current page state so you can recover without an extra call); 2 = usage error (bad command/flag/argument — fix the command).
WORKFLOW: 1) Read-only URL? 'see content <url>' or 'see extract auto <url> --format json --block images,fonts' navigates and reads in ONE call. 2) Interactive flow? 'nav <url>' gives refs + a content preview; act by ref/text/label/selector ('act click \"Sign in\"' needs no see; refs self-heal within a page). 3) On list/search/product/profile pages, 'see extract auto' FIRST: one call returns structured items (site adapters cover Reddit/X/GitHub/HN/products; on Reddit POST pages it returns the full comment tree). 4) Collapse round-trips: 'act fill' for whole forms (supports nth/role per field); 'act batch' for linear sequences; 'run' for if/while branching or --format json step receipts. 5) Big JSON payloads: @file or - (stdin) — no shell-quoting games. 6) Keep budgets default; large outputs offload to artifact files read back via 'see --artifact <path>'.
RELIABILITY: real Chromium with the full stealth stack and human-like input; per-domain knowledge (settle timing, block config, bot risk) compounds across runs. Errors are loud, never silent: unknown flags and missing values fail with exit 2; navigation errors report exact HTTP status codes (e.g. HTTP 429 + Retry-After); failed actions return current page state for recovery. Vision is the last resort — refs and deltas are cheaper.
";

/// The human manual — printed by `obu help` / `-h` / bare `obu`.
pub fn help_text() -> String {
    crate::ui::style_help(&HELP_TEXT.replace("__VERSION__", env!("CARGO_PKG_VERSION")))
}

const HELP_TEXT: &str = r#"obu __VERSION__ — agentic browser driver (CLI)

USAGE
  obu <command> [args] [--json] [--no-daemon]
  obu help [command]      this manual (start here)

The first command auto-starts a background daemon: ONE Chrome instance for
all later commands (no startup delay after the first launch). `obu stop`
shuts it down. `--no-daemon` forces isolated one-shot runs.

COMMANDS
  nav <url> [--block <classes>]   navigate (refs + content preview; usually enough to act)
  see [mode] [url] [flags]        read without acting: model|content|outline,
                                  extract <auto|links|forms|json>, --find, --scope, --logs
  act <action> [args]             interact: click, type, fill, select, clear, press, scroll,
                                  hover, navigate, upload, download, wait, dialog, eval, collect,
                                  read, batch, pdf, back, forward, reload, save, load,
                                  open-tab, close-tab, switch-tab
  state <op> [args]               cookies, set-cookie, del-cookie, ls, ss, set-ls, set-ss,
                                  rm-ls, rm-ss, clear-ls, clear-ss, tabs, open-tab,
                                  close-tab, switch-tab, save, load, compress, block
  run '<json-steps>'              batch with if/while branching + inline see reads
  vision [url] [--full] [--size WxH] [--out f.png]  screenshot (saved to a file; path printed)
  daemon | stop                   manage the persistent Chrome session
  mcp                             MCP server on stdio — add to your agent's client config
  rb on|off [sub]                 real-browser lane: drive YOUR browser (see 'help rb')
  hermes on|off|status             switch Hermes' browser to Obu; reversible MCP setup
  audit                           stealth audit — vector suite + boot self-check + drift stamp
  update | -u [--check] [--force]  self-update; --rollback restores the previous binary
  doctor | -doc                   system diagnostics (13 checks)
  -v | --version                  version + install method + update status
  help [command]                  this manual; 'help --json' for the machine version

QUICK START
  obu nav example.com                      navigate
  obu see model                            interactive elements with refs
  obu see content                          page as clean markdown
  obu see extract auto                     structured data (lists, posts, repos, products)
  obu act click e5                         click by ref
  obu act click "Sign in"                  click by text (no see needed)
  obu act type e12 "hello world"           type into one field
  obu act fill '{"e12":"John"}' --submit e20      fill a form in ONE call
  obu act batch '[{"action":"navigate","url":"x.com"},{"action":"see","mode":"content"}]'
  obu run @steps.json                      big payload from a file (- = stdin)
  obu act eval "document.title"            evaluate JS

FLAGS
  --json         one JSON object on stdout: {"ok", "is_error", "text"} (+ "image_path" for vision)
  --no-daemon    one-shot mode: launch Chrome per command
  --host/--port  drive an already-running Chrome on this debug port (skips the daemon)
  --marks        vision: overlay numbered ref badges

EXIT CODES
  0  success
  1  the command ran but failed — read the output text (it carries page state for recovery)
  2  usage error — bad command/flag/argument; the message says how to fix it

ENVIRONMENT
  OBU_HOME           isolate everything (daemon, Chrome, sessions, knowledge) in a directory
  OBU_SESSION        name this agent's session inside the data dir: its own tabs in an
                       attached browser and its own daemon; logins and knowledge stay shared
  OBU_ENDPOINT_PATH  path prefix of the debug endpoint named by --host/--port, for a host
                       application that serves /json under a secret path (e.g. /k/<token>)
  OBU_HOST_NOTE      one line from the application that runs obu about the browser it
                       gave it; told to the agent in place of obu's own lane note
  OBU_CMD_TIMEOUT    client wait for a daemon response, seconds (default 300)
  OBU_IDLE_TIMEOUT   daemon idle before Chrome shuts down, seconds (default 600)
  OBU_NO_COMPRESS=1  disable response compression
  OBU_MAIN_WORLD=1   run the driver's page reads in the page's JS world (default: isolated)
  OBU_LANE           real|agent — force the real-browser lane for this process
  OBU_RB_DEBUG=1     real lane: surface the browser's own stderr on launch failures
  OBU_PLAIN          plain output — no ANSI even on a terminal
                       (NO_COLOR also disables; CLICOLOR_FORCE=1 forces color off-TTY)
  CHROME_PATH          override the Chrome/Chromium binary

Agents: `obu help --json` returns the machine version of this manual —
tool schemas (same as MCP tools/list), per-command usage, exit codes, payload
conventions, and examples. Fetch it once, then drive the CLI with --json.
"#;

/// Closest known command for a mistyped/informal one, so the
/// unknown-command error gives a pointer instead of a dead end:
/// synonyms first (`browser` → `rb use`), then edit distance (≤2, or ≤3 for
/// inputs of six or more chars).
pub fn suggest_command(cmd: &str) -> Option<&'static str> {
    /// Words users reach for that map onto a real command.
    const ALIASES: &[(&str, &str)] = &[
        ("browser", "rb use"),
        ("browsers", "rb use"),
        ("lane", "rb"),
        ("real", "rb"),
    ];
    let lower = cmd.to_ascii_lowercase();
    if let Some((_, target)) = ALIASES.iter().find(|(a, _)| *a == lower) {
        return Some(target);
    }
    const COMMANDS: &[&str] = &[
        "nav",
        "see",
        "act",
        "state",
        "run",
        "vision",
        "daemon",
        "stop",
        "help",
        "rb",
        "realbrowser",
        "hermes",
        "mcp",
        "audit",
        "probe",
        "targets",
        "update",
        "doctor",
        "rollback",
        "version",
    ];
    let mut best: Option<(usize, &'static str)> = None;
    for c in COMMANDS {
        let d = edit_distance(&lower, c);
        let limit = if lower.chars().count() >= 6 { 3 } else { 2 };
        if d <= limit && best.map(|(bd, _)| d < bd).unwrap_or(true) {
            best = Some((d, c));
        }
    }
    best.map(|(_, c)| c)
}

/// Plain Levenshtein over chars — short strings, no dependency.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Canonical command name: resolve aliases to what the user actually types
/// on the CLI, so every help surface accepts the same spellings.
fn normalize_cmd(cmd: &str) -> &str {
    match cmd {
        "navigate" => "nav",
        "-u" => "update",
        "-doc" => "doctor",
        "--rollback" => "rollback",
        "-v" | "--version" => "version",
        _ => cmd,
    }
}

/// Per-command machine help (`help <cmd> --json` and the `commands` map in
/// the full JSON manual).
fn command_help_json(cmd: &str) -> Option<Value> {
    let cmd = normalize_cmd(cmd);
    let v = match cmd {
        "nav" => json!({
            "usage": "obu nav <url> [--block <classes>]",
            "tool": "act",
            "args": { "url": "target URL; bare domains get https://" },
            "flags": { "--block": "images,fonts,media,trackers (remembered per domain)" },
            "examples": ["obu nav example.com", "obu nav https://x.com --block images,fonts --json"],
            "notes": ["returns refs + a content preview — often enough to act without a separate see"]
        }),
        "see" => json!({
            "usage": "obu see [mode] [url] [extract <type>] [flags]",
            "tool": "see",
            "args": {
                "mode": "model (default) | content | outline",
                "url": "optional — navigates first",
                "extract": "auto | links | forms | json (json needs --template)"
            },
            "flags": {
                "--block": "with url: images,fonts,media,trackers to skip during load",
                "--filter": "role filter (model mode)",
                "--find": "search by text → refs (supports --format json)",
                "--scope": "ref id — read one element's subtree",
                "--content": "include page text in model mode",
                "--budget": "max chars (default 8000)",
                "--limit": "max extract items (default 50; Reddit post comments: all up to 1000 unless set)",
                "--logs": "console | network | xhr (all support --format json)",
                "--sort": "Reddit post auto-extract: confidence|top|new|controversial|old|qa",
                "--template": "JSON or @file — for extract=json",
                "--artifact": "read offloaded text chunks (concatenate data then parse); --items pages JSON rows; offset/limit count chars or rows",
                "--preview": "auto-extract container/schema without rows",
                "--changes": "model delta since last capture; --format json for structured fields",
                "--clear": "read then clear the selected log",
                "--format": "text (default) | json — pure JSON output for extract, artifact, changes, find, and logs reads"
            },
            "examples": [
                "obu see model",
                "obu see content example.com",
                "obu see extract auto --limit 20",
                "obu see extract auto --format json",
                "obu see --find \"Submit\"",
                "obu see --logs network"
            ],
            "notes": ["extract=auto is the first move on list/search/product/profile pages — one call returns structured items; on Reddit post pages it attempts the full comment tree; complete/partial disclose API denial and missing replies", "extract=auto envelope: container,count,items + page_total/partial/paged when the page claims more results than the DOM holds, sponsored (paid placements), duplicates_removed, redirects_unwrapped/redirects_unresolved — each explained by a note line"]
        }),
        "act" => json!({
            "usage": "obu act <action> [target] [value] [--flags]",
            "tool": "act",
            "actions": crate::action::ACT_ACTIONS,
            "universal_flags": {
                "--ref": "element ref (self-heals; page-scoped — refresh after navigation; clear/read also take labels)",
                "--label": "field label",
                "--selector": "CSS selector (searches open shadow roots)",
                "--text": "value — text to type, file path (upload), wait match value",
                "--role": "role filter for text/label resolution",
                "--nth": "1-based pick among matches",
                "--key": "press key or chord (Control+a)",
                "--url": "navigate first (any action), or the URL for navigate/download/collect",
                "--option": "select option",
                "--condition": "wait condition",
                "--timeout": "seconds",
                "--dx/--dy": "scroll distance",
                "--js": "eval expression (@file or - for scripts)",
                "--submit": "fill: submit button ref or text",
                "--next": "paginate: CSS selector or text=Next (also label=/ref=)",
                "--extract": "paginate: read kind per page — auto (default) | links | forms | json (needs --template)",
                "--limit": "paginate: per-page extraction item limit",
                "--body": "download: HTTP body, <=32MiB/30s, no browser cookies; honors HTTP(S) OBU_PROXY",
                "--sort": "Reddit post extract: confidence|top|new|controversial|old|qa",
                "--slim": "verdict-only successful actions; batch/run inherit, reads/errors retained",
                "--format": "read/extract/eval: text (default) | json — read returns live values/state/select choices; {data,note} for notices; {artifact,bytes,next_offset:0} when oversized"
            },
            "examples": [
                "obu act click e5",
                "obu act click \"Sign in\"",
                "obu act type e12 \"hello\"",
                "obu act fill '{\"e3\":\"John\"}' --submit e8",
                "obu act click Submit --url example.com/login",
                "obu act upload e5 /tmp/file.pdf",
                "obu act batch @steps.json",
                "obu act extract --format json",
                "obu act wait settle",
                "obu act dialog confirm --accept false",
                "obu act eval \"document.title\""
            ],
            "notes": [
                "unquoted multi-word values join: act click Sign in == act click \"Sign in\"",
                "big JSON payloads: @file or - (stdin)",
                "eval result contract: `result: <compact JSON>` on ONE line — json.loads that line; a returned JSON string is flagged in a note; --format json returns the value itself",
                "batch steps may include {\"action\":\"see\"|\"extract\",...} to read inline; url= on any step navigates first"
            ]
        }),
        "state" => json!({
            "usage": "obu state <op> [args] [flags]",
            "tool": "state",
            "ops": ["cookies","set-cookie","del-cookie","ls","ss","set-ls","set-ss","rm-ls","rm-ss","clear-ls","clear-ss","tabs","open-tab","close-tab","switch-tab","save","load","compress","block"],
            "flags": {
                "--url": "cookie scope / cookies filter / open-tab url",
                "--domain": "cookie domain",
                "--path": "cookie path",
                "--secure": "secure cookie",
                "--http-only": "httpOnly cookie",
                "--same-site": "Strict | Lax | None",
                "--target-id": "close-tab / switch-tab id",
                "--slim": "open-tab / switch-tab / close-tab: tab ID only, omit page model",
                "--clear": "block: stop blocking",
                "--mode": "compress: on | off | status"
            },
            "examples": [
                "obu state cookies",
                "obu state cookies example.com",
                "obu state set-cookie token abc --domain example.com",
                "obu state tabs",
                "obu state save my-session",
                "obu state block images,fonts",
                "obu state compress off"
            ],
            "notes": ["save/load persists cookies+storage; navigate to the saved origin before load, then reload"]
        }),
        "run" => json!({
            "usage": "obu run '<json-steps>' | @file | -",
            "tool": "run",
            "steps": "array; each {action:...} uses the same fields as act; plus {action:'if'|'while'|'see'}",
            "examples": [
                "obu run '[{\"action\":\"navigate\",\"url\":\"example.com\"},{\"action\":\"see\",\"mode\":\"content\"}]'",
                "obu run @steps.json",
                "cat steps.json | obu run -"
            ],
            "notes": ["stops on first error with the step number + page state", "while+see reads across pages in ONE call", "--format json: status, results per top-level step, stopped_at/error/page/note on failure; nested observations remain output text; step formats are explicit; large results offload"]
        }),
        "vision" => json!({
            "usage": "obu vision [url] [--marks] [--full] [--size WxH[@scale]] [--out file.png]",
            "tool": "vision",
            "flags": { "--marks": "numbered ref badges (Set-of-Marks)" },
            "examples": ["obu vision", "obu vision --marks --json | jq -r .image_path", "obu vision file:///tmp/deck.html --size 1920x1080@2 --out slide.png", "obu vision example.com --full --out page.png"],
            "notes": ["screenshot is saved to a file; path printed (json: image_path)", "last resort — see/act are cheaper and return actionable refs"]
        }),
        "daemon" => json!({
            "usage": "obu daemon",
            "tool": null,
            "examples": [],
            "notes": ["auto-starts on the first command; run manually only to watch startup errors"]
        }),
        "stop" => json!({
            "usage": "obu stop [--close-tabs]",
            "tool": null,
            "examples": [],
            "notes": ["graceful (flushes logins + knowledge); idempotent — exit 0 when nothing is running", "--close-tabs: attached browser only — also closes the tabs obu opened there (never the user's); run it when the work is over", "OBU_SESSION set: stops that session's daemon and closes that session's tabs only"]
        }),
        "help" => json!({
            "usage": "obu help [command] [--json]",
            "tool": null,
            "examples": ["obu help --json", "obu help act"],
            "notes": ["the single self-teaching surface — fetch 'help --json' once"]
        }),
        "rb" | "realbrowser" => json!({
            "usage": "obu rb [on|off|status|mode <m>|port [N|auto]|use [id|--binary <path|auto>]|profile [key|auto]|visible on|off|idle-hum on|off|idle-shutdown on|off|refresh|forget|pause|resume]",
            "tool": null,
            "examples": [
                "obu rb on",
                "obu rb status --json",
                "obu rb mode attach",
                "obu rb port 9222",
                "obu rb use --binary /usr/bin/chromium",
                "obu rb pause"
            ],
            "notes": [
                "real-browser lane: the agent drives YOUR Chromium-family browser (your profile data, your display) with zero page patches; the driver-side stack (perception, LPM, refs, adapters, token efficiency, biometrics) is unchanged",
                "mechanisms: clone (default — imported copy; your browser may stay open), profile (your live profile — close the browser first), attach (a running browser exposing a debug endpoint, incl. Chrome 144+ chrome://inspect#remote-debugging); auto = attach if one is live, else clone",
                "attach caveat (measured, Chrome 151): an ephemeral `--remote-debugging-port=0` arm — and Chrome's approval flow — makes Chrome itself report navigator.webdriver=true; arm a FIXED port for a false reading (the lane never masks Chrome's own value) and name it with `rb port <N>` — a fixed-port browser writes no DevToolsActivePort (Brave never does), so discovery cannot see it",
                "attached browsers (attach mode, --host/--port): the agent works in its own background window, remembered across commands, and its refs stay valid from one one-shot command to the next — your open tabs are never navigated, hidden or raised over; the agent's page is given focus emulation, so it renders and takes input as a tab being looked at while its window stays covered or minimized",
                "every surface picks a switch up at its next action — the CLI daemon restarts immediately; a running MCP session switches its browser on the next call (owned browsers relaunch; an attached browser is detached, not closed)",
                "`rb pause` refuses input, navigation, history, downloads, collecting and tab operations (reads, waits and eval stay available); `rb forget` wipes the imported copy; `rb refresh` re-imports; `rb use --binary` supports custom/nix/flatpak-wrapper binaries"
            ]
        }),
        "hermes" => json!({
            "usage": "obu hermes on|off|status [--profile <name>] [--hermes <path>] [--json]",
            "tool": null,
            "examples": ["obu hermes on", "obu hermes off", "obu hermes status --json", "obu hermes on --profile work"],
            "notes": ["Uses Hermes' public configuration CLI and the ordinary Obu MCP server; no plugin or schema copies. Preserves other disabled toolsets and web search/extraction. off restores the prior MCP entry and browser policy, retaining unrelated settings. Start a new chat and restart gateways/desktop clients after switching. HERMES_HOME or --profile selects the Hermes profile. Chrome/Chromium is required; doctor checks prerequisites."]
        }),
        "mcp" => json!({
            "usage": "obu mcp",
            "tool": null,
            "examples": ["obu mcp"],
            "notes": [
                "MCP server on stdio — the integration surface for AI agents (Claude, Cursor, opencode, pi)",
                "client config: {\"command\":\"obu\",\"args\":[\"mcp\"]}",
                "drives Chrome over WebSocket by default (navigator.webdriver stays false natively — zero page patches); OBU_TRANSPORT=pipe opts into the zero-port pipe transport instead"
            ]
        }),
        "audit" => json!({
            "usage": "obu audit",
            "tool": null,
            "examples": ["obu audit"],
            "notes": ["stealth audit — local vector suite + boot self-check + cross-restart consistency stamp; every check must pass (the run prints its own total); any FAIL exits non-zero"]
        }),
        "update" => json!({
            "usage": "obu update | -u [--check] [--force]",
            "tool": null,
            "flags": {
                "--check / -c": "dry run — check only",
                "--force / -f": "reinstall even at the same version"
            },
            "examples": ["obu -u --check", "obu -u"],
            "notes": [
                "download → verify → back up → swap, from GitHub releases",
                "npm installs are detected and redirected to `cargo install --locked --git https://github.com/aislopware/oni-browser-use obu` (override: --force)"
            ]
        }),
        "doctor" => json!({
            "usage": "obu doctor | -doc",
            "tool": null,
            "examples": ["obu doctor"],
            "notes": ["13 system checks: data root, Chrome, Xvfb, profile, logins, hygiene, locks, network, binary, disk, version"]
        }),
        "rollback" => json!({
            "usage": "obu --rollback",
            "tool": null,
            "examples": ["obu --rollback"],
            "notes": ["restores the previous binary from <data dir>/backups (written by every self-update)"]
        }),
        "version" => json!({
            "usage": "obu -v | --version",
            "tool": null,
            "examples": ["obu -v"],
            "notes": ["version + install method + update status (behind / on latest / ahead — never a false 'up to date')"]
        }),
        _ => return None,
    };
    Some(v)
}

/// The MCP tool a command maps to (for schema lookup in help --json).
fn command_tool(cmd: &str) -> Option<&'static str> {
    match cmd {
        "nav" => Some("act"),
        "see" => Some("see"),
        "act" => Some("act"),
        "state" => Some("state"),
        "run" => Some("run"),
        "vision" => Some("vision"),
        _ => None,
    }
}

/// The machine manual — `obu help --json` (optionally per command).
/// One call teaches an agent everything the CLI can do: the same depth as
/// MCP's tools + instructions, plus CLI-only details (exit codes, stdin/@file
/// payloads, universal flags, examples).
pub fn help_json(cmd: Option<&str>) -> Result<String> {
    let tools = crate::mcp::tools::tools_to_json();

    if let Some(name) = cmd {
        let norm = if name == "navigate" { "nav" } else { name };
        let detail = command_help_json(norm).ok_or_else(|| {
            ObuError::Usage(format!(
                "unknown command '{name}' — run 'obu help' for the command list"
            ))
        })?;
        let mut out = json!({ "command": norm, "detail": detail });
        if let Some(t) = command_tool(norm) {
            if let Some(def) = tools
                .iter()
                .find(|v| v.get("name").and_then(|n| n.as_str()) == Some(t))
            {
                out["tool_definition"] = def.clone();
            }
        }
        return Ok(format!("{out}"));
    }

    let mut commands = serde_json::Map::new();
    for c in [
        "nav", "see", "act", "state", "run", "vision", "daemon", "stop", "help", "rb", "mcp",
        "audit", "update", "doctor", "rollback", "version", "hermes",
    ] {
        if let Some(d) = command_help_json(c) {
            commands.insert(c.to_string(), d);
        }
    }

    let out = json!({
        "obu": { "version": env!("CARGO_PKG_VERSION"), "surface": "cli" },
        "instructions": INSTRUCTIONS,
        "quick_start": [
            "obu nav example.com",
            "obu see extract auto",
            "obu act click \"Sign in\"",
            "obu act fill '{\"e12\":\"John\"}' --submit e20",
            "obu run '[{\"action\":\"navigate\",\"url\":\"x.com\"},{\"action\":\"see\",\"mode\":\"content\"}]'",
            "obu stop"
        ],
        "commands": commands,
        "tools": tools,
        "output": {
            "human": "result text on stdout; diagnostics on stderr",
            "--json": "ONE JSON object on stdout: {\"ok\":bool,\"is_error\":bool,\"text\":string} (+ \"image_path\" for vision)",
            "payloads": {
                "@file": "read the argument from a file (steps, fields, templates, js)",
                "-": "read from stdin",
                "example": "obu run @steps.json  |  cat steps.json | obu run -"
            }
        },
        "exit_codes": {
            "0": "success",
            "1": "command ran but failed — details in text (includes page state)",
            "2": "usage error — bad command/flag/argument; the message says how to fix it"
        },
        "env": {
            "OBU_HOME": "isolate daemon/Chrome/sessions/knowledge in a directory",
            "OBU_SESSION": "name this agent's session inside the data dir (letters, digits, - _ . up to 32): its own tabs in an attached browser and its own daemon; logins, knowledge and fingerprint stay shared",
            "OBU_ENDPOINT_PATH": "path prefix of the debug endpoint named by --host/--port (e.g. /k/<token>), for a host application that serves /json and /json/version under a secret path instead of an open debug port",
            "OBU_HOST_NOTE": "one line (up to 600 bytes) from the application that runs obu about the browser it gave it, e.g. that the agent drives the editor's browser panel; added to the MCP instructions and told on the first result in place of obu's own lane note",
            "OBU_CMD_TIMEOUT": "client wait for a daemon response, seconds (default 300)",
            "OBU_IDLE_TIMEOUT": "daemon idle before Chrome shuts down, seconds (default 600)",
            "OBU_NO_COMPRESS": "set 1 to disable response compression",
            "OBU_MAIN_WORLD": "set 1 to run the driver's page reads in the page's JS world (default: an isolated world the page cannot observe)",
            "OBU_LANE": "real|agent — force the real-browser lane for this process",
            "OBU_RB_DEBUG": "real lane: set 1 to surface the browser's own stderr on launch failures",
            "OBU_PLAIN": "plain CLI output — no ANSI even on a TTY (NO_COLOR also honored; CLICOLOR_FORCE=1 forces color off-TTY)",
            "OBU_TRANSPORT": "mcp transport: WebSocket by default (native webdriver=false); set 'pipe' for the zero-port transport (its automation flag is masked)",
            "CHROME_PATH": "override the Chrome/Chromium binary",
            "RUST_LOG": "log filter (default warn,obu=info)"
        }
    });
    Ok(format!("{out}"))
}

/// Detailed per-command human help (`help <cmd>`, `<cmd> --help`).
pub fn command_help_text(cmd: &str) -> Option<String> {
    let cmd = normalize_cmd(cmd);
    let text = match cmd {
        "nav" => {
            r#"obu nav — navigate

USAGE
  obu nav <url> [--block <classes>] [--json]

Returns the page title/URL, interactive element refs, and a content preview —
usually enough to act without a separate see call. Bare domains work:
`obu nav example.com` → https://example.com. The first command
auto-starts the daemon (one Chrome shared by all later commands).

--block <classes>   block inert resources for this load AND remember the
                    choice for the domain: images,fonts,media,trackers

EXAMPLES
  obu nav example.com
  obu nav https://x.com --block images,fonts
  obu nav example.com --json | jq .text
"#
        }
        "see" => {
            r#"obu see — read the page without acting

USAGE
  obu see [mode] [url] [extract <type>] [flags]

MODES
  model (default)   interactive elements with refs (e1, e2, …)
  content           page text as clean markdown (reading articles/docs)
  outline           heading hierarchy only (cheapest read)

EXTRACTION
  extract auto      structured items in ONE call — lists, search results,
                    products, posts. Site-aware: Reddit POST pages → the
                    full comment tree (every reply incl. collapsed, thread
                    order, complete flag); feeds/GitHub/products get fields.
                    Envelope: {container,count,items,…} + page_total/partial/paged
                    when the page claims more results than the DOM holds,
                    sponsored (paid placements), duplicates_removed,
                    redirects_unwrapped — each with a `note:` line.
  extract links     all links
  extract forms     all forms with fields
  extract json      custom template (--template '{"items":{…}}' or @file)

FLAGS
  --filter <role>     only elements of a role (button, link, textbox, …)
  --find <text>       search by text → refs
  --scope <ref>       read one element's subtree
  --content           include text in model mode
  --budget <N>        max response chars (default 8000)
  --limit <N>         max extract items (default 50; Reddit comments: all ≤1000)
  --logs console|network|xhr
  --artifact <path>   paged read (--offset/--limit count chars; --items counts JSON rows)
  --preview           extract auto: container/schema without rows
  --changes           model delta since last capture; --format json for structured fields
  --clear             read then clear --logs console|network|xhr
  --sort <sort>       Reddit post auto-extract: confidence|top|new|controversial|old|qa
  --format text|json  json: pure JSON out for extract, artifact, changes, find, and logs reads
  --block <classes>   with url: skip images,fonts,media,trackers during load
  --url <url>         navigate first (or pass the URL positionally)

EXAMPLES
  obu see                            interactive elements
  obu see content                    page as markdown
  obu see extract auto --limit 20    structured items
  obu see extract auto --format json  pure JSON output
  obu see example.com content        navigate + read in one call
  obu see --find "Submit"            refs by text
  obu see --logs network             recent requests
"#
        }
        "act" => {
            r#"obu act — interact with the page

USAGE
  obu act <action> [target] [value] [--flags]

ACTIONS
  click <ref|label>        type <target> <text>      fill <fields> [--submit]
  select <target> <option> clear <ref|label>         press <key|chord>
  scroll <dx> <dy>         hover <target>            navigate <url> [--block]
  upload <target> <path>   download <url> [--path]  wait <condition> [value]
  eval <js> [--ref]        collect <url> [--max N]   read <ref|label>
  paginate <next> [--max N] [--extract auto|links|forms|json] [--limit N]
                           bounded read/advance, retains pages
  download <url> --body   save an HTTP body without browser cookies (HTTP(S) proxy honored)
  extract [auto|links|forms|json]   structured data (same as 'see extract …')
  dialog <alert|confirm|prompt|beforeunload|any|clear> [--accept true|false]
         [--prompt-text <t>] [--message <m>]   arm the NEXT dialog's answer (one use)
  batch <steps|@file|->    pdf [--path] [--landscape]
  back / forward / reload  save <name> / load <name>
  open-tab [url] / switch-tab <id> / close-tab <id>

UNIVERSAL FLAGS (mirror the MCP schema — accepted on every action)
  --ref --label --text --selector --role --nth --key --url --option --condition --timeout
  --next --body --sort --dx --dy --js --submit --block --slim --format  (per-action: --path, --max, --extract, --limit, --x/--y…)

Multi-word values don't need quotes: 'act click Sign in' works. Big JSON
payloads: @file or - (stdin). Unknown flags fail loudly (exit 2). Key chords: 'act press Control+a'.

RESULT SHAPES (stable — parse them without guessing)
  read          live text/control value, checked/indeterminate and ARIA state,
                select choices (first 80; options_total/options_truncated disclose bounds).
                --format json returns structured properties; large reads offload.
  eval          one line: `result: <compact JSON of the returned value>` — read
                that line, json.loads it. A returned string that CONTAINS JSON is
                called out in a `note:` line (parse the value once more — do not
                discard it); a DOM node/Window says what to return instead;
                undefined reads `result: undefined`. `--format json` returns the
                bare value, `{"data":…,"note":…}` when a note rides along, or an
                `{"artifact":…,"bytes":…}` envelope when oversized.
  extract auto  envelope {container,count,items,…} plus, when they apply:
                page_total/partial/paged (the page claims more results than the
                DOM holds), sponsored (paid placements — flagged per item, left in
                page order), duplicates_removed, redirects_unwrapped/redirects_unresolved. Each has a
                `note:` line explaining what to do about it.

EXAMPLES
  obu act click e5                          click a ref
  obu act click "Sign in"                   click by text
  obu act click --selector "[role=menuitem]"   click by CSS (searches shadow DOM)
  obu act type e12 "hello world"            type
  obu act fill '{"e3":"John","e5":"Doe"}' --submit e8
  obu act click Submit --url example.com/login   navigate + click in ONE call
  obu act upload e5 /tmp/file.pdf           file upload
  obu act wait settle                       wait for DOM quiet
  obu act wait element --text "Results"     wait for an element
  obu act eval "document.title"             evaluate JS
  obu act batch '[{"action":"reload"}]'     sequential steps in ONE call
  obu act collect https://x.com/list --max 100   infinite-scroll collect
  obu act extract --format json             structured data as JSON
"#
        }
        "state" => {
            r#"obu state — cookies, storage, tabs, sessions, blocking

USAGE
  obu state <op> [args] [flags]

COOKIES
  cookies [url]                        list (filtered to url / current page)
  set-cookie <name> <value> [flags]    flags: --url --domain --path --secure
                                       --http-only --same-site Strict|Lax|None
  del-cookie <name> [--url|--domain]

STORAGE
  ls / ss                              list localStorage / sessionStorage
  set-ls <key> <value> · set-ss <key> <value>
  rm-ls <key> · rm-ss <key> · clear-ls · clear-ss

TABS
  tabs                                 list (* = current)
  open-tab <url>                       open + auto-switch
  switch-tab <id> · close-tab <id>     ids come from 'tabs'

SESSIONS / CONTROL
  save <name> · load <name>            persist/restore cookies+storage
  block [classes] · block clear        image/font/media/tracker blocking
  compress on|off|status               context pruning

EXAMPLES
  obu state cookies
  obu state set-cookie token abc --domain example.com --secure
  obu state tabs
  obu state save my-session
"#
        }
        "run" => {
            r#"obu run — batch actions with branching and loops

USAGE
  obu run '<json-steps>' [--slim] [--format text|json] (or @file / -)

Steps are action objects (same fields as act) plus:
  {"action":"if","condition":…,"then":[…],"else":[…]}   branch
  {"action":"while","condition":…,"steps":[…],"max":N}  loop
  {"action":"see",…}        read inline — while+see reads across pages in ONE call
  state ops (open-tab, save, load, …) work as steps too

Stops on the first error and returns the step number + page state.
Conditions: element, title, url, text, settle, js.
--format json returns status=completed|stopped and results with 1-based top-level
step IDs, status=ok|optional_failed|failed and output text. Stopped runs include
stopped_at/error/page/note. Nested observations stay in output; set each read's
format explicitly. Oversized results use the usual artifact paging envelope.

EXAMPLES
  obu run '[{"action":"navigate","url":"example.com"},{"action":"see","mode":"content"}]'
  obu run @steps.json
  cat steps.json | obu run -
"#
        }
        "vision" => {
            r#"obu vision — screenshot

USAGE
  obu vision [url] [--marks] [--full] [--size WxH[@scale]] [--out file.png]

Always saves a PNG to a file and prints the path (--json: "image_path").
A url is opened first. --out names the file; without it the PNG is an
artifact. --full captures the whole page, not just the viewport. --size sets
the viewport for this capture only, in CSS pixels; @2 doubles the output
pixels. --marks overlays numbered ref badges (Set-of-Marks) so you can click
by ref after looking at the image. To read or act on a page, the structural
model (see/act with refs) is cheaper and gives actionable refs.

EXAMPLES
  obu vision
  obu vision --marks --json | jq -r .image_path
  obu vision file:///tmp/deck.html --size 1920x1080@2 --out slide.png
  obu vision example.com --full --out page.png
"#
        }
        "daemon" => {
            r#"obu daemon — persistent Chrome session

USAGE
  obu daemon

Auto-starts on the first command — run it manually only to watch startup
errors. One Chrome instance serves every later command (Unix socket under the
data dir; socket + pid file are 0600). Idle timeout: OBU_IDLE_TIMEOUT
seconds (default 600), then Chrome shuts down; the next command relaunches it.
"#
        }
        "rb" | "realbrowser" => {
            r#"obu rb — the real-browser lane

USAGE
  obu rb                     status (same as `rb status`)
  obu rb on | off            switch the lane (true/false also accepted)
  obu rb mode <m>            auto | clone | profile | attach
  obu rb port [N|auto]       fixed debug port to attach to (a browser
                                  started with --remote-debugging-port=N;
                                  `auto` = discover via DevToolsActivePort)
  obu rb use [id]            list / choose the browser
  obu rb use --binary <path|auto>
                                  custom browser binary (nix wrapper, flatpak
                                  launcher script, dev build)
  obu rb profile [key]       list / choose the profile (an absolute path
                                  to a profile dir also works; `auto` resets)
  obu rb visible on|off      real window (default) vs --headless=new
  obu rb idle-hum on|off     behavioral idle noise while thinking (default on)
  obu rb idle-shutdown on|off
                                  may the idle timeout close the real browser?
                                  (default off — it is the user's)
  obu rb refresh             re-import the profile into the clone
  obu rb forget              wipe the imported copy
  obu rb pause | resume      hand the browser to yourself / back
                                  (paused: input, navigation, history,
                                  downloads, collecting and tab ops refuse;
                                  reads/waits/eval stay)

The lane: instead of Obu's isolated browser, the agent drives YOUR
Chromium-family browser with YOUR profile data on YOUR display — and the
page-injection layer switches OFF entirely (a real environment needs no
masks; truth has no tells to catch). Everything driver-side still runs:
perception, refs, adapters, token efficiency, biometrics + idle hum.

Mechanisms: clone (default — imports your profile once; your browser may
stay open; the source is never written to), profile (launches on your live
profile — close the browser first; branded Google Chrome 136+ refuses CDP
on the default dir, so use clone/attach there), attach (drives a running
browser that already exposes a debug endpoint — classic
--remote-debugging-port, or Chrome 144+ chrome://inspect#remote-debugging
with per-connection approval). Attach caveat (measured, Chrome 151): an
ephemeral `--remote-debugging-port=0` arm — and the approval flow — makes
Chrome itself report navigator.webdriver=true on every page; arm a FIXED
port for a false reading. The lane never masks Chrome's own value. A
fixed-port browser writes no DevToolsActivePort (Brave never writes one), so
name the port once with `rb port <N>`: attach then uses it directly, and
auto attaches whenever a browser is listening there.

In an attached browser the agent works in a background window of its own
(remembered across commands): the tabs you have open are never navigated,
hidden or raised over, and no window is ever brought to the front. The
agent's page gets focus emulation: it renders and takes input as a tab
that is being looked at, while its window stays covered or minimized.

The switch applies to the CLI daemon and to MCP sessions — the daemon
restarts immediately, and a running MCP relaunches its browser at the next
call; no host restart is ever needed.
"#
        }
        "stop" => {
            r#"obu stop — shut the daemon down

USAGE
  obu stop [--close-tabs]

Graceful: flushes logins + domain knowledge, kills Chrome, removes the
socket. Idempotent — exit 0 even when nothing is running. The daemon runs
one command at a time: if it does not answer within 10s (busy with a long
command, or wedged), stop sends SIGTERM to the pid from the pid file — the
running command is cancelled (outcome unknown) and Chrome still closes
gracefully; SIGKILL follows if it is still alive 10s later.

A browser obu attached to (rb mode attach, --host/--port) is the user's
and is never closed. The agent's own tab there stays open as well, so the
next command resumes on it. --close-tabs is for when the work is over: it
closes every tab obu opened in that browser (never one of the user's)
and reports, per endpoint, what a readback of the tab list shows.

With OBU_SESSION set, stop acts on that session only: its daemon and,
with --close-tabs, its tabs. Other sessions in the data dir keep theirs.
"#
        }
        "help" => {
            r#"obu help — the single self-teaching surface

USAGE
  obu help              this manual
  obu help <command>    one command in depth
  obu help --json       the machine manual: tool schemas (same as MCP
                             tools/list), per-command usage, exit codes,
                             payload conventions, examples — call ONCE and
                             you know the whole CLI
  obu help <cmd> --json per-command machine help
  obu <cmd> --help      same as help <cmd> (never runs the command)
"#
        }
        "hermes" => {
            r#"obu hermes — reversible Hermes browser setup

USAGE
  obu hermes on             connect MCP, then disable Hermes' built-in browser tools
  obu hermes off            restore the previous browser policy and MCP entry
  obu hermes status         inspect the managed setup
  --profile <name>               select a Hermes profile (forwarded as hermes -p)
  --hermes <path>                select the Hermes CLI executable
  --json                        one machine-readable result

Uses the ordinary MCP server and Hermes' public configuration commands.
Web search/extraction and unrelated settings are preserved. Configuration
recovery is stored privately beside Hermes' config.yaml in .obu-browser/.
An interrupted switch can resume with on or restore with off. Changed MCP
entries are never overwritten during recovery. Use the same profile for off.
Start a new chat; restart running gateways/desktop clients after switching.
Chrome/Chromium must be installed; obu doctor checks prerequisites.
Updates use your normal Obu updater; no separate Hermes plugin to update.
"#
        }
        "mcp" => {
            r#"obu mcp — MCP server (stdio JSON-RPC)

USAGE
  obu mcp

The integration surface for AI agents. Client config:

  {"mcpServers": {"obu": {"command": "obu", "args": ["mcp"]}}}

Speaks MCP 2024-11-05 through 2026-07-28 (legacy initialize handshake +
the 2026-07-28 stateless dialect). Drives Chrome over WebSocket by default
(navigator.webdriver stays false natively — zero page patches);
OBU_TRANSPORT=pipe opts into the zero-port pipe transport instead (its
automation flag is masked — lie-engine-style detectors can see the mask).
Chrome launches lazily on the first tool call.
"#
        }
        "audit" => {
            r#"obu audit — stealth audit

USAGE
  obu audit

Runs the local vector suite (tests/vectors.html: the core stealth
surface, the permissions native-parity battery, GL-coherence checks,
worker/iframe propagation) plus a boot self-check and a cross-restart
consistency stamp (canvas/audio/geometry/UA/GL must not drift between
runs). Run it after any stealth-affecting change; every check must pass —
the run prints its own total (it depends on platform and profile), and
any FAIL line exits non-zero. The suite is built into the binary; a
tests/vectors.html in the current directory overrides it.
"#
        }
        "update" => {
            r#"obu -u / update — self-update

USAGE
  obu -u [--check | -c] [--force | -f]

Download → verify (magic + size + executes) → back up → swap, from
GitHub releases. --check is a dry run; --force reinstalls the current
version. npm installs are detected (path contains node_modules) and
redirected to `cargo install --locked --git https://github.com/aislopware/oni-browser-use obu`; --force overrides. After an
update: restart your MCP client. Regret it: obu --rollback.
"#
        }
        "doctor" => {
            r#"obu doctor / -doc — system diagnostics

USAGE
  obu doctor

13 checks: data directory, Chrome (+version), Xvfb, profile dir, login
persistence, profile hygiene, stale locks, GitHub reachability, binary
integrity, disk space, version-vs-latest. Failures print a fix.
"#
        }
        "rollback" => {
            r#"obu --rollback — restore the previous binary

USAGE
  obu --rollback

Restores the most recent backup from <data dir>/backups (written by every
self-update). The backup is verified as a valid binary first — a corrupted
newest backup falls through to the next one.
"#
        }
        "version" => {
            r#"obu -v / --version — version + update status

USAGE
  obu -v

Version + build id, install method (npm / source / binary) and update
state: behind (with the hint), on the latest release, or ahead of it
(dev builds). Never a misleading "up to date" when the check failed.
"#
        }
        _ => return None,
    };
    Some(crate::ui::style_help(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_json_is_complete_and_parses() {
        let raw = help_json(None).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert!(v["instructions"].as_str().unwrap().len() > 200);
        assert_eq!(v["tools"].as_array().unwrap().len(), 5);
        assert!(v["commands"]["act"]["usage"].as_str().is_some());
        assert!(v["commands"]["nav"].is_object());
        assert!(
            v["commands"]["mcp"].is_object(),
            "help --json must cover mcp"
        );
        assert!(v["commands"]["rb"].is_object(), "help --json must cover rb");
        assert!(v["commands"]["doctor"].is_object());
        assert!(v["exit_codes"]["2"].as_str().unwrap().contains("usage"));
        assert!(v["output"]["payloads"]["@file"].as_str().is_some());

        let raw = help_json(Some("act")).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["command"], "act");
        assert_eq!(v["tool_definition"]["name"], "act");
        assert!(help_json(Some("nope")).is_err());
    }

    #[test]
    fn every_command_has_help_text_and_json() {
        for c in [
            "nav", "see", "act", "state", "run", "vision", "daemon", "stop", "help", "rb", "mcp",
            "audit", "update", "doctor", "rollback", "version",
        ] {
            let text = command_help_text(c).unwrap_or_else(|| panic!("{c} human help"));
            assert!(text.contains("USAGE"), "{c} help lacks USAGE");
            let j = command_help_json(c).unwrap_or_else(|| panic!("{c} json help"));
            assert!(j["usage"].as_str().is_some(), "{c} json lacks usage");
        }
        // Aliases resolve to the canonical command (v3.9.10: `help mcp` etc. work).
        assert!(command_help_text("-u").is_some() && command_help_text("-doc").is_some());
        assert!(
            command_help_text("--version").is_some() && command_help_text("--rollback").is_some()
        );
        assert!(command_help_json("-u").is_some() && command_help_json("-v").is_some());
        assert!(command_help_text("bogus").is_none());
        assert!(command_help_json("bogus").is_none());
    }

    #[test]
    fn suggestions_map_typos_and_synonyms() {
        assert_eq!(suggest_command("nva"), Some("nav"));
        assert_eq!(suggest_command("stte"), Some("state"));
        assert_eq!(suggest_command("visio"), Some("vision"));
        assert_eq!(suggest_command("realbrwoser"), Some("realbrowser"));
        assert_eq!(
            suggest_command("browser"),
            Some("rb use"),
            "the word users reach for"
        );
        assert_eq!(suggest_command("browsers"), Some("rb use"));
        assert_eq!(suggest_command("completely-unrelated"), None);
    }

    /// The audit total is platform/profile-dependent and printed by the run
    /// itself (audit.rs); a number pinned in help drifted (help said 61
    /// while the suite had grown to 111). No help surface may pin one.
    #[test]
    fn audit_help_does_not_pin_a_check_count() {
        fn pins_a_count(text: &str) -> bool {
            let b = text.as_bytes();
            (0..b.len()).any(|i| {
                b[i].is_ascii_digit() && (i == 0 || !b[i - 1].is_ascii_digit()) && {
                    let end = (i..b.len())
                        .find(|&j| !b[j].is_ascii_digit())
                        .unwrap_or(b.len());
                    let tail = &text[end..];
                    tail.starts_with("-check")
                        || tail.starts_with(" checks")
                        || tail.starts_with(" vectors")
                        || (tail.starts_with('/')
                            && tail[1..].starts_with(|c: char| c.is_ascii_digit()))
                }
            })
        }
        assert!(pins_a_count("a 61-check suite") && pins_a_count("61/61 is the bar"));
        assert!(!pins_a_count("vector suite + boot self-check"));
        let manual = help_text();
        let audit_line = manual
            .lines()
            .find(|l| l.trim_start().starts_with("audit "))
            .expect("manual lists audit");
        for text in [
            audit_line.to_string(),
            command_help_text("audit").unwrap(),
            command_help_json("audit").unwrap().to_string(),
        ] {
            assert!(!pins_a_count(&text), "audit help pins a count: {text}");
        }
    }

    #[test]
    fn help_text_mentions_version_and_contract() {
        let t = help_text();
        assert!(t.contains(env!("CARGO_PKG_VERSION")));
        assert!(t.contains("EXIT CODES"));
        assert!(t.contains("help --json"));
    }
}
