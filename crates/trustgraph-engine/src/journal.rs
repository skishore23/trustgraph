//! The hash-chained event journal. `FileJournal` is the only code in the
//! engine crate that does I/O.
//!
//! One record per line. `hash` is the SHA-256 of the canonical encoding of
//! `(prev, seq, at, event)`, so any edit, reorder or gap breaks the chain.
//!
//! Limits: the chain detects edits, reordering and gaps inside the file. It
//! does NOT detect removal of trailing records or deletion of the whole file,
//! because there is no external head anchor. Protect the file with filesystem
//! permissions and keep it outside any agent workspace.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use trustgraph_core::digest;

use crate::event::Event;

/// `prev` of the first record.
pub const GENESIS_PREV: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// Longest accepted record line, in bytes, without the newline.
const MAX_LINE: usize = 1 << 20;
const FILE_NAME: &str = "journal.jsonl";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub seq: u64,
    pub at: u64,
    pub event: Event,
    pub prev: String,
    pub hash: String,
}

impl Record {
    /// The record that follows `prev` (`GENESIS_PREV` for the first one).
    pub fn seal(seq: u64, at: u64, event: Event, prev: &str) -> Self {
        let hash = Self::compute_hash(prev, seq, at, &event);
        Record {
            seq,
            at,
            event,
            prev: prev.to_string(),
            hash,
        }
    }

    /// Lowercase hex SHA-256 over the canonical encoding of the four fields.
    pub fn compute_hash(prev: &str, seq: u64, at: u64, event: &Event) -> String {
        #[derive(Serialize)]
        struct Body<'a> {
            prev: &'a str,
            seq: u64,
            at: u64,
            event: &'a Event,
        }
        digest::structured(
            "engine-journal-v1",
            &Body {
                prev,
                seq,
                at,
                event,
            },
        )
        .0
    }

    fn encode(&self) -> Result<Vec<u8>, JournalError> {
        let mut line =
            serde_json::to_vec(self).map_err(|e| JournalError::Io(format!("encode: {e}")))?;
        if line.len() > MAX_LINE {
            return Err(JournalError::Io(format!(
                "record is {} bytes; the limit is {MAX_LINE}",
                line.len()
            )));
        }
        line.push(b'\n');
        Ok(line)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum JournalError {
    #[error("journal I/O error: {0}")]
    Io(String),
    #[error("journal is corrupt at record {seq}: {reason}")]
    Corrupt { seq: u64, reason: String },
    #[error("journal is locked by another process")]
    Locked,
}

fn io(error: std::io::Error) -> JournalError {
    JournalError::Io(error.to_string())
}

pub trait Journal {
    /// Every durable record, in order, verified.
    fn load(&mut self) -> Result<Vec<Record>, JournalError>;
    /// Make `record` durable. It must continue the chain.
    fn append(&mut self, record: &Record) -> Result<(), JournalError>;
}

/// Where the chain stands: the next `seq` and `prev` a record must carry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Chain {
    last_seq: u64,
    last_hash: String,
}

impl Chain {
    fn new() -> Self {
        Chain {
            last_seq: 0,
            last_hash: GENESIS_PREV.to_string(),
        }
    }

    /// Check `record` continues the chain and its hash is right, then advance.
    fn accept(&mut self, record: &Record) -> Result<(), JournalError> {
        let corrupt = |reason: String| JournalError::Corrupt {
            seq: record.seq,
            reason,
        };
        if record.seq != self.last_seq + 1 {
            return Err(corrupt(format!(
                "expected seq {}, found {}",
                self.last_seq + 1,
                record.seq
            )));
        }
        if record.prev != self.last_hash {
            return Err(corrupt("`prev` does not match the previous hash".into()));
        }
        if record.hash != Record::compute_hash(&record.prev, record.seq, record.at, &record.event) {
            return Err(corrupt("`hash` does not match the record".into()));
        }
        self.last_seq = record.seq;
        self.last_hash = record.hash.clone();
        Ok(())
    }
}

/// A journal in one directory: `journal.jsonl`, one JSON record per line,
/// held under an exclusive lock for the journal's lifetime.
#[derive(Debug)]
pub struct FileJournal {
    path: PathBuf,
    file: File,
    chain: Chain,
    loaded: bool,
    /// Set when a failed write could not be undone, or its durability is
    /// unknown. Cleared only by a successful `load`.
    poisoned: bool,
}

const POISONED: &str = "journal poisoned: reload required";

impl FileJournal {
    pub fn open(dir: &Path) -> Result<Self, JournalError> {
        create_dir(dir)?;
        let path = dir.join(FILE_NAME);
        let existed = path.exists();
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).map_err(io)?;
        if !existed {
            sync_dir(dir)?;
        }
        if let Err(error) = file.try_lock_exclusive() {
            let contended = fs2::lock_contended_error().raw_os_error();
            let held = error.kind() == std::io::ErrorKind::WouldBlock
                || (contended.is_some() && error.raw_os_error() == contended);
            return Err(if held {
                JournalError::Locked
            } else {
                io(error)
            });
        }
        Ok(FileJournal {
            path,
            file,
            chain: Chain::new(),
            loaded: false,
            poisoned: false,
        })
    }

    /// Where the journal file lives.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn create_dir(dir: &Path) -> Result<(), JournalError> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    let existed = dir.exists();
    builder.create(dir).map_err(io)?;
    if !existed && let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        sync_dir(parent)?;
    }
    Ok(())
}

/// fsync a directory so a new entry in it is durable. Platforms that cannot
/// open or sync a directory are skipped; other failures are errors.
fn sync_dir(dir: &Path) -> Result<(), JournalError> {
    #[cfg(unix)]
    {
        use std::io::ErrorKind;
        match File::open(dir).and_then(|d| d.sync_all()) {
            Ok(()) => {}
            Err(e) if matches!(e.kind(), ErrorKind::InvalidInput | ErrorKind::Unsupported) => {}
            Err(e) => return Err(io(e)),
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

impl Journal for FileJournal {
    fn load(&mut self) -> Result<Vec<Record>, JournalError> {
        let mut data = Vec::new();
        self.file.seek(SeekFrom::Start(0)).map_err(io)?;
        self.file.read_to_end(&mut data).map_err(io)?;

        let mut chain = Chain::new();
        let mut records = Vec::new();
        let mut good_len = 0usize;
        let mut rest: &[u8] = &data;
        let mut line_no = 0u64;
        while !rest.is_empty() {
            line_no += 1;
            let (line, complete, next) = match rest.iter().position(|b| *b == b'\n') {
                Some(i) => (&rest[..i], true, &rest[i + 1..]),
                None => (rest, false, &rest[rest.len()..]),
            };
            if line.len() > MAX_LINE {
                return Err(JournalError::Corrupt {
                    seq: line_no,
                    reason: format!("line is longer than {MAX_LINE} bytes"),
                });
            }
            if !complete {
                // Only an unterminated final line is a torn tail.
                break;
            }
            let record =
                serde_json::from_slice::<Record>(line).map_err(|error| JournalError::Corrupt {
                    seq: line_no,
                    reason: format!("line {line_no} is unparseable: {error}"),
                })?;
            chain.accept(&record).map_err(|e| match e {
                JournalError::Corrupt { reason, .. } => JournalError::Corrupt {
                    seq: line_no,
                    reason: format!("line {line_no}: {reason}"),
                },
                other => other,
            })?;
            good_len += line.len() + 1;
            records.push(record);
            rest = next;
        }
        if good_len < data.len() {
            self.file.set_len(good_len as u64).map_err(io)?;
            self.file.sync_data().map_err(io)?;
        }
        self.file.seek(SeekFrom::End(0)).map_err(io)?;
        self.chain = chain;
        self.loaded = true;
        self.poisoned = false;
        Ok(records)
    }

    fn append(&mut self, record: &Record) -> Result<(), JournalError> {
        if self.poisoned {
            return Err(JournalError::Io(POISONED.into()));
        }
        if !self.loaded {
            if self.file.metadata().map_err(io)?.len() > 0 {
                return Err(JournalError::Io("journal not loaded".into()));
            }
            self.loaded = true;
        }
        let mut next = self.chain.clone();
        next.accept(record)?;
        let line = record.encode()?;
        let start = self.file.seek(SeekFrom::End(0)).map_err(io)?;
        let written = self.file.write_all(&line);
        let synced = if written.is_ok() {
            self.file.sync_data()
        } else {
            Ok(())
        };
        let file = &mut self.file;
        let outcome = settle(written, synced, || {
            file.set_len(start)?;
            file.sync_data()?;
            file.seek(SeekFrom::End(0)).map(|_| ())
        });
        match outcome {
            Settled::Done => {
                self.chain = next;
                Ok(())
            }
            Settled::Failed(error) => Err(io(error)),
            Settled::Poisoned(error) => {
                self.poisoned = true;
                Err(io(error))
            }
        }
    }
}

/// What a write attempt left behind.
#[derive(Debug)]
enum Settled {
    /// Written and durable.
    Done,
    /// Not written, and any partial line was removed: safe to continue.
    Failed(std::io::Error),
    /// A partial line may remain, or durability is unknown: reload first.
    Poisoned(std::io::Error),
}

/// Decide the state after a write. `rollback` removes a partial line.
fn settle(
    written: std::io::Result<()>,
    synced: std::io::Result<()>,
    rollback: impl FnOnce() -> std::io::Result<()>,
) -> Settled {
    match (written, synced) {
        (Err(error), _) => match rollback() {
            Ok(()) => Settled::Failed(error),
            Err(_) => Settled::Poisoned(error),
        },
        (Ok(()), Err(error)) => Settled::Poisoned(error),
        (Ok(()), Ok(())) => Settled::Done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail() -> std::io::Result<()> {
        Err(std::io::Error::other("boom"))
    }

    #[test]
    fn settle_poisons_unless_the_partial_write_was_undone() {
        assert!(matches!(settle(Ok(()), Ok(()), fail), Settled::Done));
        assert!(matches!(
            settle(fail(), Ok(()), || Ok(())),
            Settled::Failed(_)
        ));
        assert!(matches!(settle(fail(), Ok(()), fail), Settled::Poisoned(_)));
        assert!(matches!(
            settle(Ok(()), fail(), || Ok(())),
            Settled::Poisoned(_)
        ));
    }

    #[test]
    fn a_poisoned_journal_refuses_appends_until_load() {
        let dir = tempfile::tempdir().unwrap();
        let mut journal = FileJournal::open(dir.path()).unwrap();
        journal.poisoned = true;
        let record = Record::seal(1, 0, Event::Genesis { policy: "x".into() }, GENESIS_PREV);
        assert_eq!(
            journal.append(&record),
            Err(JournalError::Io(POISONED.into()))
        );
        journal.load().unwrap();
        journal.append(&record).unwrap();
    }
}

/// An in-memory journal for the simulator and tests.
#[derive(Debug, Clone, Default)]
pub struct MemoryJournal {
    records: Vec<Record>,
    fail_next: bool,
}

impl MemoryJournal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make exactly the next `append` fail with `Io`, storing nothing.
    pub fn fail_next_append(&mut self) {
        self.fail_next = true;
    }

    /// Drop the last record, as if its write never became durable.
    pub fn tear_tail(&mut self) {
        self.records.pop();
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    fn chain(&self) -> Chain {
        match self.records.last() {
            Some(last) => Chain {
                last_seq: last.seq,
                last_hash: last.hash.clone(),
            },
            None => Chain::new(),
        }
    }
}

impl Journal for MemoryJournal {
    fn load(&mut self) -> Result<Vec<Record>, JournalError> {
        let mut chain = Chain::new();
        for record in &self.records {
            chain.accept(record)?;
        }
        Ok(self.records.clone())
    }

    fn append(&mut self, record: &Record) -> Result<(), JournalError> {
        if std::mem::take(&mut self.fail_next) {
            return Err(JournalError::Io("injected append failure".into()));
        }
        self.chain().accept(record)?;
        record.encode()?;
        self.records.push(record.clone());
        Ok(())
    }
}
