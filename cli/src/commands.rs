use anyhow::{bail, Context, Result};
use qrcode::{render::unicode, QrCode};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

use crate::client::{Client, DeviceSummary, Item, NewItem, PairingWait, Resolution, Wait};
use crate::config::Config;

pub const EXIT_RESOLVED: i32 = 0;
pub const EXIT_DISMISSED: i32 = 2;
pub const EXIT_TIMEOUT: i32 = 3;
pub const EXIT_EXPIRED: i32 = 4;
pub const EXIT_RETRACTED: i32 = 5;

fn client() -> Result<Client> {
    Client::new(&Config::load()?)
}

fn print_json(v: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

pub fn init(server: String, token: String, topic: String) -> Result<i32> {
    let path = Config {
        server,
        token,
        topic,
        ntfy: None,
    }
    .save()?;
    eprintln!("wrote {}", path.display());
    Ok(0)
}

pub struct PushArgs {
    pub title: String,
    pub name: Option<String>,
    pub body: String,
    pub priority: String,
    pub choices: Vec<String>,
    pub recommendation: Option<String>,
    pub recommended_choice: Option<String>,
    pub checks: Vec<String>,
    pub link: Option<String>,
    pub ttl: Option<String>,
    pub wait: bool,
}

const MAX_TITLE_CHARACTERS: usize = 200;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_RECOMMENDATION_CHARACTERS: usize = 2_000;
const MAX_REPLY_BYTES: usize = 8 * 1024;
const MAX_CHOICE_CHARACTERS: usize = 200;
const MAX_CHECK_CHARACTERS: usize = 200;
const MAX_CHOICES: usize = 3;
const MAX_CHECKS: usize = 50;

fn validate_max_characters(flag: &str, value: &str, max: usize) -> Result<()> {
    if value.chars().count() > max {
        bail!("{flag} must be at most {max} characters");
    }
    Ok(())
}

fn validate_push(a: &PushArgs) -> Result<()> {
    if a.choices.len() > MAX_CHOICES {
        bail!("at most {MAX_CHOICES} choices");
    }
    if a.checks.len() > MAX_CHECKS {
        bail!("at most {MAX_CHECKS} checks");
    }
    if !a.choices.is_empty() && !a.checks.is_empty() {
        bail!("--choice and --check are mutually exclusive");
    }
    if !a.checks.is_empty() {
        if a.recommendation.is_some() {
            bail!("--recommendation is not allowed with --check");
        }
        if a.recommended_choice.is_some() {
            bail!("--recommended-choice is not allowed with --check");
        }
    } else {
        if a.recommendation
            .as_deref()
            .is_none_or(|recommendation| recommendation.trim().is_empty())
        {
            bail!("--recommendation is required for every non-checklist request");
        }
        if !a.choices.is_empty() {
            let recommended = a
                .recommended_choice
                .as_deref()
                .context("--recommended-choice is required when --choice is used")?;
            if !a.choices.iter().any(|choice| choice == recommended) {
                bail!("--recommended-choice must exactly match one --choice");
            }
        } else if a.recommended_choice.is_some() {
            bail!("--recommended-choice is only valid when --choice is used");
        }
    }

    validate_max_characters("--title", &a.title, MAX_TITLE_CHARACTERS)?;
    if a.body.len() > MAX_BODY_BYTES {
        bail!("--body must be at most {MAX_BODY_BYTES} UTF-8 bytes");
    }
    if let Some(recommendation) = &a.recommendation {
        validate_max_characters(
            "--recommendation",
            recommendation,
            MAX_RECOMMENDATION_CHARACTERS,
        )?;
    }
    for choice in &a.choices {
        validate_max_characters("--choice", choice, MAX_CHOICE_CHARACTERS)?;
    }
    for check in &a.checks {
        validate_max_characters("--check", check, MAX_CHECK_CHARACTERS)?;
    }
    if let Some(l) = &a.link {
        if !l.starts_with("http://") && !l.starts_with("https://") {
            bail!("--link must be an http(s) URL");
        }
    }
    Ok(())
}

pub fn push(a: PushArgs) -> Result<i32> {
    validate_push(&a)?;
    let name = crate::name::resolve(a.name)?;
    let ttl = a
        .ttl
        .as_deref()
        .map(parse_duration)
        .transpose()?
        .map(|d| d.as_secs());
    let item = client()?.push(&NewItem {
        name,
        title: a.title,
        body: a.body,
        source_host: hostname::get()?.to_string_lossy().into_owned(),
        source_project: project_name(),
        priority: a.priority,
        choices: a.choices,
        checks: a.checks,
        recommendation: a.recommendation,
        recommended_choice: a.recommended_choice,
        link: a.link,
        ttl,
    })?;
    if a.wait {
        eprintln!("pushed {} — waiting", item.id);
        return self::wait(&[item.id], false, None, "2h");
    }
    println!("{}", item.id);
    Ok(0)
}

/// Git repo dir name if inside one, else the cwd name.
fn project_name() -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    let root = cwd
        .ancestors()
        .find(|p| p.join(".git").exists())
        .map(|p| p.to_path_buf())
        .unwrap_or(cwd);
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn parse_duration(s: &str) -> Result<Duration> {
    let (num, unit) = s.split_at(s.trim_end_matches(|c: char| c.is_ascii_alphabetic()).len());
    let n: u64 = num.parse().with_context(|| format!("bad duration {s:?}"))?;
    let mult = match unit {
        "s" | "" => 1,
        "m" => 60,
        "h" => 3600,
        _ => bail!("bad duration unit {unit:?} (use s, m, h)"),
    };
    Ok(Duration::from_secs(n * mult))
}

fn exit_for(item: &Item) -> i32 {
    match item.status.as_str() {
        "dismissed" => EXIT_DISMISSED,
        "expired" => EXIT_EXPIRED,
        "retracted" => EXIT_RETRACTED,
        _ => EXIT_RESOLVED,
    }
}

/// Items this agent pushed, by name.
fn my_open_ids(c: &Client, name: &str) -> Result<Vec<String>> {
    Ok(c.list(Some("open"))?
        .into_iter()
        .filter(|i| i.name == name)
        .map(|i| i.id)
        .collect())
}

pub fn wait(ids: &[String], any: bool, name: Option<String>, timeout: &str) -> Result<i32> {
    let deadline = Instant::now() + parse_duration(timeout)?;
    let c = client()?;
    let ids: Vec<String> = if any {
        my_open_ids(&c, &crate::name::resolve(name)?)?
    } else {
        ids.to_vec()
    };
    if ids.is_empty() {
        bail!("no open items to wait on");
    }
    // Snapshot versions first so a check ticked from now on counts as a change.
    let since: Vec<u64> = ids
        .iter()
        .map(|id| c.show(id).map(|i| i.version))
        .collect::<Result<_>>()?;
    loop {
        let round = if ids.len() == 1 {
            c.wait_once(&ids[0], Some(since[0]))?
        } else {
            c.wait_any_once(&ids, Some(&since))?
        };
        match round {
            Wait::Closed(item) => {
                print_json(&item)?;
                return Ok(exit_for(&item));
            }
            Wait::Pending if Instant::now() >= deadline => {
                eprintln!("lam: timed out waiting for {}", ids.join(","));
                return Ok(EXIT_TIMEOUT);
            }
            Wait::Pending => {}
        }
    }
}

pub fn list(all: bool, json: bool) -> Result<i32> {
    let items = client()?.list(if all { None } else { Some("open") })?;
    if json {
        print_json(&items)?;
        return Ok(0);
    }
    for i in &items {
        let src = [i.name.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(":");
        let progress = if i.checks.is_empty() {
            String::new()
        } else {
            format!(" [{}/{}]", i.checks_done(), i.checks.len())
        };
        let title = format!("{}{progress}", i.title);
        let answer = i
            .response_choice
            .as_deref()
            .or(i.response_text.as_deref())
            .unwrap_or("");
        println!(
            "{:<6} {:<9} {:<8} {:<24} {}{}",
            i.id,
            i.status,
            i.priority,
            src,
            title,
            if answer.is_empty() {
                String::new()
            } else {
                format!(" → {answer}")
            }
        );
    }
    Ok(0)
}

pub fn show(id: &str) -> Result<i32> {
    print_json(&client()?.show(id)?)?;
    Ok(0)
}

pub fn done(id: &str, choice: Option<String>, message: Option<String>) -> Result<i32> {
    if message
        .as_deref()
        .is_some_and(|text| text.len() > MAX_REPLY_BYTES)
    {
        bail!("--message must be at most {MAX_REPLY_BYTES} UTF-8 bytes");
    }
    let item = client()?.resolve(
        id,
        &Resolution {
            choice,
            text: message,
        },
    )?;
    print_json(&item)?;
    Ok(0)
}

pub fn check_add(id: &str, label: &str) -> Result<i32> {
    validate_max_characters("label", label, MAX_CHECK_CHARACTERS)?;
    print_json(&client()?.add_check(id, label)?)?;
    Ok(0)
}

pub fn check_set(id: &str, n: usize, done: bool) -> Result<i32> {
    if n == 0 {
        bail!("checks are numbered from 1");
    }
    print_json(&client()?.set_check(id, n - 1, done)?)?;
    Ok(0)
}

pub fn retract(id: &str) -> Result<i32> {
    print_json(&client()?.retract(id)?)?;
    Ok(0)
}

pub fn dismiss(id: &str) -> Result<i32> {
    print_json(&client()?.dismiss(id)?)?;
    Ok(0)
}

fn print_device(device: &DeviceSummary) {
    println!("ID: {}", device.id);
    println!("Name: {}", device.name);
    println!("Android: {}", device.android_version);
    println!("App: {}", device.app_version);
    println!("Created: {}", device.created_at);
    println!(
        "Last contact: {}",
        device.last_seen_at.as_deref().unwrap_or("never")
    );
    println!(
        "Push registered: {}",
        if device.push_registered { "yes" } else { "no" }
    );
    println!("Revoked: {}", device.revoked_at.as_deref().unwrap_or("no"));
}

fn report_pairing_cancellation(status: Result<PairingWait>) {
    match status {
        Ok(PairingWait::Claimed { device }) => {
            println!("Paired {} ({})", device.name, device.id);
        }
        Ok(PairingWait::Expired) => println!("Pairing expired."),
        Ok(PairingWait::Cancelled) => println!("Pairing cancelled."),
        Ok(PairingWait::Pending) | Err(_) => {
            println!(
                "Pairing wait stopped. Cancellation is unconfirmed. Run `lam devices` to check for a paired device."
            );
        }
    }
}

pub fn pair() -> Result<i32> {
    let cfg = Config::load()?;
    let c = Client::new(&cfg)?;
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&interrupted);
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst))?;

    let pairing = c.create_pairing()?;
    if interrupted.load(Ordering::SeqCst) {
        report_pairing_cancellation(c.cancel_pairing(&pairing.session));
        return Ok(130);
    }
    let qr = QrCode::new(pairing.qr.as_bytes()).context("server returned an invalid QR payload")?;
    let rendered = qr.render::<unicode::Dense1x2>().quiet_zone(true).build();

    println!("Scan this code with the LAM Android app:");
    println!("{rendered}");
    println!("Server: {}", cfg.server);
    println!("Expires: {} (five minutes)", pairing.expires_at);

    loop {
        let wait_client = c.clone();
        let wait_session = pairing.session.clone();
        let (send, receive) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = send.send(wait_client.wait_pairing(&wait_session));
        });
        let status = loop {
            if interrupted.load(Ordering::SeqCst) {
                report_pairing_cancellation(c.cancel_pairing(&pairing.session));
                return Ok(130);
            }
            match receive.recv_timeout(Duration::from_millis(50)) {
                Ok(status) => break status?,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("pairing wait stopped unexpectedly")
                }
            }
        };
        match status {
            PairingWait::Pending => {}
            PairingWait::Claimed { device } => {
                println!("Paired {} ({})", device.name, device.id);
                return Ok(0);
            }
            PairingWait::Expired => {
                println!("Pairing expired.");
                return Ok(1);
            }
            PairingWait::Cancelled => {
                println!("Pairing cancelled.");
                return Ok(1);
            }
        }
    }
}

pub fn devices() -> Result<i32> {
    let devices = client()?.devices()?;
    for (index, device) in devices.iter().enumerate() {
        if index > 0 {
            println!();
        }
        print_device(device);
    }
    Ok(0)
}

pub fn device_rename(id: &str, name: &str) -> Result<i32> {
    print_device(&client()?.rename_device(id, name)?);
    Ok(0)
}

pub fn device_revoke(id: &str) -> Result<i32> {
    print_device(&client()?.revoke_device(id)?);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration("15").unwrap(), Duration::from_secs(15));
        assert!(parse_duration("2d").is_err());
        assert!(parse_duration("abc").is_err());
    }
}
