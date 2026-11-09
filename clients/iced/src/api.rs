// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! Async HTTP client for the pressbox API (`/api/v1`). Cheap to clone, so
//! each `Task` gets its own copy. Errors are strings because iced messages
//! must be `Clone`.

use std::time::Duration;

use pressbox_api::{
    ApproveRequest, Approved, Feed, Flag, Item, ItemPage, ItemQuery, Me, PairApproved, PairRequest,
    PairStart, Updated, Version,
};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;

pub type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl Client {
    pub fn new(server: &str, token: Option<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("pressbox-iced/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            http,
            base: normalize_server(server),
            token,
        })
    }

    pub fn server(&self) -> &str {
        &self.base
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let req = self.http.request(method, format!("{}{path}", self.base));
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    async fn send(req: RequestBuilder) -> Result<Response> {
        let resp = req.send().await.map_err(describe)?;
        match resp.status() {
            s if s.is_success() => Ok(resp),
            StatusCode::UNAUTHORIZED => Err("the server rejected the token (401)".into()),
            s => Err(format!("server returned {s}")),
        }
    }

    async fn json<T: DeserializeOwned>(req: RequestBuilder) -> Result<T> {
        Self::send(req).await?.json().await.map_err(describe)
    }

    pub async fn version(self) -> Result<Version> {
        Self::json(self.request(Method::GET, "/api/v1/version")).await
    }

    pub async fn me(self) -> Result<Me> {
        Self::json(self.request(Method::GET, "/api/v1/me")).await
    }

    pub async fn feeds(self) -> Result<Vec<Feed>> {
        Self::json(self.request(Method::GET, "/api/v1/feeds")).await
    }

    pub async fn items(self, query: ItemQuery) -> Result<ItemPage> {
        Self::json(self.request(Method::GET, "/api/v1/items").query(&query)).await
    }

    pub async fn item(self, id: i64) -> Result<Item> {
        Self::json(self.request(Method::GET, &format!("/api/v1/items/{id}"))).await
    }

    pub async fn set_read(self, id: i64, value: bool) -> Result<()> {
        let req = self
            .request(Method::PUT, &format!("/api/v1/items/{id}/read"))
            .json(&Flag { value });
        Self::send(req).await.map(drop)
    }

    pub async fn set_starred(self, id: i64, value: bool) -> Result<()> {
        let req = self
            .request(Method::PUT, &format!("/api/v1/items/{id}/starred"))
            .json(&Flag { value });
        Self::send(req).await.map(drop)
    }

    pub async fn mark_feed_read(self, feed: String) -> Result<u32> {
        let path = format!("/api/v1/feeds/{feed}/read-all");
        let updated: Updated = Self::json(self.request(Method::POST, &path)).await?;
        Ok(updated.updated)
    }

    pub async fn refresh(self) -> Result<()> {
        Self::send(self.request(Method::POST, "/api/v1/refresh"))
            .await
            .map(drop)
    }

    pub async fn pair_start(self, name: String) -> Result<PairStart> {
        Self::json(
            self.request(Method::POST, "/api/v1/pair")
                .json(&PairRequest { name }),
        )
        .await
    }

    /// Polls until the pairing is approved, denied or expired.
    pub async fn pair_wait(self, poll_id: String) -> Result<PairApproved> {
        loop {
            let resp = self
                .request(Method::GET, &format!("/api/v1/pair/{poll_id}"))
                .send()
                .await
                .map_err(describe)?;
            match resp.status() {
                StatusCode::OK => return resp.json().await.map_err(describe),
                StatusCode::ACCEPTED => tokio::time::sleep(Duration::from_secs(2)).await,
                StatusCode::NOT_FOUND => return Err("the request was denied or expired".into()),
                s => return Err(format!("server returned {s}")),
            }
        }
    }

    /// Approves another device's pairing code, for `user` if given (admins).
    pub async fn approve(self, code: String, user: Option<String>) -> Result<Approved> {
        let path = format!("/api/v1/pairings/{}/approve", code.trim());
        let resp = self
            .request(Method::POST, &path)
            .json(&ApproveRequest { user })
            .send()
            .await
            .map_err(describe)?;
        match resp.status() {
            StatusCode::NOT_FOUND => Err(format!("no pending request with code {code}")),
            StatusCode::FORBIDDEN => Err("only admins can approve devices for other users".into()),
            StatusCode::BAD_REQUEST => Err("no such user".into()),
            s if s.is_success() => resp.json().await.map_err(describe),
            s => Err(format!("server returned {s}")),
        }
    }
}

/// Trims trailing slashes and adds `https://` when no scheme is given.
pub fn normalize_server(server: &str) -> String {
    let server = server.trim().trim_end_matches('/');
    if server.contains("://") {
        server.to_owned()
    } else {
        format!("https://{server}")
    }
}

fn describe(e: reqwest::Error) -> String {
    if e.is_timeout() {
        "the server did not answer in time".into()
    } else if e.is_connect() {
        "could not connect to the server".into()
    } else if e.is_decode() {
        format!("unexpected response: {e}")
    } else {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_server() {
        assert_eq!(normalize_server("rss.example/"), "https://rss.example");
        assert_eq!(
            normalize_server(" http://10.0.0.2:8470 "),
            "http://10.0.0.2:8470"
        );
    }
}
