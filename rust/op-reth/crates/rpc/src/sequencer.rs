//! Helpers for optimism specific RPC implementations.

use crate::{SequencerClientError, SequencerMetrics};
use alloy_json_rpc::{RpcRecv, RpcSend};
use alloy_primitives::{B256, hex};
use alloy_rpc_client::{BuiltInConnectionString, ClientBuilder, RpcClient as Client};
use alloy_rpc_types_eth::erc4337::TransactionConditional;
use alloy_transport::RpcError;
use alloy_transport_http::{Http, reqwest as alloy_reqwest};
use std::{
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use thiserror::Error;
use tracing::warn;

/// Sequencer client error
#[derive(Error, Debug)]
pub enum Error {
    /// Invalid scheme
    #[error("Invalid scheme of sequencer url: {0}")]
    InvalidScheme(String),
    /// Invalid header or value provided.
    #[error("Invalid header: {0}")]
    InvalidHeader(String),
    /// Invalid url
    #[error("Invalid sequencer url: {0}")]
    InvalidUrl(String),
    /// No endpoints provided
    #[error("No sequencer endpoints provided")]
    NoEndpoints,
    /// Multiple WebSocket endpoints are not supported for failover
    #[error("Multiple WebSocket endpoints are not supported; use HTTP endpoints for failover")]
    MultipleWsEndpoints,
    /// Establishing a connection to the sequencer endpoint resulted in an error.
    #[error("Failed to connect to sequencer: {0}")]
    TransportError(
        #[from]
        #[source]
        alloy_transport::TransportError,
    ),
    /// Reqwest failed to init client
    #[error("Failed to init reqwest client for sequencer: {0}")]
    ReqwestError(
        #[from]
        #[source]
        alloy_transport_http::reqwest::Error,
    ),
}

/// Configuration for building a [`SequencerClient`].
#[derive(Debug, Clone, Default)]
pub struct SequencerClientConfig {
    /// Optional headers in the form `header=value`.
    pub headers: Vec<String>,
    /// Timeout for establishing a connection to the sequencer endpoint.
    pub dial_timeout: Option<Duration>,
    /// Timeout for individual RPC requests to the sequencer endpoint.
    pub request_timeout: Option<Duration>,
}

/// A client to interact with a Sequencer, supporting multiple endpoints with automatic failover.
#[derive(Debug, Clone)]
pub struct SequencerClient {
    inner: Arc<SequencerClientInner>,
}

impl SequencerClient {
    /// Creates a new [`SequencerClient`] for the given URL(s).
    ///
    /// Accepts a single URL or comma-separated URLs for failover.
    /// If the URL is a websocket endpoint we connect a websocket instance (single endpoint only).
    pub async fn new(sequencer_endpoint: impl Into<String>) -> Result<Self, Error> {
        Self::new_with_headers(sequencer_endpoint, Default::default()).await
    }

    /// Creates a new `SequencerClient` for the given URL(s) with the given headers.
    ///
    /// Accepts a single URL or comma-separated URLs for failover.
    /// This expects headers in the form: `header=value`
    pub async fn new_with_headers(
        sequencer_endpoint: impl Into<String>,
        headers: Vec<String>,
    ) -> Result<Self, Error> {
        Self::new_with_config(
            sequencer_endpoint,
            SequencerClientConfig { headers, ..Default::default() },
        )
        .await
    }

    /// Creates a new [`SequencerClient`] with full configuration.
    ///
    /// Accepts a single URL or comma-separated URLs for failover.
    /// Multiple endpoints require HTTP URLs; WebSocket is only supported for single endpoints.
    pub async fn new_with_config(
        sequencer_endpoint: impl Into<String>,
        config: SequencerClientConfig,
    ) -> Result<Self, Error> {
        let endpoints_str = sequencer_endpoint.into();
        let endpoints: Vec<String> = endpoints_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if endpoints.is_empty() {
            return Err(Error::NoEndpoints);
        }

        // Single endpoint: preserve existing behavior (supports both HTTP and WS)
        if endpoints.len() == 1 {
            let endpoint = &endpoints[0];
            let conn = BuiltInConnectionString::from_str(endpoint)?;

            if let BuiltInConnectionString::Http(url) = conn {
                let http_client = Self::build_http_client(&config)?;
                let client = Self::build_alloy_client_for_url(url, http_client)?;
                let inner = SequencerClientInner {
                    endpoints,
                    clients: vec![client],
                    preferred_index: AtomicUsize::new(0),
                    metrics: SequencerMetrics::default(),
                };
                return Ok(Self { inner: Arc::new(inner) });
            }

            // WebSocket single endpoint
            let client = ClientBuilder::default().connect_with(conn).await?;
            let inner = SequencerClientInner {
                endpoints,
                clients: vec![client],
                preferred_index: AtomicUsize::new(0),
                metrics: SequencerMetrics::default(),
            };
            return Ok(Self { inner: Arc::new(inner) });
        }

        // Multiple endpoints: HTTP only
        // Validate all are HTTP
        for ep in &endpoints {
            let conn = BuiltInConnectionString::from_str(ep)?;
            if !matches!(conn, BuiltInConnectionString::Http(_)) {
                return Err(Error::MultipleWsEndpoints);
            }
        }

        let http_client = Self::build_http_client(&config)?;
        let mut clients = Vec::with_capacity(endpoints.len());
        for ep in &endpoints {
            let conn = BuiltInConnectionString::from_str(ep)?;
            if let BuiltInConnectionString::Http(url) = conn {
                clients.push(Self::build_alloy_client_for_url(url, http_client.clone())?);
            }
        }

        let inner = SequencerClientInner {
            endpoints,
            clients,
            preferred_index: AtomicUsize::new(0),
            metrics: SequencerMetrics::default(),
        };
        Ok(Self { inner: Arc::new(inner) })
    }

    /// Builds a shared `reqwest::Client` with the given config (headers, timeouts, TLS).
    fn build_http_client(config: &SequencerClientConfig) -> Result<alloy_reqwest::Client, Error> {
        let mut builder = alloy_reqwest::Client::builder()
            // we force use tls to prevent native issues
            .use_rustls_tls();

        if let Some(dial_timeout) = config.dial_timeout {
            builder = builder.connect_timeout(dial_timeout);
        }

        if let Some(request_timeout) = config.request_timeout {
            builder = builder.timeout(request_timeout);
        }

        if !config.headers.is_empty() {
            let mut header_map = alloy_reqwest::header::HeaderMap::new();
            for header in &config.headers {
                if let Some((key, value)) = header.split_once('=') {
                    header_map.insert(
                        key.trim()
                            .parse::<alloy_reqwest::header::HeaderName>()
                            .map_err(|err| Error::InvalidHeader(err.to_string()))?,
                        value
                            .trim()
                            .parse::<alloy_reqwest::header::HeaderValue>()
                            .map_err(|err| Error::InvalidHeader(err.to_string()))?,
                    );
                }
            }
            builder = builder.default_headers(header_map);
        }

        Ok(builder.build()?)
    }

    /// Builds an alloy `Client` for a given HTTP URL using the provided reqwest client.
    fn build_alloy_client_for_url(
        url: impl Into<String>,
        http_client: alloy_reqwest::Client,
    ) -> Result<Client, Error> {
        let url_str: String = url.into();
        let parsed_url = url_str.parse().map_err(|_| Error::InvalidUrl(url_str.clone()))?;
        let http = Http::with_client(http_client, parsed_url);
        let is_local = http.guess_local();
        Ok(ClientBuilder::default().transport(http, is_local))
    }

    /// Creates a new [`SequencerClient`] with http transport with the given http client.
    pub fn with_http_client(
        sequencer_endpoint: impl Into<String>,
        client: alloy_reqwest::Client,
    ) -> Result<Self, Error> {
        let sequencer_endpoint: String = sequencer_endpoint.into();
        let alloy_client =
            Self::build_alloy_client_for_url(&sequencer_endpoint, client)?;

        let inner = SequencerClientInner {
            endpoints: vec![sequencer_endpoint],
            clients: vec![alloy_client],
            preferred_index: AtomicUsize::new(0),
            metrics: SequencerMetrics::default(),
        };
        Ok(Self { inner: Arc::new(inner) })
    }

    /// Returns the currently preferred endpoint URL.
    pub fn endpoint(&self) -> &str {
        let idx = self.inner.preferred_index.load(Ordering::Relaxed);
        &self.inner.endpoints[idx]
    }

    /// Returns all configured endpoint URLs.
    pub fn endpoints(&self) -> &[String] {
        &self.inner.endpoints
    }

    /// Returns the currently preferred client.
    pub fn client(&self) -> &Client {
        let idx = self.inner.preferred_index.load(Ordering::Relaxed);
        &self.inner.clients[idx]
    }

    /// Returns the number of configured endpoints.
    pub fn endpoint_count(&self) -> usize {
        self.inner.endpoints.len()
    }

    /// Returns a reference to the [`SequencerMetrics`] for tracking client metrics.
    fn metrics(&self) -> &SequencerMetrics {
        &self.inner.metrics
    }

    /// Determines whether the given RPC error should trigger a failover to the next endpoint.
    ///
    /// Transport-level errors (network failures, timeouts, connection issues) trigger failover.
    /// Application-level errors (JSON-RPC error responses from the sequencer) do not.
    fn should_failover<E>(err: &RpcError<E>) -> bool {
        match err {
            // Application-level JSON-RPC error from the sequencer (e.g., nonce too low)
            RpcError::ErrorResp(_) => false,
            // Transport-level errors (network, timeout, connection)
            RpcError::Transport(_) => true,
            // Server returned null — likely broken endpoint
            RpcError::NullResp => true,
            // Partial or corrupt response
            RpcError::DeserError { .. } => true,
            // Local serialization bug — retrying won't help
            RpcError::SerError(_) => false,
            // Local pre-processing error
            RpcError::LocalUsageError(_) => false,
            // Feature not supported
            RpcError::UnsupportedFeature(_) => false,
        }
    }

    /// Sends an RPC request to the sequencer, with automatic failover across endpoints.
    ///
    /// Tries the preferred endpoint first, then cycles through remaining endpoints on
    /// transport-level errors. Application-level errors are returned immediately without
    /// failover.
    pub async fn request<Params: RpcSend + Clone, Resp: RpcRecv>(
        &self,
        method: &str,
        params: Params,
    ) -> Result<Resp, SequencerClientError> {
        let inner = &self.inner;
        let num_endpoints = inner.endpoints.len();
        let start_index = inner.preferred_index.load(Ordering::Relaxed);

        inner.metrics.total_calls.increment(1);

        for attempt in 0..num_endpoints {
            let index = (start_index + attempt) % num_endpoints;
            let client = &inner.clients[index];

            match client.request::<Params, Resp>(method.to_string(), params.clone()).await {
                Ok(resp) => {
                    // Update sticky preference if we failed over to a new endpoint
                    if attempt > 0 {
                        inner.preferred_index.store(index, Ordering::Relaxed);
                        inner.metrics.failovers.increment(1);
                        inner.metrics.preferred_endpoint.set(index as f64);
                    }
                    inner.metrics.successes.increment(1);
                    return Ok(resp);
                }
                Err(err) => {
                    if Self::should_failover(&err) && attempt + 1 < num_endpoints {
                        warn!(
                            target: "rpc::sequencer",
                            %err,
                            endpoint = %inner.endpoints[index],
                            attempt = attempt + 1,
                            "Sequencer request failed, trying next endpoint",
                        );
                        continue;
                    }

                    // Either a non-failover error, or we've exhausted all endpoints
                    if Self::should_failover(&err) {
                        // Last endpoint also failed with transport error
                        warn!(
                            target: "rpc::sequencer",
                            %err,
                            endpoint = %inner.endpoints[index],
                            "All sequencer endpoints failed",
                        );
                        inner.metrics.failures.increment(1);
                        return Err(SequencerClientError::AllEndpointsFailed);
                    }

                    // Application-level error — return as-is
                    warn!(
                        target: "rpc::sequencer",
                        %err,
                        endpoint = %inner.endpoints[index],
                        "HTTP request to sequencer failed",
                    );
                    inner.metrics.failures.increment(1);
                    return Err(err.into());
                }
            }
        }

        // Should not reach here, but handle gracefully
        inner.metrics.failures.increment(1);
        Err(SequencerClientError::AllEndpointsFailed)
    }

    /// Forwards a transaction to the sequencer endpoint.
    pub async fn forward_raw_transaction(&self, tx: &[u8]) -> Result<B256, SequencerClientError> {
        let start = Instant::now();
        let rlp_hex = hex::encode_prefixed(tx);
        let tx_hash =
            self.request("eth_sendRawTransaction", (rlp_hex,)).await.inspect_err(|err| {
                warn!(
                    target: "rpc::eth",
                    %err,
                    "Failed to forward transaction to sequencer",
                );
            })?;
        self.metrics().record_forward_latency(start.elapsed());
        Ok(tx_hash)
    }

    /// Forwards a transaction conditional to the sequencer endpoint.
    pub async fn forward_raw_transaction_conditional(
        &self,
        tx: &[u8],
        condition: TransactionConditional,
    ) -> Result<B256, SequencerClientError> {
        let start = Instant::now();
        let rlp_hex = hex::encode_prefixed(tx);
        let tx_hash = self
            .request("eth_sendRawTransactionConditional", (rlp_hex, condition))
            .await
            .inspect_err(|err| {
                warn!(
                    target: "rpc::eth",
                    %err,
                    "Failed to forward transaction conditional for sequencer",
                );
            })?;
        self.metrics().record_forward_latency(start.elapsed());
        Ok(tx_hash)
    }
}

#[derive(Debug)]
struct SequencerClientInner {
    /// All configured sequencer endpoint URLs.
    endpoints: Vec<String>,
    /// Pre-built alloy RPC clients, one per endpoint (index-aligned with `endpoints`).
    clients: Vec<Client>,
    /// Index of the currently preferred (sticky) endpoint.
    preferred_index: AtomicUsize,
    /// Metrics for tracking sequencer forwarding.
    metrics: SequencerMetrics,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_json_rpc::ErrorPayload;
    use alloy_primitives::U64;
    use alloy_transport::TransportErrorKind;

    #[tokio::test]
    async fn test_http_body_str() {
        let client = SequencerClient::new("http://localhost:8545").await.unwrap();

        let request = client
            .client()
            .make_request("eth_getBlockByNumber", (U64::from(10),))
            .serialize()
            .unwrap()
            .take_request();
        let body = request.get();

        assert_eq!(
            body,
            r#"{"method":"eth_getBlockByNumber","params":["0xa"],"id":0,"jsonrpc":"2.0"}"#
        );

        let condition = TransactionConditional::default();

        let request = client
            .client()
            .make_request(
                "eth_sendRawTransactionConditional",
                (format!("0x{}", hex::encode("abcd")), condition),
            )
            .serialize()
            .unwrap()
            .take_request();
        let body = request.get();

        assert_eq!(
            body,
            r#"{"method":"eth_sendRawTransactionConditional","params":["0x61626364",{"knownAccounts":{}}],"id":1,"jsonrpc":"2.0"}"#
        );
    }

    #[tokio::test]
    #[ignore = "Start if WS is reachable at ws://localhost:8546"]
    async fn test_ws_body_str() {
        let client = SequencerClient::new("ws://localhost:8546").await.unwrap();

        let request = client
            .client()
            .make_request("eth_getBlockByNumber", (U64::from(10),))
            .serialize()
            .unwrap()
            .take_request();
        let body = request.get();

        assert_eq!(
            body,
            r#"{"method":"eth_getBlockByNumber","params":["0xa"],"id":0,"jsonrpc":"2.0"}"#
        );

        let condition = TransactionConditional::default();

        let request = client
            .client()
            .make_request(
                "eth_sendRawTransactionConditional",
                (format!("0x{}", hex::encode("abcd")), condition),
            )
            .serialize()
            .unwrap()
            .take_request();
        let body = request.get();

        assert_eq!(
            body,
            r#"{"method":"eth_sendRawTransactionConditional","params":["0x61626364",{"knownAccounts":{}}],"id":1,"jsonrpc":"2.0"}"#
        );
    }

    #[tokio::test]
    async fn test_multiple_endpoints_parsing() {
        let client = SequencerClient::new("http://localhost:8545,http://localhost:8546")
            .await
            .unwrap();
        assert_eq!(client.endpoint_count(), 2);
        assert_eq!(client.endpoints()[0], "http://localhost:8545");
        assert_eq!(client.endpoints()[1], "http://localhost:8546");
        // Preferred endpoint starts at index 0
        assert_eq!(client.endpoint(), "http://localhost:8545");
    }

    #[tokio::test]
    async fn test_multiple_endpoints_with_whitespace() {
        let client =
            SequencerClient::new("http://localhost:8545 , http://localhost:8546 , http://localhost:8547")
                .await
                .unwrap();
        assert_eq!(client.endpoint_count(), 3);
        assert_eq!(client.endpoints()[0], "http://localhost:8545");
        assert_eq!(client.endpoints()[1], "http://localhost:8546");
        assert_eq!(client.endpoints()[2], "http://localhost:8547");
    }

    #[tokio::test]
    async fn test_single_endpoint_backward_compat() {
        let client = SequencerClient::new("http://localhost:8545").await.unwrap();
        assert_eq!(client.endpoint_count(), 1);
        assert_eq!(client.endpoint(), "http://localhost:8545");
    }

    #[tokio::test]
    async fn test_empty_endpoints_rejected() {
        let result = SequencerClient::new("").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_multiple_ws_endpoints_rejected() {
        let result =
            SequencerClient::new("ws://localhost:8545,ws://localhost:8546").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_should_failover_error_resp() {
        let err: RpcError<TransportErrorKind> = RpcError::ErrorResp(ErrorPayload {
            code: -32000,
            message: "nonce too low".into(),
            data: None,
        });
        assert!(!SequencerClient::should_failover(&err));
    }

    #[test]
    fn test_should_failover_null_resp() {
        let err: RpcError<TransportErrorKind> = RpcError::NullResp;
        assert!(SequencerClient::should_failover(&err));
    }

    #[test]
    fn test_should_failover_deser_error() {
        let err: RpcError<TransportErrorKind> = RpcError::DeserError {
            err: serde_json::from_str::<u32>("not a number").unwrap_err(),
            text: "bad response".to_string(),
        };
        assert!(SequencerClient::should_failover(&err));
    }

    #[test]
    fn test_should_failover_ser_error() {
        let err: RpcError<TransportErrorKind> =
            RpcError::SerError(serde_json::from_str::<u32>("not a number").unwrap_err());
        assert!(!SequencerClient::should_failover(&err));
    }

    #[test]
    fn test_should_failover_unsupported_feature() {
        let err: RpcError<TransportErrorKind> = RpcError::UnsupportedFeature("batch");
        assert!(!SequencerClient::should_failover(&err));
    }

    #[test]
    fn test_should_failover_transport_error() {
        // backend_gone() returns RpcError::Transport(TransportErrorKind::BackendGone)
        let err = TransportErrorKind::backend_gone();
        assert!(SequencerClient::should_failover(&err));
    }

    #[test]
    fn test_should_failover_local_usage_error() {
        let err: RpcError<TransportErrorKind> =
            RpcError::LocalUsageError(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "local error",
            )));
        assert!(!SequencerClient::should_failover(&err));
    }

    #[tokio::test]
    async fn test_mixed_http_ws_endpoints_rejected() {
        let result =
            SequencerClient::new("http://localhost:8545,ws://localhost:8546").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_trailing_commas_ignored() {
        let client = SequencerClient::new(",http://localhost:8545,,http://localhost:8546,")
            .await
            .unwrap();
        assert_eq!(client.endpoint_count(), 2);
        assert_eq!(client.endpoints()[0], "http://localhost:8545");
        assert_eq!(client.endpoints()[1], "http://localhost:8546");
    }

    #[tokio::test]
    async fn test_with_http_client_constructor() {
        let http_client = alloy_reqwest::Client::builder()
            .use_rustls_tls()
            .build()
            .unwrap();
        let client =
            SequencerClient::with_http_client("http://localhost:8545", http_client).unwrap();
        assert_eq!(client.endpoint_count(), 1);
        assert_eq!(client.endpoint(), "http://localhost:8545");
    }

    #[tokio::test]
    async fn test_new_with_headers_delegates_to_config() {
        let client = SequencerClient::new_with_headers(
            "http://localhost:8545,http://localhost:8546",
            vec!["Authorization=Bearer token".to_string()],
        )
        .await
        .unwrap();
        assert_eq!(client.endpoint_count(), 2);
    }

    #[tokio::test]
    async fn test_config_with_timeouts() {
        let config = SequencerClientConfig {
            headers: vec![],
            dial_timeout: Some(Duration::from_secs(3)),
            request_timeout: Some(Duration::from_secs(5)),
        };
        let client = SequencerClient::new_with_config(
            "http://localhost:8545,http://localhost:8546",
            config,
        )
        .await
        .unwrap();
        assert_eq!(client.endpoint_count(), 2);
    }

}
