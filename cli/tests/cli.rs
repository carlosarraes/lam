use std::process::{Command, Stdio};

use qrcode::{render::unicode, QrCode};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn item(id: &str, status: &str, choice: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "id": id, "title": "t", "body": "", "source_host": "h", "source_project": "p",
        "name": "test:agent", "priority": "normal", "choices": [], "checks": [], "version": 0, "status": status,
        "response_choice": choice, "response_text": null, "response_by": choice.map(|_| "phone"),
        "created_at": "2026-08-25T00:00:00Z", "resolved_at": null
    })
}

async fn setup() -> (MockServer, tempfile::TempDir) {
    let server = MockServer::start().await;
    // `wait` snapshots each item's version via GET /items/:id before polling.
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(r"^/items/[a-z0-9]{5}$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(item("any", "open", None)))
        .with_priority(255)
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

fn lam(dir: &tempfile::TempDir, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lam"))
        .env("LAM_CONFIG", dir.path().join("config.toml"))
        .env("LAM_NAME", "test:agent")
        .args(args)
        .output()
        .unwrap()
}

fn pairing_created(server: &MockServer, session: &str) -> serde_json::Value {
    serde_json::json!({
        "session": session,
        "created_at": "2026-09-04T12:00:00.000Z",
        "expires_at": "2026-09-04T12:05:00.000Z",
        "qr": format!(
            "{{\"v\":1,\"server\":\"{}\",\"session\":\"{}\",\"secret\":\"{}\"}}",
            server.uri(),
            session,
            "A".repeat(43)
        )
    })
}

fn device(id: &str, name: &str, revoked_at: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "app_version": "1.2.3",
        "android_version": "16",
        "created_at": "2026-09-04T10:00:00.000Z",
        "last_seen_at": "2026-09-04T11:30:00.000Z",
        "push_registered": true,
        "revoked_at": revoked_at,
    })
}

#[tokio::test]
async fn push_accepts_an_exact_recommended_choice_and_sends_it() {
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
            "--recommendation",
            "Ship it: the release checks are green.",
            "--recommended-choice",
            "yes",
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
    assert_eq!(
        body["recommendation"],
        "Ship it: the release checks are green."
    );
    assert_eq!(body["recommended_choice"], "yes");
    assert_eq!(body["name"], "test:agent");
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
async fn push_requires_a_recommendation_before_loading_config() {
    let dir = tempfile::tempdir().unwrap();
    let out = lam(&dir, &["push", "approve the release"]);

    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr)
        .contains("--recommendation is required for every non-checklist request"));
}

#[tokio::test]
async fn push_with_choices_requires_recommendation_flags() {
    let dir = tempfile::tempdir().unwrap();

    let without_either = lam(&dir, &["push", "approve the release", "--choice", "ship"]);
    assert_eq!(without_either.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&without_either.stderr)
        .contains("--recommendation is required for every non-checklist request"));

    let without_choice = lam(
        &dir,
        &[
            "push",
            "approve the release",
            "--choice",
            "ship",
            "--recommendation",
            "Ship after the smoke test.",
        ],
    );
    assert_eq!(without_choice.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&without_choice.stderr)
        .contains("--recommended-choice is required when --choice is used"));
}

#[tokio::test]
async fn push_rejects_a_recommended_choice_outside_the_choices_before_loading_config() {
    let dir = tempfile::tempdir().unwrap();
    let out = lam(
        &dir,
        &[
            "push",
            "approve the release",
            "--choice",
            "ship",
            "--recommendation",
            "Ship after the smoke test.",
            "--recommended-choice",
            "hold",
        ],
    );

    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr)
        .contains("--recommended-choice must exactly match one --choice"));
}

#[tokio::test]
async fn push_accepts_a_checklist_without_recommendation_fields() {
    let (server, dir) = setup().await;
    Mock::given(method("POST"))
        .and(path("/items"))
        .respond_with(ResponseTemplate::new(201).set_body_json(item("abc12", "open", None)))
        .expect(1)
        .mount(&server)
        .await;

    let out = lam(&dir, &["push", "release checklist", "--check", "publish"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body: serde_json::Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(body["checks"], serde_json::json!(["publish"]));
    assert!(body.get("recommendation").is_none());
    assert!(body.get("recommended_choice").is_none());
}

#[tokio::test]
async fn push_rejects_recommendation_flags_on_checklists_and_choice_flag_without_choices() {
    let dir = tempfile::tempdir().unwrap();
    for (args, expected) in [
        (
            vec![
                "push",
                "checklist",
                "--check",
                "publish",
                "--recommendation",
                "Do it.",
            ],
            "--recommendation is not allowed with --check",
        ),
        (
            vec![
                "push",
                "checklist",
                "--check",
                "publish",
                "--recommended-choice",
                "publish",
            ],
            "--recommended-choice is not allowed with --check",
        ),
        (
            vec![
                "push",
                "decision",
                "--recommendation",
                "Do it.",
                "--recommended-choice",
                "yes",
            ],
            "--recommended-choice is only valid when --choice is used",
        ),
    ] {
        let out = lam(&dir, &args);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[tokio::test]
async fn push_rejects_oversized_fields_locally_with_readable_errors() {
    let dir = tempfile::tempdir().unwrap();
    let cases = [
        (
            vec![
                "push".to_string(),
                "x".repeat(201),
                "--recommendation".to_string(),
                "because".to_string(),
            ],
            "--title must be at most 200 characters",
        ),
        (
            vec![
                "push".to_string(),
                "body".to_string(),
                "--body".to_string(),
                "é".repeat(32_769),
                "--recommendation".to_string(),
                "because".to_string(),
            ],
            "--body must be at most 65536 UTF-8 bytes",
        ),
        (
            vec![
                "push".to_string(),
                "recommendation".to_string(),
                "--recommendation".to_string(),
                "😀".repeat(2_001),
            ],
            "--recommendation must be at most 2000 characters",
        ),
        (
            vec![
                "push".to_string(),
                "choice".to_string(),
                "--choice".to_string(),
                "x".repeat(201),
                "--recommendation".to_string(),
                "because".to_string(),
                "--recommended-choice".to_string(),
                "x".repeat(201),
            ],
            "--choice must be at most 200 characters",
        ),
        (
            vec![
                "push".to_string(),
                "check".to_string(),
                "--check".to_string(),
                "x".repeat(201),
            ],
            "--check must be at most 200 characters",
        ),
        (
            vec![
                "push".to_string(),
                "choices".to_string(),
                "--choice".to_string(),
                "a".to_string(),
                "--choice".to_string(),
                "b".to_string(),
                "--choice".to_string(),
                "c".to_string(),
                "--choice".to_string(),
                "d".to_string(),
                "--recommendation".to_string(),
                "because".to_string(),
                "--recommended-choice".to_string(),
                "a".to_string(),
            ],
            "at most 3 choices",
        ),
        (
            std::iter::once("push".to_string())
                .chain(std::iter::once("checks".to_string()))
                .chain((0..51).flat_map(|n| ["--check".to_string(), format!("check {n}")]))
                .collect(),
            "at most 50 checks",
        ),
    ];

    for (args, expected) in cases {
        let out = Command::new(env!("CARGO_BIN_EXE_lam"))
            .env("LAM_CONFIG", dir.path().join("config.toml"))
            .env("LAM_NAME", "test:agent")
            .args(args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{expected}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
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
    let mine = item("mine1", "open", None);
    let mut theirs = item("other", "open", None);
    theirs["name"] = serde_json::json!("other:agent");
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
    let out = lam(&dir, &["wait", "--any"]);
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

#[tokio::test]
async fn push_without_any_name_source_fails_with_guidance() {
    let (_server, dir) = setup().await;
    let out = Command::new(env!("CARGO_BIN_EXE_lam"))
        .env("LAM_CONFIG", dir.path().join("config.toml"))
        .env_remove("LAM_NAME")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("ZELLIJ_SESSION_NAME")
        .env_remove("STY")
        .args(["push", "who am i", "--recommendation", "Do it."])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--name"), "{err}");
    assert!(err.contains("LAM_NAME"), "{err}");
}

#[tokio::test]
async fn explicit_name_flag_wins_over_env() {
    let (server, dir) = setup().await;
    Mock::given(method("POST"))
        .and(path("/items"))
        .respond_with(ResponseTemplate::new(201).set_body_json(item("abc12", "open", None)))
        .expect(1)
        .mount(&server)
        .await;
    assert!(lam(
        &dir,
        &[
            "push",
            "x",
            "--name",
            "sweep:2",
            "--recommendation",
            "Do it."
        ],
    )
    .status
    .success());
    let req = &server.received_requests().await.unwrap()[0];
    let body: serde_json::Value = req.body_json().unwrap();
    assert_eq!(body["name"], "sweep:2");
}

#[tokio::test]
async fn pair_renders_the_exact_payload_as_unicode_without_printing_the_secret() {
    let (server, dir) = setup().await;
    let created = pairing_created(&server, "pair-1");
    let qr = created["qr"].as_str().unwrap().to_string();
    let expected_rendering = QrCode::new(qr.as_bytes())
        .unwrap()
        .render::<unicode::Dense1x2>()
        .quiet_zone(true)
        .build();
    let secret = "A".repeat(43);
    Mock::given(method("POST"))
        .and(path("/pairings"))
        .and(header("authorization", "Bearer tok"))
        .respond_with(ResponseTemplate::new(201).set_body_json(created))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pairings/pair-1/wait"))
        .and(header("authorization", "Bearer tok"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "claimed",
            "device": device("phone-1", "Carlos's phone", None)
        })))
        .expect(1)
        .mount(&server)
        .await;

    let out = lam(&dir, &["pair"]);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains(&expected_rendering),
        "fixture QR rendering missing:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("Server: {}", server.uri())),
        "{stdout}"
    );
    assert!(
        stdout.contains("Expires: 2026-09-04T12:05:00.000Z (five minutes)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Paired Carlos's phone (phone-1)"),
        "{stdout}"
    );
    for (stream_name, stream) in [("stdout", stdout.as_ref()), ("stderr", stderr.as_ref())] {
        assert!(
            !stream.contains(&secret),
            "secret leaked on {stream_name}:\n{stream}"
        );
        assert!(
            !stream.contains(&qr),
            "serialized payload leaked on {stream_name}:\n{stream}"
        );
    }
}

#[tokio::test]
async fn pair_repolls_pending_and_reports_expired_or_cancelled() {
    for (session, final_status, expected) in [
        ("pair-expired", "expired", "Pairing expired."),
        ("pair-cancelled", "cancelled", "Pairing cancelled."),
    ] {
        let (server, dir) = setup().await;
        Mock::given(method("POST"))
            .and(path("/pairings"))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(pairing_created(&server, session)),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/pairings/{session}/wait")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "status": "pending" })),
            )
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/pairings/{session}/wait")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "status": final_status })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let out = lam(&dir, &["pair"]);

        assert_eq!(out.status.code(), Some(1), "{final_status}");
        assert!(String::from_utf8_lossy(&out.stdout).contains(expected));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn pair_ctrl_c_cancels_the_session_best_effort() {
    let (server, dir) = setup().await;
    Mock::given(method("POST"))
        .and(path("/pairings"))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(pairing_created(&server, "pair-signal")),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pairings/pair-signal/wait"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_secs(2))
                .set_body_json(serde_json::json!({ "status": "pending" })),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/pairings/pair-signal"))
        .and(header("authorization", "Bearer tok"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_secs(2))
                .set_body_json(serde_json::json!({ "status": "cancelled" })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let child = Command::new(env!("CARGO_BIN_EXE_lam"))
        .env("LAM_CONFIG", dir.path().join("config.toml"))
        .args(["pair"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(250));
    let interrupted_at = std::time::Instant::now();
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }
    let out = child.wait_with_output().unwrap();

    assert_eq!(out.status.code(), Some(130));
    assert!(
        interrupted_at.elapsed() < std::time::Duration::from_secs(1),
        "Ctrl-C waited for the active long poll"
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Pairing cancelled."));
}

#[tokio::test]
async fn devices_prints_every_safe_administration_field() {
    let (server, dir) = setup().await;
    Mock::given(method("GET"))
        .and(path("/devices"))
        .and(header("authorization", "Bearer tok"))
        .respond_with(ResponseTemplate::new(200).set_body_json(vec![device(
            "phone-1",
            "Carlos's phone",
            None,
        )]))
        .expect(1)
        .mount(&server)
        .await;

    let out = lam(&dir, &["devices"]);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for expected in [
        "ID: phone-1",
        "Name: Carlos's phone",
        "Android: 16",
        "App: 1.2.3",
        "Created: 2026-09-04T10:00:00.000Z",
        "Last contact: 2026-09-04T11:30:00.000Z",
        "Push registered: yes",
        "Revoked: no",
    ] {
        assert!(stdout.contains(expected), "missing {expected:?}:\n{stdout}");
    }
}

#[tokio::test]
async fn device_rename_patches_the_name_and_prints_the_returned_summary() {
    let (server, dir) = setup().await;
    Mock::given(method("PATCH"))
        .and(path("/devices/phone-1"))
        .and(header("authorization", "Bearer tok"))
        .and(body_json(serde_json::json!({ "name": "Travel phone" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(device(
            "phone-1",
            "Travel phone",
            None,
        )))
        .expect(1)
        .mount(&server)
        .await;

    let out = lam(&dir, &["device", "rename", "phone-1", "Travel phone"]);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ID: phone-1"), "{stdout}");
    assert!(stdout.contains("Name: Travel phone"), "{stdout}");
}

#[tokio::test]
async fn device_revoke_deletes_and_confirms_the_returned_revocation() {
    let (server, dir) = setup().await;
    Mock::given(method("DELETE"))
        .and(path("/devices/phone-1"))
        .and(header("authorization", "Bearer tok"))
        .respond_with(ResponseTemplate::new(200).set_body_json(device(
            "phone-1",
            "Travel phone",
            Some("2026-09-04T12:30:00.000Z"),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let out = lam(&dir, &["device", "revoke", "phone-1"]);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ID: phone-1"), "{stdout}");
    assert!(
        stdout.contains("Revoked: 2026-09-04T12:30:00.000Z"),
        "{stdout}"
    );
}
