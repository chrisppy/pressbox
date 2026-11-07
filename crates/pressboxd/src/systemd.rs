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
    fn inactive_without_systemd() {
        // SAFETY: no other test in this crate touches these variables.
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
    }
}
