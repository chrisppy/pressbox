// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! Blocking HTTP client for the pressbox API, plus client configuration.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use pressbox_api::{
    ApproveRequest, Approved, Feed, Flag, Item, ItemPage, ItemQuery, Me, PairApproved, PairRequest,
    PairStart, PendingPairing, Updated, Version,
};
use serde::Deserialize;

/// `~/.config/pressbox/client.json`, written by the Home Manager module.
/// Birdland's bar reads the same file for its feeds indicator.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClientConfig {
    pub server: Option<String>,
    pub token_file: Option<PathBuf>,
}

impl ClientConfig {
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("pressbox/client.json"))
    }

    /// Where `pressbox pair` saves the token when no `tokenFile` is set:
    /// `$XDG_STATE_HOME/pressbox/token`. Birdland's bar uses it too.
    pub fn default_token_path() -> Option<PathBuf> {
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .map(|d| d.join("pressbox/token"))
    }

    /// Reads the config file; a missing file is an empty config.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }
}

pub struct Client {
    http: reqwest::blocking::Client,
    base: String,
    token: Option<String>,
}

impl Client {
    pub fn new(server: &str, token: Option<String>) -> Result<Self> {
        Ok(Self {
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()?,
            base: server.trim_end_matches('/').to_owned(),
            token,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::blocking::RequestBuilder {
        let req = self.http.request(method, format!("{}{path}", self.base));
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    fn send(req: reqwest::blocking::RequestBuilder) -> Result<reqwest::blocking::Response> {
        Self::send_checked(req.send()?)
    }

    pub fn feeds(&self) -> Result<Vec<Feed>> {
        Ok(Self::send(self.request(reqwest::Method::GET, "/api/v1/feeds"))?.json()?)
    }

    pub fn items(&self, query: &ItemQuery) -> Result<ItemPage> {
        let mut pairs: Vec<(&str, String)> = Vec::new();
        if let Some(f) = &query.feed {
            pairs.push(("feed", f.clone()));
        }
        if let Some(t) = &query.tag {
            pairs.push(("tag", t.clone()));
        }
        if let Some(u) = query.unread {
            pairs.push(("unread", u.to_string()));
        }
        if let Some(s) = query.starred {
            pairs.push(("starred", s.to_string()));
        }
        if let Some(q) = &query.q {
            pairs.push(("q", q.clone()));
        }
        if let Some(l) = query.limit {
            pairs.push(("limit", l.to_string()));
        }
        if let Some(o) = query.offset {
            pairs.push(("offset", o.to_string()));
        }
        Ok(Self::send(
            self.request(reqwest::Method::GET, "/api/v1/items")
                .query(&pairs),
        )?
        .json()?)
    }

    pub fn item(&self, id: i64) -> Result<Item> {
        Ok(
            Self::send(self.request(reqwest::Method::GET, &format!("/api/v1/items/{id}")))?
                .json()?,
        )
    }

    pub fn set_read(&self, id: i64, value: bool) -> Result<()> {
        let path = format!("/api/v1/items/{id}/read");
        Self::send(
            self.request(reqwest::Method::PUT, &path)
                .json(&Flag { value }),
        )?;
        Ok(())
    }

    pub fn set_starred(&self, id: i64, value: bool) -> Result<()> {
        let path = format!("/api/v1/items/{id}/starred");
        Self::send(
            self.request(reqwest::Method::PUT, &path)
                .json(&Flag { value }),
        )?;
        Ok(())
    }

    pub fn mark_feed_read(&self, feed: &str) -> Result<u32> {
        let path = format!("/api/v1/feeds/{feed}/read-all");
        let updated: Updated = Self::send(self.request(reqwest::Method::POST, &path))?.json()?;
        Ok(updated.updated)
    }

    pub fn refresh(&self) -> Result<()> {
        Self::send(self.request(reqwest::Method::POST, "/api/v1/refresh"))?;
        Ok(())
    }

    pub fn pair_start(&self, name: &str) -> Result<PairStart> {
        let req = self
            .request(reqwest::Method::POST, "/api/v1/pair")
            .json(&PairRequest {
                name: name.to_owned(),
            });
        Ok(Self::send(req)?.json()?)
    }

    /// `Some` once approved, `None` while still waiting.
    pub fn pair_poll(&self, poll_id: &str) -> Result<Option<PairApproved>> {
        let resp = self
            .request(reqwest::Method::GET, &format!("/api/v1/pair/{poll_id}"))
            .send()?;
        match resp.status() {
            reqwest::StatusCode::OK => Ok(Some(resp.json()?)),
            reqwest::StatusCode::ACCEPTED => Ok(None),
            reqwest::StatusCode::NOT_FOUND => bail!("the request was denied or expired"),
            status => bail!("server returned {status}"),
        }
    }

    pub fn pairings(&self) -> Result<Vec<PendingPairing>> {
        Ok(Self::send(self.request(reqwest::Method::GET, "/api/v1/pairings"))?.json()?)
    }

    /// The server's version; needs no token.
    pub fn version(&self) -> Result<Version> {
        Ok(Self::send(self.request(reqwest::Method::GET, "/api/v1/version"))?.json()?)
    }

    pub fn me(&self) -> Result<Me> {
        Ok(Self::send(self.request(reqwest::Method::GET, "/api/v1/me"))?.json()?)
    }

    /// Approves a pairing code, for `user` if given (admins only).
    pub fn approve(&self, code: &str, user: Option<String>) -> Result<Approved> {
        let path = format!("/api/v1/pairings/{}/approve", code.trim());
        let resp = self
            .request(reqwest::Method::POST, &path)
            .json(&ApproveRequest { user })
            .send()?;
        match resp.status() {
            reqwest::StatusCode::NOT_FOUND => bail!("no pending request with code {code}"),
            reqwest::StatusCode::FORBIDDEN => {
                bail!("only admins can approve devices for other users")
            }
            reqwest::StatusCode::BAD_REQUEST => bail!("no such user"),
            _ => Ok(Self::send_checked(resp)?.json()?),
        }
    }

    fn send_checked(resp: reqwest::blocking::Response) -> Result<reqwest::blocking::Response> {
        match resp.status() {
            s if s.is_success() => Ok(resp),
            reqwest::StatusCode::UNAUTHORIZED => bail!("the server rejected the token (401)"),
            s => bail!("server returned {s}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_home_manager_config() {
        let dir = std::env::temp_dir().join(format!("pressbox-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("client.json");
        std::fs::write(
            &path,
            r#"{"server":"https://rss.example","tokenFile":"/run/secrets/pressbox"}"#,
        )
        .unwrap();
        let config = ClientConfig::load(&path).unwrap();
        assert_eq!(config.server.as_deref(), Some("https://rss.example"));
        assert_eq!(
            config.token_file.as_deref(),
            Some(std::path::Path::new("/run/secrets/pressbox"))
        );
        assert_eq!(
            ClientConfig::load(&dir.join("missing.json")).unwrap(),
            ClientConfig::default()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
