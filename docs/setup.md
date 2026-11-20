# Setup

[Obu README](../README.md) · [Configuration](configuration.md) · [Tool guide](tools.md)

Obu needs a Chromium-family browser on the host running the process.

## Prebuilt binaries

Linux x86_64, Windows x86_64 and macOS Apple Silicon binaries, each with a SHA-256 checksum, are on [GitHub Releases](https://github.com/aislopware/oni-browser-use/releases). On any other platform, build [from source](#from-source) or install with Cargo:

```sh
cargo install --locked --git https://github.com/aislopware/oni-browser-use obu
obu doctor
```

The fork publishes no npm packages. A package called `obu` on npm is not this project.

Linux releases require glibc 2.28 or later. An existing binary can fail with `ENOENT` if its loader is missing. Check the libc version if reinstalling does not help. On musl-based systems, try building from source. See the [support matrix](../SUPPORT-MATRIX.md) for runtime coverage.

## MCP

Register the following entry in a client that accepts `mcpServers`.

```json
{
  "mcpServers": {
    "obu": {
      "command": "obu",
      "args": ["mcp"]
    }
  }
}
```

For a desktop client with a different PATH, put the absolute executable path in `command`. The server communicates over stdin and stdout. Client-side restart instructions depend on the host.

## Agent integrations

Pi and DeepSeek Harness install upstream's npm package, which the fork does not publish. Point them at the `obu` binary as a plain MCP server instead.

### Hermes

```sh
cargo install --locked --git https://github.com/aislopware/oni-browser-use obu
obu hermes on
```

Start a new Hermes chat and restart a running gateway or desktop client. Setup registers the standard MCP server and suppresses Hermes' built-in browser toolset. Web search and extraction keep their settings. Use a Hermes version with `config get --json --raw`, `config unset`, and `mcp test`.

```sh
obu hermes status
obu hermes off
```

Use the same `--profile` for setup and restore when choosing a named profile. `HERMES_HOME` and `--hermes /path/to/hermes` also select the target instance. Hermes may expose the tools through Tool Search. Interrupted changes can resume with `on` or restore with `off`. Avoid changing Hermes configuration concurrently with the switch.

## From source

The crate declares Rust 1.88 as its minimum and the checkout pins the toolchain used by CI. Use the repository pin for development.

```sh
git clone https://github.com/aislopware/oni-browser-use.git
cd oni-browser-use
cargo build --release
./target/release/obu doctor
```

See [Contributing](../CONTRIBUTING.md) for the edit and verification workflow.

## Updating

| Install | Update |
|---|---|
| Cargo | `cargo install --locked --git https://github.com/aislopware/oni-browser-use obu` |
| Standalone binary | `obu -u` |
| Source checkout | Pull changes and rebuild |

The standalone updater verifies the official release checksum and backs up the previous binary. Restore it with `obu --rollback`. Restart the CLI daemon with `obu stop` and restart any MCP client process to pick up a replacement executable.
