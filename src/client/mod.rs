//! The API client.

mod builder;
mod call;
mod env;
mod models;
mod transport;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use reqwest::Method;
use reqwest::header::HeaderMap;

pub use builder::ClientBuilder;
pub use call::{Call, RawResponse, WithResponse};
pub use env::{ENV_API_KEY, ENV_BASE_URL, ENV_DEFAULT_MODEL};
pub use models::Models;

use crate::answers::SystemOneResult;
use crate::credentials::Credentials;
use crate::request::SystemOneRequest;
use crate::retry::RetryPolicy;

/// Default API root.
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";

/// Default model.
pub const DEFAULT_MODEL: &str = "jev-latest";

/// Client for the TypeSafe API.
///
/// Cloning is cheap and clones share one connection pool.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    credentials: Credentials,
    base_url: String,
    log_bodies: bool,
    default_model: String,
    retry: RetryPolicy,
    timeout: Duration,
    total_timeout: Option<Duration>,
    /// Concurrency slots and the configured limit.
    limiter: Option<(tokio::sync::Semaphore, usize)>,
    default_headers: HeaderMap,
    http: reqwest::Client,
    /// Numbers requests so concurrent calls, and the attempts within one, can be told apart in logs.
    request_count: AtomicU64,
}

impl Client {
    /// A client configured from the environment.
    ///
    /// Reads `TYPESAFE_API_KEY` (required), `TYPESAFE_BASE_URL` and
    /// `TYPESAFE_DEFAULT_MODEL`. Use [`Client::builder`] to set values in code
    /// or to authenticate with a [`CredentialProvider`](crate::CredentialProvider).
    pub fn new() -> crate::Result<Self> {
        Self::builder().build()
    }

    /// A builder for a client. Values set in code take precedence over the environment.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// API root without trailing slashes.
    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }

    /// Model used when a request does not set one.
    pub fn default_model(&self) -> &str {
        &self.inner.default_model
    }

    /// Retry policy for calls that do not override it.
    pub fn retry(&self) -> &RetryPolicy {
        &self.inner.retry
    }

    /// Timeout per attempt for calls that do not override it.
    pub fn timeout(&self) -> Duration {
        self.inner.timeout
    }

    /// Upper bound for a whole call, for calls that do not override it.
    pub fn total_timeout(&self) -> Option<Duration> {
        self.inner.total_timeout
    }

    /// The concurrency limit shared by this client and its clones, if any.
    pub fn max_concurrent_requests(&self) -> Option<usize> {
        self.inner.limiter.as_ref().map(|(_, max)| *max)
    }

    /// Headers sent with every request.
    pub fn default_headers(&self) -> &HeaderMap {
        &self.inner.default_headers
    }

    /// Answer named questions about text or structured state.
    ///
    /// Validation errors are returned when the call is awaited, before any request is sent.
    ///
    /// ```no_run
    /// # async fn example() -> kunobi_jev::Result<()> {
    /// use kunobi_jev::{Client, Questions, SystemOneRequest, noul};
    ///
    /// let client = Client::new()?;
    /// let mut questions = Questions::new();
    /// let billing = questions.add("billing", noul("Is this about billing?"));
    ///
    /// let result = client
    ///     .system_one(SystemOneRequest::new("I was charged twice.", questions))
    ///     .await?;
    /// println!("{}", result.answer(&billing)?.noul);
    /// # Ok(()) }
    /// ```
    pub fn system_one(&self, request: SystemOneRequest) -> Call<SystemOneResult> {
        let body = request.to_body(self.default_model());
        Call::new(
            self.clone(),
            Method::POST,
            "/v1/systemone",
            body.map(Some),
            call::parse_json,
        )
    }

    /// The Models API resource.
    pub fn models(&self) -> Models {
        Models::new(self.clone())
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.inner.base_url)
            .field("default_model", &self.inner.default_model)
            .field("retry", &self.inner.retry)
            .field("timeout", &self.inner.timeout)
            .field("total_timeout", &self.inner.total_timeout)
            .field("max_concurrent_requests", &self.max_concurrent_requests())
            .field("credentials", &self.inner.credentials)
            .field("log_bodies", &self.inner.log_bodies)
            .finish_non_exhaustive()
    }
}
