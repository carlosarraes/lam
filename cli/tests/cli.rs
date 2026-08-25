use std::process::Command;

use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn item(id: &str, status: &str, choice: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "id": id, "title": "t", "body": "", "source_host": "h", "source_project": "p",
        "priority": "normal", "choices": [], "status": status,
        "response_choice": choice, "response_text": null, "response_by": choice.map(|_| "phone"),
        "created_at": "2026-08-25T00:00:00Z", "resolved_at": null
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
        .args(args)
        .output()
        .unwrap()
}

#[tokio::test]
async fn push_prints_id_and_sends_bearer() {
    let (server, dir) = setup().await;
    Mock::given(method("POST"))
        .and(path("/items"))
        .and(header("authorization", "Bearer tok"))
        .respond_with(ResponseTemplate::new(201).set_body_json(item("abc12", "open", None)))
        .expect(1)
        .mount(&server)
        .await;
    let out = lam(
        &dir,
        &[
            "push",
            "hello",
            "-c",
            "yes",
            "-c",
            "no",
            "-p",
            "critical",
            "--link",
            "https://x/pr/1",
            "--ttl",
            "30m",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "abc12");
    let req = &server.received_requests().await.unwrap()[0];
    let body: serde_json::Value = req.body_json().unwrap();
    assert_eq!(body["title"], "hello");
    assert_eq!(body["priority"], "critical");
    assert_eq!(body["choices"], serde_json::json!(["yes", "no"]));
    assert_eq!(body["link"], "https://x/pr/1");
    assert_eq!(body["ttl"], 1800);
    assert!(!body["source_host"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn push_rejects_four_choices() {
    let (_server, dir) = setup().await;
    let out = lam(
        &dir,
        &["push", "x", "-c", "a", "-c", "b", "-c", "c", "-c", "d"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("at most 3"));
}

#[tokio::test]
async fn wait_polls_until_closed_and_exits_by_status() {
    let (server, dir) = setup().await;
    Mock::given(method("GET"))
        .and(path("/items/abc12/wait"))
        .respond_with(ResponseTemplate::new(204))
        .up_to_n_times(2)
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/items/abc12/wait"))
        .respond_with(ResponseTemplate::new(200).set_body_json(item(
            "abc12",
            "resolved",
            Some("yes"),
        )))
        .mount(&server)
        .await;
    let out = lam(&dir, &["wait", "abc12"]);
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["response_choice"], "yes");

    Mock::given(method("GET"))
        .and(path("/items/gone1/wait"))
        .respond_with(ResponseTemplate::new(200).set_body_json(item("gone1", "dismissed", None)))
        .mount(&server)
        .await;
    assert_eq!(lam(&dir, &["wait", "gone1"]).status.code(), Some(2));
}

#[tokio::test]
async fn wait_times_out_with_exit_3() {
    let (server, dir) = setup().await;
    Mock::given(method("GET"))
        .and(path("/items/slow1/wait"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    assert_eq!(
        lam(&dir, &["wait", "slow1", "--timeout", "0s"])
            .status
            .code(),
        Some(3)
    );
}

#[tokio::test]
async fn done_and_list() {
    let (server, dir) = setup().await;
    Mock::given(method("POST"))
        .and(path("/items/abc12/resolve"))
        .and(body_json(
            serde_json::json!({ "choice": "yes", "text": "because" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(item(
            "abc12",
            "resolved",
            Some("yes"),
        )))
        .expect(1)
        .mount(&server)
        .await;
    assert!(lam(&dir, &["done", "abc12", "yes", "-m", "because"])
        .status
        .success());

    Mock::given(method("GET"))
        .and(path("/items"))
        .and(query_param("status", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(vec![item("abc12", "open", None)]))
        .expect(1)
        .mount(&server)
        .await;
    let out = lam(&dir, &["list"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("abc12  open"));

    Mock::given(method("GET"))
        .and(path("/items/zzzzz"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{\"error\":\"not found\"}"))
        .mount(&server)
        .await;
    let out = lam(&dir, &["show", "zzzzz"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("404"));
}

#[tokio::test]
async fn llm_flag_prints_guide_without_config() {
    let dir = tempfile::tempdir().unwrap();
    let out = lam(&dir, &["--llm"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("# lam"));
    assert!(text.contains("--wait"));
}

#[tokio::test]
async fn wait_exit_codes_for_expired_and_retracted() {
    let (server, dir) = setup().await;
    for (id, status, code) in [("exp01", "expired", 4), ("ret01", "retracted", 5)] {
        Mock::given(method("GET"))
            .and(path(format!("/items/{id}/wait")))
            .respond_with(ResponseTemplate::new(200).set_body_json(item(id, status, None)))
            .mount(&server)
            .await;
        assert_eq!(
            lam(&dir, &["wait", id]).status.code(),
            Some(code),
            "{status}"
        );
    }
}

#[tokio::test]
async fn wait_many_ids_uses_wait_any_endpoint() {
    let (server, dir) = setup().await;
    Mock::given(method("GET"))
        .and(path("/items/wait"))
        .and(query_param("ids", "aaa11,bbb22"))
        .respond_with(ResponseTemplate::new(200).set_body_json(item(
            "bbb22",
            "resolved",
            Some("ok"),
        )))
        .expect(1)
        .mount(&server)
        .await;
    let out = lam(&dir, &["wait", "aaa11", "bbb22"]);
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["id"], "bbb22");
}

#[tokio::test]
async fn wait_any_collects_my_open_items() {
    let (server, dir) = setup().await;
    let host = hostname::get().unwrap().to_string_lossy().into_owned();
    let mut mine = item("mine1", "open", None);
    mine["source_host"] = serde_json::json!(host);
    mine["source_project"] = serde_json::json!("lam");
    let theirs = item("other", "open", None);
    Mock::given(method("GET"))
        .and(path("/items"))
        .and(query_param("status", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(vec![mine, theirs]))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/items/mine1/wait"))
        .respond_with(ResponseTemplate::new(200).set_body_json(item(
            "mine1",
            "resolved",
            Some("yes"),
        )))
        .expect(1)
        .mount(&server)
        .await;
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lam"))
        .env("LAM_CONFIG", dir.path().join("config.toml"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["wait", "--any"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
async fn retract_posts() {
    let (server, dir) = setup().await;
    Mock::given(method("POST"))
        .and(path("/items/abc12/retract"))
        .respond_with(ResponseTemplate::new(200).set_body_json(item("abc12", "retracted", None)))
        .expect(1)
        .mount(&server)
        .await;
    assert!(lam(&dir, &["retract", "abc12"]).status.success());
}
