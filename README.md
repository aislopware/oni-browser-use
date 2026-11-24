# obu

Drive a real Chromium browser from an AI agent or the terminal. Five MCP tools, one Rust binary, no runtime dependencies.

> A fork of [dondai44423/bladebro](https://github.com/dondai44423/bladebro) (Apache-2.0), renamed by [`scripts/fork-rename.py`](scripts/fork-rename.py). <!-- fork-rename:keep -->
> The fork publishes no npm packages. A package called `obu` on npm is not this project.

## Why obu

Against other browser tools for agents (upstream's measurements):

- **Gets past anti-bot pages.** 68 of 80 sites (85%) on [Stealth Bench V1](STEALTH_BENCH.md), out of the box, with no proxy or captcha solver. The best published provider, Browser Use Cloud, scores 81%. Playwright MCP and Chrome DevTools MCP have no stealth layer.
- **Costs few tokens.** An action returns only what changed, 60–570 tokens, where snapshot-based tools return 1,400–2,000+. The five tool definitions are small; Playwright MCP's take about 13,700 tokens, Chrome DevTools MCP's about 8,000.
- **Refs survive re-renders.** An element keeps its ref when React or Vue replaces the DOM node, so the agent doesn't re-snapshot after every click.
- **Does more per call.** `act batch` and `run` (with `if`/`while`) run multi-step work in one call; `see extract` turns lists into JSON; `act collect` reads a whole infinite-scroll feed.
- **One 7–8 MB binary.** No Node.js, no Python, no Playwright.

Against upstream, the fork adds what driving a browser you are using needs:

- **Attach by port.** `obu rb port <N>` remembers a fixed debug port, so attach mode finds browsers that never advertise one, such as Brave.
- **Stays out of your way.** An attached session works in its own background tab. Your tabs are never navigated, hidden or raised, and a covered window still renders and takes input.
- **Refs last between commands.** In an attached browser, one-shot CLI calls on the same tab keep their refs; upstream renumbered them, so `click e12` could hit another element.
- **Quieter reads.** The driver reads the page from an isolated world, so page scripts can't watch it read the DOM.
- **Several agents, one browser.** `OBU_SESSION` gives each agent its own tabs and daemon; `obu stop --close-tabs` closes only that session's tabs.
- **Embeddable.** A host app can serve the debug endpoint under a secret path (`OBU_ENDPOINT_PATH`) and tell the agent whose browser it is (`OBU_HOST_NOTE`). This is how [Onicode](https://github.com/aislopware/onicode)'s browser panel runs it.
- **Updates only from this repository.** The updater never falls back to npm, where anyone could publish an `obu`.

## Install

Needs Chrome or Chromium on the same machine.

```sh
cargo install --locked --git https://github.com/aislopware/oni-browser-use obu
obu doctor
```

Prebuilt binaries for Linux x86_64, Windows x86_64 and macOS Apple Silicon, each with a `.sha256`, are on [Releases](https://github.com/aislopware/oni-browser-use/releases). `obu -u` updates a downloaded binary.

## Use

As an MCP server:

```json
{ "mcpServers": { "obu": { "command": "obu", "args": ["mcp"] } } }
```

From the shell:

```sh
obu nav https://example.com
obu see content
obu stop
```

| Tool | What it does |
|---|---|
| `act` | Navigate, click, type, fill, scroll, upload, download |
| `see` | Read the page: interactive elements, Markdown, outline, extracted records |
| `state` | Tabs, cookies, storage, saved sessions, resource blocking |
| `run` | A plan of reads and actions with branches and loops, in one call |
| `vision` | Screenshot, inline or to a file, of the viewport, a sized viewport or the whole page; optionally marked with element refs |

## Your own browser

`obu rb on` drives your installed Chromium-family browser instead of obu's own. To attach to a browser already running with `--remote-debugging-port=9222`:

```sh
obu rb port 9222
obu rb on
```

The agent works in its own background tab and never touches yours. Set `OBU_SESSION=<name>` to give each agent its own tabs. `obu stop --close-tabs` closes the tabs obu opened. A browser obu launches starts muted; `obu audio on` turns sound on.

## Docs

[Setup](docs/setup.md) · [Tools](docs/tools.md) · [Browser modes](docs/browser-modes.md) · [Configuration](docs/configuration.md) · [Architecture](docs/architecture.md) · [Support matrix](SUPPORT-MATRIX.md) · [Changelog](CHANGELOG.md)

## Sponsors

obu is built on the upstream author's work. To support them, see [their sponsorship page](https://github.com/dondai44423/bladebro#sponsors). <!-- fork-rename:keep -->

[Apache 2.0](LICENSE)
