//! Phone-side HTTP client for the operator backend.

// Pre-existing pedantic lints that surface now that the `operator` feature
// is compiled on macOS by default. Behaviorally unrelated patterns; kept
// allowed so the strict workspace clippy gate stays clean.
#![cfg_attr(
    feature = "operator",
    allow(
        clippy::cast_possible_truncation,
        clippy::bool_to_int_with_if,
        clippy::needless_pass_by_value,
        clippy::needless_return,
        clippy::trivially_copy_pass_by_ref,
    )
)]

use std::borrow::Cow;
use std::fmt;
use std::path::Path;

use crate::{OperatorConfig, redacted_token};
use async_trait::async_trait;
use booth_hal::{
    BoothStatus, EventBatchAck, OperatorClient, OperatorError, OperatorMessage, OperatorQuestion,
    QuestionId, SystemSnapshot, UploadSlot, redact_url,
};

#[cfg(feature = "operator")]
use {
    reqwest::header::{
        ACCEPT, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT,
    },
    serde::{Deserialize, Serialize},
    std::time::{Duration, SystemTime, UNIX_EPOCH},
    tokio::fs,
    tokio::time::sleep,
    tracing::debug,
};

#[cfg(feature = "operator")]
const JSON_CONTENT_TYPE: &str = "application/json";
#[cfg(feature = "operator")]
const FLAC_CONTENT_TYPE: &str = "audio/flac";
#[cfg(feature = "operator")]
const USER_AGENT_VALUE: &str = concat!("telephone-booth/", env!("CARGO_PKG_VERSION"));

#[cfg(feature = "operator")]
const UPLOAD_RETRIES: usize = 3;
#[cfg(feature = "operator")]
const UPLOAD_BACKOFF_BASE: Duration = Duration::from_millis(25);

/// Operator HTTP client used by the Raspberry Pi runtime.
#[derive(Clone)]
pub struct PiOperatorClient {
    config: OperatorConfig,
    base_url: String,
    #[cfg(feature = "operator")]
    client: reqwest::Client,
    #[cfg(feature = "operator")]
    upload_client: reqwest::Client,
}

impl fmt::Debug for PiOperatorClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PiOperatorClient")
            .field("base_url", &self.base_url)
            .field("token", &redacted_token(&self.config.token))
            .field("status_topic", &self.config.status_topic)
            .field("http_timeout_secs", &self.config.http_timeout_secs)
            .finish_non_exhaustive()
    }
}

/// Failure while uploading bytes directly to a presigned blob URL.
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    /// The local recording could not be read.
    #[error("recording I/O error: {0}")]
    Io(Cow<'static, str>),
    /// The upload request failed before a response was received.
    #[error("upload transport error: {0}")]
    Transport(Cow<'static, str>),
    /// The blob service rejected the upload.
    #[error("upload HTTP error: {status} {body}")]
    Http {
        /// HTTP status returned by the blob service.
        status: u16,
        /// Truncated response body for diagnostics.
        body: String,
    },
    /// The blob service reports the recording already exists.
    #[error("duplicate recording upload: {0}")]
    DuplicateRecording(Cow<'static, str>),
}

impl From<UploadError> for OperatorError {
    fn from(value: UploadError) -> Self {
        match value {
            UploadError::Io(err) | UploadError::Transport(err) => OperatorError::Transport(err),
            UploadError::DuplicateRecording(message) => OperatorError::DuplicateRecording(message),
            UploadError::Http { status: 409, body } => {
                OperatorError::DuplicateRecording(body.into())
            }
            UploadError::Http { status, body } => OperatorError::Server { status, body },
        }
    }
}

#[cfg_attr(
    not(feature = "operator"),
    allow(clippy::unused_async, unused_variables)
)]
impl PiOperatorClient {
    /// Build a client from operator configuration.
    pub fn new(config: OperatorConfig) -> Result<Self, OperatorError> {
        let base_url = normalize_base_url(&config.base_url);

        #[cfg(feature = "operator")]
        {
            let timeout = Duration::from_secs(config.http_timeout_secs);
            let client = reqwest::Client::builder()
                .timeout(timeout)
                .default_headers(default_headers(&config.token)?)
                .build()
                .map_err(operator_transport)?;
            let upload_client = reqwest::Client::builder()
                .timeout(timeout)
                .build()
                .map_err(operator_transport)?;

            Ok(Self {
                config,
                base_url,
                client,
                upload_client,
            })
        }

        #[cfg(not(feature = "operator"))]
        {
            Ok(Self { config, base_url })
        }
    }

    /// Return the absolute API URL for a path such as `/v1/status`.
    #[must_use]
    pub fn api_url_for_path(&self, path: &str) -> String {
        api_url(&self.base_url, path)
    }

    /// Fetch a random approved question.
    pub async fn get_random_question(&self) -> Result<OperatorQuestion, OperatorError> {
        #[cfg(feature = "operator")]
        {
            let question = self
                .send_json::<ApiQuestion>(reqwest::Method::GET, "/v1/questions/random", None::<&()>)
                .await?;
            return Ok(question.into());
        }

        #[cfg(not(feature = "operator"))]
        unsupported()
    }

    /// Fetch a random approved message.
    pub async fn get_random_message(&self) -> Result<OperatorMessage, OperatorError> {
        #[cfg(feature = "operator")]
        {
            let message = self
                .send_json::<ApiMessage>(reqwest::Method::GET, "/v1/messages/random", None::<&()>)
                .await?;
            return Ok(message.into());
        }

        #[cfg(not(feature = "operator"))]
        unsupported()
    }

    /// Push a coarse booth status update to the operator.
    pub async fn put_status_ref(&self, status: &BoothStatus) -> Result<(), OperatorError> {
        #[cfg(feature = "operator")]
        {
            let body = StatusUpdate::new(status);
            self.send_empty(reqwest::Method::PUT, "/v1/status", Some(&body))
                .await?;
            return Ok(());
        }

        #[cfg(not(feature = "operator"))]
        {
            let _ = status;
            unsupported()
        }
    }

    /// Request a presigned upload slot for a recording.
    pub async fn request_upload_slot(
        &self,
        question_id: Option<&QuestionId>,
        sha256_hex: &str,
        size_bytes: u64,
        duration_ms: Option<u64>,
    ) -> Result<UploadSlot, OperatorError> {
        #[cfg(feature = "operator")]
        {
            let body = UploadSlotRequest {
                sha256: sha256_hex,
                size_bytes,
                content_type: FLAC_CONTENT_TYPE,
                duration_ms,
                question_id,
            };
            let slot = self
                .send_json::<UploadSlot>(reqwest::Method::POST, "/v1/uploads", Some(&body))
                .await?;
            return Ok(slot);
        }

        #[cfg(not(feature = "operator"))]
        {
            let _ = (question_id, sha256_hex, size_bytes, duration_ms);
            unsupported()
        }
    }

    /// Notify the operator that an upload finished successfully.
    pub async fn upload_complete(
        &self,
        upload_id: &str,
        sha256_hex: &str,
        duration_ms: u64,
    ) -> Result<(), OperatorError> {
        #[cfg(feature = "operator")]
        {
            #[derive(serde::Serialize)]
            #[serde(rename_all = "camelCase")]
            struct CompleteBody<'a> {
                sha256: &'a str,
                duration_ms: u64,
            }
            let body = CompleteBody {
                sha256: sha256_hex,
                duration_ms,
            };
            let path = format!("/v1/uploads/{upload_id}/complete");
            self.send_empty(reqwest::Method::POST, &path, Some(&body))
                .await?;
            return Ok(());
        }

        #[cfg(not(feature = "operator"))]
        {
            let _ = (upload_id, sha256_hex, duration_ms);
            unsupported()
        }
    }

    /// Fetch the operator-recorded instructions prompt.
    #[allow(
        clippy::unused_async,
        reason = "kept async to mirror future operator endpoint shape"
    )]
    pub async fn get_instructions(&self) -> Result<OperatorMessage, OperatorError> {
        let _ = self;
        Err(OperatorError::Unsupported(
            "OpenAPI spec does not define a phone-side instructions endpoint".into(),
        ))
    }

    /// Upload a FLAC recording directly to a presigned blob URL.
    ///
    /// The file is streamed from disk on each attempt rather than buffered
    /// entirely in memory, bounding memory usage during retries.
    pub async fn upload_recording(
        &self,
        slot: &UploadSlot,
        local_path: &Path,
    ) -> Result<(), UploadError> {
        #[cfg(feature = "operator")]
        {
            // Validate file size against the configured cap before streaming.
            let meta = fs::metadata(local_path)
                .await
                .map_err(|err| UploadError::Io(err.to_string().into()))?;
            let file_size = meta.len();
            if file_size > self.config.max_upload_bytes {
                return Err(UploadError::Io(
                    format!(
                        "recording is {file_size} bytes, exceeds upload cap of {} bytes",
                        self.config.max_upload_bytes
                    )
                    .into(),
                ));
            }

            for attempt in 0..=UPLOAD_RETRIES {
                debug!(
                    route = "PUT <presigned-upload-url>",
                    attempt = attempt + 1,
                    "uploading recording"
                );

                // Open the file fresh on each attempt so we stream from disk
                // rather than cloning an in-memory buffer.
                let file = fs::File::open(local_path)
                    .await
                    .map_err(|err| UploadError::Io(err.to_string().into()))?;
                let body = reqwest::Body::from(file);

                let response = self
                    .upload_client
                    .put(&slot.upload_url)
                    .header(CONTENT_TYPE, slot.content_type.as_str())
                    .header(CONTENT_LENGTH, file_size)
                    .body(body)
                    .send()
                    .await;

                match response {
                    Ok(response) if response.status().is_success() => {
                        debug!(status = response.status().as_u16(), "upload accepted");
                        return Ok(());
                    }
                    Ok(response) => {
                        let status = response.status();
                        let body = truncated_body(response).await;
                        debug!(status = status.as_u16(), "upload rejected");
                        if status.as_u16() == 409 && attempt == UPLOAD_RETRIES {
                            return Err(UploadError::DuplicateRecording(body.into()));
                        }
                        if !is_retryable_upload_status(status.as_u16()) || attempt == UPLOAD_RETRIES
                        {
                            return Err(UploadError::Http {
                                status: status.as_u16(),
                                body,
                            });
                        }
                    }
                    Err(err) => {
                        let msg = redact_url(&err.to_string()).into_owned();
                        debug!(error = %msg, "upload transport failure");
                        if attempt == UPLOAD_RETRIES {
                            return Err(UploadError::Transport(msg.into()));
                        }
                    }
                }

                sleep(upload_backoff(attempt)).await;
            }

            Err(UploadError::Transport("upload retry loop exited".into()))
        }

        #[cfg(not(feature = "operator"))]
        {
            let _ = (slot, local_path);
            Err(UploadError::Transport(
                "booth-pi was compiled without the operator feature".into(),
            ))
        }
    }

    #[cfg(feature = "operator")]
    async fn send_json<T>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&(impl Serialize + Sync)>,
    ) -> Result<T, OperatorError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let response = self.send(method, path, body).await?;
        response.json::<T>().await.map_err(|err| {
            OperatorError::Protocol(format!("failed to decode operator JSON: {err}").into())
        })
    }

    #[cfg(feature = "operator")]
    async fn send_empty(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&(impl Serialize + Sync)>,
    ) -> Result<(), OperatorError> {
        let _response = self.send(method, path, body).await?;
        Ok(())
    }

    #[cfg(feature = "operator")]
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&(impl Serialize + Sync)>,
    ) -> Result<reqwest::Response, OperatorError> {
        let url = self.api_url_for_path(path);
        debug!(method = method.as_str(), path, "operator request");

        let mut request = self.client.request(method, &url);
        if let Some(body) = body {
            request = request.json(body);
        }

        let response = request.send().await.map_err(operator_transport)?;
        let status = response.status();
        debug!(status = status.as_u16(), path, "operator response");

        if status.is_success() {
            Ok(response)
        } else {
            Err(map_operator_response(status.as_u16(), response).await)
        }
    }
}

#[async_trait]
impl OperatorClient for PiOperatorClient {
    async fn random_question(&self) -> Result<OperatorQuestion, OperatorError> {
        self.get_random_question().await
    }

    async fn random_message(&self) -> Result<OperatorMessage, OperatorError> {
        self.get_random_message().await
    }

    async fn init_upload(
        &self,
        question_id: Option<&QuestionId>,
        metadata: &booth_hal::UploadMetadata,
    ) -> Result<UploadSlot, OperatorError> {
        self.request_upload_slot(
            question_id,
            &metadata.sha256_hex,
            metadata.size_bytes,
            metadata.duration_ms,
        )
        .await
    }

    async fn put_upload(&self, slot: &UploadSlot, local_path: &str) -> Result<(), OperatorError> {
        self.upload_recording(slot, Path::new(local_path))
            .await
            .map_err(OperatorError::from)
    }

    async fn complete_upload(
        &self,
        slot_id: &str,
        sha256_hex: &str,
        duration_ms: u64,
    ) -> Result<(), OperatorError> {
        self.upload_complete(slot_id, sha256_hex, duration_ms).await
    }

    async fn put_status(&self, status: BoothStatus) -> Result<(), OperatorError> {
        self.put_status_ref(&status).await
    }

    async fn push_events_json(&self, body: &str) -> Result<EventBatchAck, OperatorError> {
        #[cfg(feature = "operator")]
        {
            let url = self.api_url_for_path("/v1/events");
            debug!(path = "/v1/events", "operator request (bulk events)");
            let response = self
                .client
                .request(reqwest::Method::POST, &url)
                .header(CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE))
                .body(body.to_owned())
                .send()
                .await
                .map_err(operator_transport)?;
            let status = response.status();
            debug!(status = status.as_u16(), "operator response (bulk events)");
            if !status.is_success() {
                return Err(map_operator_response(status.as_u16(), response).await);
            }
            response.json::<EventBatchAck>().await.map_err(|err| {
                OperatorError::Protocol(
                    format!("failed to decode /v1/events response: {err}").into(),
                )
            })
        }

        #[cfg(not(feature = "operator"))]
        {
            let _ = body;
            unsupported()
        }
    }

    async fn put_system_snapshot(
        &self,
        booth_id: &str,
        snapshot: &SystemSnapshot,
    ) -> Result<(), OperatorError> {
        #[cfg(feature = "operator")]
        {
            #[derive(Serialize)]
            struct Body<'a> {
                #[serde(rename = "boothId")]
                booth_id: &'a str,
                snapshot: &'a SystemSnapshot,
            }
            let body = Body { booth_id, snapshot };
            self.send_empty(reqwest::Method::PUT, "/v1/system", Some(&body))
                .await
        }

        #[cfg(not(feature = "operator"))]
        {
            let _ = (booth_id, snapshot);
            unsupported()
        }
    }
}

#[cfg(feature = "operator")]
/// Build the default headers used for operator API requests.
pub fn default_headers(token: &str) -> Result<HeaderMap, OperatorError> {
    let mut headers = HeaderMap::new();
    let auth = format!("Bearer {token}");
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&auth).map_err(|err| {
            OperatorError::Protocol(format!("invalid Authorization header: {err}").into())
        })?,
    );
    headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
    headers.insert(ACCEPT, HeaderValue::from_static(JSON_CONTENT_TYPE));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE));
    Ok(headers)
}

fn normalize_base_url(base_url: &str) -> String {
    base_url.trim_end_matches('/').to_string()
}

fn api_url(base_url: &str, path: &str) -> String {
    if path.starts_with('/') {
        format!("{base_url}{path}")
    } else {
        format!("{base_url}/{path}")
    }
}

#[cfg(not(feature = "operator"))]
fn unsupported<T>() -> Result<T, OperatorError> {
    Err(OperatorError::Unsupported(
        "booth-pi was compiled without the operator feature".into(),
    ))
}

#[cfg(feature = "operator")]
fn operator_transport(err: reqwest::Error) -> OperatorError {
    OperatorError::Transport(redact_url(&err.to_string()).into_owned().into())
}

#[cfg(feature = "operator")]
async fn map_operator_response(status: u16, response: reqwest::Response) -> OperatorError {
    let body = truncated_body(response).await;
    match status {
        401 => OperatorError::Unauthorized(
            "operator token was rejected; rotate the configured API token".into(),
        ),
        409 => OperatorError::DuplicateRecording(body.into()),
        _ => OperatorError::Server { status, body },
    }
}

#[cfg(feature = "operator")]
async fn truncated_body(response: reqwest::Response) -> String {
    match response.text().await {
        Ok(body) => body.chars().take(512).collect(),
        Err(err) => format!(
            "<failed to read response body: {}>",
            redact_url(&err.to_string())
        ),
    }
}

#[cfg(feature = "operator")]
fn is_retryable_upload_status(status: u16) -> bool {
    matches!(status, 408 | 409 | 425 | 429 | 500..=599)
}

#[cfg(feature = "operator")]
fn upload_backoff(attempt: usize) -> Duration {
    let multiplier = 1_u32.checked_shl(attempt as u32).unwrap_or(u32::MAX);
    UPLOAD_BACKOFF_BASE.saturating_mul(multiplier)
}

#[cfg(feature = "operator")]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiAudioRef {
    url: String,
    sha256: Option<String>,
}

#[cfg(feature = "operator")]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiQuestion {
    id: String,
    prompt: String,
    audio: ApiAudioRef,
}

#[cfg(feature = "operator")]
impl From<ApiQuestion> for OperatorQuestion {
    fn from(value: ApiQuestion) -> Self {
        Self {
            id: value.id,
            audio_url: value.audio.url,
            audio_sha256: value.audio.sha256,
            description: Some(value.prompt),
        }
    }
}

#[cfg(feature = "operator")]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiMessage {
    id: String,
    question_id: Option<String>,
    audio: ApiAudioRef,
}

#[cfg(feature = "operator")]
impl From<ApiMessage> for OperatorMessage {
    fn from(value: ApiMessage) -> Self {
        Self {
            id: value.id,
            audio_url: value.audio.url,
            audio_sha256: value.audio.sha256,
            question_id: value.question_id,
        }
    }
}

#[cfg(feature = "operator")]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadSlotRequest<'a> {
    sha256: &'a str,
    size_bytes: u64,
    content_type: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    question_id: Option<&'a QuestionId>,
}

#[cfg(feature = "operator")]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusUpdate {
    state: &'static str,
    updated_at: String,
    current_question_id: Option<String>,
    current_message_id: Option<String>,
    last_error: Option<String>,
}

#[cfg(feature = "operator")]
impl StatusUpdate {
    fn new(status: &BoothStatus) -> Self {
        Self {
            state: booth_status_state(status),
            updated_at: rfc3339_now(),
            current_question_id: None,
            current_message_id: None,
            last_error: None,
        }
    }
}

#[cfg(feature = "operator")]
fn booth_status_state(status: &BoothStatus) -> &'static str {
    match status {
        BoothStatus::Idle => "idle",
        BoothStatus::DialTone => "dialTone",
        BoothStatus::PlayingQuestion => "playingQuestion",
        BoothStatus::Recording => "recording",
        BoothStatus::Uploading => "uploading",
        BoothStatus::PlayingMessage => "playingMessage",
        BoothStatus::PlayingInstructions => "playingInstructions",
    }
}

#[cfg(feature = "operator")]
fn rfc3339_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    format_unix_seconds(seconds)
}

#[cfg(feature = "operator")]
fn format_unix_seconds(seconds: u64) -> String {
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(feature = "operator")]
fn civil_from_days(days_since_epoch: u64) -> (i32, u32, u32) {
    let z = i64::try_from(days_since_epoch).unwrap_or(i64::MAX) + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (
        i32::try_from(year).unwrap_or(i32::MAX),
        u32::try_from(m).unwrap_or(12),
        u32::try_from(d).unwrap_or(31),
    )
}
