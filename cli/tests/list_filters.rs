use std::process::{Command, Output};

use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn item(id: &str, name: &str, status: &str) -> Value {
    json!({
        "id": id, "kind": "request", "name": name, "title": format!("Title {id}"),
        "body": "", "source_host": "h", "source_project": "p", "priority": "normal",
        "choices": [], "checks": [], "version": 1, "status": status,
        "recommendation": "Ship", "recommended_choice": null,
        "response_choice": null, "response_text": null, "response_by": null,
        "created_at": "2026-11-18T12:00:00Z", "resolved_at": null
    })
}

async fn setup() -> (MockServer, tempfile::TempDir) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/items"))
        .and(query_param("status", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            item("aaa01", "Alpha:1", "open"),
            item("bbb02", "beta:2", "open"),
            item("ccc03", "alpha:1", "open"),
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/items"))
        .and(query_param_is_missing("status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            item("aaa01", "Alpha:1", "open"),
            item("bbb02", "beta:2", "open"),
            item("ccc03", "alpha:1", "open"),
            item("ddd04", "ALPHA:1", "resolved"),
        ])))
        .mount(&server)
        .await;
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

fn lam(dir: &tempfile::TempDir, args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_lam"))
        .env("LAM_CONFIG", dir.path().join("config.toml"))
        .arg("list")
        .args(args)
        .output()
        .unwrap();
    out
}

fn stdout(out: &Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[tokio::test]
async fn open_flag_matches_the_default_listing() {
    let (_server, dir) = setup().await;
    assert_eq!(stdout(&lam(&dir, &["--open"])), stdout(&lam(&dir, &[])));
}

#[tokio::test]
async fn open_conflicts_with_all() {
    let (_server, dir) = setup().await;
    assert!(!lam(&dir, &["--open", "--all"]).status.success());
}

#[tokio::test]
async fn ids_prints_one_id_per_line_and_nothing_else() {
    let (_server, dir) = setup().await;
    assert_eq!(stdout(&lam(&dir, &["--ids"])), "aaa01\nbbb02\nccc03\n");
}

#[tokio::test]
async fn count_prints_only_the_number() {
    let (_server, dir) = setup().await;
    assert_eq!(stdout(&lam(&dir, &["--count"])), "3\n");
    assert_eq!(stdout(&lam(&dir, &["--all", "--count"])), "4\n");
}

#[tokio::test]
async fn name_matches_case_insensitively_and_exactly() {
    let (_server, dir) = setup().await;
    let table = stdout(&lam(&dir, &["--name", "ALPHA:1"]));
    assert_eq!(table.lines().count(), 2, "{table}");
    assert!(
        table.contains("aaa01") && table.contains("ccc03"),
        "{table}"
    );
    assert_eq!(stdout(&lam(&dir, &["--name", "alpha"])), "");
}

#[tokio::test]
async fn filters_combine() {
    let (_server, dir) = setup().await;
    assert_eq!(
        stdout(&lam(&dir, &["--name", "alpha:1", "--ids"])),
        "aaa01\nccc03\n"
    );
    assert_eq!(
        stdout(&lam(&dir, &["--open", "--name", "alpha:1", "--count"])),
        "2\n"
    );
    assert_eq!(
        stdout(&lam(&dir, &["--all", "--name", "alpha:1", "--ids"])),
        "aaa01\nccc03\nddd04\n"
    );
    let json: Value =
        serde_json::from_str(&stdout(&lam(&dir, &["--name", "Beta:2", "--json"]))).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 1);
    assert_eq!(json[0]["id"], "bbb02");
}

#[tokio::test]
async fn output_modes_are_mutually_exclusive() {
    let (_server, dir) = setup().await;
    for args in [
        ["--ids", "--count"],
        ["--json", "--ids"],
        ["--json", "--count"],
    ] {
        let out = lam(&dir, &args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
}
