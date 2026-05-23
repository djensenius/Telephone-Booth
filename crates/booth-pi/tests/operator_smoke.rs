//! Optional operator HTTP smoke tests for the Pi client.

//! Pi-feature smoke tests for the operator HTTP client.

//! Smoke tests for the Pi operator HTTP client.

#![cfg(feature = "pi")]
#![allow(
    clippy::expect_used,
    reason = "wiremock uses an expect builder method for request counts"
)]

use std::error::Error;

use booth_hal::{BoothStatus, OperatorError};
use booth_pi::operator::default_headers;
use booth_pi::{OperatorConfig, PiOperatorClient, UploadError};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::json;
use wiremock::matchers::{body_json, body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

type TestResult = Result<(), Box<dyn Error>>;

fn config(base_url: String) -> OperatorConfig {
    OperatorConfig {
        base_url,
        token: "test-token".to_string(),
        status_topic: "booth-test".to_string(),
        http_timeout_secs: 2,
        ws_reconnect_initial_ms: 1,
        ws_reconnect_max_ms: 2,
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
        Some("telephone-booth/0.1.0")
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
    assert_eq!(question.description.as_deref(), Some("What did you hear?"));
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

#[tokio::test]
async fn request_upload_slot_posts_metadata() -> TestResult {
    let (server, client) = client_with_server().await?;
    let sha = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    Mock::given(method("POST"))
        .and(path("/v1/uploads"))
        .and(header("authorization", "Bearer test-token"))
        .and(body_json(json!({
            "sha256": sha,
            "sizeBytes": 4,
            "contentType": "audio/flac",
            "durationMs": 123,
            "questionId": "11111111-1111-1111-1111-111111111111"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "33333333-3333-3333-3333-333333333333",
            "uploadUrl": "http://blob.example/upload?sas=redacted",
            "expiresAt": "2026-01-01T00:10:00Z",
            "contentType": "audio/flac"
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
    assert_eq!(slot.expires_at, "2026-01-01T00:10:00Z");
    assert_eq!(slot.content_type, "audio/flac");
    Ok(())
}

#[tokio::test]
async fn upload_complete_confirms_slot() -> TestResult {
    let (server, client) = client_with_server().await?;
    Mock::given(method("POST"))
        .and(path(
            "/v1/uploads/33333333-3333-3333-3333-333333333333/complete",
        ))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(message_body()))
        .expect(1)
        .mount(&server)
        .await;

    client
        .upload_complete("33333333-3333-3333-3333-333333333333")
        .await?;
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
async fn upload_recording_retries_5xx_then_gives_up() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/blob"))
        .and(header("content-type", "audio/flac"))
        .respond_with(ResponseTemplate::new(503).set_body_string("try later"))
        .expect(4)
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
        expires_at: "2026-01-01T00:10:00Z".to_string(),
        content_type: "audio/flac".to_string(),
    };
    let result = client.upload_recording(&slot, &recording).await;
    let _ = std::fs::remove_file(&recording);

    assert!(matches!(result, Err(UploadError::Http { status: 503, .. })));
    Ok(())
}
