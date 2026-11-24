# AGENTS.md

Obu is a browser driver for AI agents: an MCP server (`obu mcp`, stdio JSON-RPC) and a CLI (`obu nav`, `see`, `act`, …) that drive stock Chrome/Chromium over the DevTools Protocol. It ships as one Rust binary with no Node.js runtime, no Playwright or Puppeteer, and no model calls. The CDP transport, perception, stealth injection, input biometrics and session management are all built in this repo. [CONTRIBUTING.md](CONTRIBUTING.md) is the human-facing summary of the same rules.

Two facts shape most decisions. The users are LLMs: every tool output, verdict and error string is read by an agent mid-task and billed per token, so output stays dense and honest, and each new field costs every user. And stealth is measured: every page-visible difference from stock Chrome is a deliberate, documented mask, and the instruments under Verification are how that is proven.

## Commands

```bash
cargo check
cargo build --profile gate                           # fast runnable binary: ./target/gate/obu
cargo fmt --check
cargo clippy --release --all-targets -- -D warnings
cargo test --profile gate
python3 scripts/fork-rename.py --check               # see Merging upstream
cargo deny check                                     # after a dependency change (deny.toml)
```

For manual runs, export a scratch home first, so that every command in the shell stays off the real browser profile:

```bash
export OBU_HOME=$(mktemp -d)
./target/gate/obu nav example.com    # starts its own daemon and browser
./target/gate/obu see content
./target/gate/obu stop               # when done
```

A running daemon or MCP server keeps executing the old binary, so stop it after each rebuild. `obu doctor` checks the environment.

`rust-toolchain.toml` pins the toolchain CI uses; bump it deliberately and run the gate on the new version. CI also checks the MSRV (1.88), so no std APIs newer than that. Chrome or Chromium must be installed; on headless Linux obu runs its own Xvfb and window manager.

## Where to change what

| To touch… | Start in |
|---|---|
| click/type/fill/select/press, find-by-sig, action conditions | `src/action.rs` + `src/action/` (perform, find, input, verdict, edit) |
| capture, markdown, settle, block detection | `src/page/perception.rs` + `src/page/perception/` (wait, consent, block, content), `src/page/mod.rs` (JS in `src/page/js/`) |
| refs, Live Page Model, deltas | `src/page/refs.rs`, `src/page/model.rs` |
| navigation, tabs, attach, heal, page logs, client-side route guard | `src/page/` (navigate, tabs, attach, heal, logs, route) |
| stealth injection (every page-side override) | `src/stealth/inject.rs` + `src/stealth/inject/` (gpu), `src/stealth/js/` |
| mouse/typing biometrics, idle hum | `src/stealth/biometrics.rs`, `src/stealth/hum.rs` |
| Chrome discovery, launch flags, CDP transports, Xvfb | `src/browser.rs` + `src/browser/`, `src/platform.rs` |
| MCP tool schemas and dispatch | `src/mcp/tools.rs`, `src/mcp/server.rs` + `src/mcp/server/` (boot, serve, proto + tool handlers) |
| CLI commands, daemon, help text | `src/cli.rs` + `src/cli/` (args/ per-command parsers, daemon, help, rb) |
| sessions/profiles, seasoning, orphan reaper | `src/session_profile.rs` + `src/session_profile/` (reap, copy) |
| cookies/storage/tabs, session save-load | `src/state.rs` |
| site adapters (`extract=auto` fast paths) | `src/adapters.rs` + `src/adapters/js/` (engine, per-site JS), `src/reddit.rs` + `src/reddit/`, `src/x.rs` + `src/x/` |
| real-browser lane (`rb`) | `src/realbrowser.rs` + `src/realbrowser/` |
| self-update, rollback, doctor | `src/updater/` |
| human CLI output/styling | `src/ui.rs` |

Integration tests and fixtures are in `tests/`; unit tests are inline `#[cfg(test)]`. `README.md` is the user-facing source of truth for behavior.

## What a change must preserve

- **Five MCP tools:** `act`, `see`, `state`, `run`, `vision`. Their definitions sit in every agent's context, so new capabilities become parameters or behaviors of these five, not a sixth tool.
- **No model calls in the driver.** It is deterministic machinery; the agent using it is the intelligence.
- **No CAPTCHA solving.** Detect the challenge, return an honest `blocked:` verdict, and apply the remediation ladder. The one exception: a site adapter may pass a challenge that is a single check-the-box click, through the normal humanized input path, as the reddit adapter does for reddit's one-time humanity gate. It stays specific to that site, and anything harder, such as an image grid, is reported, not solved.
- **Stock Chrome/Chromium, never a Chromium fork.**
- **Site adapters only optimize.** The `extract=auto` paths make existing commands better on one site, add no tools or parameters, and are tested live on that site.
- **Verdicts are proven.** Report a success (a value written, a field cleared, a click's effect) only after a readback shows it; when no readback is possible, the verdict says "readback unverified". Errors carry the current page state so the agent can recover without another call.
- **Stealth overrides are Proxies over the native originals**, whose apply traps call the original first so native receiver and argument checks still run. Do not patch `Function.prototype.toString`: it leaks the real source of every patched function. Do not add automation flags (`--enable-automation`, `--disable-blink-features=AutomationControlled`, …): detectors read them, and Chrome shows an infobar for them.
- **Every platform still builds.** Releases ship Linux x64, Windows x64 and macOS arm64. CI lints Windows, macOS arm64 and x64 and Linux arm64 with clippy, compiles Android, and builds and unit-tests natively on Windows. Put platform differences behind `src/platform.rs`; a failure on another OS is a real failure.
- **A bug fix comes with a regression test** that fails without the fix.

Ask the maintainer (an issue or a draft PR) before you add a dependency (binary size and supply chain), change MCP tool schemas or capture/perception output formats (agents parse them and pay for every word), change the stealth injection architecture, or touch `npm/`. Do not commit secrets, real browsing-profile data or build artifacts, and do not add npm install scripts.

## Verification

Reproduce a bug before you fix it, and judge a change by live readback: a command that exits 0 does not show that the behavior works. The instruments below run as parallel CI jobs in `.github/workflows/probes.yml` (nightly, on `v*` tags, and on demand). Locally, run only the one that covers your change, with `OBU=target/gate/obu`; [tools/README.md](tools/README.md) has the details.

- `target/gate/obu audit`: stealth vectors and cross-restart stamps. The suite prints its own total, which depends on GPU and profile; the bar is no `FAIL` line and stable stamps.
- `python3 tools/diff_oracle/oracle.py`: stock Chrome against obu on the same display. Each difference is EXPECTED (a documented mask) or DIVERGENT; the bar is `divergent: 0`, and a DIVERGENT line blocks the change. `--lane real` checks the real-browser lane, where the bar is 0 expected and 0 divergent.
- `python3 tools/qol_probe/run.py`: input and action behavior against local fixtures. It is the fastest check for click/type/fill/select changes and the place to add a repro when you fix an input bug.
- `python3 tools/lane_matrix.py all 5`: cold starts across the daemon, one-shot, MCP, MCP-pipe and real lanes.
- `python3 tools/rb_live/attach_drift.py`: after you touch `src/realbrowser.rs`.
- `python3 tools/reliability_probe/persistence.py`: after you touch browser shutdown or restart.

A stealth change also needs before/after results in the PR description from bot.sannysoft.com (all pass), incolumitas.com (no bot detection) and abrahamjuliot.github.io/creepjs (headless 0%, stealth 0%). The stealth baseline depends on the Chrome version, so compare against the same Chrome before you call a difference a regression.

## Code conventions

- Error strings are product surface: specific, actionable, and naming the recovery path. Action errors include page state.
- stdout is a machine contract. `--json` prints one JSON object and MCP stdout is pure JSON-RPC; logs go to stderr, and `src/ui.rs` styling applies only on a TTY.
- Keep outputs lean: deltas first, big payloads in artifact files, no field an agent does not need.
- No `unwrap`/`expect` outside tests unless the case is provably guarded; errors bubble up with context (`anyhow`/`thiserror`).
- Injected JS stays dependency-free. One parse error silently disables every patch, so tests run `node --check` on the assembled script; to debug, set `OBU_DUMP_INJECT=/tmp/inj.js` and run `node --check /tmp/inj.js`. Embedded JS lives in `src/**/js/*.js` and is loaded with `include_str!`, so edit the `.js` file, not a Rust string.
- Help blocks in `src/cli/help.rs` are raw strings (`r#"…"#`). A `"#` inside one ends the string, and the compiler error shows up far from it.
- Large modules are a thin `foo.rs` (public surface, a `Module map:` header, re-exports) plus a `foo/` folder of focused children. When you move code, keep public paths stable with re-exports.
- Comments explain why: constraints, detection classes, what breaks if the line changes.

## Runtime traps

- An owned browser shuts down through `Browser::shutdown_gracefully()` on every exit path (normal exit, EOF, signal, idle) before process-tree cleanup. Killing the process group first stops Chrome's storage services before localStorage reaches disk. Keep emergency cleanup bounded, and never close a browser obu attached to.
- The MCP lane uses the WebSocket transport, where `navigator.webdriver` is natively false. `OBU_TRANSPORT=pipe` is opt-in: Chrome then reports `webdriver == true` and needs a mask, so it stays off by default.
- A page session's `Runtime.evaluate` runs in an isolated world (`src/cdp/reader.rs`). Page scripts cannot see the driver's reads there, but the page's JS globals, JS-assigned `on*` handlers and custom-element properties are missing too. Code that needs them (the agent's `eval`, an adapter calling the page's API, a stealth self-check) asks for `cdp.main_world()`. For one property of one node, pass the node across by backend id as `in_the_pages_world` (`src/action/find.rs`) does, rather than moving the whole script. Scripts must be a single expression (an IIFE); a statement list falls back to the page's world. `OBU_MAIN_WORLD=1` turns the routing off for A/B tests.

## Git and PRs

Use Conventional Commits (`feat:`, `fix:`, `stealth:`, `perf:`, `docs:`, `chore:`), and keep one concern per PR: a stealth fix and a refactor are two PRs. Fill in `.github/PULL_REQUEST_TEMPLATE.md` with what changed, how you verified it (commands and output), and before/after numbers for stealth or performance work. CI runs fmt, the rename check, clippy and tests on Linux, plus clippy on the other targets, the MSRV check and the native Windows checks; a change to Markdown files alone skips it. `supply-chain.yml` runs `cargo deny check` and `cargo audit` when the dependencies change and nightly.

## Shipping

Releases are maintainer work, so a PR leaves versions, `CHANGELOG.md` and tags alone. `scripts/release-prep.sh X.Y.Z --push` bumps the versions, promotes the changelog, commits, tags, pushes and starts `.github/workflows/release.yml`. A tag push alone starts no release, because the macOS runner accepts only hand-started runs. The workflow runs the gate, builds Linux x64, Windows x64 and macOS arm64, runs `obu audit` with the Windows binary on a Windows runner, and publishes the GitHub release; its npm job runs only when the repository variable `PUBLISH_NPM` is `true`. Do not build a release locally, bump a version by hand or re-tag; re-run a failed release with `workflow_dispatch` on the same tag.

## Merging upstream

This fork is [dondai44423/bladebro](https://github.com/dondai44423/bladebro) under another name. <!-- fork-rename:keep -->
The rename is done by `scripts/fork-rename.py` and by nothing else: `bladebro` → `obu`, `BLADE_*` → `OBU_*`, the data dir `blade` → `obu`, the npm packages, the repository URL. <!-- fork-rename:keep -->
Never rename by hand, and never bring the upstream name back in new code — CI runs `python3 scripts/fork-rename.py --check`.

To take upstream changes, rename upstream's tree with the same script, then merge. Build the renamed commit on the previous one, with upstream as a second parent: the merge base is then the last renamed upstream, so only real changes conflict, and `git log main..upstream/main` still lists what is new. Branching it from `upstream/main` instead puts the unrenamed tree in the base, and every fork edit next to a renamed line conflicts.

```bash
git fetch upstream
git show main:scripts/fork-rename.py > /tmp/fork-rename.py
prev=$(git log main -1 --format=%H --grep='^chore: upstream, renamed')
git switch -C upstream-renamed "$prev"
git rm -rqf . && git checkout upstream/main -- .
python3 /tmp/fork-rename.py && cargo fmt -- --style-edition 2024 && git add -A
git reset --soft "$(git commit-tree "$(git write-tree)" -p "$prev" -p upstream/main \
  -m 'chore: upstream, renamed by scripts/fork-rename.py')"
git switch main && git merge upstream-renamed
```

The fork is on the 2024 edition and upstream is not; `--style-edition 2024` formats the renamed tree the way main is, so formatting never conflicts. Upstream code that the 2024 edition rejects (an `std::env::set_var` outside `unsafe`, say) is fixed in the merge commit.

When the script stops with "fixup no longer applies", upstream moved that text: fix the entry in `FIXUPS` on main first.

A fix that only exists because the name got shorter (a test that sized its input by the name) goes into the script's `FIXUPS` table, not into the tree — otherwise the next merge conflicts on it.
