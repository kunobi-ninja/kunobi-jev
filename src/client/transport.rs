//! Sending requests: headers, timeouts, retries and logging.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tracing::Instrument;
use tracing::field::Empty;

use bytes::Bytes;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT,
};
use reqwest::{Method, StatusCode};

use super::Client;
use super::call::RawResponse;
use crate::error::{ApiError, Error, Result, request_id_from};
use crate::redact::redact_headers;
use crate::retry::RetryPolicy;

const SDK_HEADER: &str = "x-typesafe-sdk";
const RUNTIME_HEADER: &str = "x-typesafe-runtime";
const RETRY_COUNT_HEADER: &str = "x-typesafe-retry-count";

/// A call with every option resolved against the client.
pub(crate) struct Resolved {
    pub(crate) method: Method,
    pub(crate) path: &'static str,
    pub(crate) body: Option<Bytes>,
    pub(crate) headers: HeaderMap,
    pub(crate) timeout: Duration,
    pub(crate) retry: RetryPolicy,
    pub(crate) total_timeout: Option<Duration>,
}

fn sdk_identity() -> HeaderValue {
    HeaderValue::from_static(concat!(
        env!("CARGO_PKG_NAME"),
        "/",
        env!("CARGO_PKG_VERSION")
    ))
}

fn runtime_description() -> HeaderValue {
    let runtime = format!(
        "rust ({}; {})",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    HeaderValue::from_str(&runtime).unwrap_or_else(|_| HeaderValue::from_static("rust"))
}

impl Client {
    /// Send a resolved call, retrying eligible failures.
    ///
    /// Each call runs in a `typesafe.request` span with OpenTelemetry-style fields.
    /// Attempt summaries log at `info`; redacted headers, and bodies when enabled, at `debug`.
    pub(crate) async fn execute(&self, req: Resolved) -> Result<RawResponse> {
        let number = self.inner.request_count.fetch_add(1, Ordering::Relaxed) + 1;
        let span = tracing::info_span!(
            "typesafe.request",
            otel.kind = "client",
            http.request.method = %req.method,
            url.path = req.path,
            typesafe.request.number = number,
            http.request.resend_count = Empty,
            http.response.status_code = Empty,
            typesafe.request_id = Empty,
            error.type = Empty,
        );
        let result = self
            .execute_attempts(req, number)
            .instrument(span.clone())
            .await;
        if let Err(err) = &result {
            span.record("error.type", error_type(err));
        }
        result
    }

    async fn execute_attempts(&self, mut req: Resolved, number: u64) -> Result<RawResponse> {
        let inner = &self.inner;
        let tag = format!("#{number} {} {}", req.method, req.path);
        let url = format!("{}{}", inner.base_url, req.path);
        let span = tracing::Span::current();
        let deadline = req.total_timeout.map(|total| Instant::now() + total);

        // Caller headers go first so they can't clobber auth or the JSON content type.
        let mut headers = std::mem::take(&mut req.headers);
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(USER_AGENT, sdk_identity());
        headers.insert(HeaderName::from_static(SDK_HEADER), sdk_identity());
        headers.insert(
            HeaderName::from_static(RUNTIME_HEADER),
            runtime_description(),
        );
        headers.remove(RETRY_COUNT_HEADER);
        if req.body.is_some() {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        } else {
            headers.remove(CONTENT_TYPE);
        }

        let mut attempt: u32 = 0;
        loop {
            let retries_left = req.retry.max_retries - attempt;
            let timeout = attempt_timeout(req.timeout, req.total_timeout, deadline)?;
            if attempt > 0 {
                span.record("http.request.resend_count", attempt);
            }

            // Asked per attempt so a provider can refresh an expiring token between retries.
            let authorization = inner.credentials.authorization(timeout).await?;
            let mut attempt_headers = headers.clone();
            attempt_headers.insert(AUTHORIZATION, authorization);
            if attempt > 0 {
                attempt_headers.insert(
                    HeaderName::from_static(RETRY_COUNT_HEADER),
                    HeaderValue::from(attempt),
                );
            }
            if inner.log_bodies {
                tracing::debug!(
                    attempt,
                    headers = ?redact_headers(&attempt_headers),
                    body = %req.body.as_deref().map(String::from_utf8_lossy).unwrap_or_default(),
                    "{tag} -> {url}",
                );
            } else {
                tracing::debug!(attempt, headers = ?redact_headers(&attempt_headers), "{tag} -> {url}");
            }

            let started = Instant::now();
            let outcome = self
                .attempt(&tag, &url, &req, attempt_headers, timeout)
                .await;
            let (status, response_headers, body) = match outcome {
                Ok(response) => response,
                Err(err) => {
                    if retries_left == 0 || !req.retry.is_retryable_error(&err) {
                        return Err(err);
                    }
                    let delay = req.retry.delay(attempt, None, fastrand::f64());
                    if !fits_before(deadline, delay) {
                        return Err(err);
                    }
                    backoff(&tag, attempt, retries_left, &err.to_string(), delay).await;
                    attempt += 1;
                    continue;
                }
            };

            let request_id = request_id_from(&response_headers);
            let elapsed_ms = started.elapsed().as_millis() as u64;
            span.record("http.response.status_code", status.as_u16());
            if let Some(id) = &request_id {
                span.record("typesafe.request_id", id.as_str());
            }
            tracing::info!(
                attempt,
                http.response.status_code = status.as_u16(),
                elapsed_ms,
                typesafe.request_id = request_id.as_deref(),
                "{tag} <- {} in {elapsed_ms}ms{}",
                status.as_u16(),
                request_id
                    .as_deref()
                    .map(|id| format!(" (request {id})"))
                    .unwrap_or_default(),
            );
            if inner.log_bodies {
                tracing::debug!(body = %String::from_utf8_lossy(&body), "{tag} <- body");
            }

            if status.is_success() {
                return Ok(RawResponse {
                    status,
                    headers: response_headers,
                    body,
                    request_id,
                });
            }

            let error = ApiError::from_response(status, response_headers, &body);
            if retries_left == 0 || !req.retry.is_retryable_status(status.as_u16()) {
                return Err(error.into());
            }
            let delay = req
                .retry
                .delay(attempt, Some(error.headers()), fastrand::f64());
            if !fits_before(deadline, delay) {
                return Err(error.into());
            }
            let reason = status.as_u16().to_string();
            backoff(&tag, attempt, retries_left, &reason, delay).await;
            attempt += 1;
        }
    }

    /// One round trip, including the full body, bounded by `timeout`.
    async fn attempt(
        &self,
        tag: &str,
        url: &str,
        req: &Resolved,
        headers: HeaderMap,
        timeout: Duration,
    ) -> Result<(StatusCode, HeaderMap, Bytes)> {
        let mut builder = self
            .inner
            .http
            .request(req.method.clone(), url)
            .headers(headers);
        if let Some(body) = &req.body {
            builder = builder.body(body.clone());
        }
        let round_trip = async {
            let response = builder.send().await?;
            let status = response.status();
            let headers = response.headers().clone();
            let body = response.bytes().await?;
            Ok::<_, reqwest::Error>((status, headers, body))
        };

        let started = Instant::now();
        match tokio::time::timeout(timeout, round_trip).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(source)) => {
                tracing::info!(
                    error.type = "connection",
                    "{tag} connection error after {}ms: {source}",
                    started.elapsed().as_millis()
                );
                Err(Error::Connection { source })
            }
            Err(_) => {
                tracing::info!(
                    error.type = "timeout",
                    "{tag} timed out after {}ms",
                    started.elapsed().as_millis()
                );
                Err(Error::Timeout { timeout })
            }
        }
    }
}

/// The timeout for the next attempt: the per-attempt timeout, cut to what is left of the total.
fn attempt_timeout(
    per_attempt: Duration,
    total: Option<Duration>,
    deadline: Option<Instant>,
) -> Result<Duration> {
    let (Some(total), Some(deadline)) = (total, deadline) else {
        return Ok(per_attempt);
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(Error::Timeout { timeout: total });
    }
    Ok(per_attempt.min(remaining))
}

/// Whether waiting `delay` still leaves time for another attempt before the deadline.
fn fits_before(deadline: Option<Instant>, delay: Duration) -> bool {
    deadline.is_none_or(|deadline| Instant::now() + delay < deadline)
}

/// A low-cardinality `error.type` value for a failed call.
fn error_type(err: &Error) -> String {
    match err {
        Error::Api(api) => api.status().as_u16().to_string(),
        Error::Timeout { .. } => "timeout".into(),
        Error::Connection { .. } => "connection".into(),
        Error::Credentials { .. } => "credentials".into(),
        Error::Decode { .. } => "decode".into(),
        Error::Config(_) => "config".into(),
        Error::InvalidRequest(_) => "invalid_request".into(),
        Error::UnexpectedAnswer { .. } => "unexpected_answer".into(),
    }
}

async fn backoff(tag: &str, attempt: u32, retries_left: u32, reason: &str, delay: Duration) {
    tracing::info!(
        attempt,
        delay_ms = delay.as_millis() as u64,
        "{tag} retrying in {}ms (retry {}/{}) after {reason}",
        delay.as_millis(),
        attempt + 1,
        attempt + retries_left,
    );
    tokio::time::sleep(delay).await;
}
