use anyhow::{bail, Context, Result};
use std::time::{Duration, Instant};

use crate::client::{Client, Item, NewItem, Resolution, Wait};
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
    pub body: String,
    pub priority: String,
    pub choices: Vec<String>,
    pub link: Option<String>,
    pub ttl: Option<String>,
    pub wait: bool,
}

pub fn push(a: PushArgs) -> Result<i32> {
    if a.choices.len() > 3 {
        bail!("at most 3 choices");
    }
    if let Some(l) = &a.link {
        if !l.starts_with("http://") && !l.starts_with("https://") {
            bail!("--link must be an http(s) URL");
        }
    }
    let ttl = a
        .ttl
        .as_deref()
        .map(parse_duration)
        .transpose()?
        .map(|d| d.as_secs());
    let item = client()?.push(&NewItem {
        title: a.title,
        body: a.body,
        source_host: hostname::get()?.to_string_lossy().into_owned(),
        source_project: project_name(),
        priority: a.priority,
        choices: a.choices,
        link: a.link,
        ttl,
    })?;
    if a.wait {
        eprintln!("pushed {} — waiting", item.id);
        return self::wait(&[item.id], false, "2h");
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

/// Items this agent pushed: same host and project as the current invocation.
fn my_open_ids(c: &Client) -> Result<Vec<String>> {
    let host = hostname::get()?.to_string_lossy().into_owned();
    let project = project_name();
    Ok(c.list(Some("open"))?
        .into_iter()
        .filter(|i| i.source_host == host && i.source_project == project)
        .map(|i| i.id)
        .collect())
}

pub fn wait(ids: &[String], any: bool, timeout: &str) -> Result<i32> {
    let deadline = Instant::now() + parse_duration(timeout)?;
    let c = client()?;
    let ids: Vec<String> = if any { my_open_ids(&c)? } else { ids.to_vec() };
    if ids.is_empty() {
        bail!("no open items to wait on");
    }
    loop {
        let round = if ids.len() == 1 {
            c.wait_once(&ids[0])?
        } else {
            c.wait_any_once(&ids)?
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
        let src = [i.source_host.as_str(), i.source_project.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(":");
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
            i.title,
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

pub fn retract(id: &str) -> Result<i32> {
    print_json(&client()?.retract(id)?)?;
    Ok(0)
}

pub fn dismiss(id: &str) -> Result<i32> {
    print_json(&client()?.dismiss(id)?)?;
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
