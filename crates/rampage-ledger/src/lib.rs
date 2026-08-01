//! Durable append-only, hash-chained evidence ledger.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Mutex;
use thiserror::Error;

const GENESIS_HASH: &str = "GENESIS";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LedgerEvent {
    pub sequence: u64,
    pub recorded_at: DateTime<Utc>,
    pub event_type: String,
    pub subject_id: String,
    pub payload: Value,
    pub previous_hash: String,
    pub event_hash: String,
}

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("ledger sequence {sequence} has previous hash {actual}, expected {expected}")]
    BrokenLink {
        sequence: u64,
        expected: String,
        actual: String,
    },
    #[error("ledger sequence {sequence} hash does not match its contents")]
    HashMismatch { sequence: u64 },
    #[error("ledger lock is poisoned")]
    Poisoned,
    #[error("fencing epoch overflow for authority scope {0}")]
    FencingEpochOverflow(String),
}

pub struct Ledger {
    connection: Mutex<Connection>,
}

impl Ledger {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, LedgerError> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS ledger_events (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                recorded_at TEXT NOT NULL,
                event_type TEXT NOT NULL,
                subject_id TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                previous_hash TEXT NOT NULL,
                event_hash TEXT NOT NULL UNIQUE
            );
            CREATE INDEX IF NOT EXISTS idx_ledger_subject
                ON ledger_events(subject_id, sequence);
            CREATE TABLE IF NOT EXISTS authority_epochs (
                scope TEXT PRIMARY KEY,
                fencing_epoch INTEGER NOT NULL CHECK(fencing_epoch >= 0)
            );",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn in_memory() -> Result<Self, LedgerError> {
        Self::open(":memory:")
    }

    pub fn append<T: Serialize>(
        &self,
        event_type: &str,
        subject_id: &str,
        payload: &T,
    ) -> Result<LedgerEvent, LedgerError> {
        let mut connection = self.connection.lock().map_err(|_| LedgerError::Poisoned)?;
        let transaction = connection.transaction()?;
        let event = append_in_transaction(&transaction, event_type, subject_id, payload)?;
        transaction.commit()?;
        Ok(event)
    }

    /// Atomically advance and evidence a durable authority fencing epoch.
    ///
    /// Authority-revoking transitions call this before issuing any new authority. The epoch row
    /// and its hash-chained evidence event commit in the same SQLite transaction, so a crash cannot
    /// publish a new epoch without preserving the evidence that invalidates older leases.
    pub fn advance_fencing_epoch(&self, scope: &str) -> Result<u64, LedgerError> {
        let mut connection = self.connection.lock().map_err(|_| LedgerError::Poisoned)?;
        let transaction = connection.transaction()?;
        let current: Option<u64> = transaction
            .query_row(
                "SELECT fencing_epoch FROM authority_epochs WHERE scope = ?1",
                params![scope],
                |row| row.get(0),
            )
            .optional()?;
        let next = current
            .unwrap_or(0)
            .checked_add(1)
            .filter(|epoch| *epoch <= i64::MAX as u64)
            .ok_or_else(|| LedgerError::FencingEpochOverflow(scope.to_string()))?;
        transaction.execute(
            "INSERT INTO authority_epochs(scope, fencing_epoch) VALUES (?1, ?2)
             ON CONFLICT(scope) DO UPDATE SET fencing_epoch = excluded.fencing_epoch",
            params![scope, next],
        )?;
        append_in_transaction(
            &transaction,
            "authority.epoch.advanced",
            scope,
            &serde_json::json!({"fencing_epoch": next}),
        )?;
        transaction.commit()?;
        Ok(next)
    }

    pub fn current_fencing_epoch(&self, scope: &str) -> Result<u64, LedgerError> {
        let connection = self.connection.lock().map_err(|_| LedgerError::Poisoned)?;
        Ok(connection
            .query_row(
                "SELECT fencing_epoch FROM authority_epochs WHERE scope = ?1",
                params![scope],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    pub fn events(&self, after_sequence: u64, limit: u32) -> Result<Vec<LedgerEvent>, LedgerError> {
        let connection = self.connection.lock().map_err(|_| LedgerError::Poisoned)?;
        let mut statement = connection.prepare(
            "SELECT sequence, recorded_at, event_type, subject_id, payload_json,
                    previous_hash, event_hash
             FROM ledger_events WHERE sequence > ?1 ORDER BY sequence ASC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![after_sequence, limit.min(10_000)], |row| {
            let timestamp: String = row.get(1)?;
            let payload_json: String = row.get(4)?;
            Ok((
                row.get::<_, u64>(0)?,
                timestamp,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                payload_json,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        rows.map(|row| {
            let (
                sequence,
                timestamp,
                event_type,
                subject_id,
                payload_json,
                previous_hash,
                event_hash,
            ) = row?;
            let recorded_at = DateTime::parse_from_rfc3339(&timestamp)
                .map_err(|error| serde_json::Error::io(std::io::Error::other(error)))?
                .with_timezone(&Utc);
            Ok(LedgerEvent {
                sequence,
                recorded_at,
                event_type,
                subject_id,
                payload: serde_json::from_str(&payload_json)?,
                previous_hash,
                event_hash,
            })
        })
        .collect()
    }

    pub fn events_for_subject(
        &self,
        subject_id: &str,
        limit: u32,
    ) -> Result<Vec<LedgerEvent>, LedgerError> {
        let connection = self.connection.lock().map_err(|_| LedgerError::Poisoned)?;
        let mut statement = connection.prepare(
            "SELECT sequence, recorded_at, event_type, subject_id, payload_json,
                    previous_hash, event_hash
             FROM ledger_events WHERE subject_id = ?1 ORDER BY sequence ASC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![subject_id, limit.min(10_000)], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        rows.map(|row| {
            let (
                sequence,
                timestamp,
                event_type,
                subject_id,
                payload_json,
                previous_hash,
                event_hash,
            ) = row?;
            let recorded_at = DateTime::parse_from_rfc3339(&timestamp)
                .map_err(|error| serde_json::Error::io(std::io::Error::other(error)))?
                .with_timezone(&Utc);
            Ok(LedgerEvent {
                sequence,
                recorded_at,
                event_type,
                subject_id,
                payload: serde_json::from_str(&payload_json)?,
                previous_hash,
                event_hash,
            })
        })
        .collect()
    }

    pub fn verify(&self) -> Result<u64, LedgerError> {
        let mut expected_previous = GENESIS_HASH.to_string();
        let mut after_sequence = 0_u64;
        let mut verified = 0_u64;
        loop {
            let events = self.events(after_sequence, 10_000)?;
            if events.is_empty() {
                break;
            }
            after_sequence = events.last().expect("non-empty page").sequence;
            for event in &events {
                if event.previous_hash != expected_previous {
                    return Err(LedgerError::BrokenLink {
                        sequence: event.sequence,
                        expected: expected_previous,
                        actual: event.previous_hash.clone(),
                    });
                }
                let payload_json = serde_json::to_string(&event.payload)?;
                let expected_hash = calculate_hash(
                    event.sequence,
                    event.recorded_at,
                    &event.event_type,
                    &event.subject_id,
                    &payload_json,
                    &event.previous_hash,
                );
                if expected_hash != event.event_hash {
                    return Err(LedgerError::HashMismatch {
                        sequence: event.sequence,
                    });
                }
                expected_previous = event.event_hash.clone();
                verified += 1;
            }
        }
        Ok(verified)
    }
}

fn append_in_transaction<T: Serialize>(
    transaction: &Transaction<'_>,
    event_type: &str,
    subject_id: &str,
    payload: &T,
) -> Result<LedgerEvent, LedgerError> {
    let payload = serde_json::to_value(payload)?;
    let payload_json = serde_json::to_string(&payload)?;
    let recorded_at = Utc::now();
    let previous: Option<(u64, String)> = transaction
        .query_row(
            "SELECT sequence, event_hash FROM ledger_events ORDER BY sequence DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let sequence = previous.as_ref().map_or(1, |(sequence, _)| sequence + 1);
    let previous_hash = previous
        .map(|(_, hash)| hash)
        .unwrap_or_else(|| GENESIS_HASH.to_string());
    let event_hash = calculate_hash(
        sequence,
        recorded_at,
        event_type,
        subject_id,
        &payload_json,
        &previous_hash,
    );
    transaction.execute(
        "INSERT INTO ledger_events
         (recorded_at, event_type, subject_id, payload_json, previous_hash, event_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            recorded_at.to_rfc3339(),
            event_type,
            subject_id,
            payload_json,
            previous_hash,
            event_hash
        ],
    )?;
    Ok(LedgerEvent {
        sequence,
        recorded_at,
        event_type: event_type.to_string(),
        subject_id: subject_id.to_string(),
        payload,
        previous_hash,
        event_hash,
    })
}

fn calculate_hash(
    sequence: u64,
    recorded_at: DateTime<Utc>,
    event_type: &str,
    subject_id: &str,
    payload_json: &str,
    previous_hash: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(sequence.to_be_bytes());
    digest.update(recorded_at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true));
    digest.update(event_type.as_bytes());
    digest.update([0]);
    digest.update(subject_id.as_bytes());
    digest.update([0]);
    digest.update(payload_json.as_bytes());
    digest.update([0]);
    digest.update(previous_hash.as_bytes());
    hex::encode(digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn appends_and_verifies_chain() {
        let ledger = Ledger::in_memory().unwrap();
        let first = ledger
            .append("job.proposed", "job-1", &json!({"a": 1}))
            .unwrap();
        let second = ledger
            .append("job.admitted", "job-1", &json!({"node": "n1"}))
            .unwrap();
        assert_eq!(first.sequence, 1);
        assert_eq!(second.previous_hash, first.event_hash);
        assert_eq!(ledger.verify().unwrap(), 2);
    }

    #[test]
    fn verification_crosses_the_query_page_boundary() {
        let ledger = Ledger::in_memory().unwrap();
        for index in 0..10_005 {
            ledger.append("page.test", "subject", &index).unwrap();
        }
        assert_eq!(ledger.verify().unwrap(), 10_005);
    }

    #[test]
    fn fencing_epoch_is_durable_and_hash_chained() {
        let temp = std::env::temp_dir().join(format!(
            "rampage-ledger-fence-{}-{}.db",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        {
            let ledger = Ledger::open(&temp).unwrap();
            assert_eq!(ledger.current_fencing_epoch("controller").unwrap(), 0);
            assert_eq!(ledger.advance_fencing_epoch("controller").unwrap(), 1);
            assert_eq!(ledger.advance_fencing_epoch("controller").unwrap(), 2);
            assert_eq!(ledger.verify().unwrap(), 2);
        }
        {
            let reopened = Ledger::open(&temp).unwrap();
            assert_eq!(reopened.current_fencing_epoch("controller").unwrap(), 2);
            assert_eq!(reopened.advance_fencing_epoch("controller").unwrap(), 3);
            assert_eq!(reopened.verify().unwrap(), 3);
        }
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", temp.display(), suffix));
        }
    }
}
