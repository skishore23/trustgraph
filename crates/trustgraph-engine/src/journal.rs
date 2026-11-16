//! The hash-chained event journal. `FileJournal` is the only code in the
//! engine crate that does I/O.
//!
//! One record per line. `hash` is the SHA-256 of the canonical encoding of
//! `(prev, seq, at, event)`, so any edit, reorder or gap breaks the chain.

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
}

impl FileJournal {
    pub fn open(dir: &Path) -> Result<Self, JournalError> {
        create_dir(dir)?;
        let path = dir.join(FILE_NAME);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).map_err(io)?;
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
    builder.create(dir).map_err(io)
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
            let is_last = next.is_empty();
            let parsed = serde_json::from_slice::<Record>(line);
            match (complete, parsed) {
                (true, Ok(record)) => {
                    chain.accept(&record).map_err(|e| match e {
                        JournalError::Corrupt { reason, .. } => JournalError::Corrupt {
                            seq: line_no,
                            reason,
                        },
                        other => other,
                    })?;
                    good_len += line.len() + 1;
                    records.push(record);
                }
                // A torn tail: the last line is incomplete or unparseable.
                (false, _) | (true, Err(_)) if is_last => break,
                (_, Err(error)) => {
                    return Err(JournalError::Corrupt {
                        seq: line_no,
                        reason: format!("unparseable record: {error}"),
                    });
                }
                (false, Ok(_)) => unreachable!("an unterminated line is always the last"),
            }
            rest = next;
        }
        if good_len < data.len() {
            self.file.set_len(good_len as u64).map_err(io)?;
            self.file.sync_data().map_err(io)?;
        }
        self.file.seek(SeekFrom::End(0)).map_err(io)?;
        self.chain = chain;
        self.loaded = true;
        Ok(records)
    }

    fn append(&mut self, record: &Record) -> Result<(), JournalError> {
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
        let written = self
            .file
            .write_all(&line)
            .and_then(|()| self.file.sync_data());
        if let Err(error) = written {
            // Best effort: do not leave half a line behind.
            let _ = self.file.set_len(start);
            let _ = self.file.seek(SeekFrom::End(0));
            return Err(io(error));
        }
        self.chain = next;
        Ok(())
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
