use std::time::Duration;

use reqwest::header::HeaderMap;

use crate::providers::traits::ProviderError;

const BACKOFF_SECS: &[u64] = &[1, 2, 5, 10, 15, 20, 25, 30, 30, 30];
pub const MAX_RETRIES: u32 = 10;

#[derive(Debug, Clone)]
pub struct RetryEvent {
    pub attempt: u32,
    pub max_retries: u32,
    pub wait_secs: u64,
    pub error: String,
    pub provider_name: String,
}

pub struct StreamingRequest {
    pub url: String,
    /// URL to use in log messages (strips secrets like inline API keys).
    /// Falls back to `url` if None.
    pub log_url: Option<String>,
    pub headers: HeaderMap,
    pub body: serde_json::Value,
    pub provider_name: String,
}

impl StreamingRequest {
    fn display_url(&self) -> &str {
        self.log_url.as_deref().unwrap_or(&self.url)
    }
}

pub async fn send_streaming(req: &StreamingRequest) -> Result<reqwest::Response, ProviderError> {
    let client = reqwest::Client::new();
    let display_url = req.display_url();

    tracing::debug!(
        provider = %req.provider_name,
        url = %display_url,
        "sending request",
    );

    let resp = client
        .post(&req.url)
        .headers(req.headers.clone())
        .json(&req.body)
        .send()
        .await
        .map_err(|e| {
            tracing::error!(
                provider = %req.provider_name,
                url = %display_url,
                error = %e,
                "request failed",
            );
            ProviderError::HttpError(e.to_string())
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        tracing::error!(
            provider = %req.provider_name,
            url = %display_url,
            %status,
            response_body = %text,
            "bad response from API",
        );
        return Err(ProviderError::BadResponse(format!("{status}: {text}")));
    }

    tracing::debug!(
        provider = %req.provider_name,
        status = %resp.status(),
        "stream started",
    );

    Ok(resp)
}

pub fn backoff_delay(attempt: u32) -> Duration {
    let secs = BACKOFF_SECS.get(attempt as usize).copied().unwrap_or(30);
    Duration::from_secs(secs)
}

pub fn is_retryable(err: &ProviderError) -> bool {
    match err {
        ProviderError::HttpError(_) | ProviderError::Timeout => true,
        ProviderError::BadResponse(msg) => msg
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<u16>().ok())
            .is_some_and(is_retryable_status),
        _ => false,
    }
}

fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_schedule_matches_spec() {
        let expected = [1, 2, 5, 10, 15, 20, 25, 30, 30, 30];
        for (i, &secs) in expected.iter().enumerate() {
            assert_eq!(
                backoff_delay(i as u32),
                Duration::from_secs(secs),
                "attempt {i}"
            );
        }
    }

    #[test]
    fn backoff_beyond_schedule_caps_at_30() {
        assert_eq!(backoff_delay(50), Duration::from_secs(30));
        assert_eq!(backoff_delay(100), Duration::from_secs(30));
    }

    #[test]
    fn retryable_status_codes() {
        for code in [429, 500, 502, 503, 504] {
            assert!(is_retryable_status(code), "{code} should be retryable");
        }
    }

    #[test]
    fn non_retryable_status_codes() {
        for code in [200, 201, 400, 401, 403, 404, 405, 422] {
            assert!(!is_retryable_status(code), "{code} should not be retryable");
        }
    }

    #[test]
    fn retryable_provider_errors() {
        assert!(is_retryable(&ProviderError::HttpError(
            "connection refused".into()
        )));
        assert!(is_retryable(&ProviderError::Timeout));
        assert!(is_retryable(&ProviderError::BadResponse(
            "429 Too Many Requests: rate limited".into()
        )));
        assert!(is_retryable(&ProviderError::BadResponse(
            "500 Internal Server Error: oops".into()
        )));
        assert!(is_retryable(&ProviderError::BadResponse(
            "502 Bad Gateway: ".into()
        )));
        assert!(is_retryable(&ProviderError::BadResponse(
            "503 Service Unavailable: ".into()
        )));
        assert!(is_retryable(&ProviderError::BadResponse(
            "504 Gateway Timeout: ".into()
        )));
    }

    #[test]
    fn non_retryable_provider_errors() {
        assert!(!is_retryable(&ProviderError::BadResponse(
            "400 Bad Request: invalid json".into()
        )));
        assert!(!is_retryable(&ProviderError::BadResponse(
            "401 Unauthorized: bad key".into()
        )));
        assert!(!is_retryable(&ProviderError::BadResponse(
            "403 Forbidden: no access".into()
        )));
        assert!(!is_retryable(&ProviderError::MissingCredential));
        assert!(!is_retryable(&ProviderError::InvalidConfig("bad".into())));
        assert!(!is_retryable(&ProviderError::Cancelled));
    }

    #[test]
    fn max_retries_is_ten() {
        assert_eq!(MAX_RETRIES, 10);
    }

    #[test]
    fn streaming_request_display_url_uses_log_url() {
        let req = StreamingRequest {
            url: "https://api.example.com?key=SECRET".into(),
            log_url: Some("https://api.example.com".into()),
            headers: HeaderMap::new(),
            body: serde_json::json!({}),
            provider_name: "test".into(),
        };
        assert_eq!(req.display_url(), "https://api.example.com");
    }

    #[test]
    fn streaming_request_display_url_falls_back_to_url() {
        let req = StreamingRequest {
            url: "https://api.example.com/v1/chat".into(),
            log_url: None,
            headers: HeaderMap::new(),
            body: serde_json::json!({}),
            provider_name: "test".into(),
        };
        assert_eq!(req.display_url(), "https://api.example.com/v1/chat");
    }
}
