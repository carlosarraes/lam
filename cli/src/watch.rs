use anyhow::Result;
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::time::Duration;

use crate::config::Config;
use crate::notify;

#[derive(Deserialize)]
pub struct NtfyEvent {
    pub event: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub message: String,
    #[serde(default = "default_priority")]
    pub priority: u8,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_priority() -> u8 {
    3
}

/// Tails the topic's JSON stream forever, reconnecting on drop; `on_message` sees every `message` event.
pub fn subscribe(
    cfg: &Config,
    mut on_message: impl FnMut(NtfyEvent),
    mut on_status: impl FnMut(&str),
) -> ! {
    let url = format!("{}/{}/json", cfg.ntfy_url(), cfg.topic);
    let http = reqwest::blocking::Client::builder()
        .timeout(None)
        .build()
        .expect("http client");
    loop {
        match http.get(&url).send() {
            Ok(res) => {
                on_status("live");
                for line in BufReader::new(res).lines() {
                    let Ok(line) = line else { break };
                    if let Ok(ev) = serde_json::from_str::<NtfyEvent>(&line) {
                        if ev.event == "message" {
                            on_message(ev);
                        }
                    }
                }
                on_status("stream ended, reconnecting");
            }
            Err(e) => on_status(&format!("{e}, retrying")),
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}

/// `lam watch`: mirror every push to the desktop.
pub fn run() -> Result<i32> {
    let cfg = Config::load()?;
    eprintln!("lam watch: {}/{}/json", cfg.ntfy_url(), cfg.topic);
    subscribe(
        &cfg,
        |ev| {
            eprintln!("lam watch: {}", ev.title);
            if let Err(e) = notify::desktop(&ev.title, &ev.message, ev.priority >= 5) {
                eprintln!("lam watch: notify failed: {e}");
            }
        },
        |s| eprintln!("lam watch: {s}"),
    )
}
