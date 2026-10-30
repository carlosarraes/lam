use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn item(id: &str, status: &str) -> Value {
    json!({
        "id": id, "kind": "request", "name": "test:agent", "title": "Review",
        "body": "", "source_host": "h", "source_project": "p", "priority": "normal",
        "choices": [], "checks": [], "version": 1, "status": status,
        "recommendation": "Ship", "recommended_choice": null,
        "response_choice": null, "response_text": null, "response_by": null,
        "created_at": "2026-10-29T12:00:00Z", "resolved_at": null
    })
}

fn config(server: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        format!("server = \"{server}\"\ntoken = \"tok\"\ntopic = \"top\"\n"),
    )
    .unwrap();
    dir
}

fn lam(dir: &tempfile::TempDir, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lam"))
        .env("LAM_CONFIG", dir.path().join("config.toml"))
        .env("LAM_NAME", "test:agent")
        .args(args)
        .output()
        .unwrap()
}

async fn setup() -> (MockServer, tempfile::TempDir) {
    let server = MockServer::start().await;
    for id in ["first", "second"] {
        Mock::given(method("GET"))
            .and(path(format!("/items/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(item(id, "open")))
            .mount(&server)
            .await;
    }
    let dir = config(&server.uri());
    (server, dir)
}

#[tokio::test]
async fn json_wait_emits_one_value_for_single_and_multiple_ids() {
    let (server, dir) = setup().await;
    for endpoint in ["/items/first/wait", "/items/wait"] {
        Mock::given(method("GET"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(204))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_json(item("first", "resolved")))
            .mount(&server)
            .await;
    }
    for (args, batch) in [
        (vec!["wait", "first", "--json"], false),
        (vec!["wait", "first", "second", "--json"], true),
    ] {
        let out = lam(&dir, &args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        if batch {
            assert_eq!(value["first"], "first");
            assert_eq!(value["items"][0]["id"], "first");
            assert_eq!(value["items"].as_array().unwrap().len(), 1);
        } else {
            assert_eq!(value["id"], "first");
        }
    }
}

#[tokio::test]
async fn human_wait_names_the_first_item() {
    let (server, dir) = setup().await;
    Mock::given(method("GET"))
        .and(path("/items/wait"))
        .and(query_param("ids", "first,second"))
        .respond_with(ResponseTemplate::new(200).set_body_json(item("second", "resolved")))
        .mount(&server)
        .await;
    let out = lam(&dir, &["wait", "first", "second"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("resolved first: second"));
}

#[tokio::test]
async fn wait_retries_transient_http_statuses_but_stops_on_hard_errors() {
    for status in [408, 429, 500, 503, 400, 401, 403, 404] {
        let (server, dir) = setup().await;
        Mock::given(method("GET"))
            .and(path("/items/first/wait"))
            .respond_with(ResponseTemplate::new(status).set_body_string("unavailable\ntry later"))
            .up_to_n_times(1)
            .expect(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/items/first/wait"))
            .respond_with(ResponseTemplate::new(200).set_body_json(item("first", "resolved")))
            .mount(&server)
            .await;
        let out = lam(&dir, &["wait", "first", "--timeout", "5s"]);
        let error = String::from_utf8_lossy(&out.stderr);
        if matches!(status, 408 | 429 | 500 | 503) {
            assert_eq!(out.status.code(), Some(0), "{status}: {error}");
            assert_eq!(
                error.lines().filter(|s| s.contains("retrying")).count(),
                1,
                "{error}"
            );
            assert_eq!(error.lines().count(), 1, "{error}");
        } else {
            assert_eq!(out.status.code(), Some(1), "{status}: {error}");
            assert!(!error.contains("retrying"), "{error}");
            assert_eq!(server.received_requests().await.unwrap().len(), 2);
        }
    }
}

#[tokio::test]
async fn wait_retries_initial_snapshot_errors() {
    let (server, dir) = setup().await;
    Mock::given(method("GET"))
        .and(path("/items/first"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/items/first/wait"))
        .respond_with(ResponseTemplate::new(200).set_body_json(item("first", "resolved")))
        .mount(&server)
        .await;
    let out = lam(&dir, &["wait", "first", "--timeout", "5s"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("retrying"));
}

#[tokio::test]
async fn wait_deadline_bounds_requests_and_retry_backoff() {
    for delayed in [false, true] {
        let (server, dir) = setup().await;
        let response = if delayed {
            ResponseTemplate::new(204).set_delay(Duration::from_secs(5))
        } else {
            ResponseTemplate::new(503)
        };
        Mock::given(method("GET"))
            .and(path("/items/first/wait"))
            .respond_with(response)
            .mount(&server)
            .await;
        let start = Instant::now();
        let out = lam(&dir, &["wait", "first", "--json", "--timeout", "1s"]);
        assert_eq!(
            out.status.code(),
            Some(3),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(out.stdout.is_empty());
    }
}

#[cfg(unix)]
#[test]
fn wait_survives_a_connection_reset_then_resolves() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::os::fd::AsRawFd;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let dir = config(&format!("http://{}", listener.local_addr().unwrap()));
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        for (expected_path, response) in [
            ("/items/first", Some(item("first", "open"))),
            ("/items/first/wait", None),
            ("/items/first/wait", Some(item("first", "resolved"))),
        ] {
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing request {expected_path}");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                assert_eq!(socket.read(&mut byte).unwrap(), 1);
                request.push(byte[0]);
            }
            assert!(String::from_utf8_lossy(&request).starts_with(&format!("GET {expected_path}")));
            if let Some(response) = response {
                let body = response.to_string();
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            } else {
                let linger = libc::linger {
                    l_onoff: 1,
                    l_linger: 0,
                };
                unsafe {
                    assert_eq!(
                        libc::setsockopt(
                            socket.as_raw_fd(),
                            libc::SOL_SOCKET,
                            libc::SO_LINGER,
                            &linger as *const _ as *const libc::c_void,
                            std::mem::size_of_val(&linger) as libc::socklen_t
                        ),
                        0
                    );
                }
            }
        }
    });
    let out = lam(&dir, &["wait", "first", "--timeout", "5s"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("retrying"));
    server.join().unwrap();
}
