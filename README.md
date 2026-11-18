# obu

Drive a real Chromium browser from an AI agent or the terminal. Five MCP tools, one Rust binary, no runtime dependencies.

> A fork of [dondai44423/bladebro](https://github.com/dondai44423/bladebro) (Apache-2.0), renamed by [`scripts/fork-rename.py`](scripts/fork-rename.py), for driving a browser you are working in: an attached session gets its own background tab, refs survive between commands, and a host app can hand obu its browser panel. <!-- fork-rename:keep -->
> The fork publishes no npm packages. A package called `obu` on npm is not this project.

## Install

Needs Chrome or Chromium on the same machine.

```sh
cargo install --locked --git https://github.com/aislopware/oni-browser-use obu
obu doctor
```

Prebuilt binaries for Linux x86_64 and macOS Apple Silicon, each with a `.sha256`, are on [Releases](https://github.com/aislopware/oni-browser-use/releases). `obu -u` updates a downloaded binary.

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
| `vision` | Screenshot, optionally marked with element refs |

## Your own browser

`obu rb on` drives your installed Chromium-family browser instead of obu's own. To attach to a browser already running with `--remote-debugging-port=9222`:

```sh
obu rb port 9222
obu rb on
```

The agent works in its own background tab and never touches yours. Set `OBU_SESSION=<name>` to give each agent its own tabs. `obu stop --close-tabs` closes the tabs obu opened.

## Docs

[Setup](docs/setup.md) · [Tools](docs/tools.md) · [Browser modes](docs/browser-modes.md) · [Configuration](docs/configuration.md) · [Architecture](docs/architecture.md) · [Support matrix](SUPPORT-MATRIX.md) · [Changelog](CHANGELOG.md)

## Sponsors

obu is built on the upstream author's work. To support them, see [their sponsorship page](https://github.com/dondai44423/bladebro#sponsors). <!-- fork-rename:keep -->

[Apache 2.0](LICENSE)
