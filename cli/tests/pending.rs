use std::process::Command;

use serde_json::{json, Value};
use wiremock::matchers::{body_json, method, path, query_param};
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
        .env("XDG_CACHE_HOME", dir.path())
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

#[tokio::test]
async fn push_accepts_only_supported_link_schemes() {
    let (server, dir) = setup().await;
    Mock::given(method("POST"))
        .and(path("/v2/items"))
        .respond_with(ResponseTemplate::new(201).set_body_json(item()))
        .mount(&server)
        .await;
    let links = [
        "grokbot://app/v1/agent?id=agent-42&view=chat",
        "GrOkBoT://app/v1/agent?id=agent-42",
        "http://example.com/task",
        "https://example.com/task",
        "HTTPS://example.com/task",
    ];
    for link in links {
        assert_success(&lam(
            &dir,
            &[
                "push",
                "Open task",
                "--recommendation",
                "Review the linked task",
                "-l",
                link,
            ],
        ));
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), links.len());
    for (request, link) in requests.iter().zip(links.iter().cycle()) {
        assert_eq!(request.body_json::<Value>().unwrap()["link"], *link);
    }
}

#[tokio::test]
async fn push_rejects_unsupported_links_before_http() {
    let (server, dir) = setup().await;
    for link in [
        "javascript:alert(1)",
        "file:///etc/passwd",
        "ftp://x",
        "data:text/html,x",
        "mailto:user@example.com",
        "bare text",
        "grokbot:",
        "grokbot:app",
        "grokbot://",
        "http://",
        "https://",
        "grokbot:// ",
    ] {
        let out = lam(
            &dir,
            &[
                "push",
                "Open task",
                "--recommendation",
                "Review the linked task",
                "--link",
                link,
            ],
        );
        assert_eq!(out.status.code(), Some(1), "{link}");
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(
            error.contains("lam: --link must be an http://, https:// or grokbot:// URL"),
            "{link}: {error}"
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn pending_help_explains_carlos_only_lane_and_agent_lifecycle_commands() {
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
        "Carlos parks items in Pending from the TUI (p) or the Android Pending screen",
        "send a request (default) or --kind fyi instead",
        "lam pending list",
        "lam pending resolve ID",
        "lam pending resolve --key KEY",
        "-m NOTE",
    ] {
        assert!(help.contains(phrase), "missing {phrase:?} in {help}");
    }
}

fn assert_pending_rejected(out: &std::process::Output) {
    assert_eq!(out.status.code(), Some(1));
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains("pending is Carlos-only: send a request (default) or --kind fyi instead"),
        "{error}"
    );
}

#[tokio::test]
async fn pending_add_is_rejected_before_http() {
    let (server, dir) = setup().await;
    let out = lam(
        &dir,
        &["pending", "add", "Track review", "--key", "review:42"],
    );
    assert_pending_rejected(&out);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn push_pending_is_rejected_for_any_kind_before_validation_or_http() {
    let (server, dir) = setup().await;
    for args in [
        vec!["push", "Review this", "--pending"],
        vec![
            "push",
            "Review this",
            "--pending",
            "--recommendation",
            "Approve after review",
            "--key",
            "review:42",
        ],
        vec!["push", "An update", "--kind", "fyi", "--pending"],
        vec!["push", "An update", "--kind", "fyi", "--pending", "--wait"],
    ] {
        assert_pending_rejected(&lam(&dir, &args));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
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
    assert!(String::from_utf8_lossy(&out.stdout).contains("review:42"));
}

#[tokio::test]
async fn show_preserves_external_key() {
    let (server, dir) = setup().await;
    Mock::given(method("GET"))
        .and(path("/items/pend1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(item()))
        .mount(&server)
        .await;
    let out = lam(&dir, &["show", "pend1"]);
    assert_success(&out);
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["external_key"], "review:42");
}

#[tokio::test]
async fn pending_resolve_missing_key_suggests_up_to_five_open_items_or_none() {
    for count in [0, 6] {
        let (server, dir) = setup().await;
        Mock::given(method("POST"))
            .and(path("/v2/items/resolve"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;
        let items: Vec<_> = (0..count)
            .map(|n| {
                let mut value = item();
                value["id"] = format!("pending-{n}").into();
                value["external_key"] = if n == 0 {
                    Value::Null
                } else {
                    format!("key-{n}").into()
                };
                value
            })
            .collect();
        Mock::given(method("GET"))
            .and(path("/items"))
            .and(query_param("pending", "1"))
            .and(query_param("status", "open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(items))
            .expect(1)
            .mount(&server)
            .await;
        let out = lam(&dir, &["pending", "resolve", "--key", "missing"]);
        assert_eq!(out.status.code(), Some(1));
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(error.contains("no open item with key missing"), "{error}");
        assert!(
            error.contains("resolve by id: lam pending resolve ID"),
            "{error}"
        );
        if count == 0 {
            assert!(error.contains("no open pending items"), "{error}");
        } else {
            assert!(error.contains("pending-0  -  Track review"), "{error}");
            assert!(error.contains("pending-4  key-4  Track review"), "{error}");
            assert!(!error.contains("pending-5"), "{error}");
        }
    }
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
async fn pending_move_and_unmove_are_rejected_before_http() {
    let (server, dir) = setup().await;
    for command in ["move", "unmove"] {
        assert_pending_rejected(&lam(&dir, &["pending", command, "pend1"]));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
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
