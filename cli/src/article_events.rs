use crate::config::Config;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::{
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};
use tungstenite::{
    client::IntoClientRequest, client_tls_with_config, protocol::WebSocketConfig, Error, Message,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArticleEvent {
    event: String,
    article_id: String,
    version: u64,
}

fn article_event(text: &str) -> bool {
    serde_json::from_str::<ArticleEvent>(text).is_ok_and(|event| {
        matches!(
            event.event.as_str(),
            "article.published" | "article.read_changed"
        ) && event.version <= i64::MAX as u64
            && uuid::Uuid::parse_str(&event.article_id)
                .is_ok_and(|id| id.to_string() == event.article_id)
    })
}

/// Live hints have no replay. Every successful handshake refreshes the canonical list.
pub fn subscribe(cfg: &Config, mut invalidate: impl FnMut()) -> ! {
    loop {
        // Transport errors can contain the handshake headers. Do not forward their diagnostics.
        let _ = connect_once(cfg, &mut invalidate);
        std::thread::sleep(Duration::from_secs(5));
    }
}

fn connect_once(cfg: &Config, invalidate: &mut impl FnMut()) -> Result<()> {
    let mut url = reqwest::Url::parse(&format!("{}/v2/events", cfg.server.trim_end_matches('/')))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("invalid event endpoint");
    }
    let scheme = match url.scheme() {
        "https" => "wss",
        "http" => "ws",
        _ => bail!("invalid event endpoint"),
    };
    url.set_scheme(scheme)
        .map_err(|_| anyhow::anyhow!("invalid event endpoint"))?;
    let host = url.host_str().context("missing event host")?;
    let port = url.port_or_known_default().context("missing event port")?;
    let mut connected = None;
    for address in (host, port).to_socket_addrs()?.take(4) {
        if let Ok(stream) = TcpStream::connect_timeout(&address, Duration::from_secs(10)) {
            connected = Some(stream);
            break;
        }
    }
    let stream = connected.context("event connection unavailable")?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let mut request = url.as_str().into_client_request()?;
    request
        .headers_mut()
        .insert("Authorization", format!("Bearer {}", cfg.token).parse()?);
    let config = WebSocketConfig::default()
        .max_message_size(Some(4096))
        .max_frame_size(Some(4096));
    // Explicit TLS handshake on this socket, so redirects can never forward Authorization.
    let (mut socket, _) = client_tls_with_config(request, stream, Some(config), None)?;
    invalidate();
    let mut missed = false;
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                missed = false;
                if article_event(&text) {
                    invalidate();
                }
            }
            Ok(Message::Close(_)) => {
                let _ = socket.flush();
                return Ok(());
            }
            Ok(_) => {
                missed = false;
            }
            Err(Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) && !missed =>
            {
                missed = true;
                socket.send(Message::Ping(Vec::new().into()))?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        net::{TcpListener, TcpStream},
        time::{Duration, Instant},
    };
    use tungstenite::{
        accept_hdr,
        handshake::server::{Request, Response},
        Message,
    };

    fn accept(listener: &TcpListener) -> TcpStream {
        let start = Instant::now();
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    return stream;
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && start.elapsed() < Duration::from_secs(5) =>
                {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(error) => panic!("local connection missing: {error}"),
            }
        }
    }

    #[test]
    fn reconnect_and_valid_article_frames_invalidate_without_metadata_or_read_writes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let mut socket = accept_hdr(
                    accept(&listener),
                    |request: &Request, response: Response| {
                        assert_eq!(request.uri().path(), "/v2/events");
                        assert_eq!(request.headers()["authorization"], "Bearer fixture-token");
                        Ok(response)
                    },
                )
                .unwrap();
                for frame in [
                    r#"{"event":"item.changed","item_id":"one","version":1,"status":"open"}"#,
                    r#"{"event":"article.read_changed","article_id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa","version":1,"html":"secret"}"#,
                    r#"{"event":"article.read_changed","article_id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa","version":1}"#,
                ] {
                    socket.send(Message::Text(frame.into())).unwrap();
                }
                socket.close(None).unwrap();
            }
        });
        let cfg = Config {
            server: format!("http://{address}"),
            token: "fixture-token".into(),
            topic: "test".into(),
            ntfy: None,
        };
        let mut invalidations = 0;
        for _ in 0..2 {
            connect_once(&cfg, &mut || invalidations += 1).unwrap();
        }
        assert_eq!(invalidations, 4);
        server.join().unwrap();
    }
}
