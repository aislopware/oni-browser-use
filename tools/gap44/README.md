# gap44 probe — 4.4.0 acceptance evidence

`probe.py` is the live acceptance instrument for the 4.4.0 gap-fix batch
(G01 option/label addressing, G02 wait contract, G03 dialog expectations,
G06 template extraction contract, G08 PDF reading, G13 proxy redaction).
It drives the real MCP server over stdio against a fresh scratch
`OBU_HOME` and a local HTTP fixture (text PDF), and records every check
plus full request/reply receipts.

Run it twice from clean homes — that is the gate:

```bash
python3 tools/gap44/probe.py 1
python3 tools/gap44/probe.py 2
```

Each run prints one line per check (`[ok ]` / `[FAIL]`) and writes
`receipts.json` under its scratch home. Exit 0 means every check passed;
an internal probe exception fails the run loudly (it never silently drops
checks). Uses `OBU` when set, otherwise the release binary at
`target/release/obu`. The HTTP fixture handles connections concurrently:
Chrome's idle preconnect socket must not block the subsequent PDF fetch and
manufacture a browser timeout.
