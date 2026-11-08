// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: AGPL-3.0-only

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::{Notify, Semaphore};
use tracing::info;

mod admin;
mod api;
mod config;
mod db;
mod fetch;
mod pairing;
mod sanitize;
mod systemd;

use config::Config;
use db::Db;

/// pressbox RSS/Atom aggregation server.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Path to the TOML configuration.
    #[arg(short, long, env = "PRESSBOX_CONFIG", default_value = "pressbox.toml")]
    config: PathBuf,
    /// Validate the configuration and exit.
    #[arg(long)]
    check: bool,
}

/// Read-only connections in the pool; reads are short, so a few suffice.
const READERS: usize = 4;

pub struct AppState {
    /// The one writer connection. Never hold the guard across an `.await`.
    db: Mutex<Db>,
    /// Read-only connections; empty for in-memory databases (tests).
    readers: Vec<Mutex<Db>>,
    next_reader: std::sync::atomic::AtomicUsize,
    http: reqwest::Client,
    /// The static token from `token_file` or the systemd credential.
    token: Option<String>,
    refresh: Notify,
    max_items_per_feed: u32,
    /// Items published longer ago than this are removed (unless starred).
    max_age_days: Option<u32>,
    pairings: pairing::Pairings,
    /// Feeds whose full articles are being fetched, so a feed has at most
    /// one such task at a time.
    articles_running: Mutex<HashSet<String>>,
    /// Limits article fetching across feeds, apart from feed polling.
    article_slots: Semaphore,
    backup_dir: PathBuf,
    /// Trust `X-Forwarded-For` from a reverse proxy for client addresses.
    behind_proxy: bool,
}

impl AppState {
    pub fn new(db: Db, token: Option<String>, max_items_per_feed: u32) -> Result<Self> {
        Ok(Self {
            db: Mutex::new(db),
            readers: Vec::new(),
            next_reader: std::sync::atomic::AtomicUsize::new(0),
            http: fetch::client()?,
            token,
            refresh: Notify::new(),
            max_items_per_feed,
            max_age_days: None,
            pairings: pairing::Pairings::default(),
            articles_running: Mutex::default(),
            article_slots: Semaphore::const_new(fetch::ARTICLE_TASKS),
            backup_dir: PathBuf::from("backups"),
            behind_proxy: false,
        })
    }

    /// Makes every feed due and wakes the scheduler.
    pub async fn request_refresh(self: &Arc<Self>) {
        if let Err(e) = self.write(|db| db.force_refresh()).await {
            tracing::warn!("forcing a refresh: {e:#}");
        }
        self.refresh.notify_one();
    }

    /// Opens `count` read-only connections to the database at `path`.
    pub fn with_readers(mut self, path: &std::path::Path, count: usize) -> Result<Self> {
        self.readers = (0..count)
            .map(|_| Db::open_reader(path).map(Mutex::new))
            .collect::<Result<_>>()?;
        Ok(self)
    }

    /// Locks the writer connection. Never hold the guard across an `.await`.
    pub fn db(&self) -> MutexGuard<'_, Db> {
        self.db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A read-only connection: a free one if there is, else the next in
    /// turn. Reads go here so they never wait behind a write. Falls back to
    /// the writer when there is no pool. Never hold the guard across an
    /// `.await`.
    pub fn reader(&self) -> MutexGuard<'_, Db> {
        if self.readers.is_empty() {
            return self.db();
        }
        let start = self
            .next_reader
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        for i in 0..self.readers.len() {
            if let Ok(guard) = self.readers[(start + i) % self.readers.len()].try_lock() {
                return guard;
            }
        }
        self.readers[start % self.readers.len()]
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Runs `f` on a read connection on the blocking pool. SQLite calls
    /// block, and a lock may have to wait, so async code goes through here
    /// (or `write`) rather than holding up a runtime thread.
    pub async fn read<T, F>(self: &Arc<Self>, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Db) -> Result<T> + Send + 'static,
    {
        let state = self.clone();
        tokio::task::spawn_blocking(move || f(&state.reader())).await?
    }

    /// Runs `f` on the writer connection on the blocking pool; see `read`.
    pub async fn write<T, F>(self: &Arc<Self>, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Db) -> Result<T> + Send + 'static,
    {
        let state = self.clone();
        tokio::task::spawn_blocking(move || f(&mut state.db())).await?
    }
}

fn init_logging() {
    use tracing_subscriber::{EnvFilter, Registry, layer::SubscriberExt, util::SubscriberInitExt};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "pressboxd=info".into());
    // Under systemd, log to the journal directly so levels become priorities
    // (journalctl -p warning) and fields stay searchable.
    if systemd::stderr_is_journal() {
        match tracing_journald::layer() {
            Ok(journal) => {
                Registry::default()
                    .with(filter)
                    .with(journal.with_syslog_identifier("pressboxd".into()))
                    .init();
                return;
            }
            Err(e) => eprintln!("cannot connect to the journal, logging to stderr: {e}"),
        }
    }
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();

    let args = Args::parse();
    let config = Config::load(&args.config)?;
    if args.check {
        println!(
            "{}: ok, {} feeds",
            args.config.display(),
            config.feed_count()
        );
        return Ok(());
    }

    let mut db = Db::open(&config.database)
        .with_context(|| format!("opening {}", config.database.display()))?;
    db.sync(&config)?;
    let mut state = AppState::new(db, config.token()?, config.max_items_per_feed)?
        .with_readers(&config.database, READERS)?;
    state.backup_dir = config.backup_dir();
    state.behind_proxy = config.behind_proxy;
    state.max_age_days = config.max_age_days;
    let state = Arc::new(state);

    let (listener, address) = match systemd::listener()? {
        Some(socket) => {
            let address = socket.local_addr()?;
            info!("using the socket from systemd ({address}); `listen` is ignored");
            (tokio::net::TcpListener::from_std(socket)?, address)
        }
        None => (
            tokio::net::TcpListener::bind(config.listen)
                .await
                .with_context(|| format!("binding {}", config.listen))?,
            config.listen,
        ),
    };
    let control_path = config.control_socket();
    let control = admin::bind(&control_path)?;
    let feeds = config.feed_count();
    info!(
        "listening on {address} with {feeds} feeds; control socket {}",
        control_path.display()
    );
    if state.token.is_none() && state.reader().counts()?.devices == 0 {
        info!(
            "no devices paired yet: run `pressboxctl pair` on a client, then `pressboxctl approve`"
        );
    }
    systemd::ready(&format!("listening on {address}, {feeds} feeds"));
    systemd::spawn_watchdog(state.clone());

    tokio::spawn(schedule(state.clone(), config.refresh_interval));
    let admin_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = axum::serve(control, admin::router(admin_state)).await {
            tracing::error!("control socket: {e}");
        }
    });

    axum::serve(
        listener,
        api::router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    let _ = std::fs::remove_file(&control_path);
    Ok(())
}

/// Fetches feeds as they come due, sleeping until the next one (at most five
/// minutes, so newly due feeds are noticed) or until a refresh is requested.
async fn schedule(state: Arc<AppState>, default_interval: u64) {
    let mut last = None;
    loop {
        if let Some(stats) = fetch::run_due(state.clone(), default_interval).await {
            last = Some((stats, chrono::Local::now()));
        }
        if let Ok(counts) = state.read(|db| db.counts()).await {
            systemd::status(&status_line(&counts, last));
        }
        let now = fetch::now();
        let next = state
            .read(|db| db.next_due())
            .await
            .ok()
            .flatten()
            .unwrap_or(now + 300);
        let wait = Duration::from_secs(u64::try_from(next - now).unwrap_or(0).clamp(1, 300));
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = state.refresh.notified() => info!("refresh requested"),
        }
    }
}

fn status_line(
    counts: &db::Counts,
    last: Option<(fetch::RefreshStats, chrono::DateTime<chrono::Local>)>,
) -> String {
    let mut s = format!("{} feeds, {} users", counts.feeds, counts.users);
    if let Some((stats, at)) = last {
        s.push_str(&format!(
            ", {} new at {}",
            stats.new_items,
            at.format("%H:%M")
        ));
    }
    if counts.failing_feeds > 0 {
        s.push_str(&format!(", {} failing", counts.failing_feeds));
    }
    s
}

async fn shutdown() {
    let ctrl_c = tokio::signal::ctrl_c();
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("installing SIGTERM handler");
    tokio::select! {
        _ = ctrl_c => {}
        _ = term.recv() => {}
    }
    info!("shutting down");
    systemd::stopping();
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use axum::{body::Body, http::Request};
    use pressbox_api::ItemQuery;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn reads_do_not_wait_for_the_writer() {
        let path = std::env::temp_dir().join(format!("pressboxd-pool-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).unwrap();
        db.sync(&Config::parse("[feeds.a]\nurl = \"https://a.example/\"").unwrap())
            .unwrap();
        let user = db.only_user().unwrap().unwrap();
        let token = pairing::create_device(&db, &user, "laptop", fetch::now())
            .unwrap()
            .token;
        db.authenticate(&pairing::hash_token(&token), fetch::now())
            .unwrap();
        let state = Arc::new(
            AppState::new(db, None, 100)
                .unwrap()
                .with_readers(&path, 2)
                .unwrap(),
        );

        // A long write holds the writer lock.
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let writer = {
            let state = state.clone();
            std::thread::spawn(move || {
                let mut db = state.db();
                let items: Vec<db::NewItem> = (0..200)
                    .map(|n| db::NewItem {
                        guid: n.to_string(),
                        title: format!("item {n}"),
                        url: None,
                        author: None,
                        summary: None,
                        content: None,
                        published: Some(n),
                    })
                    .collect();
                db.upsert_items("https://a.example/", 0, &items).unwrap();
                held_tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(800));
            })
        };
        held_rx.recv().unwrap();

        // Reads, and authenticating a request, answer while it is held.
        let started = Instant::now();
        let page = state
            .reader()
            .items(user.id, &ItemQuery::default())
            .unwrap();
        assert_eq!(page.items.len(), 50, "sees the committed write");
        let req = Request::get("/api/v1/feeds")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = api::router(state.clone()).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        assert!(
            started.elapsed() < Duration::from_millis(300),
            "reads waited {:?} for the writer",
            started.elapsed()
        );
        writer.join().unwrap();
        drop(state);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
