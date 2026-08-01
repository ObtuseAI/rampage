//! Typed Rust client for the loopback Rampage controller API.

use anyhow::Context;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rampage_protocol::{
    ArtifactRefV1, CapabilityLeaseV1, EnrollmentInviteV1, ExecutionReceiptV1, JobSpecV1,
    ResourceOfferV1, StorageClass,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

#[derive(Clone)]
pub struct RampageClient {
    base_url: String,
    http: reqwest::Client,
    token: Option<String>,
}

impl RampageClient {
    pub fn loopback() -> Self {
        Self::new("http://127.0.0.1:47831").expect("static loopback URL is valid")
    }

    pub fn new(base_url: impl Into<String>) -> anyhow::Result<Self> {
        Self::new_with_token(base_url, std::env::var("RAMPAGE_TOKEN").ok())
    }

    pub fn new_with_token(
        base_url: impl Into<String>,
        token: Option<String>,
    ) -> anyhow::Result<Self> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let parsed = reqwest::Url::parse(&base_url)?;
        anyhow::ensure!(
            parsed
                .host_str()
                .is_some_and(|host| host == "127.0.0.1" || host == "localhost"),
            "the public controller SDK only connects to the loopback API"
        );
        Ok(Self {
            base_url,
            http: reqwest::Client::new(),
            token: token.filter(|value| !value.trim().is_empty()),
        })
    }

    pub async fn health(&self) -> anyhow::Result<Value> {
        self.get("/health").await
    }

    pub async fn create_invite(&self) -> anyhow::Result<EnrollmentInviteV1> {
        self.post("/v1/enrollment/invites", &serde_json::json!({}))
            .await
    }

    pub async fn plan(&self, job: &JobSpecV1) -> anyhow::Result<Value> {
        self.post("/v1/jobs/plan", job).await
    }

    pub async fn topology(&self) -> anyhow::Result<Vec<ResourceOfferV1>> {
        self.get("/v1/offers").await
    }

    pub async fn submit(&self, job: &JobSpecV1) -> anyhow::Result<CapabilityLeaseV1> {
        self.post("/v1/jobs", job).await
    }

    pub async fn receipts(&self, job_id: uuid::Uuid) -> anyhow::Result<Vec<ExecutionReceiptV1>> {
        self.get(&format!("/v1/receipts?job_id={job_id}")).await
    }

    pub async fn put_artifact(
        &self,
        payload: &[u8],
        media_type: &str,
        storage_class: StorageClass,
    ) -> anyhow::Result<ArtifactRefV1> {
        self.post(
            "/v1/artifacts/put",
            &serde_json::json!({
                "data_base64": BASE64.encode(payload),
                "media_type": media_type,
                "storage_class": storage_class
            }),
        )
        .await
    }

    pub async fn get_artifact(&self, digest: &str) -> anyhow::Result<Vec<u8>> {
        let response: Value = self
            .get(&format!("/v1/artifacts/get?digest={digest}"))
            .await?;
        BASE64
            .decode(
                response
                    .get("data_base64")
                    .and_then(Value::as_str)
                    .context("artifact response omitted data_base64")?,
            )
            .context("artifact response was not valid base64")
    }

    pub async fn replicate_artifact(
        &self,
        digest: &str,
        node_id: uuid::Uuid,
        media_type: &str,
        storage_class: StorageClass,
    ) -> anyhow::Result<Value> {
        self.post(
            "/v1/artifacts/replicate",
            &serde_json::json!({
                "digest": digest,
                "node_id": node_id,
                "media_type": media_type,
                "storage_class": storage_class
            }),
        )
        .await
    }

    pub async fn retrieve_artifact(
        &self,
        digest: &str,
        node_id: uuid::Uuid,
    ) -> anyhow::Result<Vec<u8>> {
        let response: Value = self
            .post(
                "/v1/artifacts/retrieve",
                &serde_json::json!({"digest": digest, "node_id": node_id}),
            )
            .await?;
        BASE64
            .decode(
                response
                    .get("data_base64")
                    .and_then(Value::as_str)
                    .context("artifact response omitted data_base64")?,
            )
            .context("artifact response was not valid base64")
    }

    pub async fn events(&self, after: u64) -> anyhow::Result<Vec<Value>> {
        self.get(&format!("/v1/events?after={after}")).await
    }

    pub async fn stop(&self) -> anyhow::Result<Value> {
        self.post("/v1/stop", &serde_json::json!({})).await
    }

    pub async fn resume(&self) -> anyhow::Result<Value> {
        self.post(
            "/v1/resume",
            &serde_json::json!({"confirmation": "OWNER_RESUME"}),
        )
        .await
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let mut request = self.http.get(format!("{}{}", self.base_url, path));
        if let Some(token) = &self.token {
            request = request.header("x-rampage-token", token);
        }
        request
            .send()
            .await
            .with_context(|| format!("Rampage request failed: {path}"))?
            .error_for_status()?
            .json()
            .await
            .context("Rampage returned an invalid response")
    }

    async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &(impl Serialize + ?Sized),
    ) -> anyhow::Result<T> {
        let mut request = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .json(body);
        if let Some(token) = &self.token {
            request = request.header("x-rampage-token", token);
        }
        request
            .send()
            .await
            .with_context(|| format!("Rampage request failed: {path}"))?
            .error_for_status()?
            .json()
            .await
            .context("Rampage returned an invalid response")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_refuses_non_loopback_control_planes() {
        assert!(RampageClient::new("https://control.example.com").is_err());
        assert!(RampageClient::new("http://127.0.0.1:47831").is_ok());
    }
}
