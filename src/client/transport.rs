//! Sending requests: headers, timeouts, retries and logging.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

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
    /// Attempt summaries log at `info`; headers and bodies at `debug`.
    pub(crate) async fn execute(&self, mut req: Resolved) -> Result<RawResponse> {
        let inner = &self.inner;
        let number = inner.request_count.fetch_add(1, Ordering::Relaxed) + 1;
        let tag = format!("#{number} {} {}", req.method, req.path);
        let url = format!("{}{}", inner.base_url, req.path);

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
            // Asked per attempt so a provider can refresh an expiring token between retries.
            let authorization = inner.credentials.authorization(req.timeout).await?;
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
                    headers = ?redact_headers(&attempt_headers),
                    body = %req.body.as_deref().map(String::from_utf8_lossy).unwrap_or_default(),
                    "{tag} -> {url}",
                );
            } else {
                tracing::debug!(headers = ?redact_headers(&attempt_headers), "{tag} -> {url}");
            }

            let started = Instant::now();
            let outcome = self.attempt(&tag, &url, &req, attempt_headers).await;
            let (status, response_headers, body) = match outcome {
                Ok(response) => response,
                Err(err) => {
                    if retries_left == 0 || !req.retry.is_retryable_error(&err) {
                        return Err(err);
                    }
                    backoff(
                        &tag,
                        &req.retry,
                        attempt,
                        retries_left,
                        &err.to_string(),
                        None,
                    )
                    .await;
                    attempt += 1;
                    continue;
                }
            };

            let request_id = request_id_from(&response_headers);
            tracing::info!(
                "{tag} <- {} in {}ms{}",
                status.as_u16(),
                started.elapsed().as_millis(),
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
            let reason = status.as_u16().to_string();
            backoff(
                &tag,
                &req.retry,
                attempt,
                retries_left,
                &reason,
                Some(error.headers()),
            )
            .await;
            attempt += 1;
        }
    }

    /// One round trip, including the full body, bounded by the per-attempt timeout.
    async fn attempt(
        &self,
        tag: &str,
        url: &str,
        req: &Resolved,
        headers: HeaderMap,
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
        match tokio::time::timeout(req.timeout, round_trip).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(source)) => {
                tracing::info!(
                    "{tag} connection error after {}ms: {source}",
                    started.elapsed().as_millis()
                );
                Err(Error::Connection { source })
            }
            Err(_) => {
                tracing::info!("{tag} timed out after {}ms", started.elapsed().as_millis());
                Err(Error::Timeout {
                    timeout: req.timeout,
                })
            }
        }
    }
}

async fn backoff(
    tag: &str,
    retry: &RetryPolicy,
    attempt: u32,
    retries_left: u32,
    reason: &str,
    headers: Option<&HeaderMap>,
) {
    let delay = retry.delay(attempt, headers, fastrand::f64());
    tracing::info!(
        "{tag} retrying in {}ms (retry {}/{}) after {reason}",
        delay.as_millis(),
        attempt + 1,
        attempt + retries_left,
    );
    tokio::time::sleep(delay).await;
}
