# Support Matrix

Verification coverage by platform, browser engine and instrument, as the workflows run it. CI builds with the toolchain pinned in `rust-toolchain.toml` and checks the MSRV in `Cargo.toml`.

## Platforms

| Platform | Blocking CI (`ci.yml` / `gate.yml`) | Release & Runtime Coverage |
|---|---|---|
| Linux x86_64 | `fmt`, rename check, `clippy --release --all-targets -D warnings`, `cargo test --profile gate`, MSRV check | glibc `2.28` `zigbuild` + ABI check + binary smoke in `release.yml`; parallel Chrome suite in `probes.yml` |
| Linux ARM64 | Cross `clippy --all-targets -D warnings` (`zig cc`) | Not built by `release.yml`; no binary published, no browser runtime in CI |
| macOS Apple Silicon (`darwin-arm64`) | Cross `clippy --all-targets -D warnings` | Native release binary in `release.yml`, built on the organisation's own Mac, where the release gate also runs native `cargo check --all-targets`, unit tests (`--lib`) and `cargo build --release` |
| macOS Intel (`darwin-x64`) | Cross `clippy --all-targets -D warnings` | Not built by `release.yml`; no binary published |
| Windows x86_64 | Cross `clippy --all-targets -D warnings` + native check, unit tests (`--lib`), and `cargo build --release` on `windows-latest` | `zigbuild` PE binary in `release.yml`, run on `windows-latest` (`--version`, `obu audit` with the runner's Chrome) before publishing |
| Android (`aarch64-linux-android`) | Source compile check (`ci.yml` NDK job) | Source-level `cfg` guard only; no binary published and no runtime claim |

## Browser Engines

| Engine | Verified scope |
|---|---|
| Google Chrome Stable (Ubuntu CI) | Parallel `probes.yml` browser jobs (`audit`, `oracle`, `qol`, `lanes`, `gap44`, `reliability`) |
| Google Chrome (Windows CI) | `obu audit` with the release binary in `release.yml` |
| Flatpak `org.chromium.Chromium` (`1.18.4` x86_64) | Discovery, sandboxed launch, host persistence, upload staging, download, and teardown verified on Linux x86_64 |
| Chromium forks (Brave, Helium, Edge, Vivaldi, Opera) | Discovery, wrapper/AppImage PID ownership, and clone unit/integration fixtures (`tools/rb_live/helium.py` available for manual Helium runs) |

## Feature-to-Instrument Map

See [tools/README.md](tools/README.md) for exact invocations and pass criteria.

| Area | Instrument |
|---|---|
| Tool schemas, preflight validation, `run` plans, JSON notices | `tools/reliability_probe/agent_contract.py` + `cargo test --profile gate` |
| Live control inspection (`act read`), option labels, `run format=json` | `tools/reliability_probe/agent_ux.py` |
| Graceful shutdown storage flush, EOF, forked-launcher teardown | `tools/reliability_probe/persistence.py` + `tools/reliability_probe/browser-setup.py` |
| Trusted input, framework editors, shadow DOM, `rb pause`, scoped reads | `tools/qol_probe/run.py` |
| Addressing, readiness waits, dialogs, templates, PDFs, proxy redaction | `tools/gap44/probe.py` + `cargo test --profile gate` |
| Cross-origin and `body: true` downloads | `tools/reliability_probe/download.py` |
| Geometry coherence & fail-closed WebRTC under proxy | `tools/gap44/webrtc_proxy.py` |
| Stealth vectors & cross-restart stamp stability | `obu audit` |
| Stock-Chrome page parity (`agent` and `real` lanes) | `tools/diff_oracle/oracle.py` (`--lane real` for zero-mask parity) |
| Cold-start stability across all 5 lanes | `tools/lane_matrix.py all 5` |
| Real-browser attach → agent-lane switch | `tools/rb_live/attach_drift.py` |
| Linux glibc ≤ `2.28` symbol gate | `tools/reliability_probe/linux-abi.py` |
| npm / Cordis / Pi / Hermes host bridges | `tools/dsh_probe/`, `tools/reliability_probe/pi-stdio.mjs`, `tests/hermes_cli.rs`, `tools/hermes_probe/` |

## Unverified Gaps

- **Remote `oracle` job in `probes.yml`**: currently fails on GitHub's Ubuntu runner before comparison starts (`stock Chrome debug endpoint did not come up`); verified locally on Linux x86_64.
- **macOS & Windows live browser probes**: `gate.yml` runs native compile, unit tests and release linking on Windows for every change and on the organisation's Mac for releases, and `release.yml` runs `obu audit` on Windows; the probe suite (`probes.yml`) runs on Linux only.
- **Physical ARM64 / Raspberry Pi & SteamOS/Silverblue hosts**: covered by the cross-target lint, host-selector unit tests, and x86_64 Flatpak runs, not physical device runs.
- **Commercial upstream proxies & non-stock fork GUIs**: proxy tests use local forward proxies; non-Chromium forks outside manual Helium runs are covered by discovery/ownership fixtures rather than full GUI suites.
- **`SharedWorker` stealth scope & scanned PDFs**: `SharedWorker` globals remain native by design to avoid breaking module/shared workers; scanned image PDFs have no in-driver OCR (`act download` hands raw bytes to the caller).
