//! Rampage-owned peer mesh facade.
//!
//! Iroh supplies audited QUIC, authenticated endpoint identities, NAT traversal, and relay wire
//! primitives. Rampage owns enrollment, peer authorization, discovery records, relay selection,
//! capability policy, and every application protocol. No Tailscale account or control plane is
//! involved, and public/default relays are deliberately never selected by this crate.

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, SecretKey, TransportAddr,
    endpoint::{Connection, RecvStream, SendStream, presets},
};
use rampage_protocol::{
    ARTIFACT_TRANSFER_CHUNK_BYTES, ArtifactRefV1, ArtifactReplicaReceiptV1,
    ArtifactTransferActionV2, ArtifactTransferOperation, ArtifactTransferProgressV1,
    ArtifactTransferRequestV2, ArtifactTransferResponseV2, MeshControlRequestV1,
    MeshControlResponseV1, MeshEndpointRecordV1, ModelInvocationFrameV1, ModelInvocationRequestV1,
    StorageLeaseV1,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use thiserror::Error;

pub const CONTROL_ALPN: &[u8] = b"rampage.mesh.control.v1";
pub const ARTIFACT_ALPN: &[u8] = b"rampage.mesh.artifact.v2";
pub const MODEL_ALPN: &[u8] = b"rampage.mesh.model.v1";
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
    #[error("model invocation frame is invalid or too large")]
    InvalidModelFrame,
    #[error("model worker rejected the invocation: {0}")]
    ModelRejected(String),
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
        .alpns(vec![
            CONTROL_ALPN.to_vec(),
            ARTIFACT_ALPN.to_vec(),
            MODEL_ALPN.to_vec(),
        ])
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
    endpoint_addr_from_record_inner(record)
}

/// Resolve a controller route pinned during a successful one-time enrollment.
///
/// The advertisement expiry is deliberately not reinterpreted as an authority grant here. The
/// endpoint public key remains the transport trust anchor, while the controller still requires an
/// enrolled peer and a fresh Governor lease for every operation. This permits restart after the
/// original ten-minute invitation has expired without trusting a new endpoint identity.
pub fn endpoint_addr_from_pinned_record(
    record: &MeshEndpointRecordV1,
) -> Result<EndpointAddr, MeshError> {
    if record.schema != MeshEndpointRecordV1::SCHEMA {
        return Err(MeshError::InvalidEndpointRecord);
    }
    endpoint_addr_from_record_inner(record)
}

fn endpoint_addr_from_record_inner(
    record: &MeshEndpointRecordV1,
) -> Result<EndpointAddr, MeshError> {
    if record.direct_addresses.len() > 16
        || record.relay_urls.len() > 16
        || record
            .direct_addresses
            .iter()
            .chain(&record.relay_urls)
            .any(|address| address.len() > 2_048)
    {
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

pub async fn read_model_request(
    receive: &mut RecvStream,
) -> anyhow::Result<ModelInvocationRequestV1> {
    let request: ModelInvocationRequestV1 = read_header(receive).await?;
    anyhow::ensure!(
        request.schema == ModelInvocationRequestV1::SCHEMA,
        MeshError::InvalidModelFrame
    );
    Ok(request)
}

pub async fn write_model_frame(
    send: &mut SendStream,
    frame: &ModelInvocationFrameV1,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        frame.schema == ModelInvocationFrameV1::SCHEMA,
        MeshError::InvalidModelFrame
    );
    write_header(send, frame).await
}

pub struct ModelResponseStream {
    _connection: Connection,
    receive: RecvStream,
    request_id: uuid::Uuid,
}

impl ModelResponseStream {
    pub async fn next_frame(&mut self) -> anyhow::Result<ModelInvocationFrameV1> {
        let frame: ModelInvocationFrameV1 = read_header(&mut self.receive).await?;
        anyhow::ensure!(
            frame.schema == ModelInvocationFrameV1::SCHEMA && frame.request_id == self.request_id,
            MeshError::InvalidModelFrame
        );
        Ok(frame)
    }
}

pub async fn invoke_model(
    endpoint: &Endpoint,
    destination: EndpointAddr,
    request: &ModelInvocationRequestV1,
) -> anyhow::Result<ModelResponseStream> {
    anyhow::ensure!(
        request.schema == ModelInvocationRequestV1::SCHEMA,
        MeshError::InvalidModelFrame
    );
    let connection = endpoint.connect(destination, MODEL_ALPN).await?;
    let (mut send, receive) = connection.open_bi().await?;
    write_header(&mut send, request).await?;
    send.finish()?;
    Ok(ModelResponseStream {
        _connection: connection,
        receive,
        request_id: request.request_id,
    })
}

pub async fn read_artifact_request(
    receive: &mut RecvStream,
) -> anyhow::Result<(ArtifactTransferRequestV2, Vec<u8>)> {
    let request: ArtifactTransferRequestV2 = read_header(receive).await?;
    anyhow::ensure!(request.is_valid(), MeshError::InvalidArtifactHeader);
    let payload = if request.action == ArtifactTransferActionV2::PutChunk {
        let mut payload = vec![0_u8; request.payload_size as usize];
        receive.read_exact(&mut payload).await?;
        payload
    } else {
        Vec::new()
    };
    Ok((request, payload))
}

pub async fn write_artifact_response(
    send: &mut SendStream,
    response: &ArtifactTransferResponseV2,
    payload: &[u8],
) -> anyhow::Result<()> {
    anyhow::ensure!(
        response.payload_size == payload.len() as u64
            && response.payload_size <= u64::from(ARTIFACT_TRANSFER_CHUNK_BYTES),
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
    request: &ArtifactTransferRequestV2,
    payload: &[u8],
) -> anyhow::Result<(ArtifactTransferResponseV2, Vec<u8>)> {
    anyhow::ensure!(request.is_valid(), MeshError::InvalidArtifactHeader);
    let expected_request_payload = if request.action == ArtifactTransferActionV2::PutChunk {
        request.payload_size
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
    let response: ArtifactTransferResponseV2 = read_header(&mut receive).await?;
    anyhow::ensure!(
        response.schema == ArtifactTransferResponseV2::SCHEMA
            && response.request_id == request.request_id
            && response.payload_size <= u64::from(ARTIFACT_TRANSFER_CHUNK_BYTES),
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

#[derive(Clone)]
pub struct ArtifactTransferContext {
    pub destination: EndpointAddr,
    pub lease: StorageLeaseV1,
    pub media_type: String,
    pub session_id: uuid::Uuid,
    pub challenge_nonce: String,
}

pub async fn artifact_put(
    endpoint: &Endpoint,
    context: ArtifactTransferContext,
) -> anyhow::Result<ArtifactTransferProgressV1> {
    anyhow::ensure!(
        context.lease.operation == ArtifactTransferOperation::Put,
        MeshError::InvalidArtifactHeader
    );
    let request = ArtifactTransferRequestV2 {
        schema: ArtifactTransferRequestV2::SCHEMA.into(),
        request_id: uuid::Uuid::now_v7(),
        session_id: context.session_id,
        lease: context.lease,
        media_type: context.media_type,
        action: ArtifactTransferActionV2::Begin,
        chunk_size: ARTIFACT_TRANSFER_CHUNK_BYTES,
        chunk_index: None,
        chunk_digest: None,
        payload_size: 0,
        challenge_nonce: context.challenge_nonce,
    };
    let (response, response_payload) =
        artifact_request(endpoint, context.destination, &request, &[]).await?;
    anyhow::ensure!(response_payload.is_empty(), MeshError::ArtifactSizeMismatch);
    response
        .progress
        .ok_or_else(|| MeshError::InvalidArtifactHeader.into())
}

pub async fn artifact_put_chunk(
    endpoint: &Endpoint,
    context: ArtifactTransferContext,
    chunk_index: u32,
    chunk_digest: String,
    chunk: &[u8],
) -> anyhow::Result<ArtifactTransferProgressV1> {
    anyhow::ensure!(
        context.lease.operation == ArtifactTransferOperation::Put,
        MeshError::InvalidArtifactHeader
    );
    let request = ArtifactTransferRequestV2 {
        schema: ArtifactTransferRequestV2::SCHEMA.into(),
        request_id: uuid::Uuid::now_v7(),
        session_id: context.session_id,
        lease: context.lease,
        media_type: context.media_type,
        action: ArtifactTransferActionV2::PutChunk,
        chunk_size: ARTIFACT_TRANSFER_CHUNK_BYTES,
        chunk_index: Some(chunk_index),
        chunk_digest: Some(chunk_digest),
        payload_size: chunk.len() as u64,
        challenge_nonce: context.challenge_nonce,
    };
    let (response, response_payload) =
        artifact_request(endpoint, context.destination, &request, chunk).await?;
    anyhow::ensure!(response_payload.is_empty(), MeshError::ArtifactSizeMismatch);
    response
        .progress
        .ok_or_else(|| MeshError::InvalidArtifactHeader.into())
}

pub async fn artifact_commit(
    endpoint: &Endpoint,
    context: ArtifactTransferContext,
) -> anyhow::Result<(ArtifactRefV1, ArtifactReplicaReceiptV1)> {
    anyhow::ensure!(
        context.lease.operation == ArtifactTransferOperation::Put,
        MeshError::InvalidArtifactHeader
    );
    let request = ArtifactTransferRequestV2 {
        schema: ArtifactTransferRequestV2::SCHEMA.into(),
        request_id: uuid::Uuid::now_v7(),
        session_id: context.session_id,
        lease: context.lease,
        media_type: context.media_type,
        action: ArtifactTransferActionV2::Commit,
        chunk_size: ARTIFACT_TRANSFER_CHUNK_BYTES,
        chunk_index: None,
        chunk_digest: None,
        payload_size: 0,
        challenge_nonce: context.challenge_nonce,
    };
    let (response, payload) =
        artifact_request(endpoint, context.destination, &request, &[]).await?;
    anyhow::ensure!(payload.is_empty(), MeshError::ArtifactSizeMismatch);
    Ok((
        response.artifact.ok_or(MeshError::InvalidArtifactHeader)?,
        response
            .replica_receipt
            .ok_or(MeshError::InvalidArtifactHeader)?,
    ))
}

pub async fn artifact_get_chunk(
    endpoint: &Endpoint,
    context: ArtifactTransferContext,
    chunk_index: u32,
) -> anyhow::Result<(ArtifactRefV1, String, Vec<u8>)> {
    anyhow::ensure!(
        context.lease.operation == ArtifactTransferOperation::Get,
        MeshError::InvalidArtifactHeader
    );
    let request = ArtifactTransferRequestV2 {
        schema: ArtifactTransferRequestV2::SCHEMA.into(),
        request_id: uuid::Uuid::now_v7(),
        session_id: context.session_id,
        lease: context.lease,
        media_type: context.media_type,
        action: ArtifactTransferActionV2::GetChunk,
        chunk_size: ARTIFACT_TRANSFER_CHUNK_BYTES,
        chunk_index: Some(chunk_index),
        chunk_digest: None,
        payload_size: 0,
        challenge_nonce: context.challenge_nonce,
    };
    let (response, payload) =
        artifact_request(endpoint, context.destination, &request, &[]).await?;
    anyhow::ensure!(
        response.chunk_index == Some(chunk_index)
            && response.chunk_digest.is_some()
            && response.payload_size == payload.len() as u64,
        MeshError::ArtifactSizeMismatch
    );
    Ok((
        response.artifact.ok_or(MeshError::InvalidArtifactHeader)?,
        response
            .chunk_digest
            .ok_or(MeshError::InvalidArtifactHeader)?,
        payload,
    ))
}

pub async fn artifact_head(
    endpoint: &Endpoint,
    context: ArtifactTransferContext,
) -> anyhow::Result<(ArtifactRefV1, ArtifactReplicaReceiptV1)> {
    anyhow::ensure!(
        context.lease.operation == ArtifactTransferOperation::Get,
        MeshError::InvalidArtifactHeader
    );
    let request = ArtifactTransferRequestV2 {
        schema: ArtifactTransferRequestV2::SCHEMA.into(),
        request_id: uuid::Uuid::now_v7(),
        session_id: context.session_id,
        lease: context.lease,
        media_type: context.media_type,
        action: ArtifactTransferActionV2::Head,
        chunk_size: ARTIFACT_TRANSFER_CHUNK_BYTES,
        chunk_index: None,
        chunk_digest: None,
        payload_size: 0,
        challenge_nonce: context.challenge_nonce,
    };
    let (response, payload) =
        artifact_request(endpoint, context.destination, &request, &[]).await?;
    anyhow::ensure!(payload.is_empty(), MeshError::ArtifactSizeMismatch);
    Ok((
        response.artifact.ok_or(MeshError::InvalidArtifactHeader)?,
        response
            .replica_receipt
            .ok_or(MeshError::InvalidArtifactHeader)?,
    ))
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
    use chrono::{Duration, Utc};
    use rampage_protocol::{
        JobState, ModelBackend, ModelChatMessageV1, ModelExecutionReceiptV1,
        ModelInvocationFrameKind, ModelParallelism, ModelSessionLeaseV1,
    };

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

    #[tokio::test]
    async fn enrolled_endpoint_pin_survives_advertisement_expiry_without_replacing_identity() {
        let endpoint = bind_endpoint([18_u8; 32], &MeshConfig::default())
            .await
            .unwrap();
        let now = Utc::now();
        let mut record = MeshEndpointRecordV1 {
            schema: MeshEndpointRecordV1::SCHEMA.into(),
            endpoint_id: endpoint.id().to_string(),
            direct_addresses: endpoint
                .bound_sockets()
                .into_iter()
                .map(|address| address.to_string())
                .collect(),
            relay_urls: Vec::new(),
            issued_at: now - Duration::minutes(2),
            expires_at: now - Duration::minutes(1),
            signature: "enrollment-verified".into(),
        };
        assert!(matches!(
            endpoint_addr_from_record(&record),
            Err(MeshError::InvalidEndpointRecord)
        ));
        assert!(endpoint_addr_from_pinned_record(&record).is_ok());

        record.endpoint_id = "replacement-identity".into();
        assert!(matches!(
            endpoint_addr_from_pinned_record(&record),
            Err(MeshError::InvalidEndpointRecord)
        ));
        endpoint.close().await;
    }

    #[tokio::test]
    async fn model_protocol_streams_bounded_frames_over_authenticated_quic() {
        let controller = bind_endpoint([31_u8; 32], &MeshConfig::default())
            .await
            .unwrap();
        let worker = bind_endpoint([32_u8; 32], &MeshConfig::default())
            .await
            .unwrap();
        let request_id = uuid::Uuid::now_v7();
        let now = Utc::now();
        let lease = ModelSessionLeaseV1 {
            schema: ModelSessionLeaseV1::SCHEMA.into(),
            lease_id: uuid::Uuid::now_v7(),
            session_id: uuid::Uuid::now_v7(),
            node_id: uuid::Uuid::now_v7(),
            controller_endpoint_id: controller.id().to_string(),
            model_id: "test".into(),
            model_digest: format!("sha256:{}", "a".repeat(64)),
            backend: ModelBackend::LocalOllama,
            runtime_digest: "shipped-local:test".into(),
            parallelism: ModelParallelism::WholeModel,
            max_prompt_bytes: 1024,
            max_output_tokens: 16,
            issued_at: now,
            expires_at: now + Duration::minutes(1),
            nonce: "nonce".into(),
            fencing_epoch: 1,
            signature: "signed".into(),
        };
        let request = ModelInvocationRequestV1 {
            schema: ModelInvocationRequestV1::SCHEMA.into(),
            request_id,
            lease: lease.clone(),
            messages: vec![ModelChatMessageV1 {
                role: "user".into(),
                content: "hello".into(),
            }],
            max_output_tokens: 16,
            stream: true,
            temperature: None,
            top_p: None,
        };
        let worker_server = worker.clone();
        let server = tokio::spawn(async move {
            let connection = worker_server.accept().await.unwrap().await.unwrap();
            assert_eq!(connection.alpn(), MODEL_ALPN);
            let (mut send, mut receive) = connection.accept_bi().await.unwrap();
            let received = read_model_request(&mut receive).await.unwrap();
            assert_eq!(received.request_id, request_id);
            write_model_frame(
                &mut send,
                &ModelInvocationFrameV1 {
                    schema: ModelInvocationFrameV1::SCHEMA.into(),
                    request_id,
                    sequence: 0,
                    kind: ModelInvocationFrameKind::Delta,
                    content: "world".into(),
                    finish_reason: None,
                    error: None,
                    receipt: None,
                },
            )
            .await
            .unwrap();
            write_model_frame(
                &mut send,
                &ModelInvocationFrameV1 {
                    schema: ModelInvocationFrameV1::SCHEMA.into(),
                    request_id,
                    sequence: 1,
                    kind: ModelInvocationFrameKind::Complete,
                    content: String::new(),
                    finish_reason: Some("stop".into()),
                    error: None,
                    receipt: Some(ModelExecutionReceiptV1 {
                        schema: ModelExecutionReceiptV1::SCHEMA.into(),
                        receipt_id: uuid::Uuid::now_v7(),
                        lease_id: lease.lease_id,
                        session_id: lease.session_id,
                        request_id,
                        node_id: lease.node_id,
                        state: JobState::Succeeded,
                        started_at: now,
                        finished_at: now,
                        output_digest: format!("sha256:{}", "b".repeat(64)),
                        output_bytes: 5,
                        usage: None,
                        error: None,
                        signature: "signed".into(),
                    }),
                },
            )
            .await
            .unwrap();
            send.finish().unwrap();
            let _ = send.stopped().await;
        });
        let mut response = invoke_model(&controller, worker.addr(), &request)
            .await
            .unwrap();
        assert_eq!(
            response.next_frame().await.unwrap().kind,
            ModelInvocationFrameKind::Delta
        );
        assert_eq!(
            response.next_frame().await.unwrap().kind,
            ModelInvocationFrameKind::Complete
        );
        server.await.unwrap();
        controller.close().await;
        worker.close().await;
    }
}
