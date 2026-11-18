# Browser modes

[Obu README](../README.md) · [Configuration](configuration.md)

Obu can start its own profile, import a copy of an existing one, launch a live profile, or attach to a browser you've started. All modes expose the same five tools.

## Agent browser

The default browser is owned by Obu and uses a dedicated profile. Browser state can persist across orderly shutdowns. To isolate a task from your ordinary Obu data, set a separate `OBU_HOME` before launching the CLI or MCP server. `OBU_FRESH=1` requests an ephemeral profile. Incognito compatibility is a separate mode with different storage behavior. [Storage options](configuration.md#data-and-profiles).

## Your browser

```sh
obu rb use
obu rb status
obu rb on
```

Real-browser mode uses your Chromium-family binary and selected profile. It runs on your display and disables the driver's page injection layer. Page perception, element refs, actions, and the CLI/MCP interface remain available.

| `rb mode` | What Obu does |
|---|---|
| `auto` · default | Attaches if a debug endpoint is available, otherwise imports and uses a clone. |
| `clone` | Imports your profile into a Obu-owned copy. The source profile can stay open and is left untouched. |
| `profile` | Launches on your live profile. Close the browser first. Branded Chrome 136+ refuses remote debugging on its default data directory. |
| `attach` | Connects to an existing debug endpoint. The browser remains caller-owned and stays open when Obu detaches. |

A browser started with `--remote-debugging-port=<N>` writes no `DevToolsActivePort`, so name the port once with `obu rb port <N>` (`obu rb port auto` clears it). An attached session works in its own background tab and never navigates, hides or raises yours; its refs stay valid from one CLI command to the next. Several agents can share one attached browser when each sets `OBU_SESSION=<name>`: each gets its own tabs and daemon, while logins and `rb` settings stay shared. `obu stop --close-tabs` closes the tabs that session opened.

For a given install, use `obu rb use <id>` and `obu rb profile <key>` to select a browser and profile. Recognized Chromium-family installs include Chromium, Chrome, Brave, Helium, Edge, Vivaldi, and Opera. Recognition and full runtime verification differ by engine. See the [support matrix](../SUPPORT-MATRIX.md).

For a portable binary, launch it once to create the profile, then use `obu rb use --binary /absolute/path/to/browser`. An explicit browser path is useful for AppImage or custom installations. `obu rb refresh` reimports the clone and `obu rb forget` removes the imported copy.

## Pause and switch back

```sh
obu rb pause
obu rb resume
obu rb off
```

Pause blocks input, navigation, downloads, collection, and tab operations while you take control. Reads, waits, and eval remain available. Resume re-enables input. Turning the mode off switches back to the agent browser. The CLI restarts its daemon on a lane change. MCP picks up the change on the next use, relaunching an owned browser or detaching an attached one. `rb status --json` reports the current selection and live state.

Real-browser mode can act in authenticated sessions. Choose the profile and account you want the agent to use before handing it control. To leave a caller-owned browser open, use attach mode. For visual and idle behavior run `obu help rb` or inspect the current settings with `rb status`.
