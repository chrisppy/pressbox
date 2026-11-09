// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: AGPL-3.0-only

//! systemd integration. Every function here is a no-op, or returns `None`,
//! when pressboxd is not started by systemd, so it runs the same by hand.
//!
//! - readiness, status text and stopping via `sd_notify` (`Type=notify`)
//! - a watchdog that only pings while the database answers (`WatchdogSec=`)
//! - socket activation (`LISTEN_FDS`, from a `.socket` unit)
//! - the API token from systemd credentials (`LoadCredential=token:...`)
//! - logging straight to the journal, with levels, when stderr is the journal

use std::{
    io,
    net::TcpListener,
    os::fd::{AsFd, FromRawFd},
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use sd_notify::NotifyState;
use tracing::{debug, warn};

use crate::AppState;

/// Held by tests that set systemd's environment variables, or that run code
/// which reads them, so they don't see each other's.
#[cfg(test)]
pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The credential name the NixOS module passes the token as.
pub const TOKEN_CREDENTIAL: &str = "token";

fn notify(states: &[NotifyState]) {
    if let Err(e) = sd_notify::notify(states) {
        warn!("sd_notify: {e}");
    }
}

/// Tells systemd the service is up; it then starts units ordered after it.
pub fn ready(status: &str) {
    notify(&[NotifyState::Ready, NotifyState::Status(status)]);
}

/// Sets the text `systemctl status pressbox` shows.
pub fn status(status: &str) {
    notify(&[NotifyState::Status(status)]);
}

pub fn stopping() {
    notify(&[NotifyState::Stopping, NotifyState::Status("shutting down")]);
}

/// The listening socket systemd passed in, if the service was socket
/// activated. Only the first socket is used.
pub fn listener() -> Result<Option<TcpListener>> {
    let mut fds = sd_notify::listen_fds().context("reading LISTEN_FDS")?;
    let Some(fd) = fds.next() else {
        return Ok(None);
    };
    if fds.len() > 0 {
        warn!("systemd passed more than one socket; using the first");
    }
    // SAFETY: systemd hands this descriptor to us and nothing else in the
    // process uses it; we take ownership exactly once.
    let listener = unsafe { TcpListener::from_raw_fd(fd) };
    if listener.local_addr().is_err() {
        bail!("socket from systemd is not a TCP socket; use ListenStream= with an address");
    }
    listener.set_nonblocking(true)?;
    Ok(Some(listener))
}

/// Path of a systemd credential, if the unit was given one by that name.
pub fn credential(name: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("CREDENTIALS_DIRECTORY")?;
    let path = PathBuf::from(dir).join(name);
    path.is_file().then_some(path)
}

/// Whether stderr is connected to the journal, per `JOURNAL_STREAM`.
pub fn stderr_is_journal() -> bool {
    let Some(stream) = std::env::var_os("JOURNAL_STREAM") else {
        return false;
    };
    let Some((dev, ino)) = stream.to_str().and_then(|s| s.split_once(':')) else {
        return false;
    };
    let stat = || -> io::Result<(u64, u64)> {
        let fd = io::stderr().as_fd().try_clone_to_owned()?;
        let meta = std::fs::File::from(fd).metadata()?;
        Ok((meta.dev(), meta.ino()))
    };
    stat().is_ok_and(|(d, i)| dev.parse() == Ok(d) && ino.parse() == Ok(i))
}

/// If systemd enabled the watchdog, pings it at half the timeout, but only
/// while the database still answers within that time. A deadlocked or
/// wedged server therefore stops pinging and systemd restarts it.
pub fn spawn_watchdog(state: Arc<AppState>) {
    let Some(timeout) = sd_notify::watchdog_enabled() else {
        return;
    };
    let interval = timeout / 2;
    debug!("watchdog enabled, pinging every {interval:?}");
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if healthy(&state, interval).await {
                notify(&[NotifyState::Watchdog]);
            } else {
                warn!("database did not answer within {interval:?}; skipping watchdog ping");
            }
        }
    });
}

async fn healthy(state: &Arc<AppState>, within: Duration) -> bool {
    let state = state.clone();
    let check = tokio::task::spawn_blocking(move || state.db().ping());
    matches!(tokio::time::timeout(within, check).await, Ok(Ok(Ok(()))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    #[tokio::test]
    async fn watchdog_health_needs_a_responsive_database() {
        let state = Arc::new(AppState::new(Db::open_in_memory().unwrap(), None, 10).unwrap());
        assert!(healthy(&state, Duration::from_secs(1)).await);

        // Simulate a wedged server: something holds the database lock.
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let holder = {
            let state = state.clone();
            std::thread::spawn(move || {
                let _guard = state.db();
                held_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
        };
        held_rx.recv().unwrap();
        assert!(!healthy(&state, Duration::from_millis(200)).await);
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        assert!(healthy(&state, Duration::from_secs(1)).await);
    }

    // These read process-wide environment variables, so they run in one test.
    #[test]
    fn follows_the_systemd_environment() {
        let _env = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: tests that read these hold ENV_LOCK too.
        unsafe {
            for var in [
                "LISTEN_PID",
                "LISTEN_FDS",
                "CREDENTIALS_DIRECTORY",
                "JOURNAL_STREAM",
                "NOTIFY_SOCKET",
            ] {
                std::env::remove_var(var);
            }
        }
        assert!(listener().unwrap().is_none());
        assert!(credential(TOKEN_CREDENTIAL).is_none());
        assert!(!stderr_is_journal());
        ready("no-op");

        let dir = std::env::temp_dir().join(format!("pressboxd-creds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("token"), "t\n").unwrap();
        // SAFETY: as above.
        unsafe {
            std::env::set_var("CREDENTIALS_DIRECTORY", &dir);
            std::env::set_var("JOURNAL_STREAM", "1:2");
        }
        assert_eq!(credential(TOKEN_CREDENTIAL), Some(dir.join("token")));
        assert!(credential("missing").is_none());
        assert!(!stderr_is_journal(), "1:2 is not our stderr");
        // SAFETY: as above.
        unsafe {
            std::env::remove_var("CREDENTIALS_DIRECTORY");
            std::env::remove_var("JOURNAL_STREAM");
        }
        std::fs::remove_dir_all(dir).unwrap();

        under_systemd();
    }

    /// The other half of the test above, kept apart for reading. Speaks
    /// systemd's side of the protocol through the same variables.
    fn under_systemd() {
        use std::os::unix::net::UnixDatagram;

        let dir = std::env::temp_dir().join(format!("pressboxd-notify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = UnixDatagram::bind(dir.join("notify")).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let stderr = std::fs::File::from(io::stderr().as_fd().try_clone_to_owned().unwrap())
            .metadata()
            .unwrap();
        // SAFETY: as above.
        unsafe {
            std::env::set_var("NOTIFY_SOCKET", dir.join("notify"));
            std::env::set_var(
                "JOURNAL_STREAM",
                format!("{}:{}", stderr.dev(), stderr.ino()),
            );
            std::env::set_var("RUNTIME_DIRECTORY", &dir);
            std::env::set_var("WATCHDOG_USEC", "100000");
            std::env::set_var("WATCHDOG_PID", std::process::id().to_string());
        }
        let receive = |socket: &UnixDatagram| {
            let mut buf = [0; 256];
            let n = socket.recv(&mut buf).unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        };
        let next = || receive(&socket);

        assert!(stderr_is_journal(), "stderr is the stream systemd named");
        assert_eq!(
            crate::config::Config::parse("").unwrap().control_socket(),
            dir.join("control.sock"),
            "the socket goes in RuntimeDirectory="
        );
        ready("listening on 127.0.0.1:8470");
        assert_eq!(next(), "READY=1\nSTATUS=listening on 127.0.0.1:8470\n");
        status("3 feeds");
        assert_eq!(next(), "STATUS=3 feeds\n");
        stopping();
        assert_eq!(next(), "STOPPING=1\nSTATUS=shutting down\n");

        // The watchdog pings at half of WatchdogSec= while the database
        // answers.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let state = Arc::new(AppState::new(Db::open_in_memory().unwrap(), None, 10).unwrap());
            spawn_watchdog(state);
            let listener = socket.try_clone().unwrap();
            let ping = tokio::task::spawn_blocking(move || receive(&listener));
            assert_eq!(ping.await.unwrap(), "WATCHDOG=1\n");
        });
        drop(runtime);

        // A socket nobody listens on is logged, not fatal.
        drop(socket);
        status("nobody hears this");

        // SAFETY: as above.
        unsafe {
            for var in [
                "NOTIFY_SOCKET",
                "JOURNAL_STREAM",
                "RUNTIME_DIRECTORY",
                "WATCHDOG_USEC",
                "WATCHDOG_PID",
            ] {
                std::env::remove_var(var);
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
