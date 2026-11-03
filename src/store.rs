//! Atomic file-backed persistence for votes, corpus, stance, and the ledger.
//!
//! Layout under the data dir:
//!   votes/<id>.json          one `VoteRecord` per file
//!   corpus/<property>/<msg_id>.json  one `CorpusItem` per file
//!   processed.json           `{schema, items: {msg_id: disposition}}`
//!   verifications.json       `{schema, items: {item: Verification}}`
//!   stance.json              `StanceFile`
//!   ledger.jsonl             one `LedgerEntry` JSON object per line, append-only
//!   scan.json                `{schema, at}` of the last completed `scan`

use std::collections::BTreeMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use pk_cli_core::CliError;
use serde::{Deserialize, Serialize};

use crate::model::{LedgerEntry, StanceFile, VoteRecord};

fn default_corpus_item_schema() -> String {
    "corpus-item/v1".into()
}

/// A single item of governance-related email correspondence, filed under a
/// property for later corpus-driven recommendations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusItem {
    #[serde(default = "default_corpus_item_schema")]
    pub schema: String,
    pub property: String,
    pub msg_id: String,
    pub date: DateTime<Utc>,
    pub subject: String,
    /// update|result
    pub kind: String,
    pub body: String,
}

impl CorpusItem {
    pub fn new(
        property: impl Into<String>,
        msg_id: impl Into<String>,
        subject: impl Into<String>,
        kind: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            schema: default_corpus_item_schema(),
            property: property.into(),
            msg_id: msg_id.into(),
            date: crate::clock::now(),
            subject: subject.into(),
            kind: kind.into(),
            body: body.into(),
        }
    }
}

/// A recorded ToS/compliance verification, e.g. from `verify record`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verification {
    pub recorded_at: DateTime<Utc>,
    pub note: Option<String>,
}

/// On-disk wrapper for `processed.json`: msg_id -> disposition.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProcessedFile {
    schema: String,
    items: BTreeMap<String, String>,
}

impl Default for ProcessedFile {
    fn default() -> Self {
        Self {
            schema: "processed/v1".into(),
            items: BTreeMap::new(),
        }
    }
}

/// On-disk wrapper for `verifications.json`: item -> `Verification`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VerificationsFile {
    schema: String,
    items: BTreeMap<String, Verification>,
}

impl Default for VerificationsFile {
    fn default() -> Self {
        Self {
            schema: "verifications/v1".into(),
            items: BTreeMap::new(),
        }
    }
}

/// On-disk wrapper for `scan.json`: when the last scan finished.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LastScanFile {
    schema: String,
    at: Option<DateTime<Utc>>,
    #[serde(default)]
    unread: u64,
    #[serde(default)]
    narrowed: bool,
}

/// What the last `scan` covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastScan {
    pub at: DateTime<Utc>,
    /// Emails it found and could not read. The next scan tries them again.
    pub unread: u64,
    /// `--query` or `--max` kept it from the whole mailbox query.
    pub narrowed: bool,
}

impl Default for LastScanFile {
    fn default() -> Self {
        Self {
            schema: "scan/v1".into(),
            at: None,
            unread: 0,
            narrowed: false,
        }
    }
}

/// Write `value` to `path` atomically: serialize to `<path>.tmp`, then
/// `fs::rename` over the destination.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), CliError> {
    let tmp = path.with_extension("tmp");
    let data = serde_json::to_vec_pretty(value)
        .map_err(|e| CliError::Other(format!("serializing {}: {e}", path.display())))?;
    std::fs::write(&tmp, data)
        .and_then(|_| std::fs::rename(&tmp, path))
        .map_err(|e| CliError::Other(format!("writing {}: {e}", path.display())))
}

/// Read and parse a JSON file, mapping "not found" to a caller-supplied
/// default rather than an error.
fn read_json_or_default<T: for<'de> Deserialize<'de> + Default>(
    path: &Path,
) -> Result<T, CliError> {
    match fs::read(path) {
        Ok(data) => serde_json::from_slice(&data)
            .map_err(|e| CliError::Other(format!("parsing {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(CliError::Other(format!("reading {}: {e}", path.display()))),
    }
}

fn resolve_data_dir() -> Result<PathBuf, CliError> {
    data_dir_from(|name| std::env::var(name).ok())
}

/// Where the data lives, given the environment. An empty variable counts as unset.
/// Native Windows has `USERPROFILE` and normally no `HOME`.
fn data_dir_from(env: impl Fn(&str) -> Option<String>) -> Result<PathBuf, CliError> {
    let set = |name: &str| env(name).filter(|v| !v.is_empty());
    if let Some(dir) = set("LOFTY_PM_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let base = match set("XDG_DATA_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => {
            let home = set("HOME").or_else(|| set("USERPROFILE")).ok_or_else(|| {
                CliError::Other("cannot locate home directory ($HOME unset)".into())
            })?;
            PathBuf::from(home).join(".local").join("share")
        }
    };
    Ok(base.join("lofty-pm"))
}

/// Atomic file-backed store rooted at a data directory.
pub struct Store {
    root: PathBuf,
}

/// The store as the MCP tools see it: reads alone, and opening it creates nothing,
/// so no tool call can change the data directory or bring one into being.
pub struct ReadStore(Store);

impl ReadStore {
    /// The default store's location, as [`Store::open`] resolves it.
    pub fn open() -> Result<ReadStore, CliError> {
        Ok(ReadStore::at(resolve_data_dir()?))
    }

    pub fn at(root: impl Into<PathBuf>) -> ReadStore {
        ReadStore(Store { root: root.into() })
    }

    pub fn votes(&self) -> Result<Vec<VoteRecord>, CliError> {
        self.0.list_votes()
    }

    pub fn corpus(&self) -> Result<Vec<CorpusItem>, CliError> {
        self.0.list_corpus(None)
    }

    pub fn ledger(&self) -> Result<Vec<LedgerEntry>, CliError> {
        self.0.read_ledger()
    }

    pub fn last_scan(&self) -> Result<Option<LastScan>, CliError> {
        self.0.last_scan()
    }
}

impl Store {
    /// Open the default store: `$LOFTY_PM_DATA_DIR` else
    /// `${XDG_DATA_HOME:-~/.local/share}/lofty-pm`.
    pub fn open() -> Result<Store, CliError> {
        let dir = resolve_data_dir()?;
        Store::open_at(dir)
    }

    /// Open (creating if needed) a store rooted at an explicit path.
    pub fn open_at(path: impl AsRef<Path>) -> Result<Store, CliError> {
        let root = path.as_ref().to_path_buf();
        fs::create_dir_all(root.join("votes"))
            .and_then(|_| fs::create_dir_all(root.join("corpus")))
            .map_err(|e| {
                CliError::Other(format!("creating data dirs under {}: {e}", root.display()))
            })?;
        Ok(Store { root })
    }

    fn votes_dir(&self) -> PathBuf {
        self.root.join("votes")
    }

    fn corpus_dir(&self) -> PathBuf {
        self.root.join("corpus")
    }

    fn processed_path(&self) -> PathBuf {
        self.root.join("processed.json")
    }

    fn verifications_path(&self) -> PathBuf {
        self.root.join("verifications.json")
    }

    fn stance_path(&self) -> PathBuf {
        self.root.join("stance.json")
    }

    fn ledger_path(&self) -> PathBuf {
        self.root.join("ledger.jsonl")
    }

    fn last_scan_path(&self) -> PathBuf {
        self.root.join("scan.json")
    }

    // -- votes ------------------------------------------------------------

    pub fn save_vote(&self, vote: &VoteRecord) -> Result<(), CliError> {
        let path = self.votes_dir().join(format!("{}.json", vote.id));
        write_json(&path, vote)
    }

    pub fn load_vote(&self, id: &str) -> Result<VoteRecord, CliError> {
        let path = self.votes_dir().join(format!("{id}.json"));
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CliError::NotFound(format!("vote {id}")))
            }
            Err(e) => return Err(CliError::Other(format!("reading {}: {e}", path.display()))),
        };
        serde_json::from_slice(&data)
            .map_err(|e| CliError::Other(format!("parsing {}: {e}", path.display())))
    }

    /// All votes, sorted by `created_at` descending (newest first).
    pub fn list_votes(&self) -> Result<Vec<VoteRecord>, CliError> {
        let dir = self.votes_dir();
        let mut votes = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            // Only a `ReadStore` can get here: `open_at` creates the directory.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(votes),
            Err(e) => return Err(CliError::Other(format!("reading {}: {e}", dir.display()))),
        };
        for entry in entries {
            let entry =
                entry.map_err(|e| CliError::Other(format!("reading {}: {e}", dir.display())))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let data = fs::read(&path)
                .map_err(|e| CliError::Other(format!("reading {}: {e}", path.display())))?;
            let vote: VoteRecord = serde_json::from_slice(&data)
                .map_err(|e| CliError::Other(format!("parsing {}: {e}", path.display())))?;
            votes.push(vote);
        }
        votes.sort_by_key(|v| std::cmp::Reverse(v.created_at));
        Ok(votes)
    }

    // -- processed index ----------------------------------------------------

    pub fn is_processed(&self, msg_id: &str) -> Result<bool, CliError> {
        let file: ProcessedFile = read_json_or_default(&self.processed_path())?;
        Ok(file.items.contains_key(msg_id))
    }

    pub fn mark_processed(&self, msg_id: &str, disposition: &str) -> Result<(), CliError> {
        let mut file: ProcessedFile = read_json_or_default(&self.processed_path())?;
        file.items
            .insert(msg_id.to_string(), disposition.to_string());
        write_json(&self.processed_path(), &file)
    }

    // -- ledger ---------------------------------------------------------------

    pub fn append_ledger(&self, entry: &LedgerEntry) -> Result<(), CliError> {
        let path = self.ledger_path();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| CliError::Other(format!("opening {}: {e}", path.display())))?;
        let line = serde_json::to_string(entry)
            .map_err(|e| CliError::Other(format!("serializing ledger entry: {e}")))?;
        writeln!(file, "{line}")
            .map_err(|e| CliError::Other(format!("writing {}: {e}", path.display())))
    }

    pub fn read_ledger(&self) -> Result<Vec<LedgerEntry>, CliError> {
        let path = self.ledger_path();
        let data = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(CliError::Other(format!("reading {}: {e}", path.display()))),
        };
        data.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line)
                    .map_err(|e| CliError::Other(format!("parsing {}: {e}", path.display())))
            })
            .collect()
    }

    // -- corpus -----------------------------------------------------------

    pub fn save_corpus_item(&self, item: &CorpusItem) -> Result<(), CliError> {
        let dir = self.corpus_dir().join(&item.property);
        fs::create_dir_all(&dir)
            .map_err(|e| CliError::Other(format!("creating {}: {e}", dir.display())))?;
        let path = dir.join(format!("{}.json", item.msg_id));
        write_json(&path, item)
    }

    /// Corpus items, optionally filtered to a single property.
    pub fn list_corpus(&self, property: Option<&str>) -> Result<Vec<CorpusItem>, CliError> {
        let corpus_dir = self.corpus_dir();
        let prop_dirs: Vec<PathBuf> = match property {
            Some(p) => vec![corpus_dir.join(p)],
            None => {
                if !corpus_dir.exists() {
                    Vec::new()
                } else {
                    fs::read_dir(&corpus_dir)
                        .map_err(|e| {
                            CliError::Other(format!("reading {}: {e}", corpus_dir.display()))
                        })?
                        .filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|p| p.is_dir())
                        .collect()
                }
            }
        };

        let mut items = Vec::new();
        for dir in prop_dirs {
            if !dir.exists() {
                continue;
            }
            for entry in fs::read_dir(&dir)
                .map_err(|e| CliError::Other(format!("reading {}: {e}", dir.display())))?
            {
                let entry = entry
                    .map_err(|e| CliError::Other(format!("reading {}: {e}", dir.display())))?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let data = fs::read(&path)
                    .map_err(|e| CliError::Other(format!("reading {}: {e}", path.display())))?;
                let item: CorpusItem = serde_json::from_slice(&data)
                    .map_err(|e| CliError::Other(format!("parsing {}: {e}", path.display())))?;
                items.push(item);
            }
        }
        Ok(items)
    }

    // -- stance -------------------------------------------------------------

    pub fn load_stance(&self) -> Result<StanceFile, CliError> {
        read_json_or_default(&self.stance_path())
    }

    pub fn save_stance(&self, stance: &StanceFile) -> Result<(), CliError> {
        write_json(&self.stance_path(), stance)
    }

    // -- verifications --------------------------------------------------------

    pub fn load_verifications(&self) -> Result<BTreeMap<String, Verification>, CliError> {
        let file: VerificationsFile = read_json_or_default(&self.verifications_path())?;
        Ok(file.items)
    }

    pub fn save_verifications(
        &self,
        verifications: &BTreeMap<String, Verification>,
    ) -> Result<(), CliError> {
        let file = VerificationsFile {
            schema: "verifications/v1".into(),
            items: verifications.clone(),
        };
        write_json(&self.verifications_path(), &file)
    }

    // -- last scan ----------------------------------------------------------

    /// Record that a scan finished, so a reader can tell how fresh the store is.
    pub fn record_scan(&self, scan: LastScan) -> Result<(), CliError> {
        let file = LastScanFile {
            at: Some(scan.at),
            unread: scan.unread,
            narrowed: scan.narrowed,
            ..Default::default()
        };
        write_json(&self.last_scan_path(), &file)
    }

    pub fn last_scan(&self) -> Result<Option<LastScan>, CliError> {
        let file: LastScanFile = read_json_or_default(&self.last_scan_path())?;
        Ok(file.at.map(|at| LastScan {
            at,
            unread: file.unread,
            narrowed: file.narrowed,
        }))
    }

    // -- diagnostics ----------------------------------------------------------

    /// Write and immediately remove a small probe file at the store root.
    /// Confirms the data dir is actually writable beyond what `open()`'s
    /// `create_dir_all` proves (that call silently no-ops if the dirs
    /// already exist, even on a filesystem that's since gone read-only).
    /// Used by `init`'s `data_dir_writable` check.
    pub fn probe_writable(&self) -> Result<(), CliError> {
        let path = self.root.join(".write-probe");
        fs::write(&path, b"ok")
            .map_err(|e| CliError::Other(format!("writing {}: {e}", path.display())))?;
        fs::remove_file(&path)
            .map_err(|e| CliError::Other(format!("removing {}: {e}", path.display())))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Actor;

    #[test]
    fn a_read_store_creates_nothing_and_reads_a_missing_directory_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("never-created");
        let reads = ReadStore::at(&root);
        assert!(reads.votes().unwrap().is_empty());
        assert!(reads.corpus().unwrap().is_empty());
        assert!(reads.ledger().unwrap().is_empty());
        assert_eq!(reads.last_scan().unwrap(), None);
        assert!(!root.exists());
    }

    #[test]
    fn the_last_scan_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::open_at(tmp.path()).unwrap();
        assert_eq!(s.last_scan().unwrap(), None);
        let at = "2026-07-24T12:00:00Z".parse().unwrap();
        let scan = LastScan {
            at,
            unread: 2,
            narrowed: true,
        };
        s.record_scan(scan).unwrap();
        assert_eq!(s.last_scan().unwrap(), Some(scan));
    }

    #[test]
    fn vote_round_trip_and_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::open_at(tmp.path()).unwrap();
        let v = VoteRecord::new("v-abc", "elm-st-12", "s", "p", vec![]);
        s.save_vote(&v).unwrap();
        assert_eq!(s.load_vote("v-abc").unwrap().property, "elm-st-12");
        assert!(matches!(s.load_vote("nope"), Err(CliError::NotFound(_))));
    }

    #[test]
    fn load_vote_non_not_found_io_error_is_not_reported_as_not_found() {
        // Make the vote "file" a directory so `fs::read` fails with a
        // non-NotFound io::Error (kind is platform-dependent, but never
        // NotFound), and assert that error is NOT misclassified as
        // CliError::NotFound.
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::open_at(tmp.path()).unwrap();
        std::fs::create_dir(tmp.path().join("votes").join("v-broken.json")).unwrap();
        let err = s.load_vote("v-broken").unwrap_err();
        assert!(
            !matches!(err, CliError::NotFound(_)),
            "expected a non-NotFound error for an unreadable path, got {err:?}"
        );
        assert!(matches!(err, CliError::Other(_)));
    }

    #[test]
    fn processed_index_dedups() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::open_at(tmp.path()).unwrap();
        assert!(!s.is_processed("m1").unwrap());
        s.mark_processed("m1", "vote").unwrap();
        assert!(s.is_processed("m1").unwrap());
    }

    #[test]
    fn ledger_appends_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::open_at(tmp.path()).unwrap();
        for e in ["a", "b", "c"] {
            s.append_ledger(&LedgerEntry::event(None, Actor::Scan, e, None))
                .unwrap();
        }
        let read = s.read_ledger().unwrap();
        assert_eq!(read.len(), 3);
        assert_eq!(read[2].event, "c");
    }

    #[test]
    fn corpus_filters_by_property() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::open_at(tmp.path()).unwrap();
        s.save_corpus_item(&CorpusItem::new("elm-st-12", "m1", "subj", "update", "b"))
            .unwrap();
        s.save_corpus_item(&CorpusItem::new("oak-ave-9", "m2", "subj", "update", "b"))
            .unwrap();
        assert_eq!(s.list_corpus(Some("elm-st-12")).unwrap().len(), 1);
        assert_eq!(s.list_corpus(None).unwrap().len(), 2);
    }

    #[test]
    fn open_creates_votes_and_corpus_subdirs() {
        let tmp = tempfile::tempdir().unwrap();
        Store::open_at(tmp.path()).unwrap();
        assert!(tmp.path().join("votes").is_dir());
        assert!(tmp.path().join("corpus").is_dir());
    }

    #[test]
    fn stance_round_trip_and_default_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::open_at(tmp.path()).unwrap();
        let default = s.load_stance().unwrap();
        assert!(default.global.risk_tolerance.is_none());

        let mut stance = StanceFile {
            schema: "stance-file/v1".into(),
            ..Default::default()
        };
        stance.global.risk_tolerance = Some("moderate".into());
        s.save_stance(&stance).unwrap();
        assert_eq!(
            s.load_stance().unwrap().global.risk_tolerance.as_deref(),
            Some("moderate")
        );
    }

    #[test]
    fn verifications_round_trip_and_default_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::open_at(tmp.path()).unwrap();
        assert!(s.load_verifications().unwrap().is_empty());

        let mut verifications = BTreeMap::new();
        verifications.insert(
            "operating-agreement".to_string(),
            Verification {
                recorded_at: crate::clock::now(),
                note: Some("reviewed 2026-07-24".into()),
            },
        );
        s.save_verifications(&verifications).unwrap();
        let loaded = s.load_verifications().unwrap();
        assert_eq!(
            loaded["operating-agreement"].note.as_deref(),
            Some("reviewed 2026-07-24")
        );
    }

    #[test]
    fn writes_are_atomic_tmp_then_rename() {
        // write_json must not leave a stray .tmp file behind on success.
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::open_at(tmp.path()).unwrap();
        let v = VoteRecord::new("v-atomic", "elm-st-12", "s", "p", vec![]);
        s.save_vote(&v).unwrap();
        let vote_path = tmp.path().join("votes").join("v-atomic.json");
        let tmp_path = vote_path.with_extension("tmp");
        assert!(vote_path.exists());
        assert!(!tmp_path.exists());
    }

    #[test]
    fn open_respects_lofty_pm_data_dir_override() {
        // Serialize with other env-var-touching tests: none currently exist
        // in this module besides this one, so no lock is needed.
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("nested").join("data-dir");
        std::env::set_var("LOFTY_PM_DATA_DIR", &target);
        let result = Store::open();
        std::env::remove_var("LOFTY_PM_DATA_DIR");
        result.unwrap();
        assert!(target.join("votes").is_dir());
        assert!(target.join("corpus").is_dir());
    }

    /// `scan.json` from before these fields existed reads as a whole scan.
    #[test]
    fn a_scan_file_without_coverage_reads_as_covering_everything() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("scan.json"),
            r#"{"schema":"scan/v1","at":"2030-03-02T06:00:00Z"}"#,
        )
        .unwrap();
        let scan = ReadStore::at(tmp.path()).last_scan().unwrap().unwrap();
        assert_eq!((scan.unread, scan.narrowed), (0, false));
    }

    #[test]
    fn the_data_dir_falls_back_to_the_windows_home() {
        let from = |vars: &[(&str, &str)]| {
            let vars: Vec<(String, String)> = vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            data_dir_from(|name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
        };
        let under = |home: &str| {
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("lofty-pm")
        };
        assert_eq!(
            from(&[("USERPROFILE", "/profile")]).unwrap(),
            under("/profile")
        );
        assert_eq!(
            from(&[("HOME", ""), ("USERPROFILE", "/profile")]).unwrap(),
            under("/profile")
        );
        assert_eq!(
            from(&[("HOME", "/home"), ("USERPROFILE", "/profile")]).unwrap(),
            under("/home")
        );
        assert_eq!(
            from(&[("LOFTY_PM_DATA_DIR", "/picked"), ("HOME", "/home")]).unwrap(),
            PathBuf::from("/picked")
        );
        assert!(from(&[]).is_err());
    }
}
