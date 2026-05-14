//! Network connectivity diagnostics for AI providers.
//!
//! Performs lightweight HTTP HEAD requests to provider endpoints
//! to check reachability without consuming API quota.

use crate::context_limits::ProviderHint;

/// Result of a connectivity check against a provider endpoint.
#[derive(Debug, Clone)]
pub struct ConnectivityResult {
    pub endpoint: String,
    pub reachable: bool,
    pub http_status: Option<u16>,
    pub latency_ms: u64,
    pub error: Option<String>,
}

/// Check connectivity to an AI provider endpoint.
///
/// Sends an HTTP HEAD request (no API key, no body) with a 10s timeout.
/// This validates DNS resolution, TCP connectivity, and TLS negotiation
/// without consuming API quota.
pub async fn connectivity_check(
    base_url: Option<&str>,
    provider: ProviderHint,
) -> ConnectivityResult {
    let endpoint = match base_url {
        Some(url) if !url.is_empty() => url.to_string(),
        _ => match provider.default_endpoint() {
            Some(url) => url.to_string(),
            None => {
                return ConnectivityResult {
                    endpoint: "(local/unknown)".into(),
                    reachable: false,
                    http_status: None,
                    latency_ms: 0,
                    error: Some("No endpoint for local/unknown provider".into()),
                };
            }
        },
    };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("MAE/0.9.0")
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return ConnectivityResult {
                endpoint,
                reachable: false,
                http_status: None,
                latency_ms: 0,
                error: Some(format!("HTTP client error: {}", e)),
            };
        }
    };

    let start = std::time::Instant::now();
    match client.head(&endpoint).send().await {
        Ok(response) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            ConnectivityResult {
                endpoint,
                reachable: true,
                http_status: Some(response.status().as_u16()),
                latency_ms,
                error: None,
            }
        }
        Err(e) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            let error_msg = if e.is_timeout() {
                "Connection timed out (10s)".to_string()
            } else if e.is_connect() {
                format!("Connection failed: {}", e)
            } else {
                format!("Request failed: {}", e)
            };
            ConnectivityResult {
                endpoint,
                reachable: false,
                http_status: None,
                latency_ms,
                error: Some(error_msg),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connectivity_result_construction() {
        let r = ConnectivityResult {
            endpoint: "https://api.anthropic.com".into(),
            reachable: true,
            http_status: Some(200),
            latency_ms: 45,
            error: None,
        };
        assert!(r.reachable);
        assert_eq!(r.http_status, Some(200));
        assert_eq!(r.latency_ms, 45);
    }
}
