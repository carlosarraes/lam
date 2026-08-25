use anyhow::{bail, Result};
use reqwest::blocking::{Client as Http, RequestBuilder, Response};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub body: String,
    pub source_host: String,
    pub source_project: String,
    pub priority: String,
    pub choices: Vec<String>,
    pub status: String,
    pub response_choice: Option<String>,
    pub response_text: Option<String>,
    pub response_by: Option<String>,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct NewItem {
    pub title: String,
    pub body: String,
    pub source_host: String,
    pub source_project: String,
    pub priority: String,
    pub choices: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct Resolution {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

pub enum Wait {
    Closed(Item),
    Pending,
}

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
        self.http.get(format!("{}{}", self.base, path)).bearer_auth(&self.token)
    }

    fn post(&self, path: &str) -> RequestBuilder {
        self.http.post(format!("{}{}", self.base, path)).bearer_auth(&self.token)
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

    pub fn show(&self, id: &str) -> Result<Item> {
        Ok(Self::ok(self.get(&format!("/items/{id}")).send()?)?.json()?)
    }

    /// One long-poll round trip; the server holds ~25s before answering Pending.
    pub fn wait_once(&self, id: &str) -> Result<Wait> {
        let res = Self::ok(self.get(&format!("/items/{id}/wait")).send()?)?;
        if res.status() == StatusCode::NO_CONTENT {
            return Ok(Wait::Pending);
        }
        Ok(Wait::Closed(res.json()?))
    }

    pub fn resolve(&self, id: &str, res: &Resolution) -> Result<Item> {
        Ok(Self::ok(self.post(&format!("/items/{id}/resolve")).json(res).send()?)?.json()?)
    }

    pub fn dismiss(&self, id: &str) -> Result<Item> {
        Ok(Self::ok(self.post(&format!("/items/{id}/dismiss")).json(&()).send()?)?.json()?)
    }
}
