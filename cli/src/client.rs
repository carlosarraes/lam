use anyhow::{bail, Context, Result};
use reqwest::blocking::{Client as Http, RequestBuilder, Response};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::time::Duration;

use crate::config::Config;

const PAIRING_CANCEL_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    #[default]
    Request,
    Fyi,
}

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
    pub kind: ItemKind,
    #[serde(default)]
    pub seen_at: Option<String>,
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
    pub fn is_fyi(&self) -> bool {
        self.kind == ItemKind::Fyi
    }

    pub fn is_actionable(&self) -> bool {
        !self.is_fyi() && self.status == "open"
    }

    pub fn has_recommendation_section(&self) -> bool {
        !self.is_fyi() && self.checks.is_empty()
    }

    pub fn priority_label(&self) -> &str {
        match self.priority.as_str() {
            "normal" if !self.is_fyi() => "Warning",
            "normal" => "Normal",
            "critical" => "Critical",
            "low" => "Low",
            other => other,
        }
    }

    pub fn status_label(&self) -> &str {
        if self.is_fyi() && self.seen_at.is_some() {
            "Seen"
        } else {
            &self.status
        }
    }

    pub fn checks_done(&self) -> usize {
        self.checks.iter().filter(|c| c.done).count()
    }
}

#[derive(Debug, Serialize)]
pub struct NewItem {
    pub kind: ItemKind,
    pub name: String,
    pub title: String,
    pub body: String,
    pub source_host: String,
    pub source_project: String,
    pub priority: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleAsset {
    pub path: String,
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
    pub disposition: ArticleAssetDisposition,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArticleAssetDisposition {
    Inline,
    Attachment,
}

#[derive(Debug, Serialize)]
pub struct ArticleDraft {
    pub title: String,
    pub summary: String,
    pub name: String,
    pub source_host: String,
    pub source_project: String,
    pub silent: bool,
    pub assets: Vec<ArticleAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Article {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub name: String,
    pub source_host: String,
    pub source_project: String,
    pub created_at: String,
    pub read_at: Option<String>,
    pub version: u64,
    pub assets: Vec<ArticleAsset>,
}

#[derive(Debug, Deserialize)]
pub struct ArticlePage {
    pub items: Vec<Article>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ArticleUploadCreated {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ArticleError {
    error: String,
}

#[derive(Debug, Deserialize)]
pub struct ArticleViewSession {
    pub url: String,
    pub expires_at: String,
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

    fn put(&self, path: &str) -> RequestBuilder {
        self.http
            .put(format!("{}{}", self.base, path))
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

    fn transient(status: StatusCode) -> bool {
        status == StatusCode::REQUEST_TIMEOUT
            || status == StatusCode::TOO_EARLY
            || status == StatusCode::TOO_MANY_REQUESTS
            || status.is_server_error()
    }

    fn send_article_with_retry(
        &self,
        mut request: impl FnMut() -> RequestBuilder,
    ) -> Result<Response> {
        const ATTEMPTS: usize = 3;
        for attempt in 0..ATTEMPTS {
            match request().send() {
                Ok(response) if Self::transient(response.status()) && attempt + 1 < ATTEMPTS => {}
                Ok(response) => return Ok(response),
                Err(_) if attempt + 1 < ATTEMPTS => {}
                Err(error) => return Err(error.into()),
            }
            std::thread::sleep(Duration::from_millis(25 * (attempt as u64 + 1)));
        }
        unreachable!("article retry loop returns on its final attempt")
    }

    fn article_ok(&self, response: Response) -> Result<Response> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let mut bytes = Vec::new();
        response
            .take(16 * 1024)
            .read_to_end(&mut bytes)
            .context("cannot read article error response")?;
        let body = String::from_utf8_lossy(&bytes);
        let message = serde_json::from_str::<ArticleError>(&body)
            .map(|error| error.error)
            .unwrap_or_else(|_| body.into_owned());
        let message = Self::redact_article_diagnostics(&message.replace(&self.token, "[redacted]"));
        bail!("server returned {status}: {message}")
    }

    fn redact_article_diagnostics(message: &str) -> String {
        let mut rest = message;
        let mut redacted = String::with_capacity(message.len());
        loop {
            let http = rest.find("http://");
            let https = rest.find("https://");
            let Some(start) = http.into_iter().chain(https).min() else {
                redacted.push_str(rest);
                return redacted;
            };
            redacted.push_str(&rest[..start]);
            redacted.push_str("[redacted URL]");
            let url = &rest[start..];
            let end = url
                .char_indices()
                .find_map(|(index, character)| {
                    (character.is_whitespace()
                        || matches!(character, '\"' | '\'' | '<' | '>' | ')' | ']'))
                    .then_some(index)
                })
                .unwrap_or(url.len());
            rest = &url[end..];
        }
    }

    fn validate_article_id(id: &str, source: &str) -> Result<()> {
        let valid =
            uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.hyphenated().to_string() == id);
        if !valid {
            if source.is_empty() {
                bail!("invalid article ID");
            }
            bail!("invalid article {source} ID");
        }
        Ok(())
    }

    pub fn create_article_upload(
        &self,
        draft: &ArticleDraft,
        idempotency_key: &str,
    ) -> Result<String> {
        let response = self.send_article_with_retry(|| {
            self.post("/v2/articles/uploads")
                .header("Idempotency-Key", idempotency_key)
                .json(draft)
        })?;
        if response.status() == StatusCode::NOT_FOUND {
            bail!("server upgrade required: article publishing needs POST /v2/articles/uploads");
        }
        let id = self
            .article_ok(response)?
            .json::<ArticleUploadCreated>()?
            .id;
        Self::validate_article_id(&id, "draft")?;
        Ok(id)
    }

    pub fn upload_article_asset(&self, id: &str, index: usize, bytes: &[u8]) -> Result<()> {
        Self::validate_article_id(id, "")?;
        let path = format!("/v2/articles/{id}/assets/{index}");
        let response = self.send_article_with_retry(|| self.put(&path).body(bytes.to_vec()))?;
        self.article_ok(response)?;
        Ok(())
    }

    pub fn publish_article(&self, id: &str) -> Result<Article> {
        Self::validate_article_id(id, "")?;
        let path = format!("/v2/articles/{id}/publish");
        let response = self.send_article_with_retry(|| self.post(&path))?;
        Ok(self.article_ok(response)?.json()?)
    }

    pub fn article_page(
        &self,
        read: &str,
        query: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<ArticlePage> {
        let mut request = self.get("/v2/articles").query(&[("read", read)]);
        if let Some(query) = query {
            request = request.query(&[("q", query)]);
        }
        if let Some(cursor) = cursor {
            request = request.query(&[("cursor", cursor)]);
        }
        let response = request.send()?;
        if response.status() == StatusCode::NOT_FOUND {
            bail!("server upgrade required: article listing needs GET /v2/articles");
        }
        Ok(self.article_ok(response)?.json()?)
    }

    pub fn article(&self, id: &str) -> Result<Article> {
        Self::validate_article_id(id, "")?;
        Ok(self
            .article_ok(self.get(&format!("/v2/articles/{id}")).send()?)?
            .json()?)
    }

    pub fn set_article_read(&self, id: &str, read: bool, version: u64) -> Result<Article> {
        Self::validate_article_id(id, "")?;
        Ok(self
            .article_ok(
                self.put(&format!("/v2/articles/{id}/read"))
                    .json(&serde_json::json!({ "read": read, "version": version }))
                    .send()?,
            )?
            .json()?)
    }

    pub fn create_article_view_session(&self, id: &str) -> Result<ArticleViewSession> {
        Self::validate_article_id(id, "")?;
        let response = self
            .post(&format!("/v2/articles/{id}/view-session"))
            .send()?;
        if response.status() == StatusCode::NOT_FOUND {
            bail!(
                "server upgrade required: article viewing needs POST /v2/articles/:id/view-session"
            );
        }
        if !response.status().is_success() {
            bail!(
                "article viewer session request failed with {}",
                response.status()
            );
        }
        Ok(response.json()?)
    }

    pub fn push(&self, item: &NewItem) -> Result<Item> {
        let response = self.post("/v2/items").json(item).send()?;
        if response.status() == StatusCode::NOT_FOUND {
            bail!("server upgrade required: typed creation needs POST /v2/items");
        }
        Ok(Self::ok(response)?.json()?)
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

    pub fn mark_seen(&self, id: &str, version: u64) -> Result<Item> {
        Ok(Self::ok(
            self.post(&format!("/v2/items/{id}/seen"))
                .json(&serde_json::json!({ "version": version }))
                .send()?,
        )?
        .json()?)
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
