use anyhow::Result;
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::time::Duration;

use crate::config::Config;
use crate::notify;

#[derive(Deserialize)]
struct NtfyEvent {
    event: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    message: String,
    #[serde(default = "default_priority")]
    priority: u8,
}

fn default_priority() -> u8 {
    3
}

/// Tails the ntfy topic's JSON stream forever, mirroring each message to the desktop.
pub fn run() -> Result<i32> {
    let cfg = Config::load()?;
    let url = format!("{}/{}/json", cfg.ntfy, cfg.topic);
    let http = reqwest::blocking::Client::builder().timeout(None).build()?;
    eprintln!("lam watch: {url}");
    loop {
        match http.get(&url).send() {
            Ok(res) => {
                for line in BufReader::new(res).lines() {
                    let Ok(line) = line else { break };
                    if let Ok(ev) = serde_json::from_str::<NtfyEvent>(&line) {
                        if ev.event == "message" {
                            eprintln!("lam watch: {}", ev.title);
                            if let Err(e) = notify::desktop(&ev.title, &ev.message, ev.priority >= 5) {
                                eprintln!("lam watch: notify failed: {e}");
                            }
                        }
                    }
                }
                eprintln!("lam watch: stream ended, reconnecting");
            }
            Err(e) => eprintln!("lam watch: {e}, retrying"),
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}
