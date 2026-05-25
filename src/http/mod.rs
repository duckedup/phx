pub mod sse;

use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::header::HeaderMap;

use crate::providers::traits::{EventStream, Provider, ProviderError, SendOptions};

const BACKOFF_SECS: &[u64] = &[1, 2, 5, 10, 15, 20, 25, 30, 30, 30];
pub const MAX_RETRIES: u32 = 10;

static SHARED_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

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
    let display_url = req.display_url();

    tracing::debug!(
        provider = %req.provider_name,
        url = %display_url,
        "sending request",
    );

    let resp = SHARED_CLIENT
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
        return Err(ProviderError::BadResponse {
            status: status.as_u16(),
            body: text,
        });
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
        ProviderError::BadResponse { status, .. } => is_retryable_status(*status),
        _ => false,
    }
}

fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

pub enum RetryOutcome {
    Success { stream: EventStream, attempts: u32 },
    Failed(ProviderError),
    Cancelled,
}

pub fn should_retry(err: &ProviderError, attempt: u32) -> bool {
    is_retryable(err) && attempt + 1 < MAX_RETRIES
}

pub fn format_retry_msg(err: &ProviderError, attempt: u32) -> String {
    let delay = backoff_delay(attempt);
    format!(
        "{err} — retrying in {}s (attempt {} of {MAX_RETRIES})",
        delay.as_secs(),
        attempt + 1,
    )
}

pub fn format_recovered_msg(attempts: u32) -> String {
    format!("Recovered after {attempts} attempts")
}

pub async fn send_with_retry(
    provider: &dyn Provider,
    opts: &SendOptions,
    cancel: Option<&AtomicBool>,
    mut on_retry: impl FnMut(u32, u64, &ProviderError),
) -> RetryOutcome {
    for attempt in 0..MAX_RETRIES {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return RetryOutcome::Cancelled;
        }

        match provider.send(opts).await {
            Ok(stream) => {
                return RetryOutcome::Success {
                    stream,
                    attempts: attempt,
                };
            }
            Err(e) => {
                if !should_retry(&e, attempt) {
                    return RetryOutcome::Failed(e);
                }

                let delay = backoff_delay(attempt);
                on_retry(attempt + 1, delay.as_secs(), &e);

                let sleep_until = tokio::time::Instant::now() + delay;
                while tokio::time::Instant::now() < sleep_until {
                    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                        return RetryOutcome::Cancelled;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    RetryOutcome::Failed(ProviderError::HttpError("all retries exhausted".into()))
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
        assert!(is_retryable(&ProviderError::BadResponse {
            status: 429,
            body: "rate limited".into()
        }));
        assert!(is_retryable(&ProviderError::BadResponse {
            status: 500,
            body: "internal server error".into()
        }));
        assert!(is_retryable(&ProviderError::BadResponse {
            status: 502,
            body: String::new()
        }));
        assert!(is_retryable(&ProviderError::BadResponse {
            status: 503,
            body: String::new()
        }));
        assert!(is_retryable(&ProviderError::BadResponse {
            status: 504,
            body: String::new()
        }));
    }

    #[test]
    fn non_retryable_provider_errors() {
        assert!(!is_retryable(&ProviderError::BadResponse {
            status: 400,
            body: "invalid json".into()
        }));
        assert!(!is_retryable(&ProviderError::BadResponse {
            status: 401,
            body: "bad key".into()
        }));
        assert!(!is_retryable(&ProviderError::BadResponse {
            status: 403,
            body: "no access".into()
        }));
        assert!(!is_retryable(&ProviderError::MissingCredential));
        assert!(!is_retryable(&ProviderError::InvalidConfig("bad".into())));
        assert!(!is_retryable(&ProviderError::Cancelled));
    }

    #[test]
    fn max_retries_is_ten() {
        assert_eq!(MAX_RETRIES, 10);
    }

    #[test]
    fn should_retry_respects_attempt_limit() {
        let err = ProviderError::HttpError("timeout".into());
        assert!(should_retry(&err, 0));
        assert!(should_retry(&err, 8));
        assert!(!should_retry(&err, 9));
    }

    #[test]
    fn should_retry_rejects_non_retryable() {
        let err = ProviderError::MissingCredential;
        assert!(!should_retry(&err, 0));
    }

    #[test]
    fn format_retry_msg_includes_details() {
        let err = ProviderError::BadResponse {
            status: 429,
            body: "rate limited".into(),
        };
        let msg = format_retry_msg(&err, 2);
        assert!(msg.contains("retrying in 5s"));
        assert!(msg.contains("attempt 3 of 10"));
    }

    #[test]
    fn format_recovered_msg_includes_count() {
        assert_eq!(format_recovered_msg(3), "Recovered after 3 attempts");
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
