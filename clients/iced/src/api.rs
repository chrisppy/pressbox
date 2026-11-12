// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! Async HTTP client for the pressbox API (`/api/v1`). Cheap to clone, so
//! each `Task` gets its own copy. Errors are strings because iced messages
//! must be `Clone`.

use std::time::Duration;

use pressbox_api::{
    ApproveRequest, Approved, Chapters, Feed, Flag, Item, ItemPage, ItemQuery, Me, PairApproved,
    PairRequest, PairStart, Position, Queue, QueueOrder, Transcript, Updated, Version,
};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;

pub type Result<T> = std::result::Result<T, String>;

/// How often `pair_wait` asks whether the code has been approved.
const PAIR_POLL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl Client {
    /// A client for `server` (`https://` is assumed without a scheme).
    /// Fails if that isn't a usable address.
    pub fn new(server: &str, token: Option<String>) -> Result<Self> {
        let base = normalize_server(server);
        reqwest::Url::parse(&base)
            .map_err(|e| format!("{server:?} is not a server address: {e}"))?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("pressbox-iced/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { http, base, token })
    }

    /// The same server, with a token.
    pub fn with_token(&self, token: String) -> Self {
        Self {
            token: Some(token),
            ..self.clone()
        }
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

    /// Saves where playback of an item's media stopped, in seconds.
    pub async fn set_position(self, id: i64, position: u32) -> Result<()> {
        let req = self
            .request(Method::PUT, &format!("/api/v1/items/{id}/position"))
            .json(&Position { position });
        Self::send(req).await.map(drop)
    }

    /// An item's chapters; none if it has no chapters file.
    pub async fn chapters(self, id: i64) -> Result<Chapters> {
        let req = self.request(Method::GET, &format!("/api/v1/items/{id}/chapters"));
        Ok(Self::optional(req).await?.unwrap_or_default())
    }

    /// An item's transcript; `None` if it has none.
    pub async fn transcript(self, id: i64) -> Result<Option<Transcript>> {
        Self::optional(self.request(Method::GET, &format!("/api/v1/items/{id}/transcript"))).await
    }

    /// The Up Next queue, in order; `None` from a server without one.
    pub async fn queue(self) -> Result<Option<Vec<Item>>> {
        let queue: Option<Queue> =
            Self::optional(self.request(Method::GET, "/api/v1/queue")).await?;
        Ok(queue.map(|q| q.items))
    }

    /// Replaces the Up Next queue with `ids`, in order; returns what the
    /// server kept.
    pub async fn set_queue(self, ids: Vec<i64>) -> Result<Vec<Item>> {
        let req = self
            .request(Method::PUT, "/api/v1/queue")
            .json(&QueueOrder { ids });
        Ok(Self::json::<Queue>(req).await?.items)
    }

    /// JSON, or `None` for a 404.
    async fn optional<T: DeserializeOwned>(req: RequestBuilder) -> Result<Option<T>> {
        let resp = req.send().await.map_err(describe)?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = match resp.status() {
            s if s.is_success() => resp,
            s => return Err(format!("server returned {s}")),
        };
        resp.json().await.map(Some).map_err(describe)
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
        self.pair_wait_every(poll_id, PAIR_POLL).await
    }

    async fn pair_wait_every(self, poll_id: String, every: Duration) -> Result<PairApproved> {
        loop {
            let resp = self
                .request(Method::GET, &format!("/api/v1/pair/{poll_id}"))
                .send()
                .await
                .map_err(describe)?;
            match resp.status() {
                StatusCode::OK => return resp.json().await.map_err(describe),
                StatusCode::ACCEPTED => tokio::time::sleep(every).await,
                StatusCode::NOT_FOUND => return Err("the request was denied or expired".into()),
                s => return Err(format!("server returned {s}")),
            }
        }
    }

    /// Approves another device's pairing code, for `user` if given (admins).
    pub async fn approve(self, code: String, user: Option<String>) -> Result<Approved> {
        let code = code.trim();
        let path = format!("/api/v1/pairings/{code}/approve");
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
    use crate::test_server::server;

    /// The request lines a canned server saw, without the HTTP version.
    fn lines(requests: &std::sync::mpsc::Receiver<String>, n: usize) -> Vec<String> {
        requests
            .iter()
            .take(n)
            .map(|head| {
                assert!(head.contains("authorization: bearer tok"), "{head}");
                head.lines()
                    .next()
                    .unwrap()
                    .trim_end_matches(" http/1.1")
                    .to_owned()
            })
            .collect()
    }

    #[tokio::test]
    async fn podcast_routes() {
        let (base, requests) = server(vec![
            (204, ""),
            (200, r#"{"chapters":[{"start":0.0,"title":"Intro"}]}"#),
            (404, "{}"),
            (200, r#"{"segments":[{"start":0.0,"end":1.0,"text":"Hi"}]}"#),
            (404, "{}"),
            (502, "{}"),
        ]);
        let client = Client::new(&base, Some("tok".into())).unwrap();
        client.clone().set_position(7, 95).await.unwrap();
        let chapters = client.clone().chapters(7).await.unwrap();
        assert_eq!(chapters.chapters[0].title.as_deref(), Some("Intro"));
        assert!(
            client
                .clone()
                .chapters(8)
                .await
                .unwrap()
                .chapters
                .is_empty()
        );
        let transcript = client.clone().transcript(7).await.unwrap().unwrap();
        assert_eq!(transcript.segments[0].text, "Hi");
        assert_eq!(client.clone().transcript(8).await.unwrap(), None);
        assert_eq!(
            client.clone().transcript(9).await.unwrap_err(),
            "server returned 502 Bad Gateway"
        );
        assert_eq!(
            lines(&requests, 6),
            [
                "put /api/v1/items/7/position",
                "get /api/v1/items/7/chapters",
                "get /api/v1/items/8/chapters",
                "get /api/v1/items/7/transcript",
                "get /api/v1/items/8/transcript",
                "get /api/v1/items/9/transcript",
            ]
        );
        let offline = Client::new("http://127.0.0.1:9", None).unwrap();
        assert_eq!(
            offline.transcript(1).await.unwrap_err(),
            "could not connect to the server"
        );
    }

    #[tokio::test]
    async fn queue_routes() {
        let one = r#"{"items":[{"id":7,"feed_id":"a","title":"Seven","url":null,"author":null,
            "summary":null,"published":0,"read":false,"starred":false}]}"#;
        let (base, requests) = server(vec![(200, one), (200, one), (404, "{}")]);
        let client = Client::new(&base, Some("tok".into())).unwrap();
        assert_eq!(client.clone().queue().await.unwrap().unwrap()[0].id, 7);
        assert_eq!(client.clone().set_queue(vec![7, 8]).await.unwrap().len(), 1);
        assert_eq!(client.queue().await.unwrap(), None, "an older server");
        assert_eq!(
            lines(&requests, 3),
            [
                "get /api/v1/queue",
                "put /api/v1/queue",
                "get /api/v1/queue"
            ]
        );
    }

    #[tokio::test]
    async fn reads_and_writes_with_the_token() {
        let feeds = r#"[{"id":"a","title":"A","url":"https://a.example/","site_url":null,
            "tags":[],"unread":1,"total":1,"last_fetched":null,"last_error":null,"archived":false}]"#;
        let item = r#"{"id":7,"feed_id":"a","title":"Seven","url":null,"author":null,
            "summary":null,"content":"<p>x</p>","published":0,"read":false,"starred":false}"#;
        let (base, requests) = server(vec![
            (200, r#"{"version":"0.1.0","api":1}"#),
            (200, r#"{"user":"chris","admin":true,"device":null}"#),
            (200, feeds),
            (200, r#"{"items":[],"next_cursor":"c"}"#),
            (200, item),
            (204, ""),
            (204, ""),
            (200, r#"{"updated":4}"#),
            (202, ""),
        ]);
        let client = Client::new(&base, Some("tok".into())).unwrap();
        assert_eq!(client.server(), base);
        assert_eq!(client.clone().version().await.unwrap().api, 1);
        assert_eq!(client.clone().me().await.unwrap().user, "chris");
        assert_eq!(client.clone().feeds().await.unwrap()[0].unread, 1);
        let query = ItemQuery {
            tag: Some("tech".into()),
            cursor: Some("c".into()),
            ..ItemQuery::default()
        };
        let page = client.clone().items(query).await.unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some("c"));
        let item = client.clone().item(7).await.unwrap();
        assert_eq!(item.content.as_deref(), Some("<p>x</p>"));
        client.clone().set_read(7, true).await.unwrap();
        client.clone().set_starred(7, false).await.unwrap();
        assert_eq!(client.clone().mark_feed_read("a".into()).await.unwrap(), 4);
        client.clone().refresh().await.unwrap();
        assert_eq!(
            lines(&requests, 9),
            [
                "get /api/v1/version",
                "get /api/v1/me",
                "get /api/v1/feeds",
                "get /api/v1/items?tag=tech&cursor=c",
                "get /api/v1/items/7",
                "put /api/v1/items/7/read",
                "put /api/v1/items/7/starred",
                "post /api/v1/feeds/a/read-all",
                "post /api/v1/refresh",
            ]
        );
    }

    #[tokio::test]
    async fn explains_failures() {
        let (base, _requests) = server(vec![(401, "{}"), (503, "{}"), (200, "not json")]);
        let client = Client::new(&base, Some("tok".into())).unwrap();
        assert_eq!(
            client.clone().feeds().await.unwrap_err(),
            "the server rejected the token (401)"
        );
        assert_eq!(
            client.clone().refresh().await.unwrap_err(),
            "server returned 503 Service Unavailable"
        );
        assert!(
            client
                .clone()
                .me()
                .await
                .unwrap_err()
                .starts_with("unexpected response")
        );

        // Nothing listens on the discard port.
        let nobody = Client::new("http://127.0.0.1:9", None).unwrap();
        assert_eq!(
            nobody.feeds().await.unwrap_err(),
            "could not connect to the server"
        );
    }

    #[tokio::test]
    async fn rejects_bad_addresses_and_describes_failures() {
        let error = Client::new("http://bad host", None).unwrap_err();
        assert!(
            error.starts_with("\"http://bad host\" is not a server address"),
            "{error}"
        );
        let client = Client::new("rss.example", None)
            .unwrap()
            .with_token("t".into());
        assert_eq!(
            (client.server(), client.token.as_deref()),
            ("https://rss.example", Some("t"))
        );

        // Connects, but never answers.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let impatient = reqwest::Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .unwrap();
        let url = format!("http://{}/", silent.local_addr().unwrap());
        let timeout = impatient.get(&url).send().await.unwrap_err();
        assert_eq!(describe(timeout), "the server did not answer in time");
        let other = impatient
            .get("http://[::1]:99999/")
            .send()
            .await
            .unwrap_err();
        assert!(!describe(other).is_empty());
    }

    #[tokio::test]
    async fn pairs_and_approves() {
        let (base, requests) = server(vec![
            (201, r#"{"code":"K7W-4QX","poll_id":"p1","expires_in":600}"#),
            (202, r#"{"status":"pending"}"#),
            (200, r#"{"token":"secret","name":"laptop"}"#),
            (404, "{}"),
            (500, "{}"),
        ]);
        let client = Client::new(&base, None).unwrap();
        let start = client.clone().pair_start("laptop".into()).await.unwrap();
        assert_eq!(start.code, "K7W-4QX");
        // Approved before the first poll: no waiting.
        let (base2, _requests2) = server(vec![(200, r#"{"token":"s","name":"n"}"#)]);
        let quick = Client::new(&base2, None).unwrap();
        assert_eq!(quick.pair_wait("p0".into()).await.unwrap().token, "s");
        let wait = |c: &Client| c.clone().pair_wait_every("p1".into(), Duration::ZERO);
        assert_eq!(wait(&client).await.unwrap().token, "secret");
        assert_eq!(
            wait(&client).await.unwrap_err(),
            "the request was denied or expired"
        );
        assert!(wait(&client).await.unwrap_err().contains("500"));
        let paths: Vec<String> = requests
            .iter()
            .take(3)
            .map(|h| h.lines().next().unwrap().to_owned())
            .collect();
        assert_eq!(
            paths,
            [
                "post /api/v1/pair http/1.1",
                "get /api/v1/pair/p1 http/1.1",
                "get /api/v1/pair/p1 http/1.1"
            ]
        );

        let (base, _requests) = server(vec![
            (404, "{}"),
            (403, "{}"),
            (400, "{}"),
            (500, "{}"),
            (200, r#"{"name":"phone","user":"anna"}"#),
        ]);
        let client = Client::new(&base, Some("tok".into())).unwrap();
        let approve = |user: Option<&str>| {
            client
                .clone()
                .approve(" K7W-4QX ".into(), user.map(str::to_owned))
        };
        assert_eq!(
            approve(None).await.unwrap_err(),
            "no pending request with code K7W-4QX"
        );
        assert!(
            approve(Some("anna"))
                .await
                .unwrap_err()
                .contains("only admins")
        );
        assert_eq!(approve(Some("nobody")).await.unwrap_err(), "no such user");
        assert!(approve(None).await.unwrap_err().contains("500"));
        assert_eq!(approve(Some("anna")).await.unwrap().user, "anna");
    }

    #[test]
    fn normalizes_server() {
        assert_eq!(normalize_server("rss.example/"), "https://rss.example");
        assert_eq!(
            normalize_server(" http://10.0.0.2:8470 "),
            "http://10.0.0.2:8470"
        );
    }
}
