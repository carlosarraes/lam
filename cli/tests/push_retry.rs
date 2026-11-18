use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "s3cr3t-push-token";

fn item(id: &str) -> Value {
    json!({
        "id": id, "kind": "fyi", "name": "test:agent", "title": "Deployed",
        "body": "", "source_host": "h", "source_project": "p", "priority": "normal",
        "choices": [], "checks": [], "version": 0, "status": "open",
        "response_choice": null, "response_text": null, "response_by": null,
        "created_at": "2026-11-18T12:00:00Z", "resolved_at": null
    })
}

struct Env {
    config: tempfile::TempDir,
    cache: tempfile::TempDir,
}

impl Env {
    fn new(server: &str) -> Self {
        let config = tempfile::tempdir().unwrap();
        std::fs::write(
            config.path().join("config.toml"),
            format!("server = \"{server}\"\ntoken = \"{TOKEN}\"\ntopic = \"top\"\n"),
        )
        .unwrap();
        Self {
            config,
            cache: tempfile::tempdir().unwrap(),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lam"));
        command
            .env("LAM_CONFIG", self.config.path().join("config.toml"))
            .env("LAM_NAME", "test:agent")
            .env("XDG_CACHE_HOME", self.cache.path())
            .env("LAM_PUSH_RETRY_BASE_MS", "10")
            .args(["push", "Deployed", "--kind", "fyi"]);
        command
    }

    fn push(&self) -> Output {
        self.command().output().unwrap()
    }

    fn last_error(&self) -> Value {
        let raw =
            std::fs::read_to_string(self.cache.path().join("lam").join("last-error.json")).unwrap();
        assert!(!raw.contains(TOKEN), "token leaked: {raw}");
        assert!(!raw.to_lowercase().contains("bearer"), "auth leaked: {raw}");
        serde_json::from_str(&raw).unwrap()
    }
}

fn retry_lines(out: &Output) -> usize {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|line| line.contains("retrying in"))
        .count()
}

async fn respond_once_then(server: &MockServer, first: u16) {
    Mock::given(method("POST"))
        .and(path("/v2/items"))
        .respond_with(ResponseTemplate::new(first).set_body_string("try later"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/items"))
        .respond_with(ResponseTemplate::new(201).set_body_json(item("ok123")))
        .mount(server)
        .await;
}

#[tokio::test]
async fn push_retries_a_503_then_succeeds_with_one_retry_line() {
    let server = MockServer::start().await;
    respond_once_then(&server, 503).await;
    let env = Env::new(&server.uri());

    let out = env.push();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ok123\n");
    assert_eq!(retry_lines(&out), 1, "{stderr}");
    assert!(stderr.contains("503"), "{stderr}");
    assert!(stderr.contains("(attempt 2/4)"), "{stderr}");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    assert!(!env
        .cache
        .path()
        .join("lam")
        .join("last-error.json")
        .exists());
}

#[tokio::test]
async fn push_retries_429() {
    let server = MockServer::start().await;
    respond_once_then(&server, 429).await;
    let env = Env::new(&server.uri());

    let out = env.push();

    assert_eq!(out.status.code(), Some(0));
    assert_eq!(retry_lines(&out), 1);
}

#[tokio::test]
async fn push_gives_up_after_four_503s_and_records_the_last_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/items"))
        .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
        .expect(4)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());

    let out = env.push();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(out.stdout.is_empty());
    assert_eq!(retry_lines(&out), 3, "{stderr}");
    assert!(stderr.contains("push failed after 4 attempts"), "{stderr}");
    assert!(stderr.contains("last-error.json"), "{stderr}");
    assert!(!stderr.contains(TOKEN));

    let record = env.last_error();
    assert_eq!(record["command"], "push");
    assert_eq!(record["status"], 503);
    assert_eq!(record["attempts"], 4);
    assert_eq!(record["retryable"], true);
    assert!(record["elapsed_ms"].as_u64().is_some(), "{record}");
    assert!(record["error"].as_str().unwrap().contains("unavailable"));
    assert!(
        chrono::DateTime::parse_from_rfc3339(record["timestamp"].as_str().unwrap()).is_ok(),
        "{record}"
    );
}

#[tokio::test]
async fn push_does_not_retry_a_400() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/items"))
        .respond_with(ResponseTemplate::new(400).set_body_string("bad title"))
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());

    let out = env.push();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert_eq!(retry_lines(&out), 0, "{stderr}");
    assert!(
        stderr.contains("push failed after 1 attempt in"),
        "{stderr}"
    );
    let record = env.last_error();
    assert_eq!(record["status"], 400);
    assert_eq!(record["attempts"], 1);
    assert_eq!(record["retryable"], false);
}

#[tokio::test]
async fn push_retries_connect_errors_and_records_no_status() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let env = Env::new(&format!("http://127.0.0.1:{port}"));

    let out = env.push();

    assert_eq!(out.status.code(), Some(1));
    assert_eq!(retry_lines(&out), 3);
    let record = env.last_error();
    assert_eq!(record["status"], Value::Null);
    assert_eq!(record["attempts"], 4);
}

#[tokio::test]
async fn push_retry_budget_bounds_total_time() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/items"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());

    let started = Instant::now();
    let out = env
        .command()
        .env("LAM_PUSH_RETRY_BASE_MS", "5000")
        .env("LAM_PUSH_RETRY_BUDGET_MS", "300")
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(1));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert!(env.last_error()["attempts"].as_u64().unwrap() < 4);
}

#[tokio::test]
async fn last_error_falls_back_to_home_cache_without_xdg() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/items"))
        .respond_with(ResponseTemplate::new(400))
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    let home = tempfile::tempdir().unwrap();

    let out = env
        .command()
        .env_remove("XDG_CACHE_HOME")
        .env("HOME", home.path())
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(1));
    assert!(home.path().join(".cache/lam/last-error.json").exists());
}
