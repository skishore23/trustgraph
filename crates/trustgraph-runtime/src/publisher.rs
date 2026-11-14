use crate::{
    journal::{private_options, storage},
    model::Error,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    path::PathBuf,
};
use trustgraph_core::digest::{self, Digest};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payload {
    pub operation: String,
    pub destination: String,
    pub content: String,
    pub hash: Digest,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Acknowledgment {
    pub provider_id: String,
    pub payload: Payload,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    Rejected,
    Unknown,
}

/// An unknown result never permits an implicit resend. Lookup is read-only.
pub trait Publisher {
    fn simulated(&self) -> bool {
        true
    }
    fn send(&mut self, payload: &Payload) -> Result<(Outcome, Option<Acknowledgment>), Error>;
    fn lookup(&mut self, operation: &str) -> Result<Option<Acknowledgment>, Error>;
}

#[derive(Debug, Clone, Copy, Default)]
pub enum PublisherFault {
    #[default]
    None,
    Reject,
    AcceptThenTimeout,
    TimeoutBeforeAccept,
}

/// Deterministic demo provider. Persistence is optional, policy never is.
#[derive(Debug, Default, Clone)]
pub struct LocalPublisher {
    accepted: BTreeMap<String, Acknowledgment>,
    directory: Option<PathBuf>,
    pub next_fault: PublisherFault,
    pub sends: usize,
}
impl LocalPublisher {
    pub fn open(directory: PathBuf) -> Result<Self, Error> {
        fs::create_dir_all(&directory).map_err(storage)?;
        let mut publisher = Self {
            directory: Some(directory.clone()),
            ..Self::default()
        };
        for entry in fs::read_dir(directory).map_err(storage)? {
            let path = entry.map_err(storage)?.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let ack: Acknowledgment =
                    serde_json::from_slice(&fs::read(&path).map_err(storage)?).map_err(storage)?;
                if ack.payload.hash != digest::content(ack.payload.content.as_bytes())
                    || path.file_stem().and_then(|s| s.to_str())
                        != Some(
                            &digest::structured("publisher-operation", &ack.payload.operation).0,
                        )
                {
                    return Err(Error::Storage("publisher ledger mismatch".into()));
                }
                publisher
                    .accepted
                    .insert(ack.payload.operation.clone(), ack);
            }
        }
        Ok(publisher)
    }
    pub fn accepted(&self) -> &BTreeMap<String, Acknowledgment> {
        &self.accepted
    }
    fn accept(&mut self, payload: &Payload) -> Result<Acknowledgment, Error> {
        if let Some(existing) = self.accepted.get(&payload.operation) {
            return if existing.payload == *payload {
                Ok(existing.clone())
            } else {
                Err(Error::Conflict)
            };
        }
        let ack = Acknowledgment {
            provider_id: format!(
                "sim-{}",
                digest::structured("publisher-operation", &payload.operation).0
            ),
            payload: payload.clone(),
        };
        if let Some(dir) = &self.directory {
            let filename = digest::structured("publisher-operation", &payload.operation).0;
            let temp = dir.join(format!("{filename}.tmp"));
            let mut file = private_options()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temp)
                .map_err(storage)?;
            file.write_all(&serde_json::to_vec(&ack).map_err(storage)?)
                .and_then(|_| file.sync_all())
                .map_err(storage)?;
            fs::rename(temp, dir.join(format!("{filename}.json"))).map_err(storage)?;
            File::open(dir)
                .and_then(|f| f.sync_all())
                .map_err(storage)?;
        }
        self.accepted.insert(payload.operation.clone(), ack.clone());
        Ok(ack)
    }
}
impl Publisher for LocalPublisher {
    fn send(&mut self, payload: &Payload) -> Result<(Outcome, Option<Acknowledgment>), Error> {
        if payload.hash != digest::content(payload.content.as_bytes())
            || !(payload.destination.starts_with("sim://northstar/")
                || payload.destination.starts_with("sim://aster-advisory/")
                || payload.destination.starts_with("sim://bank/"))
        {
            return Err(Error::Invalid(
                "simulator accepts only configured synthetic destinations".into(),
            ));
        }
        self.sends += 1;
        let fault = std::mem::take(&mut self.next_fault);
        match fault {
            PublisherFault::Reject => Ok((Outcome::Rejected, None)),
            PublisherFault::TimeoutBeforeAccept => Ok((Outcome::Unknown, None)),
            PublisherFault::AcceptThenTimeout => {
                self.accept(payload)?;
                Ok((Outcome::Unknown, None))
            }
            PublisherFault::None => Ok((Outcome::Accepted, Some(self.accept(payload)?))),
        }
    }
    fn lookup(&mut self, operation: &str) -> Result<Option<Acknowledgment>, Error> {
        Ok(self.accepted.get(operation).cloned())
    }
}
