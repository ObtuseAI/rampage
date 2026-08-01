use anyhow::Context;
use clap::{Parser, Subcommand};
use rampage_relay::{OwnerRelayConfigV1, SignedManifestAccess, spawn_owner_relay};
use serde::Deserialize;
use std::{net::SocketAddr, path::PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_CONTROLLER_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Parser)]
#[command(
    name = "rampage-relay",
    version,
    about = "Owner-operated Rampage relay"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate a fail-closed reverse-proxy relay configuration from a local controller.
    Init {
        #[arg(long)]
        public_url: String,
        #[arg(long, default_value = "rampage-relay.json")]
        config: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:47831")]
        controller_url: String,
        #[arg(long, default_value = ".rampage/runtime/controller.token")]
        controller_token_file: PathBuf,
        #[arg(long, default_value = "127.0.0.1:3340")]
        bind: SocketAddr,
    },
    /// Validate configuration and obtain a fresh Governor-signed access snapshot.
    Check {
        #[arg(long, default_value = "rampage-relay.json")]
        config: PathBuf,
    },
    /// Serve the private relay until Ctrl+C or an internal server failure.
    Serve {
        #[arg(long, default_value = "rampage-relay.json")]
        config: PathBuf,
    },
}

#[derive(Debug, Deserialize)]
struct GovernorKeyResponse {
    public_key: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    match Cli::parse().command {
        Command::Init {
            public_url,
            config,
            controller_url,
            controller_token_file,
            bind,
        } => {
            let base = validated_controller_base(&controller_url)?;
            let governor_url = base.join("/v1/governor/key")?;
            let manifest_url = base.join("/v1/mesh/relay-access")?;
            let token = read_controller_token(&controller_token_file).await?;
            let token = token.trim();
            anyhow::ensure!(
                !token.is_empty() && token.len() <= 512,
                "controller token is invalid"
            );
            let client = reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(3))
                .timeout(std::time::Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            let response = client
                .get(governor_url)
                .header("x-rampage-token", token)
                .send()
                .await
                .context("could not reach the local Rampage controller")?;
            anyhow::ensure!(
                response.status().is_success(),
                "controller denied Governor key discovery"
            );
            anyhow::ensure!(
                response.content_length().unwrap_or(0) <= MAX_CONTROLLER_RESPONSE_BYTES as u64,
                "controller Governor key response is oversized"
            );
            let governor: GovernorKeyResponse =
                serde_json::from_slice(&bounded_response(response).await?)?;
            let owner_config = OwnerRelayConfigV1::reverse_proxy(
                public_url,
                governor.public_key,
                manifest_url.to_string(),
                controller_token_file,
                bind,
            );
            owner_config.validate()?;
            SignedManifestAccess::new(&owner_config)?
                .check_access()
                .await
                .context("controller did not provide a valid signed relay manifest")?;
            let encoded = serde_json::to_vec_pretty(&owner_config)?;
            let temporary = config.with_extension("json.tmp");
            anyhow::ensure!(
                std::fs::symlink_metadata(&config).is_err(),
                "refusing to overwrite an existing relay configuration"
            );
            let mut output = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .await
                .context("could not create relay configuration staging file")?;
            output.write_all(&encoded).await?;
            output.sync_all().await?;
            drop(output);
            if let Err(error) = tokio::fs::rename(&temporary, &config).await {
                let _ = tokio::fs::remove_file(&temporary).await;
                return Err(error).context("could not publish relay configuration");
            }
            println!(
                "{}",
                serde_json::json!({
                    "result": "READY",
                    "config": config,
                    "public_url": owner_config.public_url,
                    "bind": owner_config.http_bind_addr,
                    "next": format!("rampage-relay serve --config {}", config.display())
                })
            );
        }
        Command::Check { config } => {
            let config = OwnerRelayConfigV1::load(config)?;
            let manifest = SignedManifestAccess::new(&config)?.check_access().await?;
            println!(
                "{}",
                serde_json::json!({
                    "result": "PASS",
                    "fabric_id": manifest.fabric_id,
                    "generation": manifest.generation,
                    "authorized_endpoints": manifest.allowed_endpoint_ids.len(),
                    "expires_at": manifest.expires_at,
                    "public_url": config.public_url
                })
            );
        }
        Command::Serve { config } => {
            let config = OwnerRelayConfigV1::load(config)?;
            let mut relay = spawn_owner_relay(&config).await?;
            println!(
                "{}",
                serde_json::json!({
                    "result": "READY",
                    "public_url": config.public_url,
                    "http_addr": relay.http_addr(),
                    "https_addr": relay.https_addr(),
                    "quic_addr": relay.quic_addr(),
                    "access": "governor_signed_endpoint_allowlist",
                    "public_default_relays": false
                })
            );
            tokio::select! {
                signal = tokio::signal::ctrl_c() => signal.context("relay signal handler failed")?,
                result = relay.join() => {
                    result.context("relay supervisor task failed")??;
                }
            }
            relay.shutdown().await.context("relay shutdown failed")?;
        }
    }
    Ok(())
}

fn validated_controller_base(input: &str) -> anyhow::Result<reqwest::Url> {
    let parsed = reqwest::Url::parse(input).context("controller URL is invalid")?;
    let host = parsed
        .host_str()
        .context("controller URL is missing a host")?;
    let ip: std::net::IpAddr = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .context("controller URL host must be an explicit loopback IP")?;
    anyhow::ensure!(
        matches!(parsed.scheme(), "http" | "https")
            && ip.is_loopback()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none()
            && matches!(parsed.path(), "" | "/"),
        "controller URL must be a plain loopback IP origin"
    );
    Ok(parsed)
}

async fn bounded_response(mut response: reqwest::Response) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            bytes.len().saturating_add(chunk.len()) <= MAX_CONTROLLER_RESPONSE_BYTES,
            "controller Governor key response is oversized"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn read_controller_token(path: &std::path::Path) -> anyhow::Result<String> {
    let file = tokio::fs::File::open(path)
        .await
        .context("could not open controller token file")?;
    let metadata = file
        .metadata()
        .await
        .context("could not inspect controller token file")?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= 512,
        "controller token file is not a bounded regular file"
    );
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    tokio::io::AsyncReadExt::take(file, 513)
        .read_to_end(&mut bytes)
        .await?;
    anyhow::ensure!(bytes.len() <= 512, "controller token file is oversized");
    String::from_utf8(bytes).context("controller token file is not UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_origin_is_validated_before_token_use() {
        assert!(validated_controller_base("http://127.0.0.1:47831").is_ok());
        assert!(validated_controller_base("http://[::1]:47831/").is_ok());
        assert!(validated_controller_base("https://example.test").is_err());
        assert!(validated_controller_base("http://127.0.0.1:47831/redirect").is_err());
        assert!(validated_controller_base("http://token@127.0.0.1:47831").is_err());
    }
}
