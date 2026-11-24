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
| `auto` · default | Attaches if a live, reachable debug endpoint is available, otherwise imports and uses a clone. |
| `clone` | Imports your profile into a Obu-owned copy and automatically refreshes whenever your real browser writes newer logins, cookies, or storage. The source profile can stay open and is left untouched. |
| `profile` | Launches on your live profile. Close the browser first. Branded Chrome 136+ refuses remote debugging on its default data directory. |
| `attach` | Connects to an existing debug endpoint (including `DevToolsActivePort`, Chrome 144+ `chrome://inspect#remote-debugging`, and fixed-port Linux browsers). The browser remains caller-owned and stays open when Obu detaches. |

For a given install, use `obu rb use <id>` and `obu rb profile <key>` to select a browser and profile. Recognized Chromium-family installs include Chromium, Chrome (Stable/Beta/Canary), Ungoogled Chromium, Brave, Helium, Arc, Edge, Vivaldi, Opera, Opera GX, and Thorium — including PATH installs and Linux Flatpak exports. Recognition and full runtime verification differ by engine. See the [support matrix](../SUPPORT-MATRIX.md).

For a portable binary or AppImage, launch it once to create the profile, then use `obu rb use --binary /absolute/path/to/browser` (and `obu rb profile /absolute/path/to/profile` if stored in a custom location). `obu rb refresh` forces an immediate re-import and `obu rb forget` removes the imported copy. If a running browser locks `Cookies` during a clone import (common on Windows), Obu surfaces a warning so you can close the browser and run `obu rb refresh`.

## Pause and switch back

```sh
obu rb pause
obu rb resume
obu rb off
```

Pause blocks input, navigation, downloads, collection, and tab operations while you take control. Reads, waits, and eval remain available. Resume re-enables input. Turning the mode off switches back to the agent browser. The CLI restarts its daemon on a lane change. MCP picks up the change on the next use, relaunching an owned browser or detaching an attached one. `rb status --json` reports the current selection and live state.

Real-browser mode can act in authenticated sessions. Choose the profile and account you want the agent to use before handing it control. To leave a caller-owned browser open, use attach mode. Owned launches (both agent and real-browser modes) mute audio by default so media players never produce unexpected sound; opt in with `obu audio on` (or `OBU_AUDIO=1`). For visual and idle behavior run `obu help rb` or inspect the current settings with `rb status`.
