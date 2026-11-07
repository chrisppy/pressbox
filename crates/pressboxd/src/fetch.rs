// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: AGPL-3.0-only

//! Polling feeds and turning them into items.
//!
//! Each feed has its own schedule. After a successful fetch the next one is
//! `interval` seconds later, or later still if the server's `Cache-Control:
//! max-age` asks for that. After a failure the delay doubles each time (up to
//! a day), and a `Retry-After` from a 429 or 503 is honored.

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use futures::StreamExt;
use reqwest::{
    StatusCode,
    header::{
        CACHE_CONTROL, CONTENT_TYPE, ETAG, HeaderMap, IF_MODIFIED_SINCE, IF_NONE_MATCH,
        LAST_MODIFIED, RETRY_AFTER,
    },
};
use tracing::{info, warn};

use crate::{
    AppState,
    db::{FetchResult, FetchTarget, NewItem},
    sanitize,
};

const MAX_BODY: usize = 20 * 1024 * 1024;
const CONCURRENCY: usize = 8;
/// Largest article page fetched for full text.
const MAX_PAGE: usize = 5 * 1024 * 1024;
/// Article pages fetched per feed per round; the rest wait for the next one.
const ARTICLES_PER_ROUND: u32 = 10;
/// Less text than this is not taken to be an article.
const MIN_ARTICLE_CHARS: usize = 200;
/// The longest pressbox waits between fetches of a feed, whatever the reason.
pub const MAX_DELAY: u64 = 24 * 60 * 60;

pub fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!(
            "pressbox/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/chrisppy/pressbox)"
        ))
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()?)
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// How a poll went, for scheduling the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Fetched or not modified. `max_age` is from `Cache-Control`.
    Ok { max_age: Option<u64> },
    /// Failed. `retry_after` is from a `Retry-After` header.
    Failed { retry_after: Option<u64> },
}

/// When to fetch a feed next, and its new consecutive-failure count.
pub fn schedule(now: i64, interval: u64, fails_before: u32, outcome: Outcome) -> (i64, u32) {
    let (delay, fails) = match outcome {
        Outcome::Ok { max_age } => (interval.max(max_age.unwrap_or(0).min(MAX_DELAY)), 0),
        Outcome::Failed { retry_after } => {
            let fails = fails_before.saturating_add(1);
            let backoff = interval.saturating_mul(1 << (fails - 1).min(6));
            (backoff.max(retry_after.unwrap_or(0)), fails)
        }
    };
    let delay = i64::try_from(delay.min(MAX_DELAY)).unwrap_or(i64::MAX);
    (now.saturating_add(delay), fails)
}

/// `max-age` from a `Cache-Control` header.
pub fn max_age(headers: &HeaderMap) -> Option<u64> {
    headers
        .get_all(CACHE_CONTROL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .find_map(|d| {
            d.trim()
                .strip_prefix("max-age=")?
                .trim_matches('"')
                .parse()
                .ok()
        })
}

/// Seconds to wait from a `Retry-After` header (seconds or an HTTP date).
pub fn retry_after(headers: &HeaderMap, now: i64) -> Option<u64> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = value.parse() {
        return Some(secs);
    }
    let at = chrono::DateTime::parse_from_rfc2822(value)
        .ok()?
        .timestamp();
    Some(u64::try_from(at - now).unwrap_or(0))
}

/// What one pass over the due feeds did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RefreshStats {
    pub fetched: usize,
    pub failed: usize,
    pub new_items: usize,
}

/// Fetches every feed that is due, a few at a time. `None` if none was due.
pub async fn run_due(state: Arc<AppState>, default_interval: u64) -> Option<RefreshStats> {
    let targets = match state.db().due_targets(now(), default_interval) {
        Ok(t) if t.is_empty() => return None,
        Ok(t) => t,
        Err(e) => {
            warn!("listing due feeds: {e:#}");
            return None;
        }
    };
    let fetched = targets.len();
    let results: Vec<Option<usize>> = futures::stream::iter(targets)
        .map(|target| {
            let state = state.clone();
            async move { refresh_one(&state, target).await }
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;
    let stats = RefreshStats {
        fetched,
        failed: results.iter().filter(|r| r.is_none()).count(),
        new_items: results.into_iter().flatten().sum(),
    };
    info!(
        "fetched {} feeds: {} new items, {} failed",
        stats.fetched, stats.new_items, stats.failed
    );
    Some(stats)
}

/// Returns the number of new items, or `None` if the feed failed.
async fn refresh_one(state: &AppState, target: FetchTarget) -> Option<usize> {
    let now = now();
    let (mut result, items, outcome) = match poll(&state.http, &target, now).await {
        Ok(Polled::Fresh {
            result,
            items,
            max_age,
        }) => (result, items, Outcome::Ok { max_age }),
        Ok(Polled::NotModified { max_age }) => (
            FetchResult {
                etag: target.etag.clone(),
                last_modified: target.last_modified.clone(),
                ..Default::default()
            },
            Vec::new(),
            Outcome::Ok { max_age },
        ),
        Err(e) => {
            warn!(feed = target.url, "{}", e.message);
            (
                FetchResult {
                    error: Some(e.message),
                    ..Default::default()
                },
                Vec::new(),
                Outcome::Failed {
                    retry_after: e.retry_after,
                },
            )
        }
    };
    (result.next_fetch, result.fail_count) =
        schedule(now, target.interval, target.fail_count, outcome);
    let failed = result.error.is_some();
    // Too old to keep: skipped here too, or they would come back as unread
    // on every fetch after being pruned.
    let cutoff = state
        .max_age_days
        .map(|days| now - i64::from(days) * 86_400);
    let items: Vec<_> = match cutoff {
        Some(cutoff) => items
            .into_iter()
            .filter(|i| i.published.is_none_or(|p| p >= cutoff))
            .collect(),
        None => items,
    };
    let stored = (|| -> Result<usize> {
        let mut db = state.db();
        let new = db.upsert_items(&target.url, now, &items)?;
        db.prune(&target.url, state.max_items_per_feed, cutoff)?;
        db.record_fetch(&target.url, now, &result)?;
        Ok(new)
    })();
    if target.full_text && !failed {
        fetch_articles(state, &target.url).await;
    }
    match stored {
        Ok(new) if !failed => Some(new),
        Ok(_) => None,
        Err(e) => {
            warn!(feed = target.url, "storing: {e:#}");
            None
        }
    }
}

/// Fetches the pages of a feed's newest items that don't have their full
/// article yet, a few per round, and stores what can be extracted.
async fn fetch_articles(state: &AppState, feed: &str) {
    let pending = match state.db().full_text_pending(feed, ARTICLES_PER_ROUND) {
        Ok(p) => p,
        Err(e) => {
            warn!(feed, "listing articles to fetch: {e:#}");
            return;
        }
    };
    for (id, url) in pending {
        let article = match fetch_page(&state.http, &url).await {
            Ok(html) => {
                let page = url.clone();
                tokio::task::spawn_blocking(move || extract(&html, &page))
                    .await
                    .ok()
                    .flatten()
            }
            Err(e) => {
                warn!(feed, url, "fetching article: {e}");
                None
            }
        };
        if article.is_none() {
            info!(feed, url, "no article found; keeping the feed's text");
        }
        if let Err(e) = state.db().store_full_text(id, article.as_deref()) {
            warn!(feed, url, "storing article: {e:#}");
        }
    }
}

async fn fetch_page(http: &reqwest::Client, url: &str) -> Result<String, String> {
    let resp = http.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let html = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|t| t.contains("html"));
    if !html {
        return Err("not an HTML page".to_owned());
    }
    if resp.content_length().is_some_and(|n| n > MAX_PAGE as u64) {
        return Err(format!("page is larger than {MAX_PAGE} bytes"));
    }
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    if body.len() > MAX_PAGE {
        return Err(format!("page is larger than {MAX_PAGE} bytes"));
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// The main content of an HTML page, sanitized, if there is enough of it to
/// be an article rather than a cookie banner or an index page.
pub fn extract(html: &str, url: &str) -> Option<String> {
    let article = dom_smoothie::Readability::new(html, Some(url), None)
        .ok()?
        .parse()
        .ok()?;
    if article.text_content.trim().chars().count() < MIN_ARTICLE_CHARS {
        return None;
    }
    Some(sanitize::html(&article.content, Some(url))).filter(|a| !a.trim().is_empty())
}

enum Polled {
    Fresh {
        result: FetchResult,
        items: Vec<NewItem>,
        max_age: Option<u64>,
    },
    NotModified {
        max_age: Option<u64>,
    },
}

struct PollError {
    message: String,
    retry_after: Option<u64>,
}

impl<E: std::fmt::Display> From<E> for PollError {
    fn from(e: E) -> Self {
        Self {
            message: e.to_string(),
            retry_after: None,
        }
    }
}

async fn poll(http: &reqwest::Client, target: &FetchTarget, now: i64) -> Result<Polled, PollError> {
    let mut req = http.get(&target.url);
    if let Some(etag) = &target.etag {
        req = req.header(IF_NONE_MATCH, etag);
    }
    if let Some(lm) = &target.last_modified {
        req = req.header(IF_MODIFIED_SINCE, lm);
    }
    let resp = req.send().await?;
    let status = resp.status();
    let headers = resp.headers().clone();
    if status == StatusCode::NOT_MODIFIED {
        return Ok(Polled::NotModified {
            max_age: max_age(&headers),
        });
    }
    if !status.is_success() {
        return Err(PollError {
            message: format!("HTTP {status}"),
            retry_after: matches!(
                status,
                StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
            )
            .then(|| retry_after(&headers, now))
            .flatten(),
        });
    }
    let header = |name| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    if resp.content_length().is_some_and(|n| n > MAX_BODY as u64) {
        return Err(format!("feed is larger than {MAX_BODY} bytes").into());
    }
    let body = resp.bytes().await?;
    if body.len() > MAX_BODY {
        return Err(format!("feed is larger than {MAX_BODY} bytes").into());
    }
    let (mut result, items) = parse(&body, Some(&target.url)).map_err(|e| format!("{e:#}"))?;
    result.etag = header(ETAG);
    result.last_modified = header(LAST_MODIFIED);
    Ok(Polled::Fresh {
        result,
        items,
        max_age: max_age(&headers),
    })
}

/// Parses RSS, Atom or JSON Feed into feed metadata and sanitized items.
pub fn parse(body: &[u8], uri: Option<&str>) -> Result<(FetchResult, Vec<NewItem>)> {
    let feed = feed_rs::parser::Builder::new()
        .base_uri(uri)
        .build()
        .parse(body)?;
    let feed_url = uri.unwrap_or_default();
    let items = feed
        .entries
        .into_iter()
        .map(|e| {
            let url = e
                .links
                .iter()
                .find(|l| l.rel.as_deref().is_none_or(|r| r == "alternate"))
                .or_else(|| e.links.first())
                .map(|l| l.href.clone());
            let title = e
                .title
                .map(|t| sanitize::text(&t.content))
                .filter(|t| !t.is_empty())
                .or_else(|| url.clone())
                .unwrap_or_else(|| "(untitled)".to_owned());
            let base = sanitize::base_url(url.as_deref(), feed_url);
            let base = (!base.is_empty()).then_some(base.as_str());
            NewItem {
                guid: e.id,
                title,
                author: e.authors.into_iter().find_map(|p| p.name),
                summary: e.summary.map(|s| sanitize::html(&s.content, base)),
                content: e
                    .content
                    .and_then(|c| c.body)
                    .map(|b| sanitize::html(&b, base)),
                published: e.published.or(e.updated).map(|d| d.timestamp()),
                url,
            }
        })
        .collect();
    let result = FetchResult {
        title: feed.title.map(|t| sanitize::text(&t.content)),
        site_url: feed
            .links
            .iter()
            .find(|l| l.rel.as_deref().is_none_or(|r| r == "alternate"))
            .map(|l| l.href.clone()),
        ..Default::default()
    };
    Ok((result, items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rss() {
        let rss = br#"<?xml version="1.0"?>
            <rss version="2.0"><channel>
              <title> Example </title><link>https://example.com/</link>
              <item>
                <title>First</title><link>https://example.com/1</link>
                <guid>urn:1</guid><description>&lt;p&gt;Hi&lt;/p&gt;</description>
                <pubDate>Tue, 03 Nov 2026 10:00:00 GMT</pubDate>
              </item>
              <item><link>https://example.com/2</link><guid>urn:2</guid></item>
            </channel></rss>"#;
        let (meta, items) = parse(rss, None).unwrap();
        assert_eq!(meta.title.as_deref(), Some("Example"));
        assert_eq!(meta.site_url.as_deref(), Some("https://example.com/"));
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].guid, "urn:1");
        assert_eq!(items[0].summary.as_deref(), Some("<p>Hi</p>"));
        assert_eq!(items[0].published, Some(1_793_700_000));
        assert_eq!(
            items[1].title, "https://example.com/2",
            "untitled uses the link"
        );
        assert_eq!(items[1].published, None);
    }

    #[test]
    fn parses_atom() {
        let atom = br#"<?xml version="1.0" encoding="utf-8"?>
            <feed xmlns="http://www.w3.org/2005/Atom">
              <title>Atom</title><id>urn:feed</id><updated>2026-11-01T00:00:00Z</updated>
              <link rel="self" href="https://example.com/atom.xml"/>
              <link href="https://example.com/"/>
              <entry>
                <title>Post</title><id>urn:post</id>
                <link rel="alternate" href="https://example.com/post"/>
                <updated>2026-11-01T00:00:00Z</updated>
                <author><name>Chris</name></author>
                <content type="html">&lt;b&gt;body&lt;/b&gt;</content>
              </entry>
            </feed>"#;
        let (meta, items) = parse(atom, None).unwrap();
        assert_eq!(meta.site_url.as_deref(), Some("https://example.com/"));
        assert_eq!(items[0].url.as_deref(), Some("https://example.com/post"));
        assert_eq!(items[0].author.as_deref(), Some("Chris"));
        assert_eq!(items[0].content.as_deref(), Some("<b>body</b>"));
        assert_eq!(items[0].published, Some(1_793_491_200));
    }

    #[test]
    fn sanitizes_entries() {
        let rss = br#"<?xml version="1.0"?>
            <rss version="2.0"><channel><title>T &amp;amp; &lt;b&gt;x&lt;/b&gt;</title>
              <item><title>A &lt;b&gt;bold&lt;/b&gt; title</title>
                <link>https://example.com/posts/1</link><guid>g</guid>
                <description>&lt;p&gt;hi&lt;script&gt;bad()&lt;/script&gt;
                  &lt;a href="../2"&gt;next&lt;/a&gt;
                  &lt;img src="https://t.example/p.gif" width="1" height="1"&gt;&lt;/p&gt;</description>
              </item>
            </channel></rss>"#;
        let (meta, items) = parse(rss, Some("https://example.com/feed.xml")).unwrap();
        assert_eq!(meta.title.as_deref(), Some("T & x"));
        assert_eq!(items[0].title, "A bold title", "titles are plain text");
        let summary = items[0].summary.as_deref().unwrap();
        assert!(
            !summary.contains("script") && !summary.contains("p.gif"),
            "{summary}"
        );
        assert!(
            summary.contains(r#"href="https://example.com/2""#),
            "{summary}"
        );
    }

    #[test]
    fn schedules_successes_and_backs_off_failures() {
        let ok = |max_age| Outcome::Ok { max_age };
        let fail = |retry_after| Outcome::Failed { retry_after };
        assert_eq!(schedule(1000, 600, 3, ok(None)), (1600, 0));
        assert_eq!(
            schedule(1000, 600, 0, ok(Some(60))),
            (1600, 0),
            "never sooner than interval"
        );
        assert_eq!(
            schedule(1000, 600, 0, ok(Some(7200))),
            (8200, 0),
            "max-age slows it down"
        );
        assert_eq!(
            schedule(0, 600, 0, ok(Some(10_000_000))).0,
            86400,
            "capped at a day"
        );

        assert_eq!(schedule(0, 600, 0, fail(None)), (600, 1));
        assert_eq!(schedule(0, 600, 1, fail(None)), (1200, 2));
        assert_eq!(schedule(0, 600, 2, fail(None)), (2400, 3));
        assert_eq!(
            schedule(0, 600, 30, fail(None)).0,
            38400,
            "doubling stops at 64x"
        );
        assert_eq!(
            schedule(0, 1800, 30, fail(None)).0,
            86400,
            "capped at a day"
        );
        assert_eq!(
            schedule(0, 600, 0, fail(Some(5000))),
            (5000, 1),
            "Retry-After wins"
        );
    }

    #[test]
    fn reads_cache_headers() {
        let mut h = HeaderMap::new();
        assert_eq!(max_age(&h), None);
        h.insert(CACHE_CONTROL, "public, max-age=3600".parse().unwrap());
        assert_eq!(max_age(&h), Some(3600));
        h.insert(RETRY_AFTER, "120".parse().unwrap());
        assert_eq!(retry_after(&h, 0), Some(120));
        h.insert(
            RETRY_AFTER,
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        let at = 1_445_412_480;
        assert_eq!(retry_after(&h, at - 90), Some(90));
        assert_eq!(
            retry_after(&h, at + 90),
            Some(0),
            "dates in the past mean now"
        );
    }

    #[test]
    fn parses_json_feed_and_rss1() {
        let json = br#"{
            "version": "https://jsonfeed.org/version/1.1",
            "title": "JSON blog",
            "home_page_url": "https://example.org/",
            "items": [{
                "id": "j1",
                "url": "https://example.org/posts/1",
                "title": "Hello JSON",
                "content_html": "<p>Body<script>x()</script></p>",
                "date_published": "2026-11-01T00:00:00Z",
                "authors": [{ "name": "Ana" }]
            }]
        }"#;
        let (meta, items) = parse(json, Some("https://example.org/feed.json")).unwrap();
        assert_eq!(meta.title.as_deref(), Some("JSON blog"));
        assert_eq!(items[0].guid, "j1");
        assert_eq!(items[0].url.as_deref(), Some("https://example.org/posts/1"));
        assert_eq!(items[0].author.as_deref(), Some("Ana"));
        assert_eq!(
            items[0].content.as_deref(),
            Some("<p>Body</p>"),
            "sanitized"
        );
        assert_eq!(items[0].published, Some(1_793_491_200));

        let rdf = br#"<?xml version="1.0"?>
            <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
                     xmlns="http://purl.org/rss/1.0/">
              <channel rdf:about="https://example.net/"><title>RSS 1.0</title>
                <link>https://example.net/</link></channel>
              <item rdf:about="https://example.net/a"><title>RDF item</title>
                <link>https://example.net/a</link></item>
            </rdf:RDF>"#;
        let (meta, items) = parse(rdf, None).unwrap();
        assert_eq!(meta.title.as_deref(), Some("RSS 1.0"));
        assert_eq!(items[0].title, "RDF item");
        assert_eq!(items[0].url.as_deref(), Some("https://example.net/a"));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(b"<html>not a feed</html>", None).is_err());
    }

    #[test]
    fn extracts_articles() {
        let story = "The quick brown fox jumps over the lazy dog. ".repeat(20);
        let page = format!(
            r#"<html><head><title>Story</title></head><body>
            <nav><a href="/">Home</a> <a href="/about">About</a></nav>
            <article><h1>Story</h1><p>{story}</p>
              <p><img src="/fox.png" onerror="alert(1)"> More: {story}</p>
              <script>track()</script></article>
            <footer>Copyright</footer></body></html>"#
        );
        let article = extract(&page, "https://example.org/posts/1").unwrap();
        assert!(article.contains("quick brown fox"));
        assert!(article.contains("https://example.org/fox.png"), "{article}");
        assert!(!article.contains("onerror") && !article.contains("track()"));
        assert!(
            !article.contains("About"),
            "navigation is left out: {article}"
        );

        let stub = "<html><body><p>Please enable cookies.</p></body></html>";
        assert_eq!(extract(stub, "https://example.org/"), None);
    }
}
