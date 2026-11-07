// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: AGPL-3.0-only

//! Admin API on a Unix socket, for `pressboxctl`. There is no token: access is
//! controlled by the socket's file permissions (the service user and root).
//!
//! | Method | Path                           | Body            | Response           |
//! |--------|--------------------------------|-----------------|--------------------|
//! | GET    | /admin/status                  |                 | `Status`           |
//! | GET    | /admin/users                   |                 | `[UserInfo]`       |
//! | GET    | /admin/devices                 |                 | `[Device]`         |
//! | POST   | /admin/devices                 | `NewDeviceRequest` | `NewDevice`     |
//! | DELETE | /admin/devices/{name}          |                 | 204                |
//! | GET    | /admin/pairings                |                 | `[PendingPairing]` |
//! | POST   | /admin/pairings/{code}/approve | `ApproveRequest`| `Approved`         |
//! | DELETE | /admin/pairings/{code}         |                 | 204                |
//! | POST   | /admin/backup                  | `BackupRequest` | `BackupResult`     |
//! | GET    | /admin/opml?user=NAME          |                 | OPML document      |
//! | POST   | /admin/refresh                 |                 | 202                |

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Path as UrlPath, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{delete, get, post},
};
use pressbox_api::{
    ApproveRequest, Approved, BackupRequest, BackupResult, Device, NewDevice, NewDeviceRequest,
    Status, UserInfo,
};

use crate::{
    AppState,
    api::{self, ApiError, ApiResult, found},
    db::AuthUser,
    fetch::now,
    pairing,
};

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/admin/status", get(status))
        .route("/admin/users", get(users))
        .route("/admin/devices", get(devices).post(add_device))
        .route("/admin/devices/{name}", delete(remove_device))
        .route("/admin/pairings", get(api::pairings))
        .route("/admin/pairings/{code}/approve", post(approve))
        .route("/admin/pairings/{code}", delete(api::deny))
        .route("/admin/backup", post(backup))
        .route("/admin/opml", get(opml))
        .route(
            "/admin/refresh",
            post(|State(state): State<Arc<AppState>>| async move {
                state.request_refresh();
                StatusCode::ACCEPTED
            }),
        )
        .with_state(state)
}

/// Binds the control socket, replacing a stale one, readable only by us.
pub fn bind(path: &Path) -> Result<tokio::net::UnixListener> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(e).with_context(|| format!("removing stale {}", path.display()));
        }
        _ => {}
    }
    let listener = tokio::net::UnixListener::bind(path)
        .with_context(|| format!("binding control socket {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

async fn status(State(state): State<Arc<AppState>>) -> ApiResult<Json<Status>> {
    let counts = state.reader().counts()?;
    Ok(Json(Status {
        feeds: counts.feeds,
        archived_feeds: counts.archived_feeds,
        failing_feeds: counts.failing_feeds,
        items: counts.items,
        users: counts.users,
        devices: counts.devices,
        pending_pairings: u32::try_from(state.pairings.count(now())).unwrap_or(u32::MAX),
    }))
}

async fn users(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<UserInfo>>> {
    Ok(Json(state.reader().users()?))
}

async fn devices(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<Device>>> {
    Ok(Json(state.reader().devices()?))
}

/// The named user, or the only user when none is named.
fn pick_user(state: &AppState, name: Option<&str>) -> ApiResult<AuthUser> {
    let db = state.db();
    match name {
        Some(name) => db.user(name)?.ok_or(ApiError::BadRequest("no such user")),
        None => db.only_user()?.ok_or(ApiError::BadRequest(
            "there is more than one user; say which with --user",
        )),
    }
}

async fn add_device(
    State(state): State<Arc<AppState>>,
    Json(req): Json<NewDeviceRequest>,
) -> ApiResult<Json<NewDevice>> {
    let name = pairing::device_name(&req.name).ok_or(ApiError::BadRequest(
        "name must be 1-40 letters, digits, '-', '_' or '.'",
    ))?;
    let user = pick_user(&state, req.user.as_deref())?;
    Ok(Json(pairing::create_device(
        &state.db(),
        &user,
        &name,
        now(),
    )?))
}

async fn approve(
    State(state): State<Arc<AppState>>,
    UrlPath(code): UrlPath<String>,
    body: Option<Json<ApproveRequest>>,
) -> ApiResult<Json<Approved>> {
    let wanted = body.and_then(|Json(b)| b.user);
    let user = pick_user(&state, wanted.as_deref())?;
    api::approve_for(&state, &code, &user)
}

async fn remove_device(
    State(state): State<Arc<AppState>>,
    UrlPath(name): UrlPath<String>,
) -> ApiResult<StatusCode> {
    let removed = state.db().remove_device(&name)?;
    if removed {
        tracing::info!("revoked device {name:?}");
    }
    found(removed)
}

async fn backup(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BackupRequest>,
) -> ApiResult<Json<BackupResult>> {
    let dir = state.backup_dir.clone();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    // Milliseconds keep names unique; they still sort by time.
    let name = format!(
        "pressbox-{}.db",
        chrono::Local::now().format("%Y%m%dT%H%M%S%.3f")
    );
    let path = dir.join(name);
    state.db().backup_to(&path)?;
    let removed = prune_backups(&dir, req.keep.max(1))?;
    tracing::info!("backed up to {}", path.display());
    Ok(Json(BackupResult {
        path: path.display().to_string(),
        removed: removed.iter().map(|p| p.display().to_string()).collect(),
    }))
}

/// Deletes all but the newest `keep` backups in `dir`.
fn prune_backups(dir: &Path, keep: u32) -> Result<Vec<PathBuf>> {
    let mut backups: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("pressbox-") && n.ends_with(".db"))
        })
        .collect();
    // The names sort by time.
    backups.sort_unstable_by(|a, b| b.cmp(a));
    let old = backups.split_off(backups.len().min(keep as usize));
    for path in &old {
        std::fs::remove_file(path)?;
    }
    Ok(old)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(serde::Deserialize)]
struct OpmlQuery {
    user: Option<String>,
}

async fn opml(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<OpmlQuery>,
) -> ApiResult<impl IntoResponse> {
    let user = pick_user(&state, query.user.as_deref())?;
    let feeds = state.reader().feeds(user.id)?;
    let mut doc = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"2.0\">\n  <head><title>pressbox feeds</title></head>\n  <body>\n",
    );
    for feed in feeds.iter().filter(|f| !f.archived) {
        let mut attrs = format!(
            "text=\"{t}\" title=\"{t}\" type=\"rss\" xmlUrl=\"{u}\"",
            t = xml_escape(&feed.title),
            u = xml_escape(&feed.url)
        );
        if let Some(site) = &feed.site_url {
            attrs.push_str(&format!(" htmlUrl=\"{}\"", xml_escape(site)));
        }
        if !feed.tags.is_empty() {
            attrs.push_str(&format!(
                " category=\"{}\"",
                xml_escape(&feed.tags.join(","))
            ));
        }
        doc.push_str(&format!("    <outline {attrs}/>\n"));
    }
    doc.push_str("  </body>\n</opml>\n");
    Ok(([(header::CONTENT_TYPE, "text/x-opml; charset=utf-8")], doc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prunes_oldest_backups() {
        let dir = std::env::temp_dir().join(format!("pressboxd-backups-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for stamp in ["20260101T000000", "20260102T000000", "20260103T000000"] {
            std::fs::write(dir.join(format!("pressbox-{stamp}.db")), "").unwrap();
        }
        std::fs::write(dir.join("unrelated.txt"), "").unwrap();
        let removed = prune_backups(&dir, 2).unwrap();
        assert_eq!(removed, [dir.join("pressbox-20260101T000000.db")]);
        assert!(dir.join("unrelated.txt").exists());
        assert!(prune_backups(&dir, 5).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
