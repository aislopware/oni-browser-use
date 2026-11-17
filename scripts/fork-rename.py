#!/usr/bin/env python3
"""Rename the upstream project to this fork's name, everywhere, repeatably.

    bladebro / Bladebro / BLADEBRO   ->  obu / Obu / OBU
    blade    / Blade    / BLADE      ->  obu / Obu / OBU   (BLADE_* env vars,
                                         BladeError, blade_dir, ~/.local/state/blade)
    dondai44423/bladebro             ->  aislopware/oni-browser-use

It rewrites every tracked text file and renames every tracked path. It is the
whole rename: nothing was renamed by hand, so the same script applied to an
upstream ref produces a tree that merges into this fork without a conflict on
any renamed line (see "Merging upstream" in AGENTS.md).

Left alone:
- links to upstream issues, pull requests, discussions and commits (they exist
  only there);
- any line carrying the marker `fork-rename:keep` (the fork notice);
- LICENSE files, binary files, this script.

Idempotent: a second run changes nothing.

    python3 scripts/fork-rename.py            # rewrite the working tree
    python3 scripts/fork-rename.py --check    # exit 1 if anything is left to rename
"""
import os
import re
import subprocess
import sys

UPSTREAM = "dondai44423/bladebro"
FORK = "aislopware/oni-browser-use"
KEEP = "fork-rename:keep"
SELF = "scripts/fork-rename.py"

# Upstream-only URLs: the numbers mean nothing in the fork.
UPSTREAM_ONLY = re.compile(re.escape(UPSTREAM) + r"(?=/(?:issues|pull|discussions|commit)/)")
HOLD = "\0upstream\0"
NAME = re.compile(r"blade(?:bro)?", re.IGNORECASE)
# The fork publishes nothing to npm, and a name nobody owns there is a name
# anyone can publish under: no line may tell a reader to install it from npm.
NPM_INSTALL = re.compile(r"npm (?:install|i|update) -g obu\b")
FROM_SOURCE = f"cargo install --locked --git https://github.com/{FORK} obu"


# What a rename alone gets wrong: (path after renaming, renamed text,
# replacement). Applied after the rename, so they follow upstream too; one
# that finds neither side in its file fails the run, because upstream moved.
FIXUPS = [
    # A test of the 64 KiB output bound printed the name 9000 times (72 kB);
    # three letters need more repeats to cross it.
    ("src/updater/download.rs", '"obu".repeat(9000)', '"obu".repeat(24000)'),
    # The clone lands in a directory named after the repository, not the binary.
    ("src/updater/download.rs", f"{FORK}.git && cd obu", f"{FORK}.git && cd oni-browser-use"),
    ("README.md", f"{FORK}.git\ncd obu\n", f"{FORK}.git\ncd oni-browser-use\n"),
    # The update check fell back to the npm registry. Upstream owns its name
    # there; nobody owns this one, so whoever publishes it would be believed.
    (
        "src/updater/version.rs",
        """    // 2. npm registry fallback.
    let npm_url = "https://registry.npmjs.org/obu/latest";
    if let Ok(resp) = client.get(npm_url).send().await {
        if resp.status().is_success() {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(ver) = json
                    .get("version")
                    .and_then(|v| v.as_str())
                    .filter(|v| is_version_tag(v))
                {
                    return Ok(ver.to_string());
                }
            }
        }
    }

    Err(ObuError::Other(
        "cannot reach GitHub or npm registry for version check".into(),
    ))
""",
        """    // 2. No npm registry fallback in the fork: it publishes nothing there,
    // and a name nobody owns is one anyone can publish a "newer version" under.
    Err(ObuError::Other(format!(
        "no release found at github.com/{} (the fork ships from source: \\
         git pull && cargo build --release)",
        super::GITHUB_REPO
    )))
""",
    ),
]


def new_name(match):
    old = match.group(0)
    if old.isupper():
        return "OBU"
    if old[0].isupper():
        return "Obu"
    return "obu"


def rename_text(text):
    out = []
    for line in text.split("\n"):
        if KEEP in line:
            out.append(line)
            continue
        line = UPSTREAM_ONLY.sub(HOLD, line)
        line = line.replace(UPSTREAM, FORK)
        line = NAME.sub(new_name, line)
        line = NPM_INSTALL.sub(FROM_SOURCE, line)
        out.append(line.replace(HOLD, UPSTREAM))
    return "\n".join(out)


def tracked():
    out = subprocess.run(["git", "ls-files", "-z"], check=True, capture_output=True).stdout
    return [p for p in out.decode().split("\0") if p]


def skipped(path):
    return os.path.basename(path).startswith("LICENSE") or os.path.islink(path)


def main():
    check = "--check" in sys.argv[1:]
    pending = []
    stale = []
    for path in tracked():
        if path == SELF or not os.path.isfile(path):
            continue
        with open(path, "rb") as f:
            raw = f.read()
        try:
            text = raw.decode("utf-8")
        except UnicodeDecodeError:
            text = None  # binary: only its path can change
        if skipped(path):
            text = None  # kept word for word: only its path can change
        if text is not None and "\0" not in text:
            renamed = rename_text(text)
            for where, was, now in FIXUPS:
                if where == NAME.sub(new_name, path):
                    if was not in renamed and now not in renamed:
                        stale.append(f"{where}: fixup no longer applies: {was[:60]!r}")
                    renamed = renamed.replace(was, now)
            if renamed != text:
                pending.append(path)
                if not check:
                    with open(path, "w", encoding="utf-8", newline="") as f:
                        f.write(renamed)
        target = NAME.sub(new_name, path)
        if target != path:
            pending.append(f"{path} -> {target}")
            if not check:
                os.makedirs(os.path.dirname(target) or ".", exist_ok=True)
                subprocess.run(["git", "mv", "-k", path, target], check=True)
    if stale:
        print("\n".join(stale))
        return 1
    if check and pending:
        print("\n".join(pending))
        print(f"{len(pending)} file(s) or path(s) still carry the upstream name")
        return 1
    if not check:
        print(f"renamed in {len(pending)} file(s)/path(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
