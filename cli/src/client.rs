use anyhow::{bail, Result};
use reqwest::blocking::{Client as Http, RequestBuilder, Response};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::Config;

const PAIRING_CANCEL_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Check {
    pub label: String,
    pub done: bool,
    pub at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub title: String,
    pub body: String,
    pub source_host: String,
    pub source_project: String,
    pub priority: String,
    pub choices: Vec<String>,
    #[serde(default)]
    pub recommendation: Option<String>,
    #[serde(default)]
    pub recommended_choice: Option<String>,
    #[serde(default)]
    pub checks: Vec<Check>,
    #[serde(default)]
    pub link: String,
    pub status: String,
    pub response_choice: Option<String>,
    pub response_text: Option<String>,
    pub response_by: Option<String>,
    pub created_at: String,
    pub resolved_at: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub version: u64,
}

impl Item {
    pub fn checks_done(&self) -> usize {
        self.checks.iter().filter(|c| c.done).count()
    }
}

#[derive(Debug, Serialize)]
pub struct NewItem {
    pub name: String,
    pub title: String,
    pub body: String,
    pub source_host: String,
    pub source_project: String,
    pub priority: String,
    pub choices: Vec<String>,
    pub checks: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommendation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u64>,
}

#[derive(Debug, Default, Serialize)]
pub struct Resolution {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[allow(clippy::large_enum_variant)]
pub enum Wait {
    Closed(Item),
    Pending,
}

#[derive(Debug, Deserialize)]
pub struct PairingCreated {
    pub session: String,
    pub expires_at: String,
    pub qr: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum PairingWait {
    Pending,
    Claimed { device: DeviceSummary },
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceSummary {
    pub id: String,
    pub name: String,
    pub app_version: String,
    pub android_version: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
    pub push_registered: bool,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct RenameDevice<'a> {
    name: &'a str,
}

#[derive(Clone)]
pub struct Client {
    http: Http,
    base: String,
    token: String,
}

impl Client {
    pub fn new(cfg: &Config) -> Result<Self> {
        Ok(Self {
            http: Http::builder().timeout(Duration::from_secs(40)).build()?,
            base: cfg.server.clone(),
            token: cfg.token.clone(),
        })
    }

    fn get(&self, path: &str) -> RequestBuilder {
        self.http
            .get(format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
    }

    fn post(&self, path: &str) -> RequestBuilder {
        self.http
            .post(format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
    }

    fn patch(&self, path: &str) -> RequestBuilder {
        self.http
            .patch(format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
    }

    fn delete(&self, path: &str) -> RequestBuilder {
        self.http
            .delete(format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
    }

    fn ok(res: Response) -> Result<Response> {
        let status = res.status();
        if status.is_success() {
            return Ok(res);
        }
        let msg = res.text().unwrap_or_default();
        bail!("server returned {status}: {msg}");
    }

    pub fn push(&self, item: &NewItem) -> Result<Item> {
        Ok(Self::ok(self.post("/items").json(item).send()?)?.json()?)
    }

    pub fn list(&self, status: Option<&str>) -> Result<Vec<Item>> {
        let mut req = self.get("/items");
        if let Some(s) = status {
            req = req.query(&[("status", s)]);
        }
        Ok(Self::ok(req.send()?)?.json()?)
    }

    /// A page of the queue, newest first. `before` is an exclusive `created_at` cursor: pass the
    /// `created_at` of the last row of the previous page, not of the last row you kept. A worker
    /// that predates paging ignores both params and returns everything, which the caller sees as a
    /// page that adds nothing new.
    pub fn page(&self, limit: usize, before: Option<&str>) -> Result<Vec<Item>> {
        let mut req = self.get("/items").query(&[("limit", limit.to_string())]);
        if let Some(b) = before {
            req = req.query(&[("before", b)]);
        }
        Ok(Self::ok(req.send()?)?.json()?)
    }

    pub fn show(&self, id: &str) -> Result<Item> {
        Ok(Self::ok(self.get(&format!("/items/{id}")).send()?)?.json()?)
    }

    /// One long-poll round trip; returns Closed when the item closed or, with `since`, when its version moved past it.
    pub fn wait_once(&self, id: &str, since: Option<u64>) -> Result<Wait> {
        let mut req = self.get(&format!("/items/{id}/wait"));
        if let Some(v) = since {
            req = req.query(&[("since", v.to_string())]);
        }
        let res = Self::ok(req.send()?)?;
        if res.status() == StatusCode::NO_CONTENT {
            return Ok(Wait::Pending);
        }
        Ok(Wait::Closed(res.json()?))
    }

    /// Long-poll across several items; Closed with whichever closes first.
    pub fn wait_any_once(&self, ids: &[String], since: Option<&[u64]>) -> Result<Wait> {
        let mut req = self.get("/items/wait").query(&[("ids", ids.join(","))]);
        if let Some(v) = since {
            let joined = v
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(",");
            req = req.query(&[("since", joined)]);
        }
        let res = Self::ok(req.send()?)?;
        if res.status() == StatusCode::NO_CONTENT {
            return Ok(Wait::Pending);
        }
        Ok(Wait::Closed(res.json()?))
    }

    pub fn set_check(&self, id: &str, index: usize, done: bool) -> Result<Item> {
        Ok(Self::ok(
            self.post(&format!("/items/{id}/checks/{index}"))
                .json(&serde_json::json!({ "done": done }))
                .send()?,
        )?
        .json()?)
    }

    pub fn add_check(&self, id: &str, label: &str) -> Result<Item> {
        Ok(Self::ok(
            self.post(&format!("/items/{id}/checks"))
                .json(&serde_json::json!({ "label": label }))
                .send()?,
        )?
        .json()?)
    }

    pub fn retract(&self, id: &str) -> Result<Item> {
        Ok(Self::ok(
            self.post(&format!("/items/{id}/retract"))
                .json(&())
                .send()?,
        )?
        .json()?)
    }

    pub fn resolve(&self, id: &str, res: &Resolution) -> Result<Item> {
        Ok(Self::ok(
            self.post(&format!("/items/{id}/resolve"))
                .json(res)
                .send()?,
        )?
        .json()?)
    }

    pub fn dismiss(&self, id: &str) -> Result<Item> {
        Ok(Self::ok(
            self.post(&format!("/items/{id}/dismiss"))
                .json(&())
                .send()?,
        )?
        .json()?)
    }

    pub fn create_pairing(&self) -> Result<PairingCreated> {
        Ok(Self::ok(self.post("/pairings").send()?)?.json()?)
    }

    pub fn wait_pairing(&self, id: &str) -> Result<PairingWait> {
        Ok(Self::ok(self.get(&format!("/pairings/{id}/wait")).send()?)?.json()?)
    }

    pub fn cancel_pairing(&self, id: &str) -> Result<PairingWait> {
        Ok(Self::ok(
            self.delete(&format!("/pairings/{id}"))
                .timeout(PAIRING_CANCEL_TIMEOUT)
                .send()?,
        )?
        .json()?)
    }

    pub fn devices(&self) -> Result<Vec<DeviceSummary>> {
        Ok(Self::ok(self.get("/devices").send()?)?.json()?)
    }

    pub fn rename_device(&self, id: &str, name: &str) -> Result<DeviceSummary> {
        Ok(Self::ok(
            self.patch(&format!("/devices/{id}"))
                .json(&RenameDevice { name })
                .send()?,
        )?
        .json()?)
    }

    pub fn revoke_device(&self, id: &str) -> Result<DeviceSummary> {
        Ok(Self::ok(self.delete(&format!("/devices/{id}")).send()?)?.json()?)
    }
}
