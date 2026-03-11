use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::{Client, Method, RequestBuilder, StatusCode, Url};
use serde::{Deserialize, Serialize};

use crate::config::OpencodeApiConfig;

#[derive(Debug, Clone)]
pub struct OpencodeApiClient {
    client: Client,
    base_url: Url,
    token: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct HealthResponse {
    pub healthy: bool,
    pub version: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: String,
    pub directory: String,
    pub title: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CreateSessionRequest {
    #[serde(rename = "parentID", skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub title: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Busy,
    Retry {
        attempt: u64,
        message: String,
        next: u64,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptPartInput {
    Text { text: String },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PromptModelInput {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    #[serde(rename = "modelID")]
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PromptAsyncRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<PromptModelInput>,
    #[serde(rename = "noReply", skip_serializing_if = "Option::is_none")]
    pub no_reply: Option<bool>,
    pub parts: Vec<PromptPartInput>,
}

impl OpencodeApiClient {
    pub fn from_config(config: &OpencodeApiConfig) -> Result<Self> {
        let base_url = config
            .base_url
            .as_ref()
            .context("opencode API base URL is missing from configuration")?;

        Self::new(base_url, config.token.clone(), config.timeout_secs)
    }

    pub fn new(base_url: &str, token: Option<String>, timeout_secs: u64) -> Result<Self> {
        let base_url = Url::parse(base_url)
            .with_context(|| format!("failed to parse opencode API base URL: {}", base_url))?;

        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .user_agent("forgebot")
            .build()
            .context("failed to build opencode API HTTP client")?;

        Ok(Self {
            client,
            base_url,
            token,
        })
    }

    pub async fn health(&self) -> Result<HealthResponse> {
        self.send_json(
            self.request_builder(Method::GET, "/global/health", None)?,
            "GET /global/health",
        )
        .await
    }

    pub async fn create_session(
        &self,
        directory: &Path,
        request: &CreateSessionRequest,
    ) -> Result<SessionRecord> {
        self.send_json(
            self.request_builder(Method::POST, "/session", Some(directory))?
                .json(request),
            "POST /session",
        )
        .await
    }

    pub async fn get_session(&self, directory: &Path, session_id: &str) -> Result<SessionRecord> {
        self.send_json(
            self.request_builder(
                Method::GET,
                &format!("/session/{}", session_id),
                Some(directory),
            )?,
            "GET /session/{sessionID}",
        )
        .await
    }

    pub async fn session_status(&self, directory: &Path) -> Result<HashMap<String, SessionStatus>> {
        self.send_json(
            self.request_builder(Method::GET, "/session/status", Some(directory))?,
            "GET /session/status",
        )
        .await
    }

    pub async fn prompt_async(
        &self,
        directory: &Path,
        session_id: &str,
        request: &PromptAsyncRequest,
    ) -> Result<()> {
        self.send_empty(
            self.request_builder(
                Method::POST,
                &format!("/session/{}/prompt_async", session_id),
                Some(directory),
            )?
            .json(request),
            "POST /session/{sessionID}/prompt_async",
            &[StatusCode::NO_CONTENT],
        )
        .await
    }

    pub async fn abort(&self, directory: &Path, session_id: &str) -> Result<bool> {
        self.send_json(
            self.request_builder(
                Method::POST,
                &format!("/session/{}/abort", session_id),
                Some(directory),
            )?,
            "POST /session/{sessionID}/abort",
        )
        .await
    }

    fn request_builder(
        &self,
        method: Method,
        path: &str,
        directory: Option<&Path>,
    ) -> Result<RequestBuilder> {
        let mut url = self
            .base_url
            .join(path)
            .with_context(|| format!("failed to join opencode API URL path: {}", path))?;

        if let Some(directory) = directory {
            let directory_value = directory.display().to_string();
            url.query_pairs_mut()
                .append_pair("directory", &directory_value);
        }

        let mut request = self.client.request(method, url);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }

        Ok(request)
    }

    async fn send_json<T>(&self, request: RequestBuilder, operation: &str) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let response = request
            .send()
            .await
            .with_context(|| format!("{} failed to send", operation))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            anyhow::bail!(
                "{} failed: {} {} - {}",
                operation,
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown"),
                body
            );
        }

        response
            .json::<T>()
            .await
            .with_context(|| format!("{} returned invalid JSON", operation))
    }

    async fn send_empty(
        &self,
        request: RequestBuilder,
        operation: &str,
        ok_statuses: &[StatusCode],
    ) -> Result<()> {
        let response = request
            .send()
            .await
            .with_context(|| format!("{} failed to send", operation))?;

        let status = response.status();
        if ok_statuses.contains(&status) {
            return Ok(());
        }

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            anyhow::bail!(
                "{} failed: {} {} - {}",
                operation,
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown"),
                body
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::{HeaderValue, Method, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use serde_json::json;

    use super::*;

    fn test_client(base_url: &str) -> OpencodeApiClient {
        OpencodeApiClient::new(base_url, Some("secret-token".to_string()), 5)
            .expect("client should initialize")
    }

    #[tokio::test]
    async fn test_health_parses_response() {
        let app = Router::new().route(
            "/global/health",
            get(|| async {
                (
                    StatusCode::OK,
                    axum::Json(json!({"healthy": true, "version": "1.2.3"})),
                )
            }),
        );

        let server = spawn_test_server(app).await;
        let client = test_client(&server.base_url);

        let health = client.health().await.expect("health should succeed");
        assert_eq!(
            health,
            HealthResponse {
                healthy: true,
                version: "1.2.3".to_string()
            }
        );
    }

    #[tokio::test]
    async fn test_create_session_sends_directory_query() {
        let app = Router::new().route(
            "/session",
            post(|req: Request| async move {
                assert_eq!(req.method(), Method::POST);
                assert!(
                    req.uri()
                        .query()
                        .unwrap_or_default()
                        .contains("directory=%2Ftmp%2Frepo")
                );
                (
                    StatusCode::OK,
                    axum::Json(json!({
                        "id": "ses_123",
                        "directory": "/tmp/repo",
                        "title": "Issue session",
                        "version": "0.0.3"
                    })),
                )
            }),
        );

        let server = spawn_test_server(app).await;
        let client = test_client(&server.base_url);

        let created = client
            .create_session(
                Path::new("/tmp/repo"),
                &CreateSessionRequest {
                    parent_id: None,
                    title: "Issue session".to_string(),
                },
            )
            .await
            .expect("session creation should succeed");

        assert_eq!(created.id, "ses_123");
        assert_eq!(created.directory, "/tmp/repo");
    }

    #[tokio::test]
    async fn test_get_session_sends_directory_query() {
        let app = Router::new().route(
            "/session/ses_abc",
            get(|req: Request| async move {
                assert!(
                    req.uri()
                        .query()
                        .unwrap_or_default()
                        .contains("directory=%2Ftmp%2Fwt")
                );
                (
                    StatusCode::OK,
                    axum::Json(json!({
                        "id": "ses_abc",
                        "directory": "/tmp/wt",
                        "title": "Existing",
                        "version": "0.0.3"
                    })),
                )
            }),
        );

        let server = spawn_test_server(app).await;
        let client = test_client(&server.base_url);

        let session = client
            .get_session(Path::new("/tmp/wt"), "ses_abc")
            .await
            .expect("get session should succeed");

        assert_eq!(session.id, "ses_abc");
    }

    #[tokio::test]
    async fn test_session_status_parses_variants() {
        let app = Router::new().route(
            "/session/status",
            get(|| async {
                (
                    StatusCode::OK,
                    axum::Json(json!({
                        "ses_a": {"type": "idle"},
                        "ses_b": {"type": "busy"},
                        "ses_c": {"type": "retry", "attempt": 2, "message": "waiting", "next": 1731000}
                    })),
                )
            }),
        );

        let server = spawn_test_server(app).await;
        let client = test_client(&server.base_url);

        let statuses = client
            .session_status(Path::new("/tmp/wt"))
            .await
            .expect("status call should succeed");

        assert_eq!(statuses.get("ses_a"), Some(&SessionStatus::Idle));
        assert_eq!(statuses.get("ses_b"), Some(&SessionStatus::Busy));
        assert_eq!(
            statuses.get("ses_c"),
            Some(&SessionStatus::Retry {
                attempt: 2,
                message: "waiting".to_string(),
                next: 1731000,
            })
        );
    }

    #[tokio::test]
    async fn test_prompt_async_accepts_no_content() {
        let app = Router::new().route(
            "/session/ses_1/prompt_async",
            post(|req: Request| async move {
                assert!(
                    req.uri()
                        .query()
                        .unwrap_or_default()
                        .contains("directory=%2Ftmp%2Fwt")
                );
                StatusCode::NO_CONTENT
            }),
        );

        let server = spawn_test_server(app).await;
        let client = test_client(&server.base_url);

        client
            .prompt_async(
                Path::new("/tmp/wt"),
                "ses_1",
                &PromptAsyncRequest {
                    agent: Some("build".to_string()),
                    model: Some(PromptModelInput {
                        provider_id: "opencode".to_string(),
                        model_id: "kimi-k2.5".to_string(),
                    }),
                    no_reply: Some(true),
                    parts: vec![PromptPartInput::Text {
                        text: "Please implement".to_string(),
                    }],
                },
            )
            .await
            .expect("prompt async should succeed");
    }

    #[tokio::test]
    async fn test_abort_returns_boolean() {
        let app = Router::new().route(
            "/session/ses_1/abort",
            post(|| async { (StatusCode::OK, axum::Json(json!(true))) }),
        );

        let server = spawn_test_server(app).await;
        let client = test_client(&server.base_url);

        let result = client
            .abort(Path::new("/tmp/wt"), "ses_1")
            .await
            .expect("abort should succeed");

        assert!(result);
    }

    #[tokio::test]
    async fn test_http_error_includes_context() {
        let app = Router::new().route(
            "/session/status",
            get(|| async { (StatusCode::BAD_REQUEST, "invalid directory") }),
        );

        let server = spawn_test_server(app).await;
        let client = test_client(&server.base_url);

        let err = client
            .session_status(Path::new("/tmp/wt"))
            .await
            .expect_err("request should fail");

        let err_text = err.to_string();
        assert!(err_text.contains("GET /session/status failed"));
        assert!(err_text.contains("400"));
        assert!(err_text.contains("invalid directory"));
    }

    #[tokio::test]
    async fn test_bearer_token_header_is_set() {
        let headers_seen: Arc<Mutex<HashMap<String, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let headers_seen_clone = Arc::clone(&headers_seen);

        let app = Router::new().route(
            "/global/health",
            get(move |req: Request| {
                let headers_seen = Arc::clone(&headers_seen_clone);
                async move {
                    let auth = req
                        .headers()
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    headers_seen
                        .lock()
                        .expect("header lock should succeed")
                        .insert("authorization".to_string(), auth);
                    (
                        StatusCode::OK,
                        axum::Json(json!({"healthy": true, "version": "1.0.0"})),
                    )
                }
            }),
        );

        let server = spawn_test_server(app).await;
        let client = test_client(&server.base_url);
        client.health().await.expect("health should succeed");

        let auth = headers_seen
            .lock()
            .expect("header lock should succeed")
            .get("authorization")
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            auth,
            HeaderValue::from_static("Bearer secret-token")
                .to_str()
                .unwrap()
        );
    }

    struct TestServer {
        base_url: String,
    }

    async fn spawn_test_server(app: Router) -> TestServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind should succeed");
        let addr = listener.local_addr().expect("local addr should resolve");
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .expect("server should run");
        });

        TestServer {
            base_url: format!("http://{}", addr),
        }
    }

    #[tokio::test]
    async fn test_from_config_requires_base_url() {
        let config = OpencodeApiConfig {
            base_url: None,
            token: None,
            timeout_secs: 30,
        };

        let err = OpencodeApiClient::from_config(&config).expect_err("missing base URL must fail");
        assert!(
            err.to_string()
                .contains("opencode API base URL is missing from configuration")
        );
    }

    #[test]
    fn test_new_rejects_invalid_url() {
        let err = OpencodeApiClient::new("not-a-url", None, 30)
            .expect_err("invalid base URL should fail");
        assert!(
            err.to_string()
                .contains("failed to parse opencode API base URL")
        );
    }

    #[tokio::test]
    async fn test_router_roundtrip_for_body_shape() {
        let app = Router::new().route(
            "/session/ses_1/prompt_async",
            post(|request: Request| async move {
                let bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                    .await
                    .expect("body should be readable");
                let value: serde_json::Value =
                    serde_json::from_slice(&bytes).expect("request body should be valid JSON");
                assert_eq!(value["parts"][0]["type"], "text");
                assert_eq!(value["parts"][0]["text"], "hello");
                StatusCode::NO_CONTENT.into_response()
            }),
        );

        let server = spawn_test_server(app).await;
        let client = test_client(&server.base_url);

        client
            .prompt_async(
                Path::new("/tmp/wt"),
                "ses_1",
                &PromptAsyncRequest {
                    agent: None,
                    model: None,
                    no_reply: None,
                    parts: vec![PromptPartInput::Text {
                        text: "hello".to_string(),
                    }],
                },
            )
            .await
            .expect("request should serialize expected JSON shape");
    }

    #[tokio::test]
    async fn test_health_http_error_surface() {
        let app = Router::new().route(
            "/global/health",
            get(|| async { (StatusCode::INTERNAL_SERVER_ERROR, Body::from("boom")) }),
        );

        let server = spawn_test_server(app).await;
        let client = test_client(&server.base_url);
        let err = client
            .health()
            .await
            .expect_err("health should fail on 500");
        assert!(err.to_string().contains("GET /global/health failed"));
        assert!(err.to_string().contains("boom"));
    }
}
