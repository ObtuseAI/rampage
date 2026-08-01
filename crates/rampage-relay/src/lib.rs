//! Owner-operated relay service for the Rampage private compute fabric.
//!
//! Direct authenticated QUIC remains the preferred path. This service supplies a bounded relay
//! fallback for hard NATs without a Tailscale account or a third-party coordination plane.

use anyhow::Context;
use iroh::RelayUrl;
use iroh_relay::server::{
    Access, AccessControl, CertConfig, ClientRateLimit, ClientRequest, Limits, QuicConfig,
    RelayConfig, Server, ServerConfig, TlsConfig,
};
use rampage_policy::verify_relay_access_manifest_with_key;
use rampage_protocol::RelayAccessManifestV1;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::Read,
    net::{IpAddr, SocketAddr},
    num::NonZeroU32,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex as AsyncMutex;
use webpki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};

const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_TOKEN_BYTES: u64 = 512;
const MAX_CERTIFICATE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PRIVATE_KEY_BYTES: u64 = 1024 * 1024;
const MIN_RATE_BYTES_PER_SECOND: u32 = 1024 * 1024;
const MAX_RATE_BYTES_PER_SECOND: u32 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerRelayConfigV1 {
    pub schema: String,
    pub public_url: String,
    pub http_bind_addr: SocketAddr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<ManualTlsConfigV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quic_bind_addr: Option<SocketAddr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics_bind_addr: Option<SocketAddr>,
    pub governor_public_key: String,
    pub access_source: RelayAccessSourceV1,
    pub snapshot_cache_seconds: u64,
    pub client_rx_bytes_per_second: u32,
    pub client_rx_max_burst_bytes: u32,
    pub key_cache_capacity: usize,
    pub max_connections_per_endpoint: u16,
    pub max_total_connections: u32,
}

impl OwnerRelayConfigV1 {
    pub const SCHEMA: &'static str = "rampage.owner-relay-config.v1";

    pub fn reverse_proxy(
        public_url: String,
        governor_public_key: String,
        controller_manifest_url: String,
        controller_token_file: PathBuf,
        http_bind_addr: SocketAddr,
    ) -> Self {
        Self {
            schema: Self::SCHEMA.into(),
            public_url,
            http_bind_addr,
            tls: None,
            quic_bind_addr: None,
            metrics_bind_addr: None,
            governor_public_key,
            access_source: RelayAccessSourceV1::ControllerLoopback {
                url: controller_manifest_url,
                token_file: controller_token_file,
            },
            snapshot_cache_seconds: 5,
            client_rx_bytes_per_second: 64 * 1024 * 1024,
            client_rx_max_burst_bytes: 128 * 1024 * 1024,
            key_cache_capacity: 4096,
            max_connections_per_endpoint: 8,
            max_total_connections: 1024,
        }
    }

    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let bytes = read_bounded_sync_file(path, "relay config", MAX_MANIFEST_BYTES)?;
        let config: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("relay config {} is invalid", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.schema == Self::SCHEMA,
            "unsupported relay config schema"
        );
        validate_governor_key(&self.governor_public_key)?;
        let public_url =
            reqwest::Url::parse(&self.public_url).context("invalid public relay URL")?;
        anyhow::ensure!(
            public_url.scheme() == "https",
            "public relay URL must use HTTPS"
        );
        anyhow::ensure!(
            public_url.username().is_empty()
                && public_url.password().is_none()
                && public_url.query().is_none()
                && public_url.fragment().is_none(),
            "public relay URL cannot embed credentials, query parameters, or fragments"
        );
        self.public_url
            .parse::<RelayUrl>()
            .context("public relay URL is not accepted by the mesh transport")?;
        anyhow::ensure!(
            self.http_bind_addr.ip().is_loopback(),
            "the plaintext relay listener must remain loopback-only"
        );
        if self.tls.is_none() {
            anyhow::ensure!(
                self.quic_bind_addr.is_none(),
                "QUIC address discovery requires built-in TLS"
            );
        }
        if let Some(tls) = &self.tls {
            anyhow::ensure!(
                tls.https_bind_addr != self.http_bind_addr,
                "HTTP and HTTPS relay listeners must use different sockets"
            );
        }
        if let Some(metrics) = self.metrics_bind_addr {
            anyhow::ensure!(
                metrics.ip().is_loopback(),
                "relay metrics must remain loopback-only"
            );
        }
        anyhow::ensure!(
            (1..=30).contains(&self.snapshot_cache_seconds),
            "snapshot cache must be between 1 and 30 seconds"
        );
        anyhow::ensure!(
            (MIN_RATE_BYTES_PER_SECOND..=MAX_RATE_BYTES_PER_SECOND)
                .contains(&self.client_rx_bytes_per_second),
            "per-client relay rate must be between one MiB/s and one GiB/s"
        );
        anyhow::ensure!(
            self.client_rx_max_burst_bytes >= self.client_rx_bytes_per_second
                && self.client_rx_max_burst_bytes
                    <= self.client_rx_bytes_per_second.saturating_mul(4),
            "relay burst must be between one and four seconds of the configured client rate"
        );
        anyhow::ensure!(
            (128..=65_536).contains(&self.key_cache_capacity),
            "relay key cache must contain between 128 and 65536 entries"
        );
        anyhow::ensure!(
            (1..=64).contains(&self.max_connections_per_endpoint),
            "per-endpoint connection limit must be between 1 and 64"
        );
        anyhow::ensure!(
            (1..=65_536).contains(&self.max_total_connections)
                && self.max_total_connections >= self.max_connections_per_endpoint as u32,
            "total relay connection limit is invalid"
        );
        self.access_source.validate()?;
        Ok(())
    }

    fn validate_files(&self) -> anyhow::Result<()> {
        if let Some(tls) = &self.tls {
            anyhow::ensure!(
                tls.certificate_path.is_file(),
                "TLS certificate is not a file"
            );
            anyhow::ensure!(
                tls.private_key_path.is_file(),
                "TLS private key is not a file"
            );
        }
        if let RelayAccessSourceV1::SignedFile { path } = &self.access_source {
            anyhow::ensure!(path.is_file(), "signed relay access manifest is not a file");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualTlsConfigV1 {
    pub https_bind_addr: SocketAddr,
    pub certificate_path: PathBuf,
    pub private_key_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RelayAccessSourceV1 {
    ControllerLoopback { url: String, token_file: PathBuf },
    SignedFile { path: PathBuf },
}

impl RelayAccessSourceV1 {
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::ControllerLoopback { url, token_file } => {
                let parsed = reqwest::Url::parse(url).context("invalid controller manifest URL")?;
                anyhow::ensure!(
                    parsed.scheme() == "http" || parsed.scheme() == "https",
                    "controller manifest URL must use HTTP or HTTPS"
                );
                let host = parsed
                    .host_str()
                    .context("controller manifest URL is missing a host")?;
                let ip: IpAddr = host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse()
                    .context("controller manifest host must be an explicit loopback IP")?;
                anyhow::ensure!(
                    ip.is_loopback(),
                    "controller manifest URL must be loopback-only"
                );
                anyhow::ensure!(
                    parsed.path() == "/v1/mesh/relay-access"
                        && parsed.username().is_empty()
                        && parsed.password().is_none()
                        && parsed.query().is_none()
                        && parsed.fragment().is_none(),
                    "controller manifest URL must be the exact protected relay-access route"
                );
                anyhow::ensure!(
                    !token_file.as_os_str().is_empty(),
                    "controller token file path is empty"
                );
            }
            Self::SignedFile { path } => {
                anyhow::ensure!(!path.as_os_str().is_empty(), "manifest file path is empty");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct ActiveConnections {
    total: u32,
    by_endpoint: HashMap<String, u16>,
}

#[derive(Debug)]
struct CachedManifest {
    fetched_at: Instant,
    manifest: RelayAccessManifestV1,
}

#[derive(Debug)]
pub struct SignedManifestAccess {
    source: RelayAccessSourceV1,
    governor_public_key: String,
    cache_duration: Duration,
    cache: AsyncMutex<Option<CachedManifest>>,
    http: reqwest::Client,
    active: Mutex<ActiveConnections>,
    max_per_endpoint: u16,
    max_total: u32,
}

impl SignedManifestAccess {
    pub fn new(config: &OwnerRelayConfigV1) -> anyhow::Result<Self> {
        config.validate()?;
        Ok(Self {
            source: config.access_source.clone(),
            governor_public_key: config.governor_public_key.clone(),
            cache_duration: Duration::from_secs(config.snapshot_cache_seconds),
            cache: AsyncMutex::new(None),
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("rampage-relay/0.2")
                .build()?,
            active: Mutex::new(ActiveConnections::default()),
            max_per_endpoint: config.max_connections_per_endpoint,
            max_total: config.max_total_connections,
        })
    }

    pub async fn check_access(&self) -> anyhow::Result<RelayAccessManifestV1> {
        self.load_manifest().await
    }

    async fn load_manifest(&self) -> anyhow::Result<RelayAccessManifestV1> {
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.as_ref()
            && cached.fetched_at.elapsed() < self.cache_duration
            && verify_relay_access_manifest_with_key(&self.governor_public_key, &cached.manifest)
                .is_ok()
        {
            return Ok(cached.manifest.clone());
        }
        let bytes = match &self.source {
            RelayAccessSourceV1::SignedFile { path } => {
                read_bounded_file(path, "signed relay access manifest", MAX_MANIFEST_BYTES).await?
            }
            RelayAccessSourceV1::ControllerLoopback { url, token_file } => {
                let token = String::from_utf8(
                    read_bounded_file(token_file, "controller token file", MAX_TOKEN_BYTES).await?,
                )
                .context("controller token file is not UTF-8")?;
                let token = token.trim();
                anyhow::ensure!(
                    !token.is_empty() && token.len() <= 512,
                    "controller token file is empty or oversized"
                );
                let response = self
                    .http
                    .get(url)
                    .header("x-rampage-token", token)
                    .send()
                    .await
                    .context("could not fetch controller relay authorization")?;
                anyhow::ensure!(
                    response.status().is_success(),
                    "controller denied relay authorization with status {}",
                    response.status()
                );
                anyhow::ensure!(
                    response.content_length().unwrap_or(0) <= MAX_MANIFEST_BYTES,
                    "controller relay authorization is oversized"
                );
                read_bounded_response(response, "controller relay authorization").await?
            }
        };
        let manifest: RelayAccessManifestV1 =
            serde_json::from_slice(&bytes).context("relay authorization JSON is invalid")?;
        verify_relay_access_manifest_with_key(&self.governor_public_key, &manifest)
            .context("relay authorization signature, scope, or expiry is invalid")?;
        *cache = Some(CachedManifest {
            fetched_at: Instant::now(),
            manifest: manifest.clone(),
        });
        Ok(manifest)
    }

    async fn admit(&self, endpoint_id: &str) -> Result<(), &'static str> {
        let manifest = self
            .load_manifest()
            .await
            .map_err(|_| "relay authorization is unavailable or invalid")?;
        if !manifest.allowed_endpoint_ids.contains(endpoint_id) {
            return Err("endpoint is not enrolled in this Rampage fabric");
        }
        let mut active = self
            .active
            .lock()
            .map_err(|_| "relay connection accounting is unavailable")?;
        let endpoint_count = active.by_endpoint.get(endpoint_id).copied().unwrap_or(0);
        if active.total >= self.max_total || endpoint_count >= self.max_per_endpoint {
            return Err("relay connection threshold reached");
        }
        active.total += 1;
        active
            .by_endpoint
            .insert(endpoint_id.to_string(), endpoint_count + 1);
        Ok(())
    }

    fn release(&self, endpoint_id: &str) {
        let Ok(mut active) = self.active.lock() else {
            return;
        };
        let Some(count) = active.by_endpoint.get(endpoint_id).copied() else {
            return;
        };
        active.total = active.total.saturating_sub(1);
        if count <= 1 {
            active.by_endpoint.remove(endpoint_id);
        } else {
            active
                .by_endpoint
                .insert(endpoint_id.to_string(), count - 1);
        }
    }
}

async fn read_bounded_file(path: &Path, label: &str, max_bytes: u64) -> anyhow::Result<Vec<u8>> {
    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("could not open {label}"))?;
    let metadata = file
        .metadata()
        .await
        .with_context(|| format!("could not inspect {label}"))?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= max_bytes,
        "{label} is not a bounded regular file"
    );
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .await
        .with_context(|| format!("could not read {label}"))?;
    anyhow::ensure!(bytes.len() as u64 <= max_bytes, "{label} is oversized");
    Ok(bytes)
}

async fn read_bounded_response(
    mut response: reqwest::Response,
    label: &str,
) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("could not read {label}"))?
    {
        anyhow::ensure!(
            bytes.len().saturating_add(chunk.len()) <= MAX_MANIFEST_BYTES as usize,
            "{label} is oversized"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

impl AccessControl for SignedManifestAccess {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        match self.admit(&request.endpoint_id().to_string()).await {
            Ok(()) => Access::Allow,
            Err(reason) => Access::Deny {
                reason: Some(reason.into()),
            },
        }
    }

    fn on_disconnect(
        &self,
        endpoint_id: iroh::EndpointId,
        _connection_id: iroh_relay::server::ConnectionId,
    ) {
        self.release(&endpoint_id.to_string());
    }
}

pub async fn spawn_owner_relay(config: &OwnerRelayConfigV1) -> anyhow::Result<Server> {
    config.validate()?;
    config.validate_files()?;
    let access = Arc::new(SignedManifestAccess::new(config)?);
    access
        .check_access()
        .await
        .context("refusing to start without fresh Governor-signed relay authorization")?;

    let mut relay = RelayConfig::new(config.http_bind_addr);
    relay.access = access;
    relay.key_cache_capacity = Some(config.key_cache_capacity);
    let mut rate = ClientRateLimit::new(
        NonZeroU32::new(config.client_rx_bytes_per_second)
            .context("relay client rate cannot be zero")?,
    );
    rate.max_burst_bytes = NonZeroU32::new(config.client_rx_max_burst_bytes);
    let mut limits = Limits::default();
    limits.client_rx = Some(rate);
    relay.limits = limits;

    if let Some(tls) = &config.tls {
        let server_config = load_manual_tls(tls)?;
        relay.tls = Some(TlsConfig::new(
            tls.https_bind_addr,
            CertConfig::Manual { server_config },
        ));
    }
    let quic = config.quic_bind_addr.map(QuicConfig::new);
    let mut server_config = ServerConfig::default();
    server_config.relay = Some(relay);
    server_config.quic = quic;
    server_config.metrics_addr = config.metrics_bind_addr;
    Server::spawn(server_config)
        .await
        .map_err(|error| anyhow::anyhow!("owner relay could not start: {error}"))
}

fn load_manual_tls(tls: &ManualTlsConfigV1) -> anyhow::Result<rustls::ServerConfig> {
    let certificate_bytes = read_bounded_sync_file(
        &tls.certificate_path,
        "TLS certificate chain",
        MAX_CERTIFICATE_BYTES,
    )?;
    let private_key_bytes = read_bounded_sync_file(
        &tls.private_key_path,
        "TLS private key",
        MAX_PRIVATE_KEY_BYTES,
    )?;
    let certificates = CertificateDer::pem_slice_iter(&certificate_bytes)
        .collect::<Result<Vec<_>, _>>()
        .context("could not parse TLS certificate chain")?;
    anyhow::ensure!(!certificates.is_empty(), "TLS certificate chain is empty");
    let private_key = PrivateKeyDer::from_pem_slice(&private_key_bytes)
        .context("could not parse TLS private key")?;
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certificates, private_key)
        .context("TLS certificate and private key do not form a valid server configuration")
}

fn read_bounded_sync_file(path: &Path, label: &str, max_bytes: u64) -> anyhow::Result<Vec<u8>> {
    let file = std::fs::File::open(path).with_context(|| format!("could not open {label}"))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("could not inspect {label}"))?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= max_bytes,
        "{label} is not a bounded regular file"
    );
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("could not read {label}"))?;
    anyhow::ensure!(bytes.len() as u64 <= max_bytes, "{label} is oversized");
    Ok(bytes)
}

fn validate_governor_key(public_key: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        public_key.len() == 64 && public_key.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Governor public key must be 32-byte lowercase or uppercase hex"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, Utc};
    use ed25519_dalek::SigningKey;
    use iroh::{Endpoint, RelayMode, SecretKey, endpoint::presets};
    use iroh_relay::server::{RelayConfig, ServerConfig};
    use rampage_policy::{relay_fabric_id, sign_relay_access_manifest};
    use std::collections::BTreeSet;

    fn signed_manifest(key: &SigningKey, endpoint: &str) -> RelayAccessManifestV1 {
        let now = Utc::now();
        let public_key = hex::encode(key.verifying_key().to_bytes());
        let mut manifest = RelayAccessManifestV1 {
            schema: RelayAccessManifestV1::SCHEMA.into(),
            fabric_id: relay_fabric_id(&public_key).unwrap(),
            generation: 1,
            allowed_endpoint_ids: BTreeSet::from([endpoint.into()]),
            issued_at: now,
            expires_at: now + ChronoDuration::minutes(10),
            signature: String::new(),
        };
        sign_relay_access_manifest(key, &mut manifest);
        manifest
    }

    fn file_config(path: PathBuf, governor_public_key: String) -> OwnerRelayConfigV1 {
        OwnerRelayConfigV1 {
            access_source: RelayAccessSourceV1::SignedFile { path },
            governor_public_key,
            ..OwnerRelayConfigV1::reverse_proxy(
                "https://relay.example.test".into(),
                "00".repeat(32),
                "http://127.0.0.1:47831/v1/mesh/relay-access".into(),
                PathBuf::from("controller.token"),
                "127.0.0.1:3340".parse().unwrap(),
            )
        }
    }

    #[test]
    fn reverse_proxy_mode_is_loopback_only_and_public_url_is_https() {
        let mut config = OwnerRelayConfigV1::reverse_proxy(
            "https://relay.example.test".into(),
            "11".repeat(32),
            "http://127.0.0.1:47831/v1/mesh/relay-access".into(),
            PathBuf::from("controller.token"),
            "127.0.0.1:3340".parse().unwrap(),
        );
        assert!(config.validate().is_ok());
        config.http_bind_addr = "0.0.0.0:3340".parse().unwrap();
        assert!(config.validate().is_err());
        config.tls = Some(ManualTlsConfigV1 {
            https_bind_addr: "0.0.0.0:443".parse().unwrap(),
            certificate_path: "certificate.pem".into(),
            private_key_path: "private-key.pem".into(),
        });
        assert!(config.validate().is_err());
        config.tls = None;
        config.http_bind_addr = "127.0.0.1:3340".parse().unwrap();
        config.public_url = "http://relay.example.test".into();
        assert!(config.validate().is_err());
    }

    #[tokio::test]
    async fn signed_file_source_fails_closed_on_tamper_and_expiry() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("relay-access.json");
        let key = SigningKey::from_bytes(&[41_u8; 32]);
        let endpoint = "ab".repeat(32);
        let manifest = signed_manifest(&key, &endpoint);
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let config = file_config(path.clone(), hex::encode(key.verifying_key().to_bytes()));
        let access = SignedManifestAccess::new(&config).unwrap();
        assert!(
            access
                .check_access()
                .await
                .unwrap()
                .allowed_endpoint_ids
                .contains(&endpoint)
        );

        let mut tampered = manifest.clone();
        tampered.allowed_endpoint_ids.insert("cd".repeat(32));
        std::fs::write(&path, serde_json::to_vec(&tampered).unwrap()).unwrap();
        let mut uncached = config.clone();
        uncached.snapshot_cache_seconds = 1;
        let access = SignedManifestAccess::new(&uncached).unwrap();
        assert!(access.check_access().await.is_err());

        let mut expired = manifest;
        expired.issued_at = Utc::now() - ChronoDuration::minutes(11);
        expired.expires_at = Utc::now() - ChronoDuration::minutes(1);
        sign_relay_access_manifest(&key, &mut expired);
        std::fs::write(&path, serde_json::to_vec(&expired).unwrap()).unwrap();
        let access = SignedManifestAccess::new(&uncached).unwrap();
        assert!(access.check_access().await.is_err());
    }

    #[tokio::test]
    async fn owner_relay_carries_quic_when_ip_transports_are_disabled() {
        const TEST_ALPN: &[u8] = b"rampage.owner-relay-proof.v1";
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("relay-access.json");
        let governor = SigningKey::from_bytes(&[51_u8; 32]);
        let client_key = SecretKey::from_bytes(&[52_u8; 32]);
        let server_key = SecretKey::from_bytes(&[53_u8; 32]);
        let now = Utc::now();
        let public_key = hex::encode(governor.verifying_key().to_bytes());
        let mut manifest = RelayAccessManifestV1 {
            schema: RelayAccessManifestV1::SCHEMA.into(),
            fabric_id: relay_fabric_id(&public_key).unwrap(),
            generation: 1,
            allowed_endpoint_ids: BTreeSet::from([
                client_key.public().to_string(),
                server_key.public().to_string(),
            ]),
            issued_at: now,
            expires_at: now + ChronoDuration::minutes(10),
            signature: String::new(),
        };
        sign_relay_access_manifest(&governor, &mut manifest);
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let config = file_config(path, public_key);
        let access = Arc::new(SignedManifestAccess::new(&config).unwrap());

        let mut relay_config = RelayConfig::new("127.0.0.1:0".parse::<SocketAddr>().unwrap());
        relay_config.access = access;
        let mut server_config = ServerConfig::default();
        server_config.relay = Some(relay_config);
        let relay = Server::spawn(server_config).await.unwrap();
        let relay_url: RelayUrl = format!("http://{}", relay.http_addr().unwrap())
            .parse()
            .unwrap();
        let relay_mode = RelayMode::custom(vec![relay_url]);

        let client = Endpoint::builder(presets::Minimal)
            .clear_ip_transports()
            .clear_address_lookup()
            .relay_mode(relay_mode.clone())
            .secret_key(client_key)
            .bind()
            .await
            .unwrap();
        let worker = Endpoint::builder(presets::Minimal)
            .clear_ip_transports()
            .clear_address_lookup()
            .relay_mode(relay_mode)
            .secret_key(server_key)
            .alpns(vec![TEST_ALPN.to_vec()])
            .bind()
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), client.online())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), worker.online())
            .await
            .unwrap();

        let worker_server = worker.clone();
        let echo = tokio::spawn(async move {
            let connection = worker_server.accept().await.unwrap().await.unwrap();
            let (mut send, mut receive) = connection.accept_bi().await.unwrap();
            let payload = receive.read_to_end(1024).await.unwrap();
            send.write_all(&payload).await.unwrap();
            send.finish().unwrap();
            connection.closed().await;
        });
        let connection = client.connect(worker.addr(), TEST_ALPN).await.unwrap();
        let (mut send, mut receive) = connection.open_bi().await.unwrap();
        send.write_all(b"RAMPAGE_RELAY_OK").await.unwrap();
        send.finish().unwrap();
        assert_eq!(
            receive.read_to_end(1024).await.unwrap(),
            b"RAMPAGE_RELAY_OK"
        );
        connection.close(0_u8.into(), b"complete");
        echo.await.unwrap();
        client.close().await;
        worker.close().await;
        relay.shutdown().await.unwrap();
    }
}
