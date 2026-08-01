//! Encrypted, chunked content-addressed storage with explicit durability classes.

use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, OsRng, rand_core::RngCore},
};
use chrono::{DateTime, Utc};
use rampage_protocol::{ArtifactRefV1, StorageClass};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
use thiserror::Error;

const DEFAULT_CHUNK_SIZE: usize = 4 * 1024 * 1024;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("manifest error: {0}")]
    Manifest(#[from] serde_json::Error),
    #[error("encryption or authentication failed")]
    Crypto,
    #[error("artifact {0} is not present")]
    NotFound(String),
    #[error("artifact digest mismatch")]
    DigestMismatch,
    #[error("invalid artifact digest")]
    InvalidDigest,
    #[error("protected storage requires at least two declared replicas")]
    ProtectedRequiresReplication,
    #[error("storage lock is poisoned")]
    Poisoned,
}

pub struct CasStore {
    root: PathBuf,
    cipher: Aes256Gcm,
    index: Mutex<Connection>,
    chunk_size: usize,
}

impl CasStore {
    pub fn open(root: impl AsRef<Path>, encryption_key: [u8; 32]) -> Result<Self, StorageError> {
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
            );",
        )?;
        Ok(Self {
            root,
            cipher: Aes256Gcm::new_from_slice(&encryption_key).map_err(|_| StorageError::Crypto)?,
            index: Mutex::new(index),
            chunk_size: DEFAULT_CHUNK_SIZE,
        })
    }

    pub fn put(
        &self,
        plaintext: &[u8],
        options: PutOptions,
    ) -> Result<ArtifactRefV1, StorageError> {
        if options.storage_class == StorageClass::Protected && options.required_replicas < 2 {
            return Err(StorageError::ProtectedRequiresReplication);
        }
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(plaintext)));
        let object_dir = self.object_dir(&digest)?;
        if object_dir.join("manifest.json").is_file() {
            return Ok(ArtifactRefV1 {
                schema: "rampage.artifact-ref.v1".into(),
                digest,
                size_bytes: plaintext.len() as u64,
                media_type: options.media_type,
                storage_class: options.storage_class,
                encrypted: true,
            });
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
            fs::write(staging.join(format!("{index:08}.chunk")), ciphertext)?;
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
        fs::write(
            staging.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        if let Some(parent) = object_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::rename(&staging, &object_dir) {
            Ok(()) => {}
            Err(error) if object_dir.exists() => {
                fs::remove_dir_all(&staging)?;
                let _ = error;
            }
            Err(error) => return Err(StorageError::Io(error)),
        }
        let connection = self.index.lock().map_err(|_| StorageError::Poisoned)?;
        connection.execute(
            "INSERT OR IGNORE INTO artifacts
             (digest, size_bytes, media_type, storage_class, required_replicas, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                digest,
                plaintext.len() as u64,
                options.media_type,
                storage_class_name(options.storage_class),
                options.required_replicas,
                manifest.created_at.to_rfc3339(),
            ],
        )?;
        Ok(ArtifactRefV1 {
            schema: "rampage.artifact-ref.v1".into(),
            digest,
            size_bytes: plaintext.len() as u64,
            media_type: manifest.media_type,
            storage_class: manifest.storage_class,
            encrypted: true,
        })
    }

    pub fn get(&self, digest: &str) -> Result<Vec<u8>, StorageError> {
        let object_dir = self.object_dir(digest)?;
        let manifest_path = object_dir.join("manifest.json");
        if !manifest_path.is_file() {
            return Err(StorageError::NotFound(digest.into()));
        }
        let manifest: Manifest = serde_json::from_slice(&fs::read(manifest_path)?)?;
        let mut plaintext = Vec::with_capacity(manifest.plaintext_size as usize);
        for record in &manifest.chunks {
            let ciphertext = fs::read(object_dir.join(format!("{:08}.chunk", record.index)))?;
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
        self.object_dir(digest)?;
        let connection = self.index.lock().map_err(|_| StorageError::Poisoned)?;
        let record = connection
            .query_row(
                "SELECT size_bytes, media_type, storage_class FROM artifacts WHERE digest = ?1",
                params![digest],
                |row| {
                    let class: String = row.get(2)?;
                    let storage_class = match class.as_str() {
                        "cache" => StorageClass::Cache,
                        "scratch" => StorageClass::Scratch,
                        "protected" => StorageClass::Protected,
                        _ => return Err(rusqlite::Error::InvalidQuery),
                    };
                    Ok(ArtifactRefV1 {
                        schema: "rampage.artifact-ref.v1".into(),
                        digest: digest.into(),
                        size_bytes: row.get(0)?,
                        media_type: row.get(1)?,
                        storage_class,
                        encrypted: true,
                    })
                },
            )
            .optional()?;
        record.ok_or_else(|| StorageError::NotFound(digest.into()))
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
}
