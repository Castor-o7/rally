//! Backups on another machine (RAL-15): every record, as stored, kept beside the head it reached.
//!
//! A backup is a private directory holding `records.jsonl`, one exported record per line in order,
//! and `head.json`, the head those records reach with the time it was saved. Each pull rechecks the
//! local copy, then asks the service only for what's new, and checks that every new record chains
//! on from the saved head. If the service's ledger no longer extends the saved head, it was
//! rewritten on its host: the pull refuses, writes nothing, and says so. The saved head is also
//! what `rally check --expect` needs (RAL-14).

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::access::write_private;
use crate::store::{CHAIN_START, Exported, Head, chain_hash};

/// One page from the service's export.
#[derive(Clone, Debug, Deserialize)]
pub struct Page {
    pub records: Vec<Exported>,
    pub head: Head,
    pub next: Option<u64>,
}

/// The head a backup reached, and when it was saved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub seq: u64,
    pub hash: String,
    pub at_ms: i64,
}

/// What a pull did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Pulled {
    pub added: u64,
    pub head: Saved,
}

#[derive(Debug)]
pub enum BackupError {
    /// The service's ledger no longer extends the saved head. Nothing was written.
    Rewrite(String),
    /// The local copy doesn't recheck. Nothing was written.
    Damaged(String),
    /// The service couldn't be read. Nothing was written.
    Unreachable(String),
    Io(String),
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rewrite(why) => write!(
                f,
                "rewrite: the service's ledger no longer extends this backup's saved head, so it was \
                 changed on its host; nothing was written ({why})"
            ),
            Self::Damaged(why) => write!(
                f,
                "the backup itself is damaged; nothing was written ({why})"
            ),
            Self::Unreachable(why) => write!(
                f,
                "the service couldn't be read; nothing was written ({why})"
            ),
            Self::Io(why) => write!(f, "the backup couldn't be written: {why}"),
        }
    }
}

impl std::error::Error for BackupError {}

/// The most pages one pull reads, far past a ledger at its size limit.
const MAX_PAGES: usize = 100_000;

fn records_path(dir: &Path) -> PathBuf {
    dir.join("records.jsonl")
}

fn head_path(dir: &Path) -> PathBuf {
    dir.join("head.json")
}

/// The saved head, if this backup has one.
pub fn saved(dir: &Path) -> Result<Option<Saved>, BackupError> {
    match std::fs::read_to_string(head_path(dir)) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|_| BackupError::Damaged("head.json can't be read".to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(BackupError::Io(error.to_string())),
    }
}

/// Rechecks the local records and returns the head they reach, which must be the saved one.
fn recheck(dir: &Path) -> Result<Head, BackupError> {
    let damaged = |why: String| BackupError::Damaged(why);
    let saved = saved(dir)?;
    let mut head = Head {
        seq: 0,
        hash: CHAIN_START.to_owned(),
    };
    match std::fs::File::open(records_path(dir)) {
        Ok(file) => {
            for line in std::io::BufReader::new(file).lines() {
                let line = line.map_err(|e| damaged(e.to_string()))?;
                let record: Exported = serde_json::from_str(&line)
                    .map_err(|_| damaged(format!("line {} can't be read", head.seq + 1)))?;
                if record.seq != head.seq + 1
                    || chain_hash(&head.hash, record.seq, &record.record) != record.hash
                {
                    return Err(damaged(format!("record {} doesn't chain", head.seq + 1)));
                }
                head = Head {
                    seq: record.seq,
                    hash: record.hash,
                };
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(BackupError::Io(error.to_string())),
    }
    match saved {
        Some(saved) if saved.seq == head.seq && saved.hash.eq_ignore_ascii_case(&head.hash) => {}
        None if head.seq == 0 => {}
        Some(saved) => {
            return Err(damaged(format!(
                "head.json says record {}, but the records reach {}",
                saved.seq, head.seq
            )));
        }
        None => return Err(damaged("records.jsonl has no head.json".to_owned())),
    }
    Ok(head)
}

/// Pulls whatever is new into the backup at `dir`, asking `fetch` for the page after each record
/// number. Writes nothing unless every page checks.
pub fn pull(
    dir: &Path,
    now_ms: i64,
    mut fetch: impl FnMut(u64) -> Result<Page, String>,
) -> Result<Pulled, BackupError> {
    std::fs::create_dir_all(dir).map_err(|e| BackupError::Io(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| BackupError::Io(e.to_string()))?;
    }
    let start = recheck(dir)?;
    let mut head = start.clone();
    let mut new = Vec::new();
    for _ in 0..MAX_PAGES {
        let page = fetch(head.seq).map_err(BackupError::Unreachable)?;
        // The service's head can move between pages, but never back past what this backup holds.
        if page.head.seq < head.seq {
            return Err(BackupError::Rewrite(format!(
                "the service ends at record {} but this backup holds {}",
                page.head.seq, head.seq
            )));
        }
        if page.head.seq == head.seq && !page.head.hash.eq_ignore_ascii_case(&head.hash) {
            return Err(BackupError::Rewrite(format!(
                "record {} has a different hash on the service",
                head.seq
            )));
        }
        for record in page.records {
            if record.seq != head.seq + 1
                || chain_hash(&head.hash, record.seq, &record.record) != record.hash
            {
                return Err(BackupError::Rewrite(format!(
                    "record {} doesn't chain on from record {}",
                    record.seq, head.seq
                )));
            }
            head = Head {
                seq: record.seq,
                hash: record.hash.clone(),
            };
            new.push(record);
        }
        if page.next.is_none() {
            break;
        }
    }
    // Records first, then the head, so a crash between leaves records past the head, which the
    // next pull reports as damage rather than hiding.
    if !new.is_empty() {
        let mut text = String::new();
        for record in &new {
            text.push_str(
                &serde_json::to_string(record).map_err(|e| BackupError::Io(e.to_string()))?,
            );
            text.push('\n');
        }
        append_private(&records_path(dir), text.as_bytes())?;
    }
    let saved = Saved {
        seq: head.seq,
        hash: head.hash,
        at_ms: now_ms,
    };
    let temporary = dir.join("head.json.new");
    let json = serde_json::to_string(&saved).map_err(|e| BackupError::Io(e.to_string()))?;
    write_private(&temporary, json.as_bytes()).map_err(|e| BackupError::Io(e.to_string()))?;
    std::fs::rename(&temporary, head_path(dir)).map_err(|e| BackupError::Io(e.to_string()))?;
    Ok(Pulled {
        added: new.len() as u64,
        head: saved,
    })
}

fn append_private(path: &Path, bytes: &[u8]) -> Result<(), BackupError> {
    let io = |e: std::io::Error| BackupError::Io(e.to_string());
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(io)?;
    file.write_all(bytes).map_err(io)?;
    file.sync_all().map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chain of `n` records, as the service would export them.
    fn chain(n: u64) -> Vec<Exported> {
        let mut previous = CHAIN_START.to_owned();
        (1..=n)
            .map(|seq| {
                let record = format!("{{\"n\":{seq}}}");
                let hash = chain_hash(&previous, seq, &record);
                previous.clone_from(&hash);
                Exported { seq, record, hash }
            })
            .collect()
    }

    /// A service holding `records`, giving `per_page` at a time.
    fn service(records: Vec<Exported>, per_page: usize) -> impl FnMut(u64) -> Result<Page, String> {
        move |after| {
            let head = records.last().map_or_else(
                || Head {
                    seq: 0,
                    hash: CHAIN_START.to_owned(),
                },
                |last| Head {
                    seq: last.seq,
                    hash: last.hash.clone(),
                },
            );
            let page: Vec<Exported> = records
                .iter()
                .filter(|r| r.seq > after)
                .take(per_page)
                .cloned()
                .collect();
            let next = page
                .last()
                .filter(|last| last.seq < head.seq)
                .map(|last| last.seq);
            Ok(Page {
                records: page,
                head,
                next,
            })
        }
    }

    fn files(dir: &Path) -> (String, String) {
        (
            std::fs::read_to_string(records_path(dir)).unwrap_or_default(),
            std::fs::read_to_string(head_path(dir)).unwrap_or_default(),
        )
    }

    #[test]
    fn a_first_pull_takes_everything_and_a_second_only_whats_new() {
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("b");
        let pulled = pull(&backup, 1, service(chain(7), 3)).unwrap();
        assert_eq!((pulled.added, pulled.head.seq), (7, 7));
        assert_eq!(pulled.head.hash, chain(7)[6].hash);
        let pulled = pull(&backup, 2, service(chain(12), 3)).unwrap();
        assert_eq!((pulled.added, pulled.head.seq), (5, 12));
        let pulled = pull(&backup, 3, service(chain(12), 3)).unwrap();
        assert_eq!((pulled.added, pulled.head.at_ms), (0, 3));
        assert_eq!(files(&backup).0.lines().count(), 12);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&backup), 0o700);
            assert_eq!(mode(&records_path(&backup)), 0o600);
            assert_eq!(mode(&head_path(&backup)), 0o600);
        }
    }

    #[test]
    fn a_rewrite_on_the_host_is_refused_and_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("b");
        pull(&backup, 1, service(chain(5), 2)).unwrap();
        let before = files(&backup);

        // The newest records removed.
        let error = pull(&backup, 2, service(chain(3), 2)).unwrap_err();
        assert!(error.to_string().starts_with("rewrite"), "{error}");
        // An earlier record altered and the chain recomputed from there, then grown.
        let mut altered = chain(8);
        altered[1].record = "{\"n\":99}".to_owned();
        let mut previous = altered[0].hash.clone();
        for record in &mut altered[1..] {
            record.hash = chain_hash(&previous, record.seq, &record.record);
            previous.clone_from(&record.hash);
        }
        let error = pull(&backup, 3, service(altered.clone(), 2)).unwrap_err();
        assert!(error.to_string().starts_with("rewrite"), "{error}");
        // The same, cut back to the same length, so only the head's hash differs.
        let error = pull(&backup, 4, service(altered[..5].to_vec(), 2)).unwrap_err();
        assert!(error.to_string().starts_with("rewrite"), "{error}");
        assert_eq!(files(&backup), before, "nothing was written");
    }

    #[test]
    fn a_damaged_backup_is_refused_and_left_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("b");
        pull(&backup, 1, service(chain(4), 10)).unwrap();
        let (records, head) = files(&backup);
        let changed = records.replacen("{\\\"n\\\":2}", "{\\\"n\\\":5}", 1);
        assert_ne!(changed, records);
        std::fs::write(records_path(&backup), &changed).unwrap();
        let error = pull(&backup, 2, service(chain(6), 10)).unwrap_err();
        assert!(matches!(error, BackupError::Damaged(_)), "{error}");
        assert_eq!(files(&backup), (changed, head));
        std::fs::remove_file(head_path(&backup)).unwrap();
        assert!(matches!(
            pull(&backup, 3, service(chain(6), 10)),
            Err(BackupError::Damaged(_))
        ));
    }

    #[test]
    fn an_unreachable_service_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("b");
        pull(&backup, 1, service(chain(2), 10)).unwrap();
        let before = files(&backup);
        let error = pull(&backup, 2, |_| Err("connection refused".to_owned())).unwrap_err();
        assert!(matches!(error, BackupError::Unreachable(_)), "{error}");
        assert_eq!(files(&backup), before);
    }
}
