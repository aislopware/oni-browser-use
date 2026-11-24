//! A tab's ref table on disk, for browsers that outlive the process.
//!
//! In an attached browser the tab stays open between one-shot commands, but
//! each command is a new process with an empty Live Page Model: `nav` printed
//! `e12`, and the `click e12` after it resolved against a table renumbered
//! from `e1` — a different element, clicked without an error (measured on
//! Brave 157: 0 of 3 clicks landed on the link `nav` had printed).
//!
//! `<data-dir>/refs/<targetId>.json` holds the [`RefSnapshot`] of each tab,
//! written after a command and read back by the next `Page::attach` on that
//! tab. Only attached browsers get one: a browser obu launched dies with
//! the session, and its target ids with it.
//!
//! The file names page elements (roles, accessible names) — never field
//! values — and is written 0600 like the other session state.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::Page;
use super::model::{LivePageModel, RefSnapshot};
use crate::cdp::CdpSession;

/// Tabs come and go; a table nobody resumed for a week belongs to a tab that
/// is gone.
const STALE_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);
/// A resumed table is stamped as in use at most this often: a page that does
/// not change is never rewritten, and would otherwise age out under a tab
/// that is used every day.
const STAMP_EVERY: Duration = Duration::from_secs(24 * 3600);

pub(super) fn dir() -> PathBuf {
    crate::platform::obu_dir().join("refs")
}

/// A target id becomes a file name: only the shape Chrome mints is accepted.
fn file(dir: &Path, target: &str) -> Option<PathBuf> {
    let plain = !target.is_empty()
        && target.len() <= 64
        && target
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    plain.then(|| dir.join(format!("{target}.json")))
}

/// A tab that was closed for good takes its table with it.
pub(super) fn forget(dir: &Path, target: &str) {
    if let Some(path) = file(dir, target) {
        let _ = std::fs::remove_file(path);
    }
}

/// What the last change-check compares against: a snapshot is rewritten only
/// when the table moved.
pub(super) fn digest(snap: &RefSnapshot) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    snap.next_ref.hash(&mut h);
    snap.refs.hash(&mut h);
    snap.graveyard.hash(&mut h);
    snap.url.hash(&mut h);
    h.finish()
}

/// The stored table of `target`. A missing or unreadable file is no table:
/// the tab starts fresh from the counter's beginning, as before.
pub(super) fn load(dir: &Path, target: &str) -> Option<RefSnapshot> {
    let raw = std::fs::read(file(dir, target)?).ok()?;
    serde_json::from_slice(&raw).ok()
}

pub(super) fn save(dir: &Path, target: &str, snap: &RefSnapshot) -> std::io::Result<()> {
    let Some(path) = file(dir, target) else {
        return Ok(());
    };
    std::fs::create_dir_all(dir)?;
    if !path.exists() {
        // A tab seen for the first time: the moment to drop long-gone ones.
        prune(dir);
    }
    let data = serde_json::to_vec(snap).map_err(std::io::Error::other)?;
    crate::platform::secure_write_file(&path, &data)
}

/// Mark the table of `target` as in use now. Best effort: an unstamped table
/// is only pruned a week after its last change.
fn stamp(dir: &Path, target: &str) {
    let Some(path) = file(dir, target) else {
        return;
    };
    let age = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok());
    if age.is_some_and(|age| age > STAMP_EVERY)
        && let Ok(f) = std::fs::File::options().write(true).open(&path)
    {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let stale = path.extension().is_some_and(|e| e == "json")
            && entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > STALE_AFTER);
        if stale {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The model a new `Page` starts from: empty, or — in an attached browser —
/// this tab's stored table. Returns the tab's id when its table is to be
/// kept, and the digest of what is already on disk.
pub(super) async fn resume(cdp: &CdpSession) -> (LivePageModel, Option<String>, u64) {
    let mut lpm = LivePageModel::new();
    if !crate::browser::attached_browser() {
        return (lpm, None, 0);
    }
    // No id, no table: the session works exactly as it did, per process.
    let target = cdp
        .send("Target.getTargetInfo", None)
        .await
        .ok()
        .and_then(|info| info["targetInfo"]["targetId"].as_str().map(str::to_string))
        .filter(|id| file(Path::new(""), id).is_some());
    let mut saved = 0;
    if let Some((id, snap)) = target.as_deref().and_then(|t| Some((t, load(&dir(), t)?))) {
        saved = digest(&snap);
        lpm.seed(snap);
        stamp(&dir(), id);
    }
    (lpm, target, saved)
}

/// The table on disk is not the one this process last read or wrote: someone
/// else wrote it since. Returns the first ref number neither has used.
fn written_meanwhile(on_disk: Option<RefSnapshot>, last_seen: u64) -> Option<u64> {
    on_disk
        .filter(|theirs| digest(theirs) != last_seen)
        .map(|theirs| theirs.floor())
}

impl Page {
    /// Before a command: another process may have used this tab since this
    /// one last looked (a one-shot command beside a daemon). Its table is the
    /// newer one — this process wrote its own after every command — so it is
    /// taken over and the page re-read against it: a ref the other process
    /// printed names the same element here, and none of its numbers is ever
    /// minted again.
    pub async fn refs_sync(&mut self) {
        let Some(target) = self.ref_target.as_deref() else {
            return;
        };
        let Some(stored) = load(&dir(), target) else {
            return;
        };
        let theirs = digest(&stored);
        if theirs == self.refs_saved {
            return;
        }
        self.lpm.seed(stored);
        self.refs_saved = theirs;
        // Rebuild the live elements under the adopted table. An unanswered
        // read keeps the table; the command's own capture binds it.
        let _ = self.recapture().await;
    }

    /// After a command: keep this tab's ref table for the next process.
    pub fn refs_save(&mut self) {
        let Some(target) = self.ref_target.as_deref() else {
            return;
        };
        let mut snap = self.lpm.snapshot();
        if digest(&snap) == self.refs_saved {
            return;
        }
        // Another process used this tab while this command ran (two agents,
        // or a one-shot command beside a busy daemon): both minted from the
        // same counter, so a number printed by one may name another element
        // in the other. Nothing printed can be taken back — say so, and keep
        // every later number past both.
        if let Some(floor) = written_meanwhile(load(&dir(), target), self.refs_saved) {
            self.lpm.raise_ref_floor(floor);
            snap.next_ref = snap.next_ref.max(floor);
            eprintln!(
                "[obu] another obu process used this tab during this command — refs printed by either may not match; re-read the page (`see`) before acting by ref"
            );
        }
        let now = digest(&snap);
        match save(&dir(), target, &snap) {
            Ok(()) => self.refs_saved = now,
            // Said once per change, on stderr: the command itself succeeded,
            // but the refs it printed will not resolve in a later process.
            Err(e) => eprintln!(
                "[obu] could not save this tab's refs ({e}) — refs printed by this command will not carry over to the next one-shot command; re-read the page there"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::perception::{PageCapture, RawElement};

    fn el(role: &str, name: &str) -> RawElement {
        serde_json::from_value(serde_json::json!({
            "tag": "a", "role": role, "name": name,
            "box": [0.0, 0.0, 10.0, 10.0],
            "sig": format!("{role}:{name}"),
        }))
        .unwrap()
    }

    fn page(url: &str, names: &[&str]) -> PageCapture {
        let mut cap: PageCapture = serde_json::from_value(serde_json::json!({
            "url": url, "readyState": "complete", "elements": [],
        }))
        .unwrap();
        cap.elements = names.iter().map(|n| el("link", n)).collect();
        cap
    }

    fn ref_of(lpm: &LivePageModel, name: &str) -> String {
        lpm.elements()
            .iter()
            .find(|e| e.raw.name == name)
            .map(|e| e.ref_id.clone())
            .unwrap_or_else(|| panic!("{name} is not on the page"))
    }

    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("obu-refs-{tag}-{}-{nanos}", std::process::id()))
    }

    /// The bug: process A reads the page and prints refs; process B (the next
    /// one-shot command, same tab) renumbered from e1 and `e3` named another
    /// element.
    #[test]
    fn the_next_process_resolves_the_refs_the_last_one_printed() {
        let dir = scratch("resume");
        // Process A: lands on a page, then the page changes under it, so the
        // refs are no longer 1..n in page order.
        let mut a = LivePageModel::new();
        a.ingest(page("https://x.test/", &["old", "front"]));
        a.ingest(page("https://x.test/", &["new", "past", "front", "newest"]));
        let newest = ref_of(&a, "newest");
        let front = ref_of(&a, "front");
        save(&dir, "TAB1", &a.snapshot()).unwrap();

        // Process B without the table — what shipped: the same page, and the
        // ref A printed for "newest" is bound to something else.
        let mut cold = LivePageModel::new();
        cold.ingest(page("https://x.test/", &["new", "past", "front", "newest"]));
        assert_ne!(
            cold.element(&newest).map(|e| e.raw.name.as_str()),
            Some("newest"),
            "a cold table rebinding A's ref is the bug this test is about"
        );

        // Process B with it.
        let mut b = LivePageModel::new();
        b.seed(load(&dir, "TAB1").expect("the table A saved"));
        b.ingest(page("https://x.test/", &["new", "past", "front", "newest"]));
        assert_eq!(b.element(&newest).unwrap().raw.name, "newest");
        assert_eq!(b.element(&front).unwrap().raw.name, "front");
        // Unchanged page, unchanged table: nothing to rewrite.
        assert_eq!(digest(&b.snapshot()), digest(&a.snapshot()));

        // A new element gets a number A never used; a gone one still heals.
        b.ingest(page("https://x.test/", &["front", "newest", "login"]));
        let login = ref_of(&b, "login");
        assert!(a.element(&login).is_none() && a.graveyard_lookup(&login).is_none());
        assert_eq!(
            ref_number_of(&login),
            a.snapshot().next_ref,
            "the counter continues where A stopped"
        );
        let old = a.graveyard_lookup("e1").expect("A buried `old`");
        assert_eq!(b.graveyard_lookup("e1"), Some(old));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn ref_number_of(id: &str) -> u64 {
        id.trim_start_matches('e').parse().unwrap()
    }

    /// Another page in the same tab: every old ref is retired, none reused,
    /// and none rebinds to an element that only looks the same.
    #[test]
    fn a_resumed_table_never_reuses_a_ref_on_another_page() {
        let mut a = LivePageModel::new();
        a.ingest(page("https://x.test/", &["one", "two", "three"]));
        let snap = a.snapshot();

        let mut b = LivePageModel::new();
        b.seed(snap.clone());
        // "two" on the next page has the same sig as "two" on the first.
        b.ingest(page("https://y.test/", &["alpha", "two"]));
        for e in b.elements() {
            assert!(
                ref_number_of(&e.ref_id) >= snap.next_ref,
                "{} reuses a ref of the previous page",
                e.ref_id
            );
        }
        for (id, ..) in &snap.refs {
            assert!(b.element(id).is_none(), "{id} must be dead");
            assert!(b.graveyard_lookup(id).is_none(), "{id} must not heal");
            let (_, _, old_url) = b.nav_retired_lookup(id).expect("retired by the navigation");
            assert_eq!(old_url, "https://x.test/");
        }
    }

    /// A one-shot command moved the tab to another page and printed refs
    /// there. The daemon adopting its table is on that page too: the refs
    /// stay, they are not retired as the daemon's old page.
    #[test]
    fn an_adopted_table_keeps_the_refs_of_the_page_it_was_taken_on() {
        let mut daemon = LivePageModel::new();
        daemon.ingest(page("https://x.test/", &["one", "two"]));

        let mut oneshot = LivePageModel::new();
        oneshot.seed(daemon.snapshot());
        oneshot.ingest(page("https://x.test/", &["one", "two"]));
        oneshot.ingest(page("https://y.test/", &["alpha", "beta"]));
        let alpha = ref_of(&oneshot, "alpha");

        daemon.seed(oneshot.snapshot());
        daemon.ingest(page("https://y.test/", &["alpha", "beta"]));
        assert_eq!(daemon.element(&alpha).unwrap().raw.name, "alpha");
        assert!(daemon.nav_retired_lookup(&alpha).is_none());
    }

    /// A daemon and a one-shot command on one tab: the daemon takes over the
    /// table the other process left, instead of answering "unknown ref" for
    /// what that process printed (or, before the floor existed, minting the
    /// same number for another element).
    #[test]
    fn a_long_lived_model_adopts_the_table_another_process_left() {
        let mut daemon = LivePageModel::new();
        daemon.ingest(page("https://x.test/", &["one", "two"]));
        let handed_over = daemon.snapshot();

        // The one-shot command: resumes, sees a new element, prints its ref.
        let mut oneshot = LivePageModel::new();
        oneshot.seed(handed_over);
        oneshot.ingest(page("https://x.test/", &["one", "two", "three"]));
        let three = ref_of(&oneshot, "three");
        let left_behind = oneshot.snapshot();

        // The daemon alone would know nothing of it.
        assert!(daemon.element(&three).is_none());
        // What `refs_sync` does: adopt, then read the page again.
        daemon.seed(left_behind.clone());
        daemon.ingest(page("https://x.test/", &["one", "two", "three"]));
        assert_eq!(daemon.element(&three).unwrap().raw.name, "three");
        assert_eq!(ref_of(&daemon, "one"), ref_of(&oneshot, "one"));
        assert_eq!(digest(&daemon.snapshot()), digest(&left_behind));
        // And its next mint is past everything the other process used.
        daemon.ingest(page("https://x.test/", &["one", "two", "three", "four"]));
        assert_eq!(
            ref_number_of(&ref_of(&daemon, "four")),
            left_behind.next_ref
        );
    }

    /// A file is not trusted to be coherent: a counter behind its own refs
    /// would mint a ref that is already taken.
    #[test]
    fn a_counter_behind_its_refs_cannot_mint_a_taken_ref() {
        let snap = RefSnapshot {
            next_ref: 2,
            refs: vec![("e7".into(), "link:a".into(), "link".into(), "a".into(), 0)],
            graveyard: vec![("e9".into(), "link:b".into(), "link".into(), "b".into())],
            url: String::new(),
        };
        assert_eq!(snap.floor(), 10);
        let mut lpm = LivePageModel::new();
        lpm.seed(snap);
        lpm.ingest(page("https://x.test/", &["a", "c"]));
        assert_eq!(ref_of(&lpm, "a"), "e7");
        assert_eq!(ref_of(&lpm, "c"), "e10");

        // A second process minted past this one: its numbers stay its own.
        lpm.raise_ref_floor(40);
        lpm.raise_ref_floor(12);
        lpm.ingest(page("https://x.test/", &["a", "c", "d"]));
        assert_eq!(ref_of(&lpm, "d"), "e40");
    }

    /// Two processes on one tab at the same moment both mint from the counter
    /// they read. The second to save sees the first one's table on disk and
    /// moves its own counter past it, so the clash cannot repeat.
    #[test]
    fn a_table_written_during_a_command_is_noticed_and_never_minted_into() {
        let mut start = LivePageModel::new();
        start.ingest(page("https://x.test/", &["one"]));
        let read_by_both = start.snapshot();
        let last_seen = digest(&read_by_both);

        // Nobody else wrote: nothing to say.
        assert_eq!(
            written_meanwhile(Some(read_by_both.clone()), last_seen),
            None
        );
        assert_eq!(written_meanwhile(None, last_seen), None);

        // The other process saw three new elements and saved first.
        let mut other = LivePageModel::new();
        other.seed(read_by_both.clone());
        other.ingest(page("https://x.test/", &["one", "a", "b", "c"]));
        let theirs = other.snapshot();

        let mut mine = LivePageModel::new();
        mine.seed(read_by_both);
        mine.ingest(page("https://x.test/", &["one", "mine"]));
        let floor = written_meanwhile(Some(theirs.clone()), last_seen).expect("noticed");
        assert_eq!(floor, theirs.next_ref);
        mine.raise_ref_floor(floor);
        mine.ingest(page("https://x.test/", &["one", "mine", "later"]));
        assert!(
            ref_number_of(&ref_of(&mine, "later")) >= theirs.next_ref,
            "a number the other process printed is not minted again"
        );
    }

    /// A page that never changes is never rewritten; in daily use its table
    /// must not age out like the table of a closed tab.
    #[test]
    fn a_table_in_use_is_not_pruned_as_a_closed_tabs() {
        let dir = scratch("stamp");
        let mut lpm = LivePageModel::new();
        lpm.ingest(page("https://x.test/", &["one"]));
        let snap = lpm.snapshot();
        let eight_days_ago = std::time::SystemTime::now() - Duration::from_secs(8 * 24 * 3600);
        for tab in ["INUSE", "CLOSED"] {
            save(&dir, tab, &snap).unwrap();
        }
        for tab in ["INUSE", "CLOSED"] {
            std::fs::File::options()
                .write(true)
                .open(dir.join(format!("{tab}.json")))
                .unwrap()
                .set_modified(eight_days_ago)
                .unwrap();
        }
        // What resuming a tab does, then a new tab's first save prunes.
        stamp(&dir, "INUSE");
        save(&dir, "NEW", &snap).unwrap();
        assert_eq!(load(&dir, "INUSE"), Some(snap.clone()));
        assert_eq!(load(&dir, "CLOSED"), None);
        assert_eq!(load(&dir, "NEW"), Some(snap));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_store_is_per_tab_private_and_survives_garbage() {
        let dir = scratch("store");
        let mut lpm = LivePageModel::new();
        lpm.ingest(page("https://x.test/", &["one"]));
        let snap = lpm.snapshot();
        save(&dir, "A1B2", &snap).unwrap();

        assert_eq!(load(&dir, "A1B2"), Some(snap.clone()));
        assert_eq!(load(&dir, "OTHER"), None, "another tab has its own table");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("A1B2.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // A target id is used as a file name only in the shape Chrome mints.
        for bad in ["", "../x", "a/b", "a.b", &"f".repeat(65)] {
            save(&dir, bad, &snap).unwrap();
            assert_eq!(load(&dir, bad), None, "{bad:?}");
        }
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        // Garbage on disk is no table, not an error.
        std::fs::write(dir.join("A1B2.json"), b"{not json").unwrap();
        assert_eq!(load(&dir, "A1B2"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
