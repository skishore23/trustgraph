use crate::model::*;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use trustgraph_core::digest::{self, Digest};

pub trait Journal {
    fn events(&self) -> Result<Vec<Event>, Error>;
    fn append(&mut self, event: &Event) -> Result<(), Error>;
}

#[derive(Debug, Default, Clone)]
pub struct MemoryJournal {
    pub entries: Vec<Event>,
    pub fault: Option<WriteFault>,
    pub writes_before_fault: usize,
}
#[derive(Debug, Clone, Copy)]
pub enum WriteFault {
    BeforeCommit,
    AfterCommit,
}
impl Journal for MemoryJournal {
    fn events(&self) -> Result<Vec<Event>, Error> {
        Ok(self.entries.clone())
    }
    fn append(&mut self, event: &Event) -> Result<(), Error> {
        let fault = if self.writes_before_fault > 0 {
            self.writes_before_fault -= 1;
            None
        } else {
            self.fault.take()
        };
        if matches!(fault, Some(WriteFault::BeforeCommit)) {
            return Err(Error::Storage("injected before commit".into()));
        }
        self.entries.push(event.clone());
        if matches!(fault, Some(WriteFault::AfterCommit)) {
            return Err(Error::Storage("injected lost commit acknowledgment".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    sequence: u64,
    previous: Digest,
    event: Event,
    hash: Digest,
}
impl Record {
    fn digest(&self) -> Digest {
        digest::structured(
            "journal-record",
            &(self.sequence, &self.previous, &self.event),
        )
    }
}

/// Private append-only journal with process ownership and fsync-before-effect.
/// A torn tail blocks startup; it is never silently treated as a completed write.
pub struct DiskJournal {
    _lock: File,
    file: File,
    events: Vec<Event>,
    head: Digest,
    directory: PathBuf,
}
impl DiskJournal {
    pub fn open(directory: &Path) -> Result<Self, Error> {
        fs::create_dir_all(directory).map_err(storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).map_err(storage)?;
        }
        let lock = private_options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("coordinator.lock"))
            .map_err(storage)?;
        lock.try_lock_exclusive()
            .map_err(|_| Error::Storage("another coordinator owns this directory".into()))?;
        let path = directory.join("journal.jsonl");
        let file = private_options()
            .append(true)
            .read(true)
            .create(true)
            .open(&path)
            .map_err(storage)?;
        // Sync directory metadata as well as file data so first creation is durable.
        File::open(directory)
            .and_then(|f| f.sync_all())
            .map_err(storage)?;
        let bytes = fs::read(&path).map_err(storage)?;
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            return Err(Error::Storage(
                "torn journal tail; recovery required".into(),
            ));
        }
        let mut events = vec![];
        let mut head = digest::structured("journal-genesis", &1);
        for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
            let record: Record = serde_json::from_slice(line).map_err(storage)?;
            if record.sequence != events.len() as u64
                || record.previous != head
                || record.hash != record.digest()
            {
                return Err(Error::Storage("journal hash chain mismatch".into()));
            }
            head = record.hash;
            events.push(record.event);
        }
        for event in &events {
            store_content(directory, event)?;
        }
        Ok(Self {
            _lock: lock,
            file,
            events,
            head,
            directory: directory.to_path_buf(),
        })
    }
}
impl Journal for DiskJournal {
    fn events(&self) -> Result<Vec<Event>, Error> {
        Ok(self.events.clone())
    }
    fn append(&mut self, event: &Event) -> Result<(), Error> {
        store_content(&self.directory, event)?;
        let mut record = Record {
            sequence: self.events.len() as u64,
            previous: self.head.clone(),
            event: event.clone(),
            hash: self.head.clone(),
        };
        record.hash = record.digest();
        let mut bytes = serde_json::to_vec(&record).map_err(storage)?;
        bytes.push(b'\n');
        self.file
            .write_all(&bytes)
            .and_then(|_| self.file.sync_all())
            .map_err(storage)?;
        self.head = record.hash;
        self.events.push(event.clone());
        Ok(())
    }
}
fn store_content(directory: &Path, event: &Event) -> Result<(), Error> {
    let artifacts: Vec<_> = match event {
        Event::Initialized { evidence } => evidence.artifacts.values().collect(),
        Event::DraftCreated { artifact, .. } => vec![artifact],
        _ => return Ok(()),
    };
    let blob_dir = directory.join("blobs");
    fs::create_dir_all(&blob_dir).map_err(storage)?;
    for artifact in artifacts {
        let bytes = artifact.content.as_bytes();
        let hash = digest::content(bytes);
        if hash != artifact.hash {
            return Err(Error::Storage("artifact content digest mismatch".into()));
        }
        let path = blob_dir.join(&hash.0);
        if path.exists() {
            if fs::read(&path).map_err(storage)? != bytes {
                return Err(Error::Storage("content-addressed blob was modified".into()));
            }
        } else {
            let mut blob = private_options()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(storage)?;
            blob.write_all(bytes)
                .and_then(|_| blob.sync_all())
                .map_err(storage)?;
        }
    }
    File::open(&blob_dir)
        .and_then(|f| f.sync_all())
        .map_err(storage)?;
    File::open(directory)
        .and_then(|f| f.sync_all())
        .map_err(storage)?;
    Ok(())
}
pub(crate) fn storage(error: impl std::fmt::Display) -> Error {
    Error::Storage(error.to_string())
}
pub(crate) fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}
