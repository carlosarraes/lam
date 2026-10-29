use std::process::Command;

use serde_json::{json, Value};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn item() -> Value {
    json!({
        "id": "pend1", "kind": "fyi", "name": "test:agent", "title": "Track review",
        "body": "Waiting for review", "source_host": "h", "source_project": "p",
        "priority": "normal", "choices": [], "status": "open",
        "response_choice": null, "response_text": null, "response_by": null,
        "created_at": "2026-10-29T12:00:00Z", "resolved_at": null,
        "pending_at": "2026-10-29T12:00:00Z", "external_key": "review:42"
    })
}

async fn setup() -> (MockServer, tempfile::TempDir) {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        format!(
            "server = \"{}\"\ntoken = \"tok\"\ntopic = \"top\"\n",
            server.uri()
        ),
    )
    .unwrap();
    (server, dir)
}

fn lam(dir: &tempfile::TempDir, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lam"))
        .env("LAM_CONFIG", dir.path().join("config.toml"))
        .env("LAM_NAME", "test:agent")
        .args(args)
        .output()
        .unwrap()
}

fn assert_success(out: &std::process::Output) {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn pending_help_explains_silence_dedupe_and_lifecycle_commands() {
    let out = Command::new(env!("CARGO_BIN_EXE_lam"))
        .args(["pending", "--help"])
        .output()
        .unwrap();
    assert_success(&out);
    let help = String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for phrase in [
        "silent (no ntfy/FCM/bell alerts)",
        "hidden from lam list and Requests",
        "--key is a stable dedupe key",
        "same open key updates title/body/link and returns the same id",
        "after it closes the key can be reused",
        "lam pending resolve ID",
        "lam pending resolve --key KEY",
        "-m NOTE",
        "lam pending move ID",
        "lam pending unmove ID",
    ] {
        assert!(help.contains(phrase), "missing {phrase:?} in {help}");
    }
}

#[tokio::test]
async fn pending_add_creates_keyed_fyi_and_accepts_upsert_response() {
    let (server, dir) = setup().await;
    Mock::given(method("POST"))
        .and(path("/v2/items"))
        .and(header("authorization", "Bearer tok"))
        .respond_with(ResponseTemplate::new(200).set_body_json(item()))
        .mount(&server)
        .await;
    let out = lam(
        &dir,
        &[
            "pending",
            "add",
            "Track review",
            "--key",
            "review:42",
            "-n",
            "reviewer",
            "-b",
            "Waiting for review",
            "-l",
            "https://example.com/42",
        ],
    );
    assert_success(&out);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "pend1");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: Value = requests[0].body_json().unwrap();
    assert_eq!(body["kind"], "fyi");
    assert_eq!(body["pending"], true);
    assert_eq!(body["external_key"], "review:42");
    assert_eq!(body["name"], "reviewer");
    assert_eq!(body["title"], "Track review");
    assert_eq!(body["body"], "Waiting for review");
    assert_eq!(body["link"], "https://example.com/42");
    assert!(body.get("recommendation").is_none());
}

#[tokio::test]
async fn push_pending_preserves_request_fields_and_optional_key() {
    let (server, dir) = setup().await;
    Mock::given(method("POST"))
        .and(path("/v2/items"))
        .respond_with(ResponseTemplate::new(201).set_body_json(item()))
        .mount(&server)
        .await;
    for key in [vec![], vec!["--key", "review:42"]] {
        let args = [
            vec![
                "push",
                "Review this",
                "--pending",
                "--recommendation",
                "Approve after review",
            ],
            key.clone(),
        ]
        .concat();
        let out = lam(&dir, &args);
        assert_success(&out);
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        let body: Value = request.body_json().unwrap();
        assert_eq!(body["kind"], "request");
        assert_eq!(body["pending"], true);
        assert_eq!(body["recommendation"], "Approve after review");
    }
    let first: Value = requests[0].body_json().unwrap();
    assert!(first.get("external_key").is_none());
    let second: Value = requests[1].body_json().unwrap();
    assert_eq!(second["external_key"], "review:42");
}

#[tokio::test]
async fn ordinary_push_omits_pending_fields_for_older_servers() {
    let (server, dir) = setup().await;
    let mut legacy = item();
    legacy.as_object_mut().unwrap().remove("pending_at");
    legacy.as_object_mut().unwrap().remove("external_key");
    Mock::given(method("POST"))
        .and(path("/v2/items"))
        .respond_with(ResponseTemplate::new(201).set_body_json(legacy))
        .mount(&server)
        .await;
    let out = lam(&dir, &["push", "An update", "--kind", "fyi"]);
    assert_success(&out);
    let requests = server.received_requests().await.unwrap();
    let body: Value = requests[0].body_json().unwrap();
    assert!(body.get("pending").is_none());
    assert!(body.get("external_key").is_none());
}

#[tokio::test]
async fn pending_list_requests_only_open_pending_items_and_prints_json() {
    let (server, dir) = setup().await;
    Mock::given(method("GET"))
        .and(path("/items"))
        .and(query_param("pending", "1"))
        .and(query_param("status", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(vec![item()]))
        .expect(2)
        .mount(&server)
        .await;
    let out = lam(&dir, &["pending", "list", "--json"]);
    assert_success(&out);
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value[0]["external_key"], "review:42");
    assert_eq!(value[0]["pending_at"], "2026-10-29T12:00:00Z");
    let out = lam(&dir, &["pending", "list"]);
    assert_success(&out);
    assert!(String::from_utf8_lossy(&out.stdout).contains("Track review"));
}

#[tokio::test]
async fn pending_resolve_by_key_or_id_sends_optional_note() {
    let (server, dir) = setup().await;
    for (args, endpoint, body) in [
        (
            vec!["pending", "resolve", "--key", "review:42", "-m", "Merged"],
            "/v2/items/resolve",
            json!({"external_key": "review:42", "text": "Merged"}),
        ),
        (
            vec!["pending", "resolve", "pend1"],
            "/items/pend1/resolve",
            json!({}),
        ),
    ] {
        let mut resolved = item();
        resolved["status"] = "resolved".into();
        Mock::given(method("POST"))
            .and(path(endpoint))
            .and(body_json(body))
            .respond_with(ResponseTemplate::new(200).set_body_json(resolved))
            .expect(1)
            .mount(&server)
            .await;
        let out = lam(&dir, &args);
        assert_success(&out);
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["status"], "resolved");
    }
}

#[tokio::test]
async fn pending_move_and_unmove_change_lane_by_id() {
    let (server, dir) = setup().await;
    for (command, pending) in [("move", true), ("unmove", false)] {
        Mock::given(method("POST"))
            .and(path("/v2/items/pend1/pending"))
            .and(body_json(json!({"pending": pending})))
            .respond_with(ResponseTemplate::new(200).set_body_json(item()))
            .expect(1)
            .mount(&server)
            .await;
        let out = lam(&dir, &["pending", command, "pend1"]);
        assert_success(&out);
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["id"], "pend1");
    }
}

#[tokio::test]
async fn pending_resolve_requires_exactly_one_identifier_before_http() {
    let (server, dir) = setup().await;
    for args in [
        vec!["pending", "resolve"],
        vec!["pending", "resolve", "pend1", "--key", "review:42"],
        vec!["pending", "add", "Track review"],
    ] {
        let out = lam(&dir, &args);
        assert_eq!(out.status.code(), Some(2));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}
