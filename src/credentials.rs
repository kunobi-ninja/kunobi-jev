//! Credentials: a static API key, or a provider that supplies a bearer token per attempt.
//!
//! Applications that run on user machines should not embed a TypeSafe API key:
//! anyone with the binary can extract it. Point the client at your own backend
//! instead, and use a [`CredentialProvider`] that returns a short-lived token for
//! that backend, for example one issued by kunobi-auth.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use reqwest::header::HeaderValue;
pub use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// A boxed error from a credential provider.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// The future returned by [`CredentialProvider::bearer_token`].
pub type TokenFuture<'a> =
    Pin<Box<dyn Future<Output = std::result::Result<SecretString, BoxError>> + Send + 'a>>;

/// Supplies the bearer token sent with each attempt.
///
/// The client asks for a token before every attempt, including retries, and
/// does not cache it. Cache and refresh tokens inside the provider.
///
/// For a closure, use [`ClientBuilder::credentials_fn`](crate::ClientBuilder::credentials_fn).
pub trait CredentialProvider: Send + Sync + 'static {
    /// The token to send as `Authorization: Bearer <token>`.
    fn bearer_token(&self) -> TokenFuture<'_>;
}

/// Adapts an async closure into a [`CredentialProvider`].
pub(crate) struct FnProvider<F>(pub(crate) F);

impl<F, Fut, T, E> CredentialProvider for FnProvider<F>
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = std::result::Result<T, E>> + Send + 'static,
    T: Into<SecretString>,
    E: Into<BoxError>,
{
    fn bearer_token(&self) -> TokenFuture<'_> {
        let token = (self.0)();
        Box::pin(async move { token.await.map(Into::into).map_err(Into::into) })
    }
}

/// Where the client gets its `Authorization` header.
pub(crate) enum Credentials {
    ApiKey(SecretString),
    Provider(Box<dyn CredentialProvider>),
}

impl Credentials {
    /// The `Authorization` header for one attempt.
    ///
    /// A provider that does not answer within `timeout` fails the call; provider
    /// failures are not retried.
    pub(crate) async fn authorization(&self, timeout: Duration) -> Result<HeaderValue> {
        match self {
            Credentials::ApiKey(key) => bearer_header(key)
                .map_err(|message| Error::Config(format!("The API key {message}."))),
            Credentials::Provider(provider) => {
                let token = tokio::time::timeout(timeout, provider.bearer_token())
                    .await
                    .map_err(|_| Error::Credentials {
                        source: format!(
                            "the credential provider did not return a token within {}ms",
                            timeout.as_millis()
                        )
                        .into(),
                    })?
                    .map_err(|source| Error::Credentials { source })?;
                bearer_header(&token).map_err(|message| Error::Credentials {
                    source: format!("the token {message}").into(),
                })
            }
        }
    }
}

/// `Bearer <token>`, marked sensitive. Errors describe what is wrong with the token.
pub(crate) fn bearer_header(
    token: &SecretString,
) -> std::result::Result<HeaderValue, &'static str> {
    let token = token.expose_secret().trim();
    if token.is_empty() {
        return Err("is blank");
    }
    // The formatted value is wiped on drop; the header keeps its own copy for the request.
    let value = Zeroizing::new(format!("Bearer {token}"));
    let mut header = HeaderValue::from_str(&value)
        .map_err(|_| "contains characters that are not valid in an HTTP header")?;
    header.set_sensitive(true);
    Ok(header)
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Credentials::ApiKey(_) => f.write_str("ApiKey(***)"),
            Credentials::Provider(_) => f.write_str("Provider"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_headers_are_sensitive_and_trimmed() {
        let header = bearer_header(&SecretString::from(" sk-1 ")).unwrap();
        assert!(header.is_sensitive());
        assert_eq!(header.to_str().unwrap(), "Bearer sk-1");
    }

    #[test]
    fn bearer_headers_reject_blank_and_invalid_tokens() {
        assert_eq!(bearer_header(&SecretString::from("  ")), Err("is blank"));
        assert!(bearer_header(&SecretString::from("a\nb")).is_err());
    }

    #[test]
    fn debug_output_hides_the_key() {
        let debug = format!("{:?}", Credentials::ApiKey(SecretString::from("sk-secret")));
        assert_eq!(debug, "ApiKey(***)");
    }

    #[tokio::test]
    async fn provider_failures_and_hangs_are_credential_errors() {
        let failing = Credentials::Provider(Box::new(FnProvider(|| async {
            Err::<String, _>(std::io::Error::other("refresh token expired"))
        })));
        let err = failing
            .authorization(Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Credentials { .. }));
        assert!(err.to_string().contains("refresh token expired"), "{err}");

        let hanging = Credentials::Provider(Box::new(FnProvider(|| async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok::<_, BoxError>("late".to_owned())
        })));
        let err = hanging
            .authorization(Duration::from_millis(20))
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("did not return a token within 20ms"),
            "{err}"
        );

        let invalid = Credentials::Provider(Box::new(FnProvider(|| async {
            Ok::<_, BoxError>("bad\ntoken".to_owned())
        })));
        let err = invalid
            .authorization(Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("not valid in an HTTP header"),
            "{err}"
        );
        assert!(
            !err.to_string().contains("bad"),
            "the token must not leak: {err}"
        );
    }
}
