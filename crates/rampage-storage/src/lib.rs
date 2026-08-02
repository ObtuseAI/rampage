//! Encrypted, chunked content-addressed storage with explicit durability classes.

use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, OsRng, rand_core::RngCore},
};
use chrono::{DateTime, Utc};
use rampage_protocol::{ArtifactRefV1, MAX_ARTIFACT_TRANSFER_BYTES, StorageClass};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
use thiserror::Error;

const DEFAULT_CHUNK_SIZE: usize = 4 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_ENCRYPTED_CHUNK_BYTES: u64 = DEFAULT_CHUNK_SIZE as u64 + 64;

pub const RESUMABLE_CHUNK_SIZE: u32 = DEFAULT_CHUNK_SIZE as u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageLimits {
    pub cache_bytes: u64,
    pub scratch_bytes: u64,
    pub protected_bytes: u64,
}

impl StorageLimits {
    fn for_class(self, storage_class: StorageClass) -> u64 {
        match storage_class {
            StorageClass::Cache => self.cache_bytes,
            StorageClass::Scratch => self.scratch_bytes,
            StorageClass::Protected => self.protected_bytes,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResumablePutSpec {
    pub session_id: String,
    pub lease_id: String,
    pub authority_scope: String,
    pub fencing_epoch: u64,
    pub authority_nonce: String,
    pub expires_at: DateTime<Utc>,
    pub digest: String,
    pub size_bytes: u64,
    pub media_type: String,
    pub storage_class: StorageClass,
    pub required_replicas: u8,
    pub chunk_size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumablePutStatus {
    pub session_id: String,
    pub digest: String,
    pub size_bytes: u64,
    pub chunk_size: u32,
    pub chunk_count: u32,
    pub received_chunks: Vec<u32>,
    pub missing_chunks: Vec<u32>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactChunkLayout {
    pub digest: String,
    pub size_bytes: u64,
    pub chunk_count: u32,
}

#[derive(Debug, Clone)]
pub struct PutOptions {
    pub media_type: String,
    pub storage_class: StorageClass,
    pub required_replicas: u8,
}

impl Default for PutOptions {
    fn default() -> Self {
        Self {
            media_type: "application/octet-stream".into(),
            storage_class: StorageClass::Cache,
            required_replicas: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ChunkRecord {
    index: u32,
    nonce: String,
    plaintext_size: u64,
    ciphertext_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    schema: String,
    plaintext_digest: String,
    plaintext_size: u64,
    media_type: String,
    storage_class: StorageClass,
    required_replicas: u8,
    created_at: DateTime<Utc>,
    chunks: Vec<ChunkRecord>,
}

#[derive(Debug, Clone)]
struct ArtifactContract {
    artifact: ArtifactRefV1,
    required_replicas: u8,
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("manifest error: {0}")]
    Manifest(#[from] serde_json::Error),
    #[error("artifact manifest is malformed or outside storage bounds")]
    InvalidManifest,
    #[error("encryption or authentication failed")]
    Crypto,
    #[error("artifact {0} is not present")]
    NotFound(String),
    #[error("artifact digest mismatch")]
    DigestMismatch,
    #[error("artifact metadata conflicts with the existing content address")]
    ArtifactConflict,
    #[error("artifact exceeds the owner-contributed storage-class quota")]
    StorageQuotaExceeded,
    #[error("invalid artifact digest")]
    InvalidDigest,
    #[error("protected storage requires at least two declared replicas")]
    ProtectedRequiresReplication,
    #[error("invalid authority scope, nonce, or fencing epoch")]
    InvalidAuthority,
    #[error("authority lease has expired")]
    ExpiredAuthority,
    #[error("authority nonce has already been consumed")]
    ReplayedAuthorityNonce,
    #[error("stale authority fencing epoch {supplied}; current epoch is {current}")]
    StaleFencingEpoch { current: u64, supplied: u64 },
    #[error("invalid resumable transfer contract")]
    InvalidTransfer,
    #[error("resumable transfer {0} is not present")]
    TransferNotFound(String),
    #[error("resumable transfer conflicts with its durable session binding")]
    TransferConflict,
    #[error("resumable transfer lease has expired")]
    TransferExpired,
    #[error("chunk index is outside the transfer contract")]
    ChunkOutOfRange,
    #[error("chunk size does not match the transfer contract")]
    ChunkSizeMismatch,
    #[error("chunk digest does not match its payload")]
    ChunkDigestMismatch,
    #[error("resumable transfer is incomplete")]
    TransferIncomplete,
    #[error("storage lock is poisoned")]
    Poisoned,
}

pub struct CasStore {
    root: PathBuf,
    cipher: Aes256Gcm,
    index: Mutex<Connection>,
    mutations: Mutex<()>,
    chunk_size: usize,
    limits: Option<StorageLimits>,
}

impl CasStore {
    pub fn open(root: impl AsRef<Path>, encryption_key: [u8; 32]) -> Result<Self, StorageError> {
        Self::open_with_limits(root, encryption_key, None)
    }

    pub fn open_with_limits(
        root: impl AsRef<Path>,
        encryption_key: [u8; 32],
        limits: Option<StorageLimits>,
    ) -> Result<Self, StorageError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("tmp"))?;
        let index = Connection::open(root.join("index.db"))?;
        index.pragma_update(None, "journal_mode", "WAL")?;
        index.execute_batch(
            "CREATE TABLE IF NOT EXISTS artifacts (
                digest TEXT PRIMARY KEY,
                size_bytes INTEGER NOT NULL,
                media_type TEXT NOT NULL,
                storage_class TEXT NOT NULL,
                required_replicas INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS authority_fences (
                scope TEXT PRIMARY KEY,
                fencing_epoch INTEGER NOT NULL CHECK(fencing_epoch >= 0)
            );
            CREATE TABLE IF NOT EXISTS consumed_authority_nonces (
                scope TEXT NOT NULL,
                nonce TEXT NOT NULL,
                expires_at_millis INTEGER NOT NULL,
                PRIMARY KEY(scope, nonce)
            );
            CREATE INDEX IF NOT EXISTS idx_authority_nonce_expiry
                ON consumed_authority_nonces(expires_at_millis);
            CREATE TABLE IF NOT EXISTS resumable_put_sessions (
                session_id TEXT PRIMARY KEY,
                lease_id TEXT NOT NULL,
                authority_scope TEXT NOT NULL,
                fencing_epoch INTEGER NOT NULL CHECK(fencing_epoch >= 0),
                authority_nonce TEXT NOT NULL,
                expires_at_millis INTEGER NOT NULL,
                digest TEXT NOT NULL,
                size_bytes INTEGER NOT NULL CHECK(size_bytes > 0),
                media_type TEXT NOT NULL,
                storage_class TEXT NOT NULL,
                required_replicas INTEGER NOT NULL,
                chunk_size INTEGER NOT NULL CHECK(chunk_size > 0),
                chunk_count INTEGER NOT NULL CHECK(chunk_count > 0),
                state TEXT NOT NULL CHECK(state IN ('active', 'complete')),
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS resumable_put_chunks (
                session_id TEXT NOT NULL,
                chunk_index INTEGER NOT NULL CHECK(chunk_index >= 0),
                plaintext_size INTEGER NOT NULL CHECK(plaintext_size > 0),
                plaintext_digest TEXT NOT NULL,
                nonce TEXT NOT NULL,
                ciphertext_digest TEXT NOT NULL,
                PRIMARY KEY(session_id, chunk_index),
                FOREIGN KEY(session_id) REFERENCES resumable_put_sessions(session_id)
                    ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_resumable_put_digest
                ON resumable_put_sessions(digest, state);",
        )?;
        Ok(Self {
            root,
            cipher: Aes256Gcm::new_from_slice(&encryption_key).map_err(|_| StorageError::Crypto)?,
            index: Mutex::new(index),
            mutations: Mutex::new(()),
            chunk_size: DEFAULT_CHUNK_SIZE,
            limits,
        })
    }

    pub fn put(
        &self,
        plaintext: &[u8],
        options: PutOptions,
    ) -> Result<ArtifactRefV1, StorageError> {
        let _mutation_guard = self.mutations.lock().map_err(|_| StorageError::Poisoned)?;
        if options.storage_class == StorageClass::Protected && options.required_replicas < 2 {
            return Err(StorageError::ProtectedRequiresReplication);
        }
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(plaintext)));
        let object_dir = self.object_dir(&digest)?;
        if object_dir.exists() {
            match self.verify_object(&digest) {
                Ok(existing) => {
                    ensure_artifact_contract(
                        &existing,
                        plaintext.len() as u64,
                        &options.media_type,
                        options.storage_class,
                        options.required_replicas,
                    )?;
                    self.index_artifact(&existing.artifact, options.required_replicas)?;
                    return Ok(existing.artifact);
                }
                Err(error) if is_repairable_object_error(&error) => {
                    self.quarantine_object(&digest)?;
                }
                Err(error) => return Err(error),
            }
        }
        {
            let connection = self.index.lock().map_err(|_| StorageError::Poisoned)?;
            ensure_storage_capacity(
                &*connection,
                self.limits,
                options.storage_class,
                plaintext.len() as u64,
                Some(&digest),
            )?;
        }
        let staging = self.root.join("tmp").join(uuid_like_nonce());
        fs::create_dir_all(&staging)?;
        let mut records = Vec::new();
        for (index, chunk) in plaintext.chunks(self.chunk_size).enumerate() {
            let mut nonce = [0_u8; 12];
            OsRng.fill_bytes(&mut nonce);
            let ciphertext = self
                .cipher
                .encrypt((&nonce).into(), chunk)
                .map_err(|_| StorageError::Crypto)?;
            let ciphertext_digest = hex::encode(Sha256::digest(&ciphertext));
            durable_write_new(&staging.join(format!("{index:08}.chunk")), &ciphertext)?;
            records.push(ChunkRecord {
                index: index as u32,
                nonce: hex::encode(nonce),
                plaintext_size: chunk.len() as u64,
                ciphertext_digest,
            });
        }
        let manifest = Manifest {
            schema: "rampage.cas-manifest.v1".into(),
            plaintext_digest: digest.clone(),
            plaintext_size: plaintext.len() as u64,
            media_type: options.media_type.clone(),
            storage_class: options.storage_class,
            required_replicas: options.required_replicas,
            created_at: Utc::now(),
            chunks: records,
        };
        durable_write_new(
            &staging.join("manifest.json"),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
        sync_directory(&staging)?;
        if let Some(parent) = object_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::rename(&staging, &object_dir) {
            Ok(()) => {
                if let Some(parent) = object_dir.parent() {
                    sync_directory(parent)?;
                }
            }
            Err(error) if object_dir.exists() => {
                fs::remove_dir_all(&staging)?;
                let _ = error;
            }
            Err(error) => return Err(StorageError::Io(error)),
        }
        let artifact = ArtifactRefV1 {
            schema: "rampage.artifact-ref.v1".into(),
            digest,
            size_bytes: plaintext.len() as u64,
            media_type: manifest.media_type,
            storage_class: manifest.storage_class,
            encrypted: true,
        };
        let committed = self.verify_object(&artifact.digest)?;
        ensure_artifact_contract(
            &committed,
            artifact.size_bytes,
            &artifact.media_type,
            artifact.storage_class,
            options.required_replicas,
        )?;
        self.index_artifact(&committed.artifact, committed.required_replicas)?;
        Ok(committed.artifact)
    }

    pub fn get(&self, digest: &str) -> Result<Vec<u8>, StorageError> {
        let object_dir = self.object_dir(digest)?;
        let manifest_path = object_dir.join("manifest.json");
        if !manifest_path.is_file() {
            return Err(StorageError::NotFound(digest.into()));
        }
        let manifest = self.read_manifest(digest)?;
        let mut plaintext = Vec::with_capacity(manifest.plaintext_size as usize);
        for record in &manifest.chunks {
            let ciphertext = read_regular_file_bounded(
                &object_dir.join(format!("{:08}.chunk", record.index)),
                MAX_ENCRYPTED_CHUNK_BYTES,
            )?;
            if hex::encode(Sha256::digest(&ciphertext)) != record.ciphertext_digest {
                return Err(StorageError::DigestMismatch);
            }
            let nonce = hex::decode(&record.nonce).map_err(|_| StorageError::Crypto)?;
            let chunk = self
                .cipher
                .decrypt(nonce.as_slice().into(), ciphertext.as_ref())
                .map_err(|_| StorageError::Crypto)?;
            if chunk.len() as u64 != record.plaintext_size {
                return Err(StorageError::DigestMismatch);
            }
            plaintext.extend_from_slice(&chunk);
        }
        let actual = format!("sha256:{}", hex::encode(Sha256::digest(&plaintext)));
        if actual != digest || actual != manifest.plaintext_digest {
            return Err(StorageError::DigestMismatch);
        }
        Ok(plaintext)
    }

    pub fn head(&self, digest: &str) -> Result<ArtifactRefV1, StorageError> {
        let contract = artifact_contract_from_manifest(digest, self.read_manifest(digest)?);
        let connection = self.index.lock().map_err(|_| StorageError::Poisoned)?;
        let record = connection
            .query_row(
                "SELECT size_bytes, media_type, storage_class, required_replicas
                 FROM artifacts WHERE digest = ?1",
                params![digest],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, u8>(3)?,
                    ))
                },
            )
            .optional()?;
        let (size_bytes, media_type, storage_class, required_replicas) =
            record.ok_or_else(|| StorageError::NotFound(digest.into()))?;
        ensure_artifact_contract(
            &contract,
            size_bytes,
            &media_type,
            parse_storage_class(&storage_class)?,
            required_replicas,
        )?;
        Ok(contract.artifact)
    }

    /// Authenticate every encrypted chunk and recompute the complete content address.
    ///
    /// This is intentionally more expensive than `head`: callers use it before signing a fresh
    /// whole-artifact possession receipt.
    pub fn verify(&self, digest: &str) -> Result<ArtifactRefV1, StorageError> {
        let verified = self.verify_object(digest)?;
        self.head(digest)?;
        Ok(verified.artifact)
    }

    /// Begin or renew a restart-safe encrypted upload session.
    ///
    /// A retry with the exact lease binding is idempotent. A renewed Governor lease may resume
    /// the same immutable content contract at the same or a newer fencing epoch; its new nonce is
    /// consumed atomically with the session update.
    pub fn begin_resumable_put(
        &self,
        spec: &ResumablePutSpec,
    ) -> Result<ResumablePutStatus, StorageError> {
        let _mutation_guard = self.mutations.lock().map_err(|_| StorageError::Poisoned)?;
        validate_resumable_spec(spec)?;
        self.object_dir(&spec.digest)?;
        let now_millis = Utc::now().timestamp_millis();
        let expires_at_millis = spec.expires_at.timestamp_millis();
        if expires_at_millis <= now_millis {
            return Err(StorageError::TransferExpired);
        }
        let epoch =
            i64::try_from(spec.fencing_epoch).map_err(|_| StorageError::InvalidAuthority)?;
        let size_bytes =
            i64::try_from(spec.size_bytes).map_err(|_| StorageError::InvalidTransfer)?;
        let chunk_size = i64::from(spec.chunk_size);
        let chunk_count = i64::from(chunk_count(spec.size_bytes, spec.chunk_size)?);
        let mut connection = self.index.lock().map_err(|_| StorageError::Poisoned)?;
        let transaction = connection.transaction()?;
        let existing: Option<DurablePutSession> = transaction
            .query_row(
                "SELECT lease_id, authority_scope, fencing_epoch, authority_nonce,
                        expires_at_millis, digest, size_bytes, media_type, storage_class,
                        required_replicas, chunk_size, chunk_count, state
                 FROM resumable_put_sessions WHERE session_id = ?1",
                params![spec.session_id],
                durable_session_from_row,
            )
            .optional()?;
        if let Some(existing) = existing {
            ensure_session_contract(&existing, spec, chunk_count)?;
            if existing.state == "complete" {
                transaction.commit()?;
                drop(connection);
                return self.resumable_put_status(&spec.session_id);
            }
            if existing.lease_id != spec.lease_id
                || existing.authority_nonce != spec.authority_nonce
                || existing.fencing_epoch != epoch
                || existing.expires_at_millis != expires_at_millis
            {
                accept_authority_in_transaction(
                    &transaction,
                    &spec.authority_scope,
                    spec.fencing_epoch,
                    &spec.authority_nonce,
                    expires_at_millis,
                    now_millis,
                )?;
                transaction.execute(
                    "UPDATE resumable_put_sessions
                     SET lease_id = ?2, fencing_epoch = ?3, authority_nonce = ?4,
                         expires_at_millis = ?5, updated_at = ?6
                     WHERE session_id = ?1",
                    params![
                        spec.session_id,
                        spec.lease_id,
                        epoch,
                        spec.authority_nonce,
                        expires_at_millis,
                        Utc::now().to_rfc3339(),
                    ],
                )?;
            } else {
                ensure_epoch_is_current(&transaction, &spec.authority_scope, spec.fencing_epoch)?;
            }
        } else {
            ensure_storage_capacity(
                &transaction,
                self.limits,
                spec.storage_class,
                spec.size_bytes,
                Some(&spec.digest),
            )?;
            accept_authority_in_transaction(
                &transaction,
                &spec.authority_scope,
                spec.fencing_epoch,
                &spec.authority_nonce,
                expires_at_millis,
                now_millis,
            )?;
            transaction.execute(
                "INSERT INTO resumable_put_sessions
                 (session_id, lease_id, authority_scope, fencing_epoch, authority_nonce,
                  expires_at_millis, digest, size_bytes, media_type, storage_class,
                  required_replicas, chunk_size, chunk_count, state, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                         'active', ?14)",
                params![
                    spec.session_id,
                    spec.lease_id,
                    spec.authority_scope,
                    epoch,
                    spec.authority_nonce,
                    expires_at_millis,
                    spec.digest,
                    size_bytes,
                    spec.media_type,
                    storage_class_name(spec.storage_class),
                    spec.required_replicas,
                    chunk_size,
                    chunk_count,
                    Utc::now().to_rfc3339(),
                ],
            )?;
        }
        transaction.commit()?;
        fs::create_dir_all(self.transfer_dir(&spec.session_id)?)?;
        drop(connection);
        self.resumable_put_status(&spec.session_id)
    }

    /// Store one encrypted transfer chunk. Exact duplicates are accepted without rewriting.
    pub fn put_resumable_chunk(
        &self,
        session_id: &str,
        index: u32,
        declared_digest: &str,
        plaintext: &[u8],
    ) -> Result<ResumablePutStatus, StorageError> {
        let transfer_dir = self.transfer_dir(session_id)?;
        let mut connection = self.index.lock().map_err(|_| StorageError::Poisoned)?;
        let transaction = connection.transaction()?;
        let session: DurablePutSession = transaction
            .query_row(
                "SELECT lease_id, authority_scope, fencing_epoch, authority_nonce,
                        expires_at_millis, digest, size_bytes, media_type, storage_class,
                        required_replicas, chunk_size, chunk_count, state
                 FROM resumable_put_sessions WHERE session_id = ?1",
                params![session_id],
                durable_session_from_row,
            )
            .optional()?
            .ok_or_else(|| StorageError::TransferNotFound(session_id.into()))?;
        if session.state == "complete" {
            transaction.commit()?;
            drop(connection);
            return self.resumable_put_status(session_id);
        }
        if session.expires_at_millis <= Utc::now().timestamp_millis() {
            return Err(StorageError::TransferExpired);
        }
        ensure_epoch_is_current(
            &transaction,
            &session.authority_scope,
            session.fencing_epoch as u64,
        )?;
        let chunk_count =
            u32::try_from(session.chunk_count).map_err(|_| StorageError::InvalidTransfer)?;
        if index >= chunk_count {
            return Err(StorageError::ChunkOutOfRange);
        }
        let expected_size =
            expected_chunk_size(session.size_bytes as u64, session.chunk_size as u32, index)?;
        if plaintext.len() as u64 != expected_size {
            return Err(StorageError::ChunkSizeMismatch);
        }
        let actual_digest = sha256_digest(plaintext);
        if actual_digest != declared_digest {
            return Err(StorageError::ChunkDigestMismatch);
        }
        let existing: Option<(i64, String, String)> = transaction
            .query_row(
                "SELECT plaintext_size, plaintext_digest, ciphertext_digest
                 FROM resumable_put_chunks WHERE session_id = ?1 AND chunk_index = ?2",
                params![session_id, index],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let chunk_path = transfer_dir.join(format!("{index:08}.chunk"));
        if let Some((stored_size, stored_digest, ciphertext_digest)) = existing {
            if stored_size as u64 != expected_size || stored_digest != actual_digest {
                return Err(StorageError::TransferConflict);
            }
            if chunk_path.is_file()
                && sha256_hex(&read_regular_file_bounded(
                    &chunk_path,
                    MAX_ENCRYPTED_CHUNK_BYTES,
                )?) == ciphertext_digest
            {
                transaction.commit()?;
                drop(connection);
                return self.resumable_put_status(session_id);
            }
        }
        fs::create_dir_all(&transfer_dir)?;
        let mut nonce = [0_u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher
            .encrypt((&nonce).into(), plaintext)
            .map_err(|_| StorageError::Crypto)?;
        let ciphertext_digest = sha256_hex(&ciphertext);
        let temporary = transfer_dir.join(format!("{index:08}.{}.tmp", uuid_like_nonce()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&ciphertext)?;
        file.sync_all()?;
        drop(file);
        if chunk_path.exists() {
            fs::remove_file(&chunk_path)?;
        }
        fs::rename(&temporary, &chunk_path)?;
        sync_directory(&transfer_dir)?;
        transaction.execute(
            "INSERT INTO resumable_put_chunks
             (session_id, chunk_index, plaintext_size, plaintext_digest, nonce,
              ciphertext_digest)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(session_id, chunk_index) DO UPDATE SET
                plaintext_size = excluded.plaintext_size,
                plaintext_digest = excluded.plaintext_digest,
                nonce = excluded.nonce,
                ciphertext_digest = excluded.ciphertext_digest",
            params![
                session_id,
                index,
                expected_size,
                actual_digest,
                hex::encode(nonce),
                ciphertext_digest,
            ],
        )?;
        transaction.execute(
            "UPDATE resumable_put_sessions SET updated_at = ?2 WHERE session_id = ?1",
            params![session_id, Utc::now().to_rfc3339()],
        )?;
        transaction.commit()?;
        drop(connection);
        self.resumable_put_status(session_id)
    }

    pub fn resumable_put_status(
        &self,
        session_id: &str,
    ) -> Result<ResumablePutStatus, StorageError> {
        self.transfer_dir(session_id)?;
        let connection = self.index.lock().map_err(|_| StorageError::Poisoned)?;
        let session: (String, i64, i64, i64, String) = connection
            .query_row(
                "SELECT digest, size_bytes, chunk_size, chunk_count, state
                 FROM resumable_put_sessions WHERE session_id = ?1",
                params![session_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::TransferNotFound(session_id.into()))?;
        let mut received = connection
            .prepare(
                "SELECT chunk_index FROM resumable_put_chunks
                 WHERE session_id = ?1 ORDER BY chunk_index ASC",
            )?
            .query_map(params![session_id], |row| row.get::<_, u32>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        received.retain(|index| {
            self.transfer_dir(session_id)
                .is_ok_and(|dir| dir.join(format!("{index:08}.chunk")).is_file())
        });
        let count = u32::try_from(session.3).map_err(|_| StorageError::InvalidTransfer)?;
        let missing = (0..count)
            .filter(|index| received.binary_search(index).is_err())
            .collect();
        Ok(ResumablePutStatus {
            session_id: session_id.into(),
            digest: session.0,
            size_bytes: session.1 as u64,
            chunk_size: session.2 as u32,
            chunk_count: count,
            received_chunks: received,
            missing_chunks: missing,
            complete: session.4 == "complete",
        })
    }

    /// Verify every encrypted chunk and atomically promote the session into the CAS namespace.
    pub fn commit_resumable_put(&self, session_id: &str) -> Result<ArtifactRefV1, StorageError> {
        let _mutation_guard = self.mutations.lock().map_err(|_| StorageError::Poisoned)?;
        let transfer_dir = self.transfer_dir(session_id)?;
        let mut connection = self.index.lock().map_err(|_| StorageError::Poisoned)?;
        let transaction = connection.transaction()?;
        let session: DurablePutSession = transaction
            .query_row(
                "SELECT lease_id, authority_scope, fencing_epoch, authority_nonce,
                        expires_at_millis, digest, size_bytes, media_type, storage_class,
                        required_replicas, chunk_size, chunk_count, state
                 FROM resumable_put_sessions WHERE session_id = ?1",
                params![session_id],
                durable_session_from_row,
            )
            .optional()?
            .ok_or_else(|| StorageError::TransferNotFound(session_id.into()))?;
        let storage_class = parse_storage_class(&session.storage_class)?;
        let artifact = ArtifactRefV1 {
            schema: "rampage.artifact-ref.v1".into(),
            digest: session.digest.clone(),
            size_bytes: session.size_bytes as u64,
            media_type: session.media_type.clone(),
            storage_class,
            encrypted: true,
        };
        let object_dir = self.object_dir(&session.digest)?;
        if object_dir.exists() {
            match self.verify_object(&session.digest) {
                Ok(existing) => {
                    ensure_artifact_contract(
                        &existing,
                        artifact.size_bytes,
                        &artifact.media_type,
                        artifact.storage_class,
                        session.required_replicas as u8,
                    )?;
                    index_artifact_in_transaction(
                        &transaction,
                        &existing.artifact,
                        existing.required_replicas,
                    )?;
                    transaction.execute(
                        "UPDATE resumable_put_sessions SET state = 'complete', updated_at = ?2
                         WHERE session_id = ?1",
                        params![session_id, Utc::now().to_rfc3339()],
                    )?;
                    transaction.commit()?;
                    return Ok(existing.artifact);
                }
                Err(error) if is_repairable_object_error(&error) => {
                    self.quarantine_object(&session.digest)?;
                }
                Err(error) => return Err(error),
            }
        }
        if session.state == "complete" {
            return Err(StorageError::DigestMismatch);
        }
        if session.expires_at_millis <= Utc::now().timestamp_millis() {
            return Err(StorageError::TransferExpired);
        }
        ensure_epoch_is_current(
            &transaction,
            &session.authority_scope,
            session.fencing_epoch as u64,
        )?;
        let mut statement = transaction.prepare(
            "SELECT chunk_index, plaintext_size, plaintext_digest, nonce, ciphertext_digest
             FROM resumable_put_chunks WHERE session_id = ?1 ORDER BY chunk_index ASC",
        )?;
        let records = statement
            .query_map(params![session_id], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if records.len() != session.chunk_count as usize {
            return Err(StorageError::TransferIncomplete);
        }
        let mut plaintext_hasher = Sha256::new();
        let mut plaintext_size = 0_u64;
        let mut manifest_chunks = Vec::with_capacity(records.len());
        for (expected_index, record) in records.into_iter().enumerate() {
            if record.0 != expected_index as u32 {
                return Err(StorageError::TransferIncomplete);
            }
            let ciphertext = read_regular_file_bounded(
                &transfer_dir.join(format!("{:08}.chunk", record.0)),
                MAX_ENCRYPTED_CHUNK_BYTES,
            )?;
            if sha256_hex(&ciphertext) != record.4 {
                return Err(StorageError::DigestMismatch);
            }
            let nonce = hex::decode(&record.3).map_err(|_| StorageError::Crypto)?;
            let plaintext = self
                .cipher
                .decrypt(nonce.as_slice().into(), ciphertext.as_ref())
                .map_err(|_| StorageError::Crypto)?;
            if plaintext.len() as u64 != record.1 || sha256_digest(&plaintext) != record.2 {
                return Err(StorageError::DigestMismatch);
            }
            plaintext_size = plaintext_size.saturating_add(plaintext.len() as u64);
            plaintext_hasher.update(&plaintext);
            manifest_chunks.push(ChunkRecord {
                index: record.0,
                nonce: record.3,
                plaintext_size: record.1,
                ciphertext_digest: record.4,
            });
        }
        let actual_digest = format!("sha256:{}", hex::encode(plaintext_hasher.finalize()));
        if actual_digest != session.digest || plaintext_size != session.size_bytes as u64 {
            return Err(StorageError::DigestMismatch);
        }
        let manifest = Manifest {
            schema: "rampage.cas-manifest.v1".into(),
            plaintext_digest: session.digest.clone(),
            plaintext_size,
            media_type: session.media_type.clone(),
            storage_class,
            required_replicas: session.required_replicas as u8,
            created_at: Utc::now(),
            chunks: manifest_chunks,
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        let manifest_path = transfer_dir.join("manifest.json");
        if manifest_path.exists() {
            let existing: Manifest = serde_json::from_slice(&read_regular_file_bounded(
                &manifest_path,
                MAX_MANIFEST_BYTES,
            )?)?;
            validate_manifest(&session.digest, &existing)?;
            if existing.plaintext_digest != manifest.plaintext_digest
                || existing.plaintext_size != manifest.plaintext_size
                || existing.media_type != manifest.media_type
                || existing.storage_class != manifest.storage_class
                || existing.required_replicas != manifest.required_replicas
                || existing.chunks != manifest.chunks
            {
                return Err(StorageError::TransferConflict);
            }
        } else {
            durable_write_new(&manifest_path, &manifest_bytes)?;
        }
        sync_directory(&transfer_dir)?;
        if let Some(parent) = object_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::rename(&transfer_dir, &object_dir) {
            Ok(()) => {
                if let Some(parent) = object_dir.parent() {
                    sync_directory(parent)?;
                }
                if let Some(parent) = transfer_dir.parent() {
                    sync_directory(parent)?;
                }
            }
            Err(error) if object_dir.join("manifest.json").is_file() => {
                let _ = fs::remove_dir_all(&transfer_dir);
                let _ = error;
            }
            Err(error) => return Err(StorageError::Io(error)),
        }
        let committed = self.verify_object(&session.digest)?;
        ensure_artifact_contract(
            &committed,
            artifact.size_bytes,
            &artifact.media_type,
            artifact.storage_class,
            session.required_replicas as u8,
        )?;
        index_artifact_in_transaction(
            &transaction,
            &committed.artifact,
            committed.required_replicas,
        )?;
        transaction.execute(
            "UPDATE resumable_put_sessions SET state = 'complete', updated_at = ?2
             WHERE session_id = ?1",
            params![session_id, Utc::now().to_rfc3339()],
        )?;
        transaction.commit()?;
        Ok(committed.artifact)
    }

    pub fn chunk_layout(&self, digest: &str) -> Result<ArtifactChunkLayout, StorageError> {
        let manifest = self.read_manifest(digest)?;
        Ok(ArtifactChunkLayout {
            digest: manifest.plaintext_digest,
            size_bytes: manifest.plaintext_size,
            chunk_count: manifest.chunks.len() as u32,
        })
    }

    /// Read and authenticate one plaintext CAS chunk without allocating the full artifact.
    pub fn get_chunk(&self, digest: &str, index: u32) -> Result<Vec<u8>, StorageError> {
        let object_dir = self.object_dir(digest)?;
        let manifest = self.read_manifest(digest)?;
        let record = manifest
            .chunks
            .get(index as usize)
            .filter(|record| record.index == index)
            .ok_or(StorageError::ChunkOutOfRange)?;
        let ciphertext = read_regular_file_bounded(
            &object_dir.join(format!("{index:08}.chunk")),
            MAX_ENCRYPTED_CHUNK_BYTES,
        )?;
        if sha256_hex(&ciphertext) != record.ciphertext_digest {
            return Err(StorageError::DigestMismatch);
        }
        let nonce = hex::decode(&record.nonce).map_err(|_| StorageError::Crypto)?;
        let plaintext = self
            .cipher
            .decrypt(nonce.as_slice().into(), ciphertext.as_ref())
            .map_err(|_| StorageError::Crypto)?;
        if plaintext.len() as u64 != record.plaintext_size {
            return Err(StorageError::DigestMismatch);
        }
        Ok(plaintext)
    }

    /// Atomically consume one signed authority nonce while enforcing a monotonic fencing epoch.
    ///
    /// The state lives in the local CAS index beside encrypted artifact payloads, so replay and
    /// stale-epoch protection survive process restarts. Callers must verify the lease signature
    /// before invoking this method; this store deliberately does not possess the Governor's
    /// verification key.
    pub fn accept_authority(
        &self,
        scope: &str,
        fencing_epoch: u64,
        nonce: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        if scope.is_empty()
            || scope.len() > 128
            || nonce.is_empty()
            || nonce.len() > 128
            || !scope.is_ascii()
            || !nonce.is_ascii()
        {
            return Err(StorageError::InvalidAuthority);
        }
        let now_millis = Utc::now().timestamp_millis();
        let expires_at_millis = expires_at.timestamp_millis();
        if expires_at_millis <= now_millis {
            return Err(StorageError::ExpiredAuthority);
        }
        let mut connection = self.index.lock().map_err(|_| StorageError::Poisoned)?;
        let transaction = connection.transaction()?;
        accept_authority_in_transaction(
            &transaction,
            scope,
            fencing_epoch,
            nonce,
            expires_at_millis,
            now_millis,
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn read_manifest(&self, digest: &str) -> Result<Manifest, StorageError> {
        let object_dir = self.object_dir(digest)?;
        if fs::symlink_metadata(&object_dir)
            .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_dir())
        {
            return Err(StorageError::InvalidManifest);
        }
        let path = object_dir.join("manifest.json");
        if !path.is_file() {
            return Err(StorageError::NotFound(digest.into()));
        }
        let manifest: Manifest =
            serde_json::from_slice(&read_regular_file_bounded(&path, MAX_MANIFEST_BYTES)?)?;
        validate_manifest(digest, &manifest)?;
        Ok(manifest)
    }

    fn verify_object(&self, digest: &str) -> Result<ArtifactContract, StorageError> {
        let object_dir = self.object_dir(digest)?;
        let manifest = self.read_manifest(digest)?;
        let mut plaintext_hasher = Sha256::new();
        let mut plaintext_size = 0_u64;
        for record in &manifest.chunks {
            let ciphertext = read_regular_file_bounded(
                &object_dir.join(format!("{:08}.chunk", record.index)),
                MAX_ENCRYPTED_CHUNK_BYTES,
            )?;
            if sha256_hex(&ciphertext) != record.ciphertext_digest {
                return Err(StorageError::DigestMismatch);
            }
            let nonce = hex::decode(&record.nonce).map_err(|_| StorageError::Crypto)?;
            let plaintext = self
                .cipher
                .decrypt(nonce.as_slice().into(), ciphertext.as_ref())
                .map_err(|_| StorageError::Crypto)?;
            if plaintext.len() as u64 != record.plaintext_size {
                return Err(StorageError::DigestMismatch);
            }
            plaintext_size = plaintext_size
                .checked_add(plaintext.len() as u64)
                .ok_or(StorageError::InvalidManifest)?;
            plaintext_hasher.update(&plaintext);
        }
        let actual_digest = format!("sha256:{}", hex::encode(plaintext_hasher.finalize()));
        if actual_digest != digest || plaintext_size != manifest.plaintext_size {
            return Err(StorageError::DigestMismatch);
        }
        Ok(artifact_contract_from_manifest(digest, manifest))
    }

    fn index_artifact(
        &self,
        artifact: &ArtifactRefV1,
        required_replicas: u8,
    ) -> Result<(), StorageError> {
        let mut connection = self.index.lock().map_err(|_| StorageError::Poisoned)?;
        let transaction = connection.transaction()?;
        index_artifact_in_transaction(&transaction, artifact, required_replicas)?;
        transaction.commit()?;
        Ok(())
    }

    fn transfer_dir(&self, session_id: &str) -> Result<PathBuf, StorageError> {
        if session_id.len() != 32
            || !session_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(StorageError::InvalidTransfer);
        }
        Ok(self.root.join("tmp").join("transfers").join(session_id))
    }

    fn object_dir(&self, digest: &str) -> Result<PathBuf, StorageError> {
        let hash = digest
            .strip_prefix("sha256:")
            .filter(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            .ok_or(StorageError::InvalidDigest)?;
        Ok(self.root.join("objects").join(&hash[..2]).join(hash))
    }

    fn quarantine_object(&self, digest: &str) -> Result<(), StorageError> {
        let object_dir = self.object_dir(digest)?;
        match fs::symlink_metadata(&object_dir) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        let hash = digest
            .strip_prefix("sha256:")
            .ok_or(StorageError::InvalidDigest)?;
        let corrupt_root = self.root.join("tmp").join("corrupt");
        fs::create_dir_all(&corrupt_root)?;
        let target = corrupt_root.join(format!("{hash}.{}", uuid_like_nonce()));
        fs::rename(&object_dir, &target)?;
        sync_directory(&corrupt_root)?;
        if let Some(parent) = object_dir.parent() {
            sync_directory(parent)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct DurablePutSession {
    lease_id: String,
    authority_scope: String,
    fencing_epoch: i64,
    authority_nonce: String,
    expires_at_millis: i64,
    digest: String,
    size_bytes: i64,
    media_type: String,
    storage_class: String,
    required_replicas: i64,
    chunk_size: i64,
    chunk_count: i64,
    state: String,
}

fn durable_session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DurablePutSession> {
    Ok(DurablePutSession {
        lease_id: row.get(0)?,
        authority_scope: row.get(1)?,
        fencing_epoch: row.get(2)?,
        authority_nonce: row.get(3)?,
        expires_at_millis: row.get(4)?,
        digest: row.get(5)?,
        size_bytes: row.get(6)?,
        media_type: row.get(7)?,
        storage_class: row.get(8)?,
        required_replicas: row.get(9)?,
        chunk_size: row.get(10)?,
        chunk_count: row.get(11)?,
        state: row.get(12)?,
    })
}

fn validate_resumable_spec(spec: &ResumablePutSpec) -> Result<(), StorageError> {
    if spec.lease_id.is_empty()
        || spec.lease_id.len() > 128
        || !spec.lease_id.is_ascii()
        || spec.authority_scope.is_empty()
        || spec.authority_scope.len() > 128
        || !spec.authority_scope.is_ascii()
        || spec.authority_nonce.is_empty()
        || spec.authority_nonce.len() > 128
        || !spec.authority_nonce.is_ascii()
        || spec.media_type.is_empty()
        || spec.media_type.len() > 255
        || !spec.media_type.is_ascii()
        || spec.size_bytes == 0
        || spec.size_bytes > MAX_ARTIFACT_TRANSFER_BYTES
        || spec.chunk_size == 0
        || spec.chunk_size > RESUMABLE_CHUNK_SIZE
        || spec.required_replicas == 0
        || (spec.storage_class == StorageClass::Protected && spec.required_replicas < 2)
    {
        return Err(StorageError::InvalidTransfer);
    }
    if spec.session_id.len() != 32
        || !spec
            .session_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(StorageError::InvalidTransfer);
    }
    chunk_count(spec.size_bytes, spec.chunk_size)?;
    Ok(())
}

fn chunk_count(size_bytes: u64, chunk_size: u32) -> Result<u32, StorageError> {
    if size_bytes == 0 || chunk_size == 0 {
        return Err(StorageError::InvalidTransfer);
    }
    u32::try_from(size_bytes.div_ceil(u64::from(chunk_size)))
        .map_err(|_| StorageError::InvalidTransfer)
}

fn expected_chunk_size(size_bytes: u64, chunk_size: u32, index: u32) -> Result<u64, StorageError> {
    let count = chunk_count(size_bytes, chunk_size)?;
    if index >= count {
        return Err(StorageError::ChunkOutOfRange);
    }
    let offset = u64::from(index) * u64::from(chunk_size);
    Ok((size_bytes - offset).min(u64::from(chunk_size)))
}

fn ensure_session_contract(
    existing: &DurablePutSession,
    spec: &ResumablePutSpec,
    chunk_count: i64,
) -> Result<(), StorageError> {
    if existing.authority_scope != spec.authority_scope
        || existing.digest != spec.digest
        || existing.size_bytes != spec.size_bytes as i64
        || existing.media_type != spec.media_type
        || existing.storage_class != storage_class_name(spec.storage_class)
        || existing.required_replicas != i64::from(spec.required_replicas)
        || existing.chunk_size != i64::from(spec.chunk_size)
        || existing.chunk_count != chunk_count
    {
        return Err(StorageError::TransferConflict);
    }
    Ok(())
}

fn ensure_epoch_is_current(
    transaction: &rusqlite::Transaction<'_>,
    scope: &str,
    fencing_epoch: u64,
) -> Result<(), StorageError> {
    let current: Option<i64> = transaction
        .query_row(
            "SELECT fencing_epoch FROM authority_fences WHERE scope = ?1",
            params![scope],
            |row| row.get(0),
        )
        .optional()?;
    if current.is_some_and(|current| fencing_epoch < current as u64) {
        return Err(StorageError::StaleFencingEpoch {
            current: current.unwrap_or_default() as u64,
            supplied: fencing_epoch,
        });
    }
    Ok(())
}

fn accept_authority_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    scope: &str,
    fencing_epoch: u64,
    nonce: &str,
    expires_at_millis: i64,
    now_millis: i64,
) -> Result<(), StorageError> {
    ensure_epoch_is_current(transaction, scope, fencing_epoch)?;
    transaction.execute(
        "DELETE FROM consumed_authority_nonces WHERE expires_at_millis <= ?1",
        params![now_millis],
    )?;
    let inserted = transaction.execute(
        "INSERT OR IGNORE INTO consumed_authority_nonces(scope, nonce, expires_at_millis)
         VALUES (?1, ?2, ?3)",
        params![scope, nonce, expires_at_millis],
    )?;
    if inserted != 1 {
        return Err(StorageError::ReplayedAuthorityNonce);
    }
    let epoch = i64::try_from(fencing_epoch).map_err(|_| StorageError::InvalidAuthority)?;
    transaction.execute(
        "INSERT INTO authority_fences(scope, fencing_epoch) VALUES (?1, ?2)
         ON CONFLICT(scope) DO UPDATE SET fencing_epoch = MAX(fencing_epoch, excluded.fencing_epoch)",
        params![scope, epoch],
    )?;
    Ok(())
}

fn index_artifact_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    artifact: &ArtifactRefV1,
    required_replicas: u8,
) -> Result<(), StorageError> {
    transaction.execute(
        "INSERT OR IGNORE INTO artifacts
         (digest, size_bytes, media_type, storage_class, required_replicas, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            artifact.digest,
            artifact.size_bytes,
            artifact.media_type,
            storage_class_name(artifact.storage_class),
            required_replicas,
            Utc::now().to_rfc3339(),
        ],
    )?;
    let indexed = transaction.query_row(
        "SELECT size_bytes, media_type, storage_class, required_replicas
         FROM artifacts WHERE digest = ?1",
        params![artifact.digest],
        |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u8>(3)?,
            ))
        },
    )?;
    if indexed.0 != artifact.size_bytes
        || indexed.1 != artifact.media_type
        || indexed.2 != storage_class_name(artifact.storage_class)
        || indexed.3 != required_replicas
    {
        return Err(StorageError::ArtifactConflict);
    }
    Ok(())
}

trait StorageQuery {
    fn query_storage_total(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> rusqlite::Result<u64>;
}

impl StorageQuery for Connection {
    fn query_storage_total(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> rusqlite::Result<u64> {
        self.query_row(sql, params, |row| row.get(0))
    }
}

impl StorageQuery for rusqlite::Transaction<'_> {
    fn query_storage_total(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> rusqlite::Result<u64> {
        self.query_row(sql, params, |row| row.get(0))
    }
}

fn ensure_storage_capacity(
    query: &impl StorageQuery,
    limits: Option<StorageLimits>,
    storage_class: StorageClass,
    requested_bytes: u64,
    replacing_digest: Option<&str>,
) -> Result<(), StorageError> {
    let Some(limit) = limits.map(|limits| limits.for_class(storage_class)) else {
        return Ok(());
    };
    let class = storage_class_name(storage_class);
    let excluded = replacing_digest.unwrap_or("");
    let already_indexed = query.query_storage_total(
        "SELECT COUNT(*) FROM artifacts WHERE storage_class = ?1 AND digest = ?2",
        &[&class, &excluded],
    )? > 0;
    let used = query.query_storage_total(
        "SELECT COALESCE(SUM(size_bytes), 0) FROM artifacts
         WHERE storage_class = ?1 AND digest != ?2",
        &[&class, &excluded],
    )?;
    let reserved = query.query_storage_total(
        "SELECT COALESCE(SUM(size_bytes), 0) FROM resumable_put_sessions
         WHERE storage_class = ?1 AND state = 'active'",
        &[&class],
    )?;
    if used
        .checked_add(reserved)
        .and_then(|total| total.checked_add(if already_indexed { 0 } else { requested_bytes }))
        .is_none_or(|total| total > limit)
    {
        return Err(StorageError::StorageQuotaExceeded);
    }
    Ok(())
}

fn artifact_contract_from_manifest(digest: &str, manifest: Manifest) -> ArtifactContract {
    ArtifactContract {
        artifact: ArtifactRefV1 {
            schema: "rampage.artifact-ref.v1".into(),
            digest: digest.into(),
            size_bytes: manifest.plaintext_size,
            media_type: manifest.media_type,
            storage_class: manifest.storage_class,
            encrypted: true,
        },
        required_replicas: manifest.required_replicas,
    }
}

fn ensure_artifact_contract(
    existing: &ArtifactContract,
    size_bytes: u64,
    media_type: &str,
    storage_class: StorageClass,
    required_replicas: u8,
) -> Result<(), StorageError> {
    if existing.artifact.size_bytes != size_bytes
        || existing.artifact.media_type != media_type
        || existing.artifact.storage_class != storage_class
        || existing.required_replicas != required_replicas
    {
        return Err(StorageError::ArtifactConflict);
    }
    Ok(())
}

fn parse_storage_class(value: &str) -> Result<StorageClass, StorageError> {
    match value {
        "cache" => Ok(StorageClass::Cache),
        "scratch" => Ok(StorageClass::Scratch),
        "protected" => Ok(StorageClass::Protected),
        _ => Err(StorageError::InvalidTransfer),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

fn read_regular_file_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>, StorageError> {
    let path_metadata = fs::symlink_metadata(path)?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(StorageError::InvalidManifest);
    }
    let mut file = OpenOptions::new().read(true).open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err(StorageError::InvalidManifest);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::io::Read::by_ref(&mut file)
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(StorageError::InvalidManifest);
    }
    Ok(bytes)
}

fn durable_write_new(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), StorageError> {
    std::fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), StorageError> {
    Ok(())
}

fn is_repairable_object_error(error: &StorageError) -> bool {
    match error {
        StorageError::DigestMismatch
        | StorageError::InvalidManifest
        | StorageError::Manifest(_)
        | StorageError::NotFound(_) => true,
        StorageError::Io(error) => matches!(
            error.kind(),
            std::io::ErrorKind::NotFound
                | std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::InvalidData
        ),
        _ => false,
    }
}

fn validate_manifest(expected_digest: &str, manifest: &Manifest) -> Result<(), StorageError> {
    if manifest.schema != "rampage.cas-manifest.v1"
        || manifest.plaintext_digest != expected_digest
        || manifest.plaintext_size > MAX_ARTIFACT_TRANSFER_BYTES
        || manifest.media_type.is_empty()
        || manifest.media_type.len() > 255
        || !manifest.media_type.is_ascii()
        || manifest.required_replicas == 0
        || (manifest.storage_class == StorageClass::Protected && manifest.required_replicas < 2)
        || (manifest.plaintext_size == 0) != manifest.chunks.is_empty()
    {
        return Err(StorageError::InvalidManifest);
    }
    let nominal_chunk_size = manifest
        .chunks
        .first()
        .map_or(0, |chunk| chunk.plaintext_size);
    if nominal_chunk_size > DEFAULT_CHUNK_SIZE as u64 {
        return Err(StorageError::InvalidManifest);
    }
    let mut total = 0_u64;
    for (expected_index, chunk) in manifest.chunks.iter().enumerate() {
        let remaining = manifest
            .plaintext_size
            .checked_sub(total)
            .ok_or(StorageError::InvalidManifest)?;
        let is_last = expected_index + 1 == manifest.chunks.len();
        let expected_size = if is_last {
            if remaining > nominal_chunk_size {
                return Err(StorageError::InvalidManifest);
            }
            remaining
        } else {
            nominal_chunk_size
        };
        if chunk.index != expected_index as u32
            || chunk.plaintext_size == 0
            || chunk.plaintext_size != expected_size
            || chunk.nonce.len() != 24
            || !is_lower_hex(&chunk.nonce)
            || chunk.ciphertext_digest.len() != 64
            || !is_lower_hex(&chunk.ciphertext_digest)
        {
            return Err(StorageError::InvalidManifest);
        }
        total = total
            .checked_add(chunk.plaintext_size)
            .ok_or(StorageError::InvalidManifest)?;
    }
    if total != manifest.plaintext_size {
        return Err(StorageError::InvalidManifest);
    }
    Ok(())
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn storage_class_name(class: StorageClass) -> &'static str {
    match class {
        StorageClass::Cache => "cache",
        StorageClass::Scratch => "scratch",
        StorageClass::Protected => "protected",
    }
}

fn uuid_like_nonce() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transfer_spec(payload: &[u8], epoch: u64, nonce: &str) -> ResumablePutSpec {
        ResumablePutSpec {
            session_id: "0123456789abcdef0123456789abcdef".into(),
            lease_id: format!("lease-{epoch}"),
            authority_scope: "governor".into(),
            fencing_epoch: epoch,
            authority_nonce: nonce.into(),
            expires_at: Utc::now() + chrono::Duration::minutes(5),
            digest: sha256_digest(payload),
            size_bytes: payload.len() as u64,
            media_type: "application/octet-stream".into(),
            storage_class: StorageClass::Protected,
            required_replicas: 2,
            chunk_size: 1_024,
        }
    }

    #[test]
    fn encrypted_round_trip_is_content_addressed() {
        let temp = tempfile::tempdir().unwrap();
        let store = CasStore::open(temp.path(), [7_u8; 32]).unwrap();
        let payload = vec![42_u8; DEFAULT_CHUNK_SIZE + 19];
        let artifact = store.put(&payload, PutOptions::default()).unwrap();
        assert!(artifact.encrypted);
        assert_eq!(store.get(&artifact.digest).unwrap(), payload);
        let raw = fs::read(
            store
                .object_dir(&artifact.digest)
                .unwrap()
                .join("00000000.chunk"),
        )
        .unwrap();
        assert_ne!(&raw[..32], &payload[..32]);
    }

    #[test]
    fn protected_artifact_requires_replication() {
        let temp = tempfile::tempdir().unwrap();
        let store = CasStore::open(temp.path(), [9_u8; 32]).unwrap();
        let error = store
            .put(
                b"irreplaceable",
                PutOptions {
                    storage_class: StorageClass::Protected,
                    required_replicas: 1,
                    ..PutOptions::default()
                },
            )
            .unwrap_err();
        assert!(matches!(error, StorageError::ProtectedRequiresReplication));
    }

    #[test]
    fn wrong_key_cannot_read_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let first = CasStore::open(temp.path(), [1_u8; 32]).unwrap();
        let artifact = first.put(b"secret", PutOptions::default()).unwrap();
        drop(first);
        let second = CasStore::open(temp.path(), [2_u8; 32]).unwrap();
        assert!(matches!(
            second.get(&artifact.digest),
            Err(StorageError::Crypto)
        ));
    }

    #[test]
    fn corrupt_content_address_is_quarantined_and_rebuilt_from_verified_plaintext() {
        let temp = tempfile::tempdir().unwrap();
        let store = CasStore::open(temp.path(), [31_u8; 32]).unwrap();
        let payload = b"recover this immutable object";
        let artifact = store.put(payload, PutOptions::default()).unwrap();
        let chunk_path = store
            .object_dir(&artifact.digest)
            .unwrap()
            .join("00000000.chunk");
        fs::write(&chunk_path, b"objectively corrupt ciphertext").unwrap();

        let rebuilt = store.put(payload, PutOptions::default()).unwrap();
        assert_eq!(rebuilt.digest, artifact.digest);
        assert_eq!(store.get(&rebuilt.digest).unwrap(), payload);
        let quarantined = fs::read_dir(temp.path().join("tmp").join("corrupt"))
            .unwrap()
            .count();
        assert_eq!(quarantined, 1);
    }

    #[test]
    fn wrong_key_does_not_quarantine_a_potentially_valid_object() {
        let temp = tempfile::tempdir().unwrap();
        let first = CasStore::open(temp.path(), [32_u8; 32]).unwrap();
        let payload = b"key identity is not corruption";
        let artifact = first.put(payload, PutOptions::default()).unwrap();
        let object_dir = first.object_dir(&artifact.digest).unwrap();
        drop(first);

        let second = CasStore::open(temp.path(), [33_u8; 32]).unwrap();
        assert!(matches!(
            second.put(payload, PutOptions::default()),
            Err(StorageError::Crypto)
        ));
        assert!(object_dir.is_dir());
        assert!(!temp.path().join("tmp").join("corrupt").exists());
    }

    #[test]
    fn active_sessions_reserve_owner_contributed_class_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let store = CasStore::open_with_limits(
            temp.path(),
            [34_u8; 32],
            Some(StorageLimits {
                cache_bytes: 2_000,
                scratch_bytes: 0,
                protected_bytes: 3_000,
            }),
        )
        .unwrap();
        let first_payload = vec![1_u8; 1_600];
        let first = transfer_spec(&first_payload, 1, "quota-one");
        store.begin_resumable_put(&first).unwrap();

        let second_payload = vec![2_u8; 1_600];
        let mut second = transfer_spec(&second_payload, 1, "quota-two");
        second.session_id = "fedcba9876543210fedcba9876543210".into();
        second.lease_id = "lease-quota-two".into();
        assert!(matches!(
            store.begin_resumable_put(&second),
            Err(StorageError::StorageQuotaExceeded)
        ));
    }

    #[test]
    fn head_returns_durable_metadata_without_decrypting_payload() {
        let temp = tempfile::tempdir().unwrap();
        let store = CasStore::open(temp.path(), [3_u8; 32]).unwrap();
        let artifact = store
            .put(
                b"metadata",
                PutOptions {
                    media_type: "text/plain".into(),
                    storage_class: StorageClass::Scratch,
                    required_replicas: 1,
                },
            )
            .unwrap();
        assert_eq!(
            store.head(&artifact.digest).unwrap().media_type,
            "text/plain"
        );
        assert_eq!(
            store.head(&artifact.digest).unwrap().storage_class,
            StorageClass::Scratch
        );
    }

    #[test]
    fn challenged_possession_rejects_corrupt_ciphertext_that_metadata_head_can_still_see() {
        let temp = tempfile::tempdir().unwrap();
        let store = CasStore::open(temp.path(), [23_u8; 32]).unwrap();
        let artifact = store
            .put(b"prove possession", PutOptions::default())
            .unwrap();
        let chunk_path = store
            .object_dir(&artifact.digest)
            .unwrap()
            .join("00000000.chunk");
        let mut ciphertext = fs::read(&chunk_path).unwrap();
        ciphertext[0] ^= 0xff;
        fs::write(chunk_path, ciphertext).unwrap();

        assert!(store.head(&artifact.digest).is_ok());
        assert!(matches!(
            store.verify(&artifact.digest),
            Err(StorageError::DigestMismatch)
        ));
    }

    #[test]
    fn an_existing_content_address_rejects_conflicting_durability_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let store = CasStore::open(temp.path(), [24_u8; 32]).unwrap();
        let payload = b"one immutable address";
        store.put(payload, PutOptions::default()).unwrap();

        assert!(matches!(
            store.put(
                payload,
                PutOptions {
                    media_type: "text/plain".into(),
                    storage_class: StorageClass::Protected,
                    required_replicas: 2,
                }
            ),
            Err(StorageError::ArtifactConflict)
        ));
    }

    #[test]
    fn rejects_digest_paths_before_filesystem_access() {
        let temp = tempfile::tempdir().unwrap();
        let store = CasStore::open(temp.path(), [4_u8; 32]).unwrap();
        assert!(matches!(
            store.get("sha256:../../controller.token"),
            Err(StorageError::InvalidDigest)
        ));
        assert!(matches!(
            store.head("not-a-digest"),
            Err(StorageError::InvalidDigest)
        ));
    }

    #[test]
    fn authority_replay_and_stale_epochs_are_rejected_across_restart() {
        let temp = tempfile::tempdir().unwrap();
        let expiry = Utc::now() + chrono::Duration::minutes(5);
        {
            let store = CasStore::open(temp.path(), [5_u8; 32]).unwrap();
            store
                .accept_authority("governor", 7, "nonce-one", expiry)
                .unwrap();
            assert!(matches!(
                store.accept_authority("governor", 7, "nonce-one", expiry),
                Err(StorageError::ReplayedAuthorityNonce)
            ));
            store
                .accept_authority("governor", 8, "nonce-two", expiry)
                .unwrap();
            assert!(matches!(
                store.accept_authority("governor", 7, "nonce-three", expiry),
                Err(StorageError::StaleFencingEpoch {
                    current: 8,
                    supplied: 7
                })
            ));
        }
        let reopened = CasStore::open(temp.path(), [5_u8; 32]).unwrap();
        assert!(matches!(
            reopened.accept_authority("governor", 8, "nonce-two", expiry),
            Err(StorageError::ReplayedAuthorityNonce)
        ));
        assert!(matches!(
            reopened.accept_authority("governor", 7, "nonce-four", expiry),
            Err(StorageError::StaleFencingEpoch {
                current: 8,
                supplied: 7
            })
        ));
    }

    #[test]
    fn expired_authority_is_not_consumed() {
        let temp = tempfile::tempdir().unwrap();
        let store = CasStore::open(temp.path(), [6_u8; 32]).unwrap();
        assert!(matches!(
            store.accept_authority(
                "governor",
                1,
                "expired",
                Utc::now() - chrono::Duration::seconds(1)
            ),
            Err(StorageError::ExpiredAuthority)
        ));
    }

    #[test]
    fn resumable_put_survives_restart_and_commits_without_whole_artifact_staging() {
        let temp = tempfile::tempdir().unwrap();
        let payload = (0..3_111)
            .map(|value| (value % 251) as u8)
            .collect::<Vec<_>>();
        let spec = transfer_spec(&payload, 7, "resume-one");
        {
            let store = CasStore::open(temp.path(), [17_u8; 32]).unwrap();
            let status = store.begin_resumable_put(&spec).unwrap();
            assert_eq!(status.missing_chunks, vec![0, 1, 2, 3]);
            let first = &payload[..1_024];
            store
                .put_resumable_chunk(&spec.session_id, 0, &sha256_digest(first), first)
                .unwrap();
            let encrypted = fs::read(
                store
                    .transfer_dir(&spec.session_id)
                    .unwrap()
                    .join("00000000.chunk"),
            )
            .unwrap();
            assert_ne!(&encrypted[..32], &first[..32]);
        }
        let store = CasStore::open(temp.path(), [17_u8; 32]).unwrap();
        let resumed = store.begin_resumable_put(&spec).unwrap();
        assert_eq!(resumed.received_chunks, vec![0]);
        assert_eq!(resumed.missing_chunks, vec![1, 2, 3]);
        for index in resumed.missing_chunks {
            let start = index as usize * spec.chunk_size as usize;
            let end = (start + spec.chunk_size as usize).min(payload.len());
            let chunk = &payload[start..end];
            store
                .put_resumable_chunk(&spec.session_id, index, &sha256_digest(chunk), chunk)
                .unwrap();
        }
        let artifact = store.commit_resumable_put(&spec.session_id).unwrap();
        assert_eq!(artifact.digest, spec.digest);
        assert_eq!(store.get(&artifact.digest).unwrap(), payload);
        assert!(
            store
                .resumable_put_status(&spec.session_id)
                .unwrap()
                .complete
        );
    }

    #[test]
    fn resumable_chunks_are_idempotent_and_reject_conflicting_payloads() {
        let temp = tempfile::tempdir().unwrap();
        let payload = vec![29_u8; 1_200];
        let spec = transfer_spec(&payload, 3, "chunk-idempotency");
        let store = CasStore::open(temp.path(), [18_u8; 32]).unwrap();
        store.begin_resumable_put(&spec).unwrap();
        let chunk = &payload[..1_024];
        let digest = sha256_digest(chunk);
        store
            .put_resumable_chunk(&spec.session_id, 0, &digest, chunk)
            .unwrap();
        let before = fs::read(
            store
                .transfer_dir(&spec.session_id)
                .unwrap()
                .join("00000000.chunk"),
        )
        .unwrap();
        store
            .put_resumable_chunk(&spec.session_id, 0, &digest, chunk)
            .unwrap();
        let after = fs::read(
            store
                .transfer_dir(&spec.session_id)
                .unwrap()
                .join("00000000.chunk"),
        )
        .unwrap();
        assert_eq!(before, after);
        let corrupt = vec![30_u8; 1_024];
        assert!(matches!(
            store.put_resumable_chunk(&spec.session_id, 0, &sha256_digest(&corrupt), &corrupt),
            Err(StorageError::TransferConflict)
        ));
        assert!(matches!(
            store.commit_resumable_put(&spec.session_id),
            Err(StorageError::TransferIncomplete)
        ));
    }

    #[test]
    fn renewed_transfer_authority_advances_fence_and_rejects_stale_resume() {
        let temp = tempfile::tempdir().unwrap();
        let payload = vec![7_u8; 1_500];
        let original = transfer_spec(&payload, 11, "renew-one");
        let store = CasStore::open(temp.path(), [19_u8; 32]).unwrap();
        store.begin_resumable_put(&original).unwrap();
        let mut renewed = original.clone();
        renewed.lease_id = "lease-12".into();
        renewed.authority_nonce = "renew-two".into();
        renewed.fencing_epoch = 12;
        renewed.expires_at = Utc::now() + chrono::Duration::minutes(6);
        store.begin_resumable_put(&renewed).unwrap();
        assert!(matches!(
            store.begin_resumable_put(&original),
            Err(StorageError::StaleFencingEpoch {
                current: 12,
                supplied: 11
            })
        ));
        drop(store);
        let reopened = CasStore::open(temp.path(), [19_u8; 32]).unwrap();
        assert!(reopened.begin_resumable_put(&renewed).is_ok());
    }

    #[test]
    fn chunk_reads_authenticate_one_cas_chunk_at_a_time() {
        let temp = tempfile::tempdir().unwrap();
        let payload = vec![41_u8; DEFAULT_CHUNK_SIZE + 17];
        let store = CasStore::open(temp.path(), [20_u8; 32]).unwrap();
        let artifact = store.put(&payload, PutOptions::default()).unwrap();
        let layout = store.chunk_layout(&artifact.digest).unwrap();
        assert_eq!(layout.chunk_count, 2);
        assert_eq!(
            store.get_chunk(&artifact.digest, 0).unwrap().len(),
            DEFAULT_CHUNK_SIZE
        );
        assert_eq!(store.get_chunk(&artifact.digest, 1).unwrap().len(), 17);
        assert!(matches!(
            store.get_chunk(&artifact.digest, 2),
            Err(StorageError::ChunkOutOfRange)
        ));
    }

    #[test]
    fn manifest_and_ciphertext_growth_are_rejected_before_unbounded_reads() {
        let manifest_temp = tempfile::tempdir().unwrap();
        let manifest_store = CasStore::open(manifest_temp.path(), [21_u8; 32]).unwrap();
        let manifest_artifact = manifest_store
            .put(b"bounded", PutOptions::default())
            .unwrap();
        fs::write(
            manifest_store
                .object_dir(&manifest_artifact.digest)
                .unwrap()
                .join("manifest.json"),
            vec![b' '; MAX_MANIFEST_BYTES as usize + 1],
        )
        .unwrap();
        assert!(matches!(
            manifest_store.get(&manifest_artifact.digest),
            Err(StorageError::InvalidManifest)
        ));

        let chunk_temp = tempfile::tempdir().unwrap();
        let chunk_store = CasStore::open(chunk_temp.path(), [22_u8; 32]).unwrap();
        let chunk_artifact = chunk_store.put(b"bounded", PutOptions::default()).unwrap();
        fs::write(
            chunk_store
                .object_dir(&chunk_artifact.digest)
                .unwrap()
                .join("00000000.chunk"),
            vec![0_u8; MAX_ENCRYPTED_CHUNK_BYTES as usize + 1],
        )
        .unwrap();
        assert!(matches!(
            chunk_store.get_chunk(&chunk_artifact.digest, 0),
            Err(StorageError::InvalidManifest)
        ));
    }
}
