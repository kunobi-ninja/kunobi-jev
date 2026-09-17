//! What reaches the logs. Kept in its own test binary: `tracing` caches callsite
//! interest process-wide, so concurrent tests without a subscriber would hide events.

use std::io::Write;
use std::sync::{Arc, Mutex};

use kunobi_jev::{Client, Questions, SystemOneRequest, noul};
use serde_json::json;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

const SECRET: &str = "test-key-0123456789abcdef";

/// A log sink shared with the test.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl Write for Logs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Logs {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

async fn run_logged(log_bodies: bool) -> String {
    let server = MockServer::start().await;
    Mock::given(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-latest",
            "answers": {"q": {"type": "noul", "noul": 0.5}},
            "usage": {"input_tokens": 1, "output_tokens": 1},
            "echo": "response-marker"
        })))
        .mount(&server)
        .await;

    let logs = Logs::default();
    let sink = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .with_writer(move || sink.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let client = Client::builder()
        .api_key(SECRET)
        .base_url(server.uri())
        .log_bodies(log_bodies)
        .build()
        .unwrap();
    let mut questions = Questions::new();
    questions.add("q", noul("request-marker?"));
    client
        .system_one(SystemOneRequest::new("state-marker", questions))
        .await
        .unwrap();
    logs.text()
}

#[tokio::test(flavor = "current_thread")]
async fn logs_never_contain_the_key_and_skip_bodies_by_default() {
    let quiet = run_logged(false).await;
    assert!(quiet.contains("POST /v1/systemone"), "{quiet}");
    for field in [
        "typesafe.request{",
        "http.request.method=POST",
        "url.path=\"/v1/systemone\"",
        "http.response.status_code=200",
    ] {
        assert!(quiet.contains(field), "{field} missing: {quiet}");
    }
    assert!(quiet.contains("Bearer ***cdef"), "{quiet}");
    assert!(!quiet.contains(SECRET), "{quiet}");
    for marker in ["state-marker", "request-marker", "response-marker"] {
        assert!(!quiet.contains(marker), "{marker} leaked: {quiet}");
    }

    let verbose = run_logged(true).await;
    assert!(!verbose.contains(SECRET), "{verbose}");
    for marker in ["state-marker", "request-marker", "response-marker"] {
        assert!(verbose.contains(marker), "{marker} missing: {verbose}");
    }
}
