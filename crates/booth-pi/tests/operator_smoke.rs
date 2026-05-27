//! Optional operator HTTP smoke tests for the Pi client.

//! Pi-feature smoke tests for the operator HTTP client.

//! Smoke tests for the Pi operator HTTP client.

#![cfg(feature = "operator")]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "wiremock uses an expect builder method for request counts; tests use unwrap for assertions"
)]

use std::error::Error;

use booth_hal::{BoothStatus, OperatorClient, OperatorError, RuntimeMode, SystemSnapshot};
use booth_pi::operator::default_headers;
use booth_pi::{OperatorConfig, PiOperatorClient, UploadError};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::json;
use wiremock::matchers::{body_json, body_string_contains, header, method, path};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

type TestResult = Result<(), Box<dyn Error>>;

struct EmptyBody;

impl Match for EmptyBody {
    fn matches(&self, request: &Request) -> bool {
        request.body.is_empty()
    }
}

fn config(base_url: String) -> OperatorConfig {
    OperatorConfig {
        base_url,
        token: "test-token".to_string(),
        status_topic: "booth-test".to_string(),
        http_timeout_secs: 2,
        ws_reconnect_initial_ms: 1,
        ws_reconnect_max_ms: 2,
        ..OperatorConfig::default()
    }
}

fn question_body() -> serde_json::Value {
    json!({
        "id": "11111111-1111-1111-1111-111111111111",
        "prompt": "What did you hear?",
        "createdAt": "2026-01-01T00:00:00Z",
        "audio": {
            "url": "https://blob.example/question.flac",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sizeBytes": 123,
            "durationMs": 1000,
            "contentType": "audio/flac"
        }
    })
}

fn message_body() -> serde_json::Value {
    json!({
        "id": "22222222-2222-2222-2222-222222222222",
        "status": "approved",
        "questionId": "11111111-1111-1111-1111-111111111111",
        "notes": null,
        "createdAt": "2026-01-01T00:00:00Z",
        "audio": {
            "url": "https://blob.example/message.flac",
            "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "sizeBytes": 456,
            "durationMs": 2000,
            "contentType": "audio/flac"
        }
    })
}

async fn client_with_server() -> Result<(MockServer, PiOperatorClient), OperatorError> {
    let server = MockServer::start().await;
    let client = PiOperatorClient::new(config(server.uri()));
    client.map(|client| (server, client))
}

#[test]
fn builds_urls_and_headers_without_leaking_token() -> TestResult {
    let client = PiOperatorClient::new(config("https://operator.example.com/".to_string()))?;
    assert_eq!(
        client.api_url_for_path("/v1/status"),
        "https://operator.example.com/v1/status"
    );
    assert_eq!(
        client.api_url_for_path("v1/messages/random"),
        "https://operator.example.com/v1/messages/random"
    );

    let headers = default_headers("super-secret-token")?;
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .map(reqwest::header::HeaderValue::to_str)
            .transpose()?,
        Some("Bearer super-secret-token")
    );
    assert_eq!(
        headers
            .get(USER_AGENT)
            .map(reqwest::header::HeaderValue::to_str)
            .transpose()?,
        Some(concat!("telephone-booth/", env!("CARGO_PKG_VERSION")))
    );
    assert_eq!(
        headers
            .get(ACCEPT)
            .map(reqwest::header::HeaderValue::to_str)
            .transpose()?,
        Some("application/json")
    );
    assert_eq!(
        headers
            .get(CONTENT_TYPE)
            .map(reqwest::header::HeaderValue::to_str)
            .transpose()?,
        Some("application/json")
    );

    let debug = format!(
        "{:?}",
        PiOperatorClient::new(config("https://operator.example.com".to_string()))?
    );
    assert!(!debug.contains("test-token"));
    assert!(debug.contains("<redacted:oken>"));
    Ok(())
}

#[tokio::test]
async fn get_random_question_deserializes_200() -> TestResult {
    let (server, client) = client_with_server().await?;
    Mock::given(method("GET"))
        .and(path("/v1/questions/random"))
        .and(header("authorization", "Bearer test-token"))
        .and(header("accept", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(question_body()))
        .expect(1)
        .mount(&server)
        .await;

    let question = client.get_random_question().await?;
    assert_eq!(question.id, "11111111-1111-1111-1111-111111111111");
    assert_eq!(question.audio_url, "https://blob.example/question.flac");
    assert_eq!(
        question.audio_sha256.as_deref(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert_eq!(question.description.as_deref(), Some("What did you hear?"));
    Ok(())
}

#[tokio::test]
async fn get_random_message_deserializes_audio_sha() -> TestResult {
    let (server, client) = client_with_server().await?;
    Mock::given(method("GET"))
        .and(path("/v1/messages/random"))
        .and(header("authorization", "Bearer test-token"))
        .and(header("accept", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(message_body()))
        .expect(1)
        .mount(&server)
        .await;

    let message = client.get_random_message().await?;
    assert_eq!(message.id, "22222222-2222-2222-2222-222222222222");
    assert_eq!(message.audio_url, "https://blob.example/message.flac");
    assert_eq!(
        message.audio_sha256.as_deref(),
        Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
    );
    assert_eq!(
        message.question_id.as_deref(),
        Some("11111111-1111-1111-1111-111111111111")
    );
    Ok(())
}

#[tokio::test]
async fn get_random_question_maps_404() -> TestResult {
    let (server, client) = client_with_server().await?;
    Mock::given(method("GET"))
        .and(path("/v1/questions/random"))
        .respond_with(ResponseTemplate::new(404).set_body_string("none"))
        .expect(1)
        .mount(&server)
        .await;

    let result = client.get_random_question().await;
    assert!(matches!(
        result,
        Err(OperatorError::Server { status: 404, .. })
    ));
    Ok(())
}

#[tokio::test]
async fn put_status_sends_shape_and_headers() -> TestResult {
    let (server, client) = client_with_server().await?;
    Mock::given(method("PUT"))
        .and(path("/v1/status"))
        .and(header("authorization", "Bearer test-token"))
        .and(header("content-type", "application/json"))
        .and(body_string_contains("\"state\":\"dialTone\""))
        .and(body_string_contains("\"updatedAt\""))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    client.put_status_ref(&BoothStatus::DialTone).await?;
    Ok(())
}

struct NoRuntimeMode;

impl Match for NoRuntimeMode {
    fn matches(&self, request: &Request) -> bool {
        !std::str::from_utf8(&request.body)
            .unwrap_or("")
            .contains("runtimeMode")
    }
}

#[tokio::test]
async fn put_status_omits_runtime_mode_by_default() -> TestResult {
    let (server, client) = client_with_server().await?;
    Mock::given(method("PUT"))
        .and(path("/v1/status"))
        .and(NoRuntimeMode)
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    client.put_status_ref(&BoothStatus::DialTone).await?;
    Ok(())
}

#[tokio::test]
async fn put_status_includes_runtime_mode_when_configured() -> TestResult {
    let server = MockServer::start().await;
    let client = PiOperatorClient::new(config(server.uri()))
        .expect("client")
        .with_runtime_mode(RuntimeMode::Mock);
    Mock::given(method("PUT"))
        .and(path("/v1/status"))
        .and(body_string_contains("\"runtimeMode\":\"mock\""))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    client.put_status_ref(&BoothStatus::DialTone).await?;
    Ok(())
}

#[tokio::test]
async fn request_upload_slot_posts_metadata() -> TestResult {
    let (server, client) = client_with_server().await?;
    let sha = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("authorization", "Bearer test-token"))
        .and(body_json(json!({
            "sha256": sha,
            "durationMs": 123,
            "questionId": "11111111-1111-1111-1111-111111111111"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "33333333-3333-3333-3333-333333333333",
            "uploadUrl": "http://blob.example/upload?sas=redacted",
            "blobName": "recordings/33333333-3333-3333-3333-333333333333.flac"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let question_id = "11111111-1111-1111-1111-111111111111".to_string();
    let slot = client
        .request_upload_slot(Some(&question_id), sha, 4, Some(123))
        .await?;
    assert_eq!(slot.id, "33333333-3333-3333-3333-333333333333");
    assert_eq!(slot.upload_url, "http://blob.example/upload?sas=redacted");
    assert_eq!(
        slot.blob_name,
        "recordings/33333333-3333-3333-3333-333333333333.flac"
    );
    Ok(())
}

#[tokio::test]
async fn request_upload_slot_requires_duration() -> TestResult {
    let (server, client) = client_with_server().await?;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&server)
        .await;

    let result = client
        .request_upload_slot(
            None,
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            4,
            None,
        )
        .await;

    assert!(matches!(result, Err(OperatorError::InvalidArgument(_))));
    Ok(())
}

#[tokio::test]
async fn upload_complete_confirms_slot() -> TestResult {
    let (server, client) = client_with_server().await?;
    Mock::given(method("POST"))
        .and(path(
            "/v1/messages/33333333-3333-3333-3333-333333333333/complete",
        ))
        .and(header("authorization", "Bearer test-token"))
        .and(EmptyBody)
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "33333333-3333-3333-3333-333333333333",
            "status": "received",
            "receivedAt": "2026-01-01T00:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;

    client
        .upload_complete(
            "33333333-3333-3333-3333-333333333333",
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            5000,
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn complete_maps_413_and_422_to_non_retryable_errors() -> TestResult {
    for (status, body) in [
        (
            413,
            json!({ "code": "audio_too_large", "maxBytes": 26_214_400 }).to_string(),
        ),
        (422, json!({ "code": "sha256_mismatch" }).to_string()),
    ] {
        let (server, client) = client_with_server().await?;
        Mock::given(method("POST"))
            .and(path(
                "/v1/messages/33333333-3333-3333-3333-333333333333/complete",
            ))
            .and(EmptyBody)
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;

        let result = client
            .upload_complete(
                "33333333-3333-3333-3333-333333333333",
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
                5000,
            )
            .await;

        match (status, result) {
            (413, Err(OperatorError::PayloadTooLarge { max_bytes, .. })) => {
                assert_eq!(max_bytes, Some(26_214_400));
            }
            (422, Err(OperatorError::Unprocessable(message))) => {
                assert!(message.contains("sha256_mismatch"));
            }
            (_, other) => panic!("unexpected result for {status}: {other:?}"),
        }
    }
    Ok(())
}

#[tokio::test]
async fn maps_401_to_unauthorized() -> TestResult {
    let (server, client) = client_with_server().await?;
    Mock::given(method("GET"))
        .and(path("/v1/messages/random"))
        .respond_with(ResponseTemplate::new(401).set_body_string("bad token"))
        .expect(1)
        .mount(&server)
        .await;

    let result = client.get_random_message().await;
    assert!(matches!(result, Err(OperatorError::Unauthorized(_))));
    Ok(())
}

#[tokio::test]
async fn upload_recording_rejects_http_localhost_url() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/blob"))
        .respond_with(ResponseTemplate::new(503))
        .expect(0) // validation rejects before any request is made
        .mount(&server)
        .await;

    let client = PiOperatorClient::new(config("https://operator.example.com".to_string()))?;
    let dir = std::env::current_dir()?.join("target/operator-smoke");
    std::fs::create_dir_all(&dir)?;
    let recording = dir.join(format!("upload-{}.flac", std::process::id()));
    std::fs::write(&recording, b"flac-data")?;

    let slot = booth_hal::UploadSlot {
        id: "33333333-3333-3333-3333-333333333333".to_string(),
        upload_url: format!("{}/blob", server.uri()),
        blob_name: "recordings/33333333-3333-3333-3333-333333333333.flac".to_string(),
    };
    let result = client.upload_recording(&slot, &recording).await;
    let _ = std::fs::remove_file(&recording);

    // URL is http:// on localhost — rejected by validation before any network I/O
    assert!(matches!(result, Err(UploadError::InvalidUrl(_))));
    Ok(())
}

#[tokio::test]
async fn put_recording_retries_5xx_then_gives_up() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/blob"))
        .and(header("content-type", "audio/flac"))
        .respond_with(ResponseTemplate::new(503).set_body_string("try later"))
        .expect(4) // initial attempt + 3 retries
        .mount(&server)
        .await;

    let client = PiOperatorClient::new(config(server.uri()))?;
    let dir = std::env::current_dir()?.join("target/operator-smoke");
    std::fs::create_dir_all(&dir)?;
    let recording = dir.join(format!("upload-retry-{}.flac", std::process::id()));
    std::fs::write(&recording, b"flac-data")?;

    let slot = booth_hal::UploadSlot {
        id: "33333333-3333-3333-3333-333333333333".to_string(),
        upload_url: format!("{}/blob", server.uri()),
        blob_name: "recordings/33333333-3333-3333-3333-333333333333.flac".to_string(),
    };
    // Call put_recording directly to bypass URL validation (mock is HTTP on localhost)
    let result = client.put_recording(&slot, &recording).await;
    let _ = std::fs::remove_file(&recording);

    assert!(matches!(result, Err(UploadError::Http { status: 503, .. })));
    Ok(())
}

// ---------------------------------------------------------------------------
// Upload URL validation tests
// ---------------------------------------------------------------------------

#[test]
fn validate_upload_url_accepts_valid_https() {
    let hosts = vec!["myaccount.blob.core.windows.net".to_string()];
    let result = booth_pi::validate_upload_url(
        "https://myaccount.blob.core.windows.net/container/blob?sv=2021-08-06&sig=abc",
        &hosts,
    );
    assert!(result.is_ok());
}

#[test]
fn validate_upload_url_rejects_http() {
    let hosts: Vec<String> = vec![];
    let result = booth_pi::validate_upload_url("http://storage.example.com/blob", &hosts);
    assert!(matches!(result, Err(UploadError::InvalidUrl(_))));
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("HTTPS"), "error should mention HTTPS: {msg}");
}

#[test]
fn validate_upload_url_rejects_non_https_scheme() {
    let hosts: Vec<String> = vec![];
    // file:// URLs have no host and a different scheme
    let result = booth_pi::validate_upload_url("file:///etc/passwd", &hosts);
    assert!(matches!(result, Err(UploadError::InvalidUrl(_))));
}

#[test]
fn validate_upload_url_rejects_private_ipv4() {
    let hosts: Vec<String> = vec![];
    for addr in &[
        "https://10.0.0.1/upload",
        "https://192.168.1.1/upload",
        "https://172.16.0.1/upload",
        "https://127.0.0.1/upload",
        "https://169.254.1.1/upload",
        "https://100.100.1.1/upload",
    ] {
        let result = booth_pi::validate_upload_url(addr, &hosts);
        assert!(
            matches!(result, Err(UploadError::InvalidUrl(_))),
            "expected rejection for {addr}"
        );
    }
}

#[test]
fn validate_upload_url_rejects_private_ipv6() {
    let hosts: Vec<String> = vec![];
    for addr in &[
        "https://[::1]/upload",
        "https://[fc00::1]/upload",
        "https://[fe80::1]/upload",
    ] {
        let result = booth_pi::validate_upload_url(addr, &hosts);
        assert!(
            matches!(result, Err(UploadError::InvalidUrl(_))),
            "expected rejection for {addr}"
        );
    }
}

#[test]
fn validate_upload_url_rejects_localhost() {
    let hosts: Vec<String> = vec![];
    let result = booth_pi::validate_upload_url("https://localhost/upload", &hosts);
    assert!(matches!(result, Err(UploadError::InvalidUrl(_))));
}

#[test]
fn validate_upload_url_rejects_unlisted_host() {
    let hosts = vec!["allowed.blob.core.windows.net".to_string()];
    let result = booth_pi::validate_upload_url("https://evil.attacker.com/exfil", &hosts);
    assert!(matches!(result, Err(UploadError::InvalidUrl(_))));
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("not in the allowed hosts"), "error: {msg}");
}

#[test]
fn validate_upload_url_allows_any_public_host_when_list_empty() {
    let hosts: Vec<String> = vec![];
    let result =
        booth_pi::validate_upload_url("https://any-storage.example.com/blob?sig=abc", &hosts);
    assert!(result.is_ok());
}

#[test]
fn validate_upload_url_host_check_is_case_insensitive() {
    let hosts = vec!["MyAccount.blob.core.windows.net".to_string()];
    let result =
        booth_pi::validate_upload_url("https://myaccount.blob.core.windows.net/c/b?sv=x", &hosts);
    assert!(result.is_ok());
}

#[tokio::test]
async fn put_system_snapshot_includes_booth_id_and_version() -> TestResult {
    let (server, client) = client_with_server().await?;
    Mock::given(method("PUT"))
        .and(path("/v1/system"))
        .and(header("authorization", "Bearer test-token"))
        .and(header("content-type", "application/json"))
        .and(body_string_contains("\"boothId\":\"booth-test\""))
        .and(body_string_contains("\"version\":\"9.9.9-test\""))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let snapshot = SystemSnapshot::default();
    client
        .put_system_snapshot("booth-test", "9.9.9-test", &snapshot)
        .await?;
    Ok(())
}

#[tokio::test]
async fn upload_rejects_file_exceeding_max_upload_bytes() -> TestResult {
    let server = MockServer::start().await;
    let mut cfg = config(server.uri());
    cfg.max_upload_bytes = 10; // 10 byte cap

    let client = PiOperatorClient::new(cfg)?;
    let dir = std::env::current_dir()?.join("target/operator-smoke");
    std::fs::create_dir_all(&dir)?;
    let recording = dir.join(format!("upload-cap-{}.flac", std::process::id()));
    std::fs::write(&recording, b"this is more than ten bytes of data")?;

    // Use a valid HTTPS URL so URL validation passes; size check runs before network I/O.
    let slot = booth_hal::UploadSlot {
        id: "44444444-4444-4444-4444-444444444444".to_string(),
        upload_url: "https://storage.example.com/blob".to_string(),
        blob_name: "recordings/44444444-4444-4444-4444-444444444444.flac".to_string(),
    };
    let result = client.upload_recording(&slot, &recording).await;
    let _ = std::fs::remove_file(&recording);

    let err_msg = match result {
        Err(UploadError::Io(err)) => err.to_string(),
        Err(err) => panic!("expected I/O error for upload cap, got {err}"),
        Ok(()) => panic!("expected upload to fail when recording exceeds max upload bytes"),
    };
    assert!(
        err_msg.contains("exceeds upload cap"),
        "unexpected error: {err_msg}"
    );
    Ok(())
}
