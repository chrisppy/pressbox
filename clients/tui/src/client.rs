// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! Blocking HTTP client for the pressbox API, plus client configuration.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use pressbox_api::{
    ApproveRequest, Approved, Feed, Flag, Item, ItemPage, ItemQuery, MarkRead, Me, PairApproved,
    PairRequest, PairStart, PendingPairing, Updated, Version,
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
        // `ItemQuery` serializes to exactly the query string the server
        // reads, unset fields left out, so new fields can't be forgotten.
        Ok(Self::send(
            self.request(reqwest::Method::GET, "/api/v1/items")
                .query(query),
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

    /// Marks a list's items read (from `request.from` down, if set).
    pub fn mark_read(&self, request: &MarkRead) -> Result<u32> {
        let resp = self
            .request(reqwest::Method::POST, "/api/v1/items/read-all")
            .json(request)
            .send()?;
        // Servers without the route answer 404, or 405 (for /items/{id}).
        if matches!(
            resp.status(),
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::METHOD_NOT_ALLOWED
        ) {
            bail!("the server is too old to mark a list read; upgrade pressboxd");
        }
        let updated: Updated = Self::send_checked(resp)?.json()?;
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
        let code = code.trim();
        let path = format!("/api/v1/pairings/{code}/approve");
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
    use crate::test_server::server;

    const PAGE: &str = r#"{"items":[],"next_cursor":null}"#;

    #[test]
    fn items_send_every_query_field() {
        let (base, requests) = server(vec![(200, PAGE)]);
        let client = Client::new(&format!("{base}/"), Some("tok".into())).unwrap();
        let query = ItemQuery {
            q: Some("rust nix".into()),
            feed: Some("a".into()),
            unread: Some(true),
            limit: Some(100),
            cursor: Some("1793700000.42".into()),
            ..ItemQuery::default()
        };
        assert!(client.items(&query).unwrap().items.is_empty());
        let head = requests.recv().unwrap();
        let line = head.lines().next().unwrap();
        assert!(line.starts_with("get /api/v1/items?"), "{line}");
        for part in [
            "q=rust+nix",
            "feed=a",
            "unread=true",
            "limit=100",
            "cursor=1793700000.42",
        ] {
            assert!(line.contains(part), "{part} missing from {line}");
        }
        assert!(!line.contains("offset") && !line.contains("tag"), "{line}");
        assert!(head.contains("authorization: bearer tok"), "{head}");
    }

    #[test]
    fn errors_are_explained() {
        let (base, _requests) = server(vec![
            (401, "{}"),
            (500, "{}"),
            (202, r#"{"status":"pending"}"#),
            (404, "{}"),
            (404, "{}"),
            (403, "{}"),
            (200, r#"{"name":"phone","user":"chris"}"#),
        ]);
        let client = Client::new(&base, None).unwrap();
        let error = |r: Result<Vec<Feed>>| format!("{:#}", r.unwrap_err());
        assert_eq!(error(client.feeds()), "the server rejected the token (401)");
        assert!(error(client.feeds()).contains("500"));
        assert_eq!(client.pair_poll("p").unwrap(), None, "still waiting");
        assert!(format!("{:#}", client.pair_poll("p").unwrap_err()).contains("denied or expired"));
        assert!(
            format!("{:#}", client.approve(" K7W-4QX ", None).unwrap_err())
                .ends_with("with code K7W-4QX")
        );
        assert!(
            format!(
                "{:#}",
                client.approve("K7W-4QX", Some("anna".into())).unwrap_err()
            )
            .contains("only admins")
        );
        let approved = client.approve("K7W-4QX", None).unwrap();
        assert_eq!(
            (approved.name.as_str(), approved.user.as_str()),
            ("phone", "chris")
        );
    }

    #[test]
    fn writes_use_the_right_routes() {
        let (base, requests) = server(vec![
            (204, ""),
            (204, ""),
            (200, r#"{"updated":3}"#),
            (202, ""),
            (200, r#"{"updated":2}"#),
            (404, "{}"),
        ]);
        let client = Client::new(&base, Some("tok".into())).unwrap();
        client.set_read(7, true).unwrap();
        client.set_starred(7, false).unwrap();
        assert_eq!(client.mark_feed_read("blog").unwrap(), 3);
        client.refresh().unwrap();
        let request = MarkRead {
            query: ItemQuery {
                feed: Some("blog".into()),
                ..ItemQuery::default()
            },
            from: Some(9),
        };
        assert_eq!(client.mark_read(&request).unwrap(), 2);
        let old = client.mark_read(&request).unwrap_err();
        assert!(format!("{old:#}").contains("too old"), "{old:#}");
        let lines: Vec<String> = requests
            .iter()
            .take(6)
            .map(|h| h.lines().next().unwrap().to_owned())
            .collect();
        assert_eq!(
            lines,
            [
                "put /api/v1/items/7/read http/1.1",
                "put /api/v1/items/7/starred http/1.1",
                "post /api/v1/feeds/blog/read-all http/1.1",
                "post /api/v1/refresh http/1.1",
                "post /api/v1/items/read-all http/1.1",
                "post /api/v1/items/read-all http/1.1",
            ]
        );
    }

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
