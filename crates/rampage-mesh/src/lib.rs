//! Rampage-owned peer mesh facade.
//!
//! Iroh supplies audited QUIC, authenticated endpoint identities, NAT traversal, and relay wire
//! primitives. Rampage owns enrollment, peer authorization, discovery records, relay selection,
//! capability policy, and every application protocol. No Tailscale account or control plane is
//! involved, and public/default relays are deliberately never selected by this crate.

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, SecretKey, TransportAddr,
    endpoint::{RecvStream, SendStream, presets},
};
use rampage_protocol::{
    ArtifactRefV1, ArtifactTransferOperation, ArtifactTransferRequestV1,
    ArtifactTransferResponseV1, MAX_ARTIFACT_TRANSFER_BYTES, MeshControlRequestV1,
    MeshControlResponseV1, MeshEndpointRecordV1, StorageLeaseV1,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use thiserror::Error;

pub const CONTROL_ALPN: &[u8] = b"rampage.mesh.control.v1";
pub const ARTIFACT_ALPN: &[u8] = b"rampage.mesh.artifact.v1";
const MAX_ARTIFACT_HEADER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum MeshMode {
    /// Direct IP/QUIC only. Best for a LAN or manually supplied endpoint addresses.
    LocalOnly,
    /// A relay operated by Rampage or the owner; never the dependency's public relay fleet.
    PrivateRelay { urls: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshConfig {
    pub schema: String,
    pub mode: MeshMode,
    /// Authenticated endpoint public keys admitted by the Rampage enrollment ledger.
    pub allowed_peer_keys: BTreeSet<String>,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            schema: "rampage.mesh-config.v1".into(),
            mode: MeshMode::LocalOnly,
            allowed_peer_keys: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MeshError {
    #[error("unsupported mesh configuration schema")]
    WrongSchema,
    #[error("private relay mode requires at least one HTTPS relay URL")]
    MissingRelay,
    #[error("relay URL is invalid or is not HTTPS: {0}")]
    InvalidRelay(String),
    #[error("peer is not enrolled in this Rampage fabric")]
    PeerDenied,
    #[error("mesh endpoint record has expired or is malformed")]
    InvalidEndpointRecord,
    #[error("mesh response did not match the request")]
    MismatchedResponse,
    #[error("mesh response exceeded the maximum control message size")]
    OversizedResponse,
    #[error("artifact transfer header is invalid or too large")]
    InvalidArtifactHeader,
    #[error("artifact payload size does not match its lease")]
    ArtifactSizeMismatch,
    #[error("artifact peer rejected the transfer: {0}")]
    ArtifactRejected(String),
}

impl MeshConfig {
    pub fn validate(&self) -> Result<(), MeshError> {
        if self.schema != "rampage.mesh-config.v1" {
            return Err(MeshError::WrongSchema);
        }
        if let MeshMode::PrivateRelay { urls } = &self.mode {
            if urls.is_empty() {
                return Err(MeshError::MissingRelay);
            }
            for url in urls {
                if !url.starts_with("https://") || url.parse::<RelayUrl>().is_err() {
                    return Err(MeshError::InvalidRelay(url.clone()));
                }
            }
        }
        Ok(())
    }

    pub fn authorize_peer(&self, authenticated_public_key: &str) -> Result<(), MeshError> {
        if self.allowed_peer_keys.contains(authenticated_public_key) {
            Ok(())
        } else {
            Err(MeshError::PeerDenied)
        }
    }

    fn relay_mode(&self) -> Result<RelayMode, MeshError> {
        self.validate()?;
        match &self.mode {
            MeshMode::LocalOnly => Ok(RelayMode::Disabled),
            MeshMode::PrivateRelay { urls } => {
                let parsed = urls
                    .iter()
                    .map(|url| {
                        url.parse::<RelayUrl>()
                            .map_err(|_| MeshError::InvalidRelay(url.clone()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(RelayMode::custom(parsed))
            }
        }
    }
}

/// Bind an authenticated QUIC endpoint using only direct paths or explicitly configured private
/// relays. Address lookup is empty: the Rampage controller distributes signed endpoint records.
pub async fn bind_endpoint(
    secret_bytes: [u8; 32],
    config: &MeshConfig,
) -> anyhow::Result<Endpoint> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .clear_address_lookup()
        .relay_mode(config.relay_mode()?)
        .secret_key(SecretKey::from_bytes(&secret_bytes))
        .alpns(vec![CONTROL_ALPN.to_vec(), ARTIFACT_ALPN.to_vec()])
        .bind()
        .await
        .map_err(|error| anyhow::anyhow!("mesh bind failed: {error}"))?;
    Ok(endpoint)
}

pub struct MeshNode {
    endpoint: Endpoint,
    mode: &'static str,
}

impl MeshNode {
    pub fn endpoint_id(&self) -> String {
        self.endpoint.id().to_string()
    }

    pub fn bound_sockets(&self) -> Vec<SocketAddr> {
        self.endpoint.bound_sockets()
    }

    pub fn mode(&self) -> &'static str {
        self.mode
    }

    pub fn endpoint(&self) -> Endpoint {
        self.endpoint.clone()
    }

    pub fn endpoint_addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    pub async fn close(self) {
        self.endpoint.close().await;
    }
}

pub fn endpoint_addr_from_record(record: &MeshEndpointRecordV1) -> Result<EndpointAddr, MeshError> {
    if record.schema != MeshEndpointRecordV1::SCHEMA || record.expires_at <= chrono::Utc::now() {
        return Err(MeshError::InvalidEndpointRecord);
    }
    let endpoint_id = record
        .endpoint_id
        .parse::<EndpointId>()
        .map_err(|_| MeshError::InvalidEndpointRecord)?;
    let mut addresses = Vec::new();
    for address in &record.direct_addresses {
        addresses.push(TransportAddr::Ip(
            address
                .parse()
                .map_err(|_| MeshError::InvalidEndpointRecord)?,
        ));
    }
    for relay in &record.relay_urls {
        addresses.push(TransportAddr::Relay(
            relay
                .parse::<RelayUrl>()
                .map_err(|_| MeshError::InvalidEndpointRecord)?,
        ));
    }
    if addresses.is_empty() {
        return Err(MeshError::InvalidEndpointRecord);
    }
    Ok(EndpointAddr::from_parts(endpoint_id, addresses))
}

pub async fn control_request(
    endpoint: &Endpoint,
    destination: EndpointAddr,
    request: &MeshControlRequestV1,
) -> anyhow::Result<MeshControlResponseV1> {
    anyhow::ensure!(
        request.schema == MeshControlRequestV1::SCHEMA,
        "unsupported mesh control request schema"
    );
    let connection = endpoint.connect(destination, CONTROL_ALPN).await?;
    let (mut send, mut receive) = connection.open_bi().await?;
    let encoded = serde_json::to_vec(request)?;
    anyhow::ensure!(
        encoded.len() <= 1024 * 1024,
        "mesh control request is too large"
    );
    send.write_all(&encoded).await?;
    send.finish()?;
    let response_bytes = receive
        .read_to_end(1024 * 1024)
        .await
        .map_err(|_| MeshError::OversizedResponse)?;
    let response: MeshControlResponseV1 = serde_json::from_slice(&response_bytes)?;
    if response.schema != MeshControlResponseV1::SCHEMA || response.request_id != request.request_id
    {
        return Err(MeshError::MismatchedResponse.into());
    }
    connection.close(0_u8.into(), b"complete");
    Ok(response)
}

async fn write_header<T: Serialize>(send: &mut SendStream, value: &T) -> anyhow::Result<()> {
    let encoded = serde_json::to_vec(value)?;
    anyhow::ensure!(
        encoded.len() <= MAX_ARTIFACT_HEADER_BYTES,
        MeshError::InvalidArtifactHeader
    );
    send.write_all(&(encoded.len() as u32).to_be_bytes())
        .await?;
    send.write_all(&encoded).await?;
    Ok(())
}

async fn read_header<T: for<'de> Deserialize<'de>>(receive: &mut RecvStream) -> anyhow::Result<T> {
    let mut length_bytes = [0_u8; 4];
    receive.read_exact(&mut length_bytes).await?;
    let length = u32::from_be_bytes(length_bytes) as usize;
    anyhow::ensure!(
        length <= MAX_ARTIFACT_HEADER_BYTES,
        MeshError::InvalidArtifactHeader
    );
    let mut encoded = vec![0_u8; length];
    receive.read_exact(&mut encoded).await?;
    Ok(serde_json::from_slice(&encoded)?)
}

pub async fn read_artifact_request(
    receive: &mut RecvStream,
) -> anyhow::Result<(ArtifactTransferRequestV1, Vec<u8>)> {
    let request: ArtifactTransferRequestV1 = read_header(receive).await?;
    anyhow::ensure!(
        request.schema == ArtifactTransferRequestV1::SCHEMA
            && request.lease.schema == StorageLeaseV1::SCHEMA
            && request.lease.size_bytes <= MAX_ARTIFACT_TRANSFER_BYTES,
        MeshError::InvalidArtifactHeader
    );
    let payload = if request.lease.operation == ArtifactTransferOperation::Put {
        let mut payload = vec![0_u8; request.lease.size_bytes as usize];
        receive.read_exact(&mut payload).await?;
        payload
    } else {
        Vec::new()
    };
    Ok((request, payload))
}

pub async fn write_artifact_response(
    send: &mut SendStream,
    response: &ArtifactTransferResponseV1,
    payload: &[u8],
) -> anyhow::Result<()> {
    anyhow::ensure!(
        response.payload_size == payload.len() as u64
            && response.payload_size <= MAX_ARTIFACT_TRANSFER_BYTES,
        MeshError::ArtifactSizeMismatch
    );
    write_header(send, response).await?;
    if !payload.is_empty() {
        send.write_all(payload).await?;
    }
    send.finish()?;
    Ok(())
}

async fn artifact_request(
    endpoint: &Endpoint,
    destination: EndpointAddr,
    request: &ArtifactTransferRequestV1,
    payload: &[u8],
) -> anyhow::Result<(ArtifactTransferResponseV1, Vec<u8>)> {
    anyhow::ensure!(
        request.schema == ArtifactTransferRequestV1::SCHEMA
            && request.lease.size_bytes <= MAX_ARTIFACT_TRANSFER_BYTES,
        MeshError::InvalidArtifactHeader
    );
    let expected_request_payload = if request.lease.operation == ArtifactTransferOperation::Put {
        request.lease.size_bytes
    } else {
        0
    };
    anyhow::ensure!(
        payload.len() as u64 == expected_request_payload,
        MeshError::ArtifactSizeMismatch
    );
    let connection = endpoint.connect(destination, ARTIFACT_ALPN).await?;
    let (mut send, mut receive) = connection.open_bi().await?;
    write_header(&mut send, request).await?;
    if !payload.is_empty() {
        send.write_all(payload).await?;
    }
    send.finish()?;
    let response: ArtifactTransferResponseV1 = read_header(&mut receive).await?;
    anyhow::ensure!(
        response.schema == ArtifactTransferResponseV1::SCHEMA
            && response.request_id == request.request_id
            && response.payload_size <= MAX_ARTIFACT_TRANSFER_BYTES,
        MeshError::MismatchedResponse
    );
    if !(200..300).contains(&response.status) {
        return Err(MeshError::ArtifactRejected(
            response
                .error
                .clone()
                .unwrap_or_else(|| "unknown error".into()),
        )
        .into());
    }
    let mut response_payload = vec![0_u8; response.payload_size as usize];
    receive.read_exact(&mut response_payload).await?;
    connection.close(0_u8.into(), b"complete");
    Ok((response, response_payload))
}

pub async fn artifact_put(
    endpoint: &Endpoint,
    destination: EndpointAddr,
    lease: StorageLeaseV1,
    media_type: String,
    payload: &[u8],
) -> anyhow::Result<ArtifactRefV1> {
    anyhow::ensure!(
        lease.operation == ArtifactTransferOperation::Put,
        MeshError::InvalidArtifactHeader
    );
    let request = ArtifactTransferRequestV1 {
        schema: ArtifactTransferRequestV1::SCHEMA.into(),
        request_id: uuid::Uuid::now_v7(),
        lease,
        media_type,
    };
    let (response, response_payload) =
        artifact_request(endpoint, destination, &request, payload).await?;
    anyhow::ensure!(response_payload.is_empty(), MeshError::ArtifactSizeMismatch);
    response
        .artifact
        .ok_or_else(|| MeshError::InvalidArtifactHeader.into())
}

pub async fn artifact_get(
    endpoint: &Endpoint,
    destination: EndpointAddr,
    lease: StorageLeaseV1,
    media_type: String,
) -> anyhow::Result<(ArtifactRefV1, Vec<u8>)> {
    anyhow::ensure!(
        lease.operation == ArtifactTransferOperation::Get,
        MeshError::InvalidArtifactHeader
    );
    let request = ArtifactTransferRequestV1 {
        schema: ArtifactTransferRequestV1::SCHEMA.into(),
        request_id: uuid::Uuid::now_v7(),
        lease,
        media_type,
    };
    let (response, payload) = artifact_request(endpoint, destination, &request, &[]).await?;
    let artifact = response.artifact.ok_or(MeshError::InvalidArtifactHeader)?;
    anyhow::ensure!(
        artifact.size_bytes == payload.len() as u64,
        MeshError::ArtifactSizeMismatch
    );
    Ok((artifact, payload))
}

pub async fn bind_node(secret_bytes: [u8; 32], config: &MeshConfig) -> anyhow::Result<MeshNode> {
    let mode = match config.mode {
        MeshMode::LocalOnly => "local_only",
        MeshMode::PrivateRelay { .. } => "private_relay",
    };
    Ok(MeshNode {
        endpoint: bind_endpoint(secret_bytes, config).await?,
        mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_falls_back_to_a_public_default_relay() {
        let config = MeshConfig::default();
        assert!(matches!(config.relay_mode().unwrap(), RelayMode::Disabled));
    }

    #[test]
    fn custom_relay_must_be_explicit_and_https() {
        let mut config = MeshConfig {
            mode: MeshMode::PrivateRelay { urls: vec![] },
            ..MeshConfig::default()
        };
        assert_eq!(config.validate(), Err(MeshError::MissingRelay));
        config.mode = MeshMode::PrivateRelay {
            urls: vec!["http://relay.example.test".into()],
        };
        assert!(matches!(config.validate(), Err(MeshError::InvalidRelay(_))));
    }

    #[test]
    fn authenticated_transport_identity_is_still_denied_until_enrolled() {
        let mut config = MeshConfig::default();
        assert_eq!(config.authorize_peer("peer-a"), Err(MeshError::PeerDenied));
        config.allowed_peer_keys.insert("peer-a".into());
        assert!(config.authorize_peer("peer-a").is_ok());
    }

    #[tokio::test]
    async fn local_endpoint_binds_without_a_relay_dependency() {
        let endpoint = bind_endpoint([17_u8; 32], &MeshConfig::default())
            .await
            .unwrap();
        endpoint.close().await;
    }
}
