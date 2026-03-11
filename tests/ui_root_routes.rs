use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use forgebot::config::{
    Config, DatabaseConfig, ForgejoConfig, OpencodeApiConfig, OpencodeConfig, OpencodeTransport,
    ServerConfig,
};
use forgebot::db::init_db_at_path;
use forgebot::forgejo::ForgejoClient;
use forgebot::webhook::{AppState, create_app_router};
use tower::ServiceExt;

fn unique_temp_dir() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "forgebot-test-{}-{}",
        std::process::id(),
        timestamp
    ))
}

async fn make_test_app() -> Result<(axum::Router, PathBuf)> {
    let temp_dir = unique_temp_dir();
    std::fs::create_dir_all(&temp_dir)
        .with_context(|| format!("failed to create temp dir {}", temp_dir.display()))?;

    let db_path = temp_dir.join("forgebot.db");
    let db = init_db_at_path(&db_path).await?;

    let config = Config {
        server: ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8765,
            webhook_secret: "test-secret".to_string(),
            forgebot_host: "http://127.0.0.1:8765".to_string(),
        },
        forgejo: ForgejoConfig {
            url: "http://127.0.0.1:1".to_string(),
            token: "test-token".to_string(),
            bot_username: "forgebot".to_string(),
        },
        opencode: OpencodeConfig {
            binary: "opencode".to_string(),
            worktree_base: temp_dir.join("worktrees"),
            config_dir: temp_dir.join("config"),
            git_binary: "git".to_string(),
            model: "opencode/kimi-k2.5".to_string(),
            web_host: None,
            transport: OpencodeTransport::Cli,
            api: OpencodeApiConfig {
                base_url: None,
                token: None,
                timeout_secs: 30,
            },
        },
        database: DatabaseConfig { path: db_path },
    };

    let forgejo = ForgejoClient::new(
        &config.forgejo.url,
        &config.forgejo.token,
        &config.forgejo.bot_username,
    )?;
    let state = AppState::new(Arc::new(config), db, forgejo);

    Ok((create_app_router(state), temp_dir))
}

fn cleanup_temp_dir(temp_dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn test_dashboard_route_is_root_mounted() {
    let (app, temp_dir) = make_test_app().await.expect("failed to create app");

    let response = app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .expect("request to / should succeed");

    assert_eq!(response.status(), StatusCode::OK);

    let old_response = app
        .oneshot(Request::builder().uri("/ui").body(Body::empty()).unwrap())
        .await
        .expect("request to /ui should return a response");

    assert_eq!(old_response.status(), StatusCode::NOT_FOUND);

    cleanup_temp_dir(&temp_dir);
}

#[tokio::test]
async fn test_add_repo_uses_root_path_and_old_ui_path_is_missing() {
    let (app, temp_dir) = make_test_app().await.expect("failed to create app");
    let form = "full_name=riley%2Fterminal-config&default_branch=main&env_loader=none";

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/repos")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .expect("request to /repos should succeed");

    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let old_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ui/repos")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .expect("request to /ui/repos should return a response");

    assert_eq!(old_response.status(), StatusCode::NOT_FOUND);

    cleanup_temp_dir(&temp_dir);
}
