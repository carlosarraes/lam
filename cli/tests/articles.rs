use std::collections::VecDeque;
use std::fs;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use wiremock::matchers::{body_json, method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const ARTICLE_ID: &str = "11111111-1111-4111-8111-111111111111";
const TOKEN: &str = "super-secret-bearer";

fn article(id: &str, title: &str, read: bool, version: u64) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": title,
        "summary": "A short summary",
        "name": "test:agent",
        "source_host": "test-host",
        "source_project": "fixture",
        "created_at": "2026-09-08T12:00:00.000Z",
        "read_at": read.then_some("2026-09-08T12:01:00.000Z"),
        "version": version,
        "assets": [
            {
                "path": "index.html",
                "media_type": "text/html",
                "size": 42,
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "disposition": "inline"
            }
        ]
    })
}

async fn setup() -> (MockServer, tempfile::TempDir) {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("config.toml"),
        format!(
            "server = \"{}\"\ntoken = \"{TOKEN}\"\ntopic = \"top\"\n",
            server.uri()
        ),
    )
    .unwrap();
    fs::write(
        dir.path().join("entry.html"),
        "<!doctype html><html><body>Hello</body></html>",
    )
    .unwrap();
    (server, dir)
}

fn lam_command(dir: &tempfile::TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lam"));
    command
        .current_dir(dir.path())
        .env("LAM_CONFIG", dir.path().join("config.toml"))
        .env("LAM_NAME", "test:agent");
    command
}

fn lam(dir: &tempfile::TempDir, args: &[&str]) -> Output {
    lam_command(dir).args(args).output().unwrap()
}

#[cfg(unix)]
#[derive(Debug)]
struct ScriptedRequest {
    target: String,
    headers: String,
    body: Vec<u8>,
}

#[cfg(unix)]
enum ScriptedReply {
    Full(&'static str, String),
    Truncated(&'static str, String),
}

#[cfg(unix)]
fn spawn_scripted_server(
    replies: Vec<ScriptedReply>,
) -> (
    std::net::SocketAddr,
    Arc<Mutex<Vec<ScriptedRequest>>>,
    std::thread::JoinHandle<()>,
) {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    fn read_request(stream: &mut TcpStream) -> ScriptedRequest {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut header_bytes = Vec::new();
        let mut byte = [0_u8; 1];
        while !header_bytes.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            header_bytes.push(byte[0]);
        }
        let headers = String::from_utf8(header_bytes).unwrap();
        let target = headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .to_owned();
        let length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let mut body = vec![0; length];
        stream.read_exact(&mut body).unwrap();
        ScriptedRequest {
            target,
            headers,
            body,
        }
    }

    fn write_response(stream: &mut TcpStream, status: &str, body: &str, truncate: bool) {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        let bytes = body.as_bytes();
        let sent = if truncate {
            bytes.len().saturating_sub(1).max(1)
        } else {
            bytes.len()
        };
        stream.write_all(&bytes[..sent]).unwrap();
        stream.flush().unwrap();
    }

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let server_captured = Arc::clone(&captured);
    let server = std::thread::spawn(move || {
        for reply in replies {
            let (mut stream, _) = listener.accept().unwrap();
            server_captured
                .lock()
                .unwrap()
                .push(read_request(&mut stream));
            match reply {
                ScriptedReply::Full(status, body) => {
                    write_response(&mut stream, status, &body, false)
                }
                ScriptedReply::Truncated(status, body) => {
                    write_response(&mut stream, status, &body, true)
                }
            }
        }
    });
    (address, captured, server)
}

#[cfg(unix)]
fn setup_for_server(address: std::net::SocketAddr) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("config.toml"),
        format!("server = \"http://{address}\"\ntoken = \"{TOKEN}\"\ntopic = \"top\"\n"),
    )
    .unwrap();
    fs::write(
        dir.path().join("entry.html"),
        "<!doctype html><html><body>Hello</body></html>",
    )
    .unwrap();
    dir
}

async fn mount_success(server: &MockServer, article_value: &serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/v2/articles/uploads"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "id": ARTICLE_ID
        })))
        .mount(server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(format!(
            r"^/v2/articles/{ARTICLE_ID}/assets/[0-9]+$"
        )))
        .respond_with(ResponseTemplate::new(204))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/v2/articles/{ARTICLE_ID}/publish")))
        .respond_with(ResponseTemplate::new(200).set_body_json(article_value))
        .mount(server)
        .await;
}

#[tokio::test]
async fn publish_sends_only_explicit_assets_in_manifest_order_then_publishes() {
    let (server, dir) = setup().await;
    fs::create_dir(dir.path().join("images")).unwrap();
    fs::write(dir.path().join("images/chart.png"), b"explicit image bytes").unwrap();
    fs::write(
        dir.path().join("unrelated-secret.txt"),
        b"DO-NOT-UPLOAD-THIS-SECRET",
    )
    .unwrap();
    mount_success(&server, &article(ARTICLE_ID, "Report", false, 0)).await;

    let out = lam(
        &dir,
        &[
            "article",
            "publish",
            "--file",
            "entry.html",
            "--title",
            "Report",
            "--summary",
            "A short summary",
            "--asset",
            "images/chart.png",
            "--silent",
        ],
    );

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), ARTICLE_ID);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|request| (request.method.as_str(), request.url.path()))
            .collect::<Vec<_>>(),
        [
            ("POST", "/v2/articles/uploads"),
            ("PUT", &format!("/v2/articles/{ARTICLE_ID}/assets/0")),
            ("PUT", &format!("/v2/articles/{ARTICLE_ID}/assets/1")),
            ("POST", &format!("/v2/articles/{ARTICLE_ID}/publish")),
        ]
    );
    let manifest: serde_json::Value = requests[0].body_json().unwrap();
    assert_eq!(manifest["title"], "Report");
    assert_eq!(manifest["summary"], "A short summary");
    assert_eq!(manifest["name"], "test:agent");
    assert_eq!(manifest["silent"], true);
    assert_eq!(manifest["assets"][0]["path"], "index.html");
    assert_eq!(manifest["assets"][0]["media_type"], "text/html");
    assert_eq!(manifest["assets"][0]["disposition"], "inline");
    assert_eq!(
        manifest["assets"][0]["sha256"],
        "49371198838c87d63325fa248424869e9460e3b715a8fd9e7304e80eef5f6e46"
    );
    assert_eq!(manifest["assets"][1]["path"], "images/chart.png");
    assert_eq!(manifest["assets"][1]["media_type"], "image/png");
    assert_eq!(manifest["assets"][1]["disposition"], "inline");
    assert_eq!(
        manifest["assets"][1]["sha256"],
        "4ef065e1296944bf4cad0fec2cc1be56da4ea091932d5905809de66ed25edb4f"
    );
    assert_eq!(
        requests[1].body,
        fs::read(dir.path().join("entry.html")).unwrap()
    );
    assert_eq!(requests[2].body, b"explicit image bytes");
    for request in requests {
        assert!(!request
            .body
            .windows(25)
            .any(|bytes| bytes == b"DO-NOT-UPLOAD-THIS-SECRET"));
        assert_eq!(
            request
                .headers
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap(),
            format!("Bearer {TOKEN}")
        );
    }
}

#[tokio::test]
async fn publish_rejects_traversal_missing_files_and_unknown_types_before_http() {
    let (server, dir) = setup().await;
    fs::write(dir.path().join("secret.txt"), "secret").unwrap();

    for assets in [
        ["--asset", "../secret.txt"],
        ["--asset", "missing.png"],
        ["--asset", "secret.exe"],
        ["--asset", "https://example.test/file.png"],
        ["--asset", "image.png?cache=1"],
    ] {
        let out = lam(
            &dir,
            &[
                "article",
                "publish",
                "--file",
                "entry.html",
                "--title",
                "Report",
                "--summary",
                "Summary",
                assets[0],
                assets[1],
            ],
        );
        assert_eq!(out.status.code(), Some(1), "{assets:?}");
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn publish_rejects_duplicate_nfc_paths_before_http() {
    let (server, dir) = setup().await;
    fs::write(dir.path().join("café.txt"), "composed").unwrap();
    fs::write(dir.path().join("café.txt"), "decomposed").unwrap();
    let out = lam(
        &dir,
        &[
            "article",
            "publish",
            "--file",
            "entry.html",
            "--title",
            "Report",
            "--summary",
            "Summary",
            "--asset",
            "café.txt",
            "--asset",
            "café.txt",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("duplicate asset path"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn publish_rejects_symlinked_entry_leaf_parent_and_asset() {
    use std::os::unix::fs::symlink;

    let (server, dir) = setup().await;
    fs::create_dir(dir.path().join("real")).unwrap();
    fs::write(
        dir.path().join("real/inside.html"),
        "<!doctype html><p>x</p>",
    )
    .unwrap();
    fs::write(dir.path().join("real/image.png"), b"image").unwrap();
    symlink("real/inside.html", dir.path().join("entry-link.html")).unwrap();
    symlink("real", dir.path().join("parent-link")).unwrap();
    symlink("real/image.png", dir.path().join("asset-link.png")).unwrap();

    for (file, asset) in [
        ("entry-link.html", None),
        ("parent-link/inside.html", None),
        ("entry.html", Some("asset-link.png")),
    ] {
        let mut args = vec![
            "article",
            "publish",
            "--file",
            file,
            "--title",
            "Report",
            "--summary",
            "Summary",
        ];
        if let Some(asset) = asset {
            args.extend(["--asset", asset]);
        }
        let out = lam(&dir, &args);
        assert_eq!(
            out.status.code(),
            Some(1),
            "{file}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains("symlink"));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn publish_enforces_entry_asset_total_and_count_limits_before_http() {
    let (server, dir) = setup().await;

    let oversized_entry = dir.path().join("oversized.html");
    fs::File::create(&oversized_entry)
        .unwrap()
        .set_len(2 * 1024 * 1024 + 1)
        .unwrap();
    let out = lam(
        &dir,
        &[
            "article",
            "publish",
            "--file",
            "oversized.html",
            "--title",
            "T",
            "--summary",
            "S",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("2 MiB"));

    fs::File::create(dir.path().join("oversized.pdf"))
        .unwrap()
        .set_len(20 * 1024 * 1024 + 1)
        .unwrap();
    let out = lam(
        &dir,
        &[
            "article",
            "publish",
            "--file",
            "entry.html",
            "--title",
            "T",
            "--summary",
            "S",
            "--asset",
            "oversized.pdf",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("20 MiB"));

    let mut command = lam_command(&dir);
    command.args([
        "article",
        "publish",
        "--file",
        "entry.html",
        "--title",
        "T",
        "--summary",
        "S",
    ]);
    for n in 0..3 {
        let path = format!("part-{n}.pdf");
        fs::File::create(dir.path().join(&path))
            .unwrap()
            .set_len(17 * 1024 * 1024)
            .unwrap();
        command.args(["--asset", &path]);
    }
    let out = command.output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("50 MiB"));

    let mut owned = Vec::new();
    for n in 0..51 {
        let path = format!("attachment-{n}.txt");
        fs::write(dir.path().join(&path), "x").unwrap();
        owned.push(path);
    }
    let mut command = lam_command(&dir);
    command.args([
        "article",
        "publish",
        "--file",
        "entry.html",
        "--title",
        "T",
        "--summary",
        "S",
    ]);
    for path in &owned {
        command.args(["--asset", path]);
    }
    let out = command.output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("50 additional assets"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[derive(Clone)]
struct SequenceResponder {
    responses: Arc<Mutex<VecDeque<ResponseTemplate>>>,
}

impl SequenceResponder {
    fn new(responses: impl IntoIterator<Item = ResponseTemplate>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().collect())),
        }
    }
}

impl Respond for SequenceResponder {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let mut responses = self.responses.lock().unwrap();
        if responses.len() == 1 {
            return responses[0].clone();
        }
        responses.pop_front().unwrap()
    }
}

#[tokio::test]
async fn publish_retries_transient_stage_and_publish_with_one_attempt_identity() {
    let (server, dir) = setup().await;
    Mock::given(method("POST"))
        .and(path("/v2/articles/uploads"))
        .respond_with(SequenceResponder::new([
            ResponseTemplate::new(503),
            ResponseTemplate::new(201).set_body_json(serde_json::json!({ "id": ARTICLE_ID })),
        ]))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/v2/articles/{ARTICLE_ID}/assets/0")))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/v2/articles/{ARTICLE_ID}/publish")))
        .respond_with(SequenceResponder::new([
            ResponseTemplate::new(503),
            ResponseTemplate::new(200).set_body_json(article(ARTICLE_ID, "Retry", false, 0)),
        ]))
        .expect(2)
        .mount(&server)
        .await;

    let out = lam(
        &dir,
        &[
            "article",
            "publish",
            "--file",
            "entry.html",
            "--title",
            "Retry",
            "--summary",
            "Summary",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), ARTICLE_ID);
    let requests = server.received_requests().await.unwrap();
    let keys = requests
        .iter()
        .filter(|request| request.url.path() == "/v2/articles/uploads")
        .map(|request| {
            request
                .headers
                .get("idempotency-key")
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(keys.len(), 2);
    assert!(!keys[0].is_empty());
    assert_eq!(keys[0], keys[1]);
}

#[cfg(unix)]
#[test]
fn publish_retries_a_truncated_successful_stage_body_with_the_same_key() {
    let stage = serde_json::json!({ "id": ARTICLE_ID }).to_string();
    let (address, captured, server) = spawn_scripted_server(vec![
        ScriptedReply::Truncated("201 Created", stage.clone()),
        ScriptedReply::Full("201 Created", stage),
        ScriptedReply::Full("204 No Content", String::new()),
        ScriptedReply::Full(
            "200 OK",
            article(ARTICLE_ID, "Stage retry", false, 0).to_string(),
        ),
    ]);
    let dir = setup_for_server(address);

    let out = lam(
        &dir,
        &[
            "article",
            "publish",
            "--file",
            "entry.html",
            "--title",
            "Stage retry",
            "--summary",
            "Summary",
        ],
    );

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    server.join().unwrap();
    let requests = captured.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.target.as_str())
            .collect::<Vec<_>>(),
        [
            "/v2/articles/uploads",
            "/v2/articles/uploads",
            &format!("/v2/articles/{ARTICLE_ID}/assets/0"),
            &format!("/v2/articles/{ARTICLE_ID}/publish"),
        ]
    );
    let keys = requests[..2]
        .iter()
        .map(|request| {
            request
                .headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("idempotency-key")
                        .then(|| value.trim())
                })
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(keys[0], keys[1]);
    assert_eq!(requests[0].body, requests[1].body);
}

#[cfg(unix)]
#[test]
fn publish_retries_a_truncated_successful_publish_body() {
    let published = article(ARTICLE_ID, "Publish retry", false, 0).to_string();
    let (address, captured, server) = spawn_scripted_server(vec![
        ScriptedReply::Full(
            "201 Created",
            serde_json::json!({ "id": ARTICLE_ID }).to_string(),
        ),
        ScriptedReply::Full("204 No Content", String::new()),
        ScriptedReply::Truncated("200 OK", published.clone()),
        ScriptedReply::Full("200 OK", published),
    ]);
    let dir = setup_for_server(address);

    let out = lam(
        &dir,
        &[
            "article",
            "publish",
            "--file",
            "entry.html",
            "--title",
            "Publish retry",
            "--summary",
            "Summary",
        ],
    );

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    server.join().unwrap();
    let requests = captured.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.target.ends_with("/publish"))
            .count(),
        2
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), ARTICLE_ID);
}

#[cfg(unix)]
#[test]
fn publish_reports_unknown_outcome_after_exhausted_truncated_successes() {
    let published = article(ARTICLE_ID, "Unknown", false, 0).to_string();
    let (address, captured, server) = spawn_scripted_server(vec![
        ScriptedReply::Full(
            "201 Created",
            serde_json::json!({ "id": ARTICLE_ID }).to_string(),
        ),
        ScriptedReply::Full("204 No Content", String::new()),
        ScriptedReply::Truncated("200 OK", published.clone()),
        ScriptedReply::Truncated("200 OK", published.clone()),
        ScriptedReply::Truncated("200 OK", published),
    ]);
    let dir = setup_for_server(address);

    let out = lam(
        &dir,
        &[
            "article",
            "publish",
            "--file",
            "entry.html",
            "--title",
            "Unknown",
            "--summary",
            "Summary",
        ],
    );

    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("publication outcome is unknown"),
        "{stderr}"
    );
    assert!(stderr.contains("may have published"), "{stderr}");
    assert!(!stderr.contains("was not published"), "{stderr}");
    assert_eq!(stderr.matches(ARTICLE_ID).count(), 1, "{stderr}");
    server.join().unwrap();
    let requests = captured.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.target.ends_with("/publish"))
            .count(),
        3
    );
}

#[cfg(unix)]
#[tokio::test]
async fn publish_retries_an_interrupted_upload_with_the_same_snapshotted_bytes() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    #[derive(Debug)]
    struct Captured {
        target: String,
        body: Vec<u8>,
    }

    fn read_request(stream: &mut TcpStream, body: bool) -> Captured {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut headers = Vec::new();
        let mut byte = [0_u8; 1];
        while !headers.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            headers.push(byte[0]);
        }
        let text = String::from_utf8(headers).unwrap();
        let target = text
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .to_owned();
        let length = text
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let mut bytes = vec![0; if body { length } else { 0 }];
        stream.read_exact(&mut bytes).unwrap();
        Captured {
            target,
            body: bytes,
        }
    }

    fn respond(stream: &mut TcpStream, status: &str, body: &str) {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        stream.flush().unwrap();
    }

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let server_captured = Arc::clone(&captured);
    let server = thread::spawn(move || {
        let (mut stage, _) = listener.accept().unwrap();
        server_captured
            .lock()
            .unwrap()
            .push(read_request(&mut stage, true));
        respond(
            &mut stage,
            "201 Created",
            &serde_json::json!({ "id": ARTICLE_ID }).to_string(),
        );

        let (mut interrupted, _) = listener.accept().unwrap();
        server_captured
            .lock()
            .unwrap()
            .push(read_request(&mut interrupted, false));
        drop(interrupted);

        let (mut retried, _) = listener.accept().unwrap();
        server_captured
            .lock()
            .unwrap()
            .push(read_request(&mut retried, true));
        respond(&mut retried, "204 No Content", "");

        let (mut publish, _) = listener.accept().unwrap();
        server_captured
            .lock()
            .unwrap()
            .push(read_request(&mut publish, true));
        respond(
            &mut publish,
            "200 OK",
            &article(ARTICLE_ID, "Interrupted", false, 0).to_string(),
        );
    });

    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("config.toml"),
        format!("server = \"http://{address}\"\ntoken = \"{TOKEN}\"\ntopic = \"top\"\n"),
    )
    .unwrap();
    fs::write(
        dir.path().join("entry.html"),
        "<!doctype html><html><body>same bytes</body></html>",
    )
    .unwrap();
    let expected = fs::read(dir.path().join("entry.html")).unwrap();

    let out = lam(
        &dir,
        &[
            "article",
            "publish",
            "--file",
            "entry.html",
            "--title",
            "Interrupted",
            "--summary",
            "Summary",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    server.join().unwrap();
    let requests = captured.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.target.as_str())
            .collect::<Vec<_>>(),
        [
            "/v2/articles/uploads",
            &format!("/v2/articles/{ARTICLE_ID}/assets/0"),
            &format!("/v2/articles/{ARTICLE_ID}/assets/0"),
            &format!("/v2/articles/{ARTICLE_ID}/publish"),
        ]
    );
    assert_eq!(requests[2].body, expected);
}

#[tokio::test]
async fn failed_upload_never_publishes_and_reports_the_draft_once_without_secrets() {
    let (server, dir) = setup().await;
    Mock::given(method("POST"))
        .and(path("/v2/articles/uploads"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "id": ARTICLE_ID
        })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/v2/articles/{ARTICLE_ID}/assets/0")))
        .respond_with(ResponseTemplate::new(400).set_body_string(format!(
            "upload rejected for {ARTICLE_ID}; credential={TOKEN}; temporary=https://viewer.invalid/session/capability"
        )))
        .mount(&server)
        .await;

    let out = lam(
        &dir,
        &[
            "article",
            "publish",
            "--file",
            "entry.html",
            "--title",
            "Report",
            "--summary",
            "Summary",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.matches(ARTICLE_ID).count(), 1, "{stderr}");
    assert!(!stderr.contains(TOKEN), "{stderr}");
    assert!(!stderr.contains("viewer.invalid"), "{stderr}");
    assert!(server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|request| !request.url.path().ends_with("/publish")));
}

#[tokio::test]
async fn publish_reports_an_actionable_upgrade_error_for_an_older_server() {
    let (server, dir) = setup().await;
    Mock::given(method("POST"))
        .and(path("/v2/articles/uploads"))
        .respond_with(ResponseTemplate::new(404).set_body_string(format!(
            "missing https://viewer.invalid/session?token={TOKEN}"
        )))
        .mount(&server)
        .await;

    let out = lam(
        &dir,
        &[
            "article",
            "publish",
            "--file",
            "entry.html",
            "--title",
            "Report",
            "--summary",
            "Summary",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("server upgrade required"), "{stderr}");
    assert!(stderr.contains("POST /v2/articles/uploads"), "{stderr}");
    assert!(!stderr.contains(TOKEN), "{stderr}");
    assert!(!stderr.contains("viewer.invalid"), "{stderr}");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn article_errors_redact_http_urls_with_case_insensitive_schemes() {
    let (server, dir) = setup().await;
    let capabilities = [
        "HTTPS://viewer.example/session/uppercase-private-capability",
        "hTtP://viewer.example/session/mixed-case-private-capability",
    ];
    Mock::given(method("GET"))
        .and(path("/v2/articles"))
        .respond_with(SequenceResponder::new(capabilities.map(|capability| {
            ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": format!("temporary={capability}")
            }))
        })))
        .expect(capabilities.len() as u64)
        .mount(&server)
        .await;

    for capability in capabilities {
        let out = lam(&dir, &["article", "list"]);
        assert_eq!(out.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("[redacted URL]"), "{stderr}");
        assert!(!stderr.contains(capability), "{stderr}");
        assert!(!stderr.contains("private-capability"), "{stderr}");
    }
}

#[tokio::test]
async fn publish_rejects_a_malformed_draft_id_before_sending_asset_bytes() {
    let (server, dir) = setup().await;
    Mock::given(method("POST"))
        .and(path("/v2/articles/uploads"))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(serde_json::json!({ "id": "../items" })),
        )
        .mount(&server)
        .await;

    let out = lam(
        &dir,
        &[
            "article",
            "publish",
            "--file",
            "entry.html",
            "--title",
            "Report",
            "--summary",
            "Summary",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid article draft ID"));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/v2/articles/uploads");
}

#[tokio::test]
async fn article_id_commands_reject_noncanonical_ids_before_http() {
    let (server, dir) = setup().await;
    for args in [
        ["article", "read", "../items"],
        ["article", "unread", "%2fitems"],
        ["article", "open", "NOT-A-UUID"],
    ] {
        let out = lam(&dir, &args);
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains("invalid article ID"));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn article_list_follows_pages_and_passes_read_and_query_filters() {
    let (server, dir) = setup().await;
    let second_id = "22222222-2222-4222-8222-222222222222";
    Mock::given(method("GET"))
        .and(path("/v2/articles"))
        .and(query_param("read", "unread"))
        .and(query_param("q", "quarterly"))
        .respond_with(SequenceResponder::new([
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "items": [article(ARTICLE_ID, "Quarterly report", false, 0)],
                "next_cursor": "opaque-cursor"
            })),
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "items": [article(second_id, "Quarterly appendix", false, 0)],
                "next_cursor": null
            })),
        ]))
        .expect(2)
        .mount(&server)
        .await;

    let out = lam(
        &dir,
        &[
            "article",
            "list",
            "--read",
            "unread",
            "--query",
            "quarterly",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(ARTICLE_ID));
    assert!(stdout.contains("unread"));
    assert!(stdout.contains("Quarterly report"));
    assert!(stdout.contains(second_id));
    assert!(stdout.contains("Quarterly appendix"));
    let requests = server.received_requests().await.unwrap();
    assert!(!requests[0].url.query().unwrap().contains("cursor="));
    assert!(requests[1]
        .url
        .query()
        .unwrap()
        .contains("cursor=opaque-cursor"));
}

#[tokio::test]
async fn article_read_and_unread_send_the_current_version_and_print_canonical_json() {
    let (server, dir) = setup().await;
    let unread_id = "22222222-2222-4222-8222-222222222222";
    Mock::given(method("GET"))
        .and(path(format!("/v2/articles/{ARTICLE_ID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(article(ARTICLE_ID, "R", false, 7)))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/v2/articles/{ARTICLE_ID}/read")))
        .and(body_json(serde_json::json!({ "read": true, "version": 7 })))
        .respond_with(ResponseTemplate::new(200).set_body_json(article(ARTICLE_ID, "R", true, 8)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/articles/{unread_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(article(unread_id, "U", true, 4)))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/v2/articles/{unread_id}/read")))
        .and(body_json(
            serde_json::json!({ "read": false, "version": 4 }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(article(unread_id, "U", false, 5)))
        .mount(&server)
        .await;

    let read = lam(&dir, &["article", "read", ARTICLE_ID]);
    assert!(read.status.success());
    let value: serde_json::Value = serde_json::from_slice(&read.stdout).unwrap();
    assert_eq!(value["read_at"], "2026-09-08T12:01:00.000Z");
    assert_eq!(value["version"], 8);

    let unread = lam(&dir, &["article", "unread", unread_id]);
    assert!(unread.status.success());
    let value: serde_json::Value = serde_json::from_slice(&unread.stdout).unwrap();
    assert!(value["read_at"].is_null());
    assert_eq!(value["version"], 5);
}

#[tokio::test]
async fn stale_read_conflict_fetches_and_reports_canonical_state_without_retrying_put() {
    let (server, dir) = setup().await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/articles/{ARTICLE_ID}")))
        .respond_with(SequenceResponder::new([
            ResponseTemplate::new(200).set_body_json(article(ARTICLE_ID, "R", false, 7)),
            ResponseTemplate::new(200).set_body_json(article(ARTICLE_ID, "R", false, 9)),
        ]))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/v2/articles/{ARTICLE_ID}/read")))
        .and(body_json(serde_json::json!({ "read": true, "version": 7 })))
        .respond_with(
            ResponseTemplate::new(409)
                .set_body_json(serde_json::json!({ "error": "stale article version" })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let out = lam(&dir, &["article", "read", ARTICLE_ID]);

    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("read update conflicted"), "{stderr}");
    assert!(stderr.contains("canonical state"), "{stderr}");
    assert!(stderr.contains("\"read_at\": null"), "{stderr}");
    assert!(stderr.contains("\"version\": 9"), "{stderr}");
    let requests = server.received_requests().await.unwrap();
    let detail_path = format!("/v2/articles/{ARTICLE_ID}");
    let read_path = format!("/v2/articles/{ARTICLE_ID}/read");
    assert_eq!(
        requests
            .iter()
            .map(|request| (request.method.as_str(), request.url.path()))
            .collect::<Vec<_>>(),
        [
            ("GET", detail_path.as_str()),
            ("PUT", read_path.as_str()),
            ("GET", detail_path.as_str()),
        ]
    );
}

#[tokio::test]
async fn article_open_reports_an_older_server_without_exposing_response_secrets() {
    let (server, dir) = setup().await;
    Mock::given(method("POST"))
        .and(path(format!("/v2/articles/{ARTICLE_ID}/view-session")))
        .respond_with(ResponseTemplate::new(404).set_body_string(format!(
            "missing https://viewer.invalid/capability?token={TOKEN}"
        )))
        .mount(&server)
        .await;

    let out = lam(&dir, &["article", "open", ARTICLE_ID]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("server upgrade required"), "{stderr}");
    assert!(stderr.contains("view-session"), "{stderr}");
    assert!(!stderr.contains("viewer.invalid"), "{stderr}");
    assert!(!stderr.contains(TOKEN), "{stderr}");
}

#[cfg(unix)]
#[tokio::test]
async fn article_open_launches_valid_session_without_printing_or_marking_read() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    let (server, dir) = setup().await;
    let capability = "HTTPS://Viewer.Example:443/a/../view/articles/11111111-1111-4111-8111-111111111111#opaque-capability";
    let normalized = "https://viewer.example/view/articles/11111111-1111-4111-8111-111111111111#opaque-capability";
    Mock::given(method("POST"))
        .and(path(format!("/v2/articles/{ARTICLE_ID}/view-session")))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "url": capability,
            "expires_at": "2026-09-08T12:05:00.000Z"
        })))
        .mount(&server)
        .await;

    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let opened = dir.path().join("opened.txt");
    let script = bin.join(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    });
    fs::write(
        &script,
        format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", opened.display()),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let out = lam_command(&dir)
        .env("PATH", &bin)
        .args(["article", "open", ARTICLE_ID])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains(capability));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(capability));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("opaque-capability"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("opaque-capability"));
    let deadline = Instant::now() + Duration::from_secs(2);
    while !opened.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fs::read_to_string(opened).unwrap(), normalized);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "POST");
}

#[cfg(unix)]
#[tokio::test]
async fn article_open_rejects_ambiguous_session_urls_without_exposing_them() {
    let (server, dir) = setup().await;
    let capabilities = [
        "https://credential@viewer.example/session",
        "https://viewer.example/\ncapability",
        "https://viewer.example/\tcapability",
        "https://viewer.example\\@attacker.example/session",
        " https://viewer.example/session",
    ];
    Mock::given(method("POST"))
        .and(path(format!("/v2/articles/{ARTICLE_ID}/view-session")))
        .respond_with(SequenceResponder::new(capabilities.map(|url| {
            ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "url": url,
                "expires_at": "2026-09-08T12:05:00.000Z"
            }))
        })))
        .expect(capabilities.len() as u64)
        .mount(&server)
        .await;

    for capability in capabilities {
        let out = lam_command(&dir)
            .env("PATH", dir.path())
            .args(["article", "open", ARTICLE_ID])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("invalid article viewer URL"), "{stderr}");
        assert!(!stderr.contains(capability), "{stderr}");
    }
}

#[test]
fn article_help_exposes_the_settled_command_contract() {
    let dir = tempfile::tempdir().unwrap();
    let output = lam(&dir, &["article", "publish", "--help"]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    for flag in ["--file", "--title", "--summary", "--asset", "--silent"] {
        assert!(help.contains(flag), "missing {flag}: {help}");
    }
    let output = lam(&dir, &["article", "list", "--help"]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--read"));
    assert!(help.contains("--query"));
}
