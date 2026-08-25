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
        &["push", "hello", "-c", "yes", "-c", "no", "-p", "critical"],
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
