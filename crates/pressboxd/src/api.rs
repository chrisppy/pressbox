// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: AGPL-3.0-only

//! The HTTP API used by every client.
//!
//! | Method | Path                          | Body / query     | Response           |
//! |--------|-------------------------------|------------------|--------------------|
//! | GET    | /health                       |                  | `ok`               |
//! | POST   | /api/v1/pair                         | `PairRequest`    | `PairStart`        |
//! | GET    | /api/v1/pair/{poll_id}               |                  | 202, `PairApproved`, 404 |
//! | GET    | /api/v1/feeds                    |                  | `[Feed]`           |
//! | GET    | /api/v1/items                    | `ItemQuery`      | `ItemPage`         |
//! | GET    | /api/v1/items/{id}               |                  | `Item`             |
//! | PUT    | /api/v1/items/{id}/read          | `Flag`           | 204                |
//! | PUT    | /api/v1/items/{id}/starred       | `Flag`           | 204                |
//! | POST   | /api/v1/feeds/{id}/read-all      |                  | `Updated`          |
//! | POST   | /api/v1/refresh                  |                  | 202                |
//! | GET    | /api/v1/me                       |                  | `Me`               |
//! | GET    | /api/v1/pairings                 |                  | `[PendingPairing]` |
//! | POST   | /api/v1/pairings/{code}/approve  | `ApproveRequest` | `Approved`         |
//! | DELETE | /api/v1/pairings/{code}          |                  | 204                |
//!
//! Every route except `/health` and `/api/v1/pair*` needs
//! `Authorization: Bearer <token>`, where the token
//! is a paired device's token or the static `token_file` token. A token
//! belongs to one user, and every answer is about that user's feeds and
//! state. Admins may approve a pairing for another user.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, Path, Query, Request, State},
    http::{StatusCode, header::AUTHORIZATION},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use pressbox_api::{
    ApproveRequest, Approved, Feed, Flag, Item, ItemPage, ItemQuery, Me, PairRequest, PairStart,
    PendingPairing, Updated, Version,
};
use serde_json::json;
use tower_http::compression::CompressionLayer;

use crate::{
    AppState,
    db::{AuthUser, BadCursor},
    fetch::now,
    pairing::{PollState, StartError, hash_token},
};

pub fn router(state: Arc<AppState>) -> Router {
    let authenticated = Router::new()
        .route("/feeds", get(feeds))
        .route("/feeds/{id}/read-all", post(read_all))
        .route("/items", get(items))
        .route("/items/{id}", get(item))
        .route("/items/{id}/read", put(set_read))
        .route("/items/{id}/starred", put(set_starred))
        .route("/refresh", post(refresh))
        .route("/me", get(me))
        .route("/pairings", get(pairings))
        .route("/pairings/{code}/approve", post(approve))
        .route("/pairings/{code}", delete(deny))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth));
    // Pairing is how a client gets a token, so it needs none.
    let v1 = Router::new()
        .route("/pair", post(pair_start))
        .route("/pair/{poll_id}", get(pair_poll))
        .route("/version", get(version))
        .merge(authenticated);
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .nest("/api/v1", v1)
        // Item HTML and long lists compress several times over; clients
        // that don't ask for it get plain responses.
        .layer(CompressionLayer::new())
        .with_state(state)
}

/// Public, so a client can check what it talks to before pairing.
async fn version() -> Json<Version> {
    Json(Version {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        api: pressbox_api::API_VERSION,
    })
}

pub enum ApiError {
    NotFound,
    Unauthorized,
    Forbidden(&'static str),
    BadRequest(&'static str),
    TooManyRequests,
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self::Internal(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "not found"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            Self::TooManyRequests => (StatusCode::TOO_MANY_REQUESTS, "too many pending pairings"),
            Self::Internal(e) => {
                tracing::error!("{e:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// Finds the user a bearer token belongs to and attaches it to the request.
async fn auth(State(state): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
    let given = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    let Some(given) = given.filter(|g| !g.is_empty()) else {
        return ApiError::Unauthorized.into_response();
    };
    let static_ok = state
        .token
        .as_ref()
        .is_some_and(|t| constant_time_eq(given.as_bytes(), t.as_bytes()));
    // Device tokens are looked up by hash, so timing reveals nothing useful.
    // Looked up on the read pool; the writer is only needed, at most once a
    // minute per device, to record when it was last seen.
    let user = if static_ok {
        state.read(|db| db.static_token_user()).await
    } else {
        let hash = hash_token(given);
        let lookup = hash.clone();
        let found = state.read(move |db| db.find_device(&lookup, now())).await;
        match found {
            Ok(Some((user, stale))) => {
                if stale && let Err(e) = state.write(move |db| db.touch_device(&hash, now())).await
                {
                    tracing::warn!("recording last_seen: {e:#}");
                }
                Ok(Some(user))
            }
            other => other.map(|_| None),
        }
    };
    match user {
        Ok(Some(user)) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Ok(None) => ApiError::Unauthorized.into_response(),
        Err(e) => ApiError::Internal(e).into_response(),
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

type User = Extension<AuthUser>;

async fn me(Extension(user): User) -> Json<Me> {
    Json(Me {
        user: user.name,
        admin: user.admin,
        device: user.device,
    })
}

async fn feeds(
    State(state): State<Arc<AppState>>,
    Extension(user): User,
) -> ApiResult<Json<Vec<Feed>>> {
    Ok(Json(state.read(move |db| db.feeds(user.id)).await?))
}

async fn items(
    State(state): State<Arc<AppState>>,
    Extension(user): User,
    Query(query): Query<ItemQuery>,
) -> ApiResult<Json<ItemPage>> {
    match state.read(move |db| db.items(user.id, &query)).await {
        Ok(page) => Ok(Json(page)),
        Err(e) if e.is::<BadCursor>() => Err(ApiError::BadRequest("invalid cursor")),
        Err(e) => Err(e.into()),
    }
}

async fn item(
    State(state): State<Arc<AppState>>,
    Extension(user): User,
    Path(id): Path<i64>,
) -> ApiResult<Json<Item>> {
    state
        .read(move |db| db.item(user.id, id))
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

async fn set_read(
    State(state): State<Arc<AppState>>,
    Extension(user): User,
    Path(id): Path<i64>,
    Json(flag): Json<Flag>,
) -> ApiResult<StatusCode> {
    found(
        state
            .write(move |db| db.set_read(user.id, id, flag.value))
            .await?,
    )
}

async fn set_starred(
    State(state): State<Arc<AppState>>,
    Extension(user): User,
    Path(id): Path<i64>,
    Json(flag): Json<Flag>,
) -> ApiResult<StatusCode> {
    found(
        state
            .write(move |db| db.set_starred(user.id, id, flag.value))
            .await?,
    )
}

pub fn found(changed: bool) -> ApiResult<StatusCode> {
    if changed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

async fn read_all(
    State(state): State<Arc<AppState>>,
    Extension(user): User,
    Path(id): Path<String>,
) -> ApiResult<Json<Updated>> {
    let updated = state
        .write(move |db| db.mark_feed_read(user.id, &id))
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(Updated { updated }))
}

async fn refresh(State(state): State<Arc<AppState>>) -> StatusCode {
    state.request_refresh().await;
    StatusCode::ACCEPTED
}

/// The address a request came from: the peer's, or, behind a trusted
/// reverse proxy, the last address in `X-Forwarded-For` (the one the proxy
/// itself added; earlier entries could be forged by the client).
fn client_address(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    peer: Option<SocketAddr>,
) -> String {
    if state.behind_proxy
        && let Some(forwarded) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    {
        return forwarded.to_owned();
    }
    peer.map_or_else(|| "unknown".to_owned(), |p| p.ip().to_string())
}

async fn pair_start(
    State(state): State<Arc<AppState>>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<PairRequest>,
) -> ApiResult<(StatusCode, Json<PairStart>)> {
    let client = client_address(&state, &headers, peer.map(|Extension(ConnectInfo(p))| p));
    match state.pairings.start(&req.name, &client, now()) {
        Ok(start) => {
            tracing::info!(
                code = start.code,
                client,
                "pairing requested for {:?}",
                req.name
            );
            Ok((StatusCode::CREATED, Json(start)))
        }
        Err(StartError::InvalidName) => Err(ApiError::BadRequest(
            "name must be 1-40 letters, digits, '-', '_' or '.'",
        )),
        Err(StartError::TooMany | StartError::TooManyFromClient) => Err(ApiError::TooManyRequests),
    }
}

async fn pair_poll(State(state): State<Arc<AppState>>, Path(poll_id): Path<String>) -> Response {
    match state.pairings.poll(&poll_id, now()) {
        PollState::Pending => {
            (StatusCode::ACCEPTED, Json(json!({ "status": "pending" }))).into_response()
        }
        PollState::Approved(approved) => Json(approved).into_response(),
        PollState::Gone => ApiError::NotFound.into_response(),
    }
}

pub async fn pairings(State(state): State<Arc<AppState>>) -> Json<Vec<PendingPairing>> {
    Json(state.pairings.list(now()))
}

async fn approve(
    State(state): State<Arc<AppState>>,
    Extension(approver): User,
    Path(code): Path<String>,
    body: Option<Json<ApproveRequest>>,
) -> ApiResult<Json<Approved>> {
    let wanted = body.and_then(|Json(b)| b.user);
    let target = match wanted {
        Some(name) if name != approver.name => {
            if !approver.admin {
                return Err(ApiError::Forbidden(
                    "only admins can approve devices for other users",
                ));
            }
            state
                .read(move |db| db.user(&name))
                .await?
                .ok_or(ApiError::BadRequest("no such user"))?
        }
        _ => approver,
    };
    approve_for(&state, code, target).await
}

/// Approves a pairing code for `user` (shared with the control socket).
pub async fn approve_for(
    state: &Arc<AppState>,
    code: String,
    user: AuthUser,
) -> ApiResult<Json<Approved>> {
    let pairings = state.clone();
    let approved = state
        .write(move |db| pairings.pairings.approve(&code, db, &user, now()))
        .await?
        .ok_or(ApiError::NotFound)?;
    tracing::info!("paired device {:?} for {}", approved.name, approved.user);
    Ok(Json(approved))
}

pub async fn deny(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
) -> ApiResult<StatusCode> {
    found(state.pairings.deny(&code, now()))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use pressbox_api::PairApproved;
    use tower::ServiceExt;

    use super::*;
    use crate::{
        config::Config,
        db::{Db, NewItem},
    };

    const T: Option<&str> = Some("static-token");

    fn state() -> Arc<AppState> {
        let mut db = Db::open_in_memory().unwrap();
        db.sync(&Config::parse("[feeds.a]\nurl = \"https://a.example/\"").unwrap())
            .unwrap();
        db.upsert_items(
            "https://a.example/",
            0,
            &[NewItem {
                guid: "1".into(),
                title: "One".into(),
                url: None,
                author: None,
                summary: None,
                content: Some("full".into()),
                published: Some(1),
            }],
        )
        .unwrap();
        Arc::new(AppState::new(db, T.map(str::to_owned), 500).unwrap())
    }

    async fn call(
        state: &Arc<AppState>,
        method: &str,
        uri: &str,
        body: Option<&str>,
        token: Option<&str>,
    ) -> (StatusCode, String) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header(AUTHORIZATION, format!("Bearer {t}"));
        }
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        let req = req
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_owned())))
            .unwrap();
        let resp = router(state.clone()).oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn read_flow() {
        let s = state();
        let (status, body) = call(&s, "GET", "/api/v1/feeds", None, T).await;
        assert_eq!(status, StatusCode::OK);
        let feeds: Vec<Feed> = serde_json::from_str(&body).unwrap();
        assert_eq!(feeds[0].unread, 1);
        assert!(!feeds[0].archived);

        let (_, body) = call(&s, "GET", "/api/v1/items?unread=true&feed=a", None, T).await;
        let page: ItemPage = serde_json::from_str(&body).unwrap();
        let id = page.items[0].id;
        assert!(!body.contains("full"), "lists leave out content");

        let (_, body) = call(&s, "GET", "/api/v1/items?q=one", None, T).await;
        assert_eq!(
            serde_json::from_str::<ItemPage>(&body).unwrap().items.len(),
            1
        );
        let (_, body) = call(&s, "GET", "/api/v1/items?q=nothing", None, T).await;
        assert!(
            serde_json::from_str::<ItemPage>(&body)
                .unwrap()
                .items
                .is_empty()
        );

        let (_, body) = call(&s, "GET", &format!("/api/v1/items/{id}"), None, T).await;
        assert_eq!(
            serde_json::from_str::<Item>(&body)
                .unwrap()
                .content
                .as_deref(),
            Some("full")
        );

        let uri = format!("/api/v1/items/{id}/read");
        let (status, _) = call(&s, "PUT", &uri, Some(r#"{"value":true}"#), T).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = call(&s, "GET", "/api/v1/items?unread=true", None, T).await;
        assert!(
            serde_json::from_str::<ItemPage>(&body)
                .unwrap()
                .items
                .is_empty()
        );

        let uri = format!("/api/v1/items/{id}/starred");
        let (status, _) = call(&s, "PUT", &uri, Some(r#"{"value":true}"#), T).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, body) = call(&s, "POST", "/api/v1/feeds/a/read-all", None, T).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::OK, r#"{"updated":0}"#)
        );
        let (status, _) = call(&s, "POST", "/api/v1/refresh", None, T).await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn responses_are_compressed_when_asked() {
        let s = state();
        let get = |encoding: Option<&str>| {
            let mut req =
                Request::get("/api/v1/items/1").header(AUTHORIZATION, "Bearer static-token");
            if let Some(e) = encoding {
                req = req.header("accept-encoding", e);
            }
            router(s.clone()).oneshot(req.body(Body::empty()).unwrap())
        };
        let resp = get(Some("gzip")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["content-encoding"], "gzip");
        let resp = get(Some("br")).await.unwrap();
        assert_eq!(resp.headers()["content-encoding"], "br");
        let resp = get(None).await.unwrap();
        assert!(resp.headers().get("content-encoding").is_none());
    }

    #[tokio::test]
    async fn a_bad_cursor_is_400() {
        let s = state();
        let (status, body) = call(&s, "GET", "/api/v1/items?cursor=nope", None, T).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let (status, body) = call(&s, "GET", "/api/v1/items?limit=1", None, T).await;
        assert_eq!(status, StatusCode::OK);
        let page: ItemPage = serde_json::from_str(&body).unwrap();
        assert_eq!(page.next_cursor, None, "one item, one page");
    }

    #[tokio::test]
    async fn missing_things_are_404() {
        let s = state();
        for (method, uri, body) in [
            ("GET", "/api/v1/items/99", None),
            ("PUT", "/api/v1/items/99/read", Some(r#"{"value":true}"#)),
            ("POST", "/api/v1/feeds/nope/read-all", None),
            ("POST", "/api/v1/pairings/AAA-AAA/approve", None),
            ("DELETE", "/api/v1/pairings/AAA-AAA", None),
        ] {
            let (status, _) = call(&s, method, uri, body, T).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
        }
        assert_eq!(
            call(&s, "GET", "/api/v1/pair/nope", None, None).await.0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn a_token_is_always_required() {
        let s = state();
        for token in [None, Some("wrong"), Some("")] {
            assert_eq!(
                call(&s, "GET", "/api/v1/feeds", None, token).await.0,
                StatusCode::UNAUTHORIZED,
                "{token:?}"
            );
        }
        assert_eq!(
            call(&s, "GET", "/health", None, None).await.0,
            StatusCode::OK
        );

        // Without a static token, only paired devices get in.
        let mut db = Db::open_in_memory().unwrap();
        db.sync(&Config::parse("").unwrap()).unwrap();
        let open = Arc::new(AppState::new(db, None, 500).unwrap());
        assert_eq!(
            call(&open, "GET", "/api/v1/feeds", None, None).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn pairing_a_device() {
        let s = state();
        let (status, body) = call(
            &s,
            "POST",
            "/api/v1/pair",
            Some(r#"{"name":"new phone"}"#),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let start: PairStart = serde_json::from_str(&body).unwrap();
        let poll = format!("/api/v1/pair/{}", start.poll_id);
        assert_eq!(
            call(&s, "GET", &poll, None, None).await.0,
            StatusCode::ACCEPTED
        );

        // Listing and approving need a token; anyone can't approve themselves.
        assert_eq!(
            call(&s, "GET", "/api/v1/pairings", None, None).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (_, body) = call(&s, "GET", "/api/v1/pairings", None, T).await;
        let pending: Vec<PendingPairing> = serde_json::from_str(&body).unwrap();
        assert_eq!(
            (pending[0].code.as_str(), pending[0].name.as_str()),
            (start.code.as_str(), "new-phone")
        );
        let approve = format!("/api/v1/pairings/{}/approve", start.code);
        let (status, body) = call(&s, "POST", &approve, None, T).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::OK, r#"{"name":"new-phone","user":"default"}"#)
        );

        // The new device collects its token once and can use the API with it.
        let (status, body) = call(&s, "GET", &poll, None, None).await;
        assert_eq!(status, StatusCode::OK);
        let approved: PairApproved = serde_json::from_str(&body).unwrap();
        assert_eq!(
            call(&s, "GET", &poll, None, None).await.0,
            StatusCode::NOT_FOUND
        );
        let device = Some(approved.token.as_str());
        assert_eq!(
            call(&s, "GET", "/api/v1/feeds", None, device).await.0,
            StatusCode::OK
        );

        // A paired device can deny other requests.
        let (_, body) = call(
            &s,
            "POST",
            "/api/v1/pair",
            Some(r#"{"name":"intruder"}"#),
            None,
        )
        .await;
        let other: PairStart = serde_json::from_str(&body).unwrap();
        let deny = format!("/api/v1/pairings/{}", other.code);
        assert_eq!(
            call(&s, "DELETE", &deny, None, device).await.0,
            StatusCode::NO_CONTENT
        );
        let poll = format!("/api/v1/pair/{}", other.poll_id);
        assert_eq!(
            call(&s, "GET", &poll, None, None).await.0,
            StatusCode::NOT_FOUND
        );

        // Revoking the device locks it out.
        assert!(s.db().remove_device("new-phone").unwrap());
        assert_eq!(
            call(&s, "GET", "/api/v1/feeds", None, device).await.0,
            StatusCode::UNAUTHORIZED
        );

        let (status, _) = call(&s, "POST", "/api/v1/pair", Some(r#"{"name":"a/b"}"#), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// chris (admin) and anna, each with a paired device.
    async fn household() -> (Arc<AppState>, String, String) {
        let mut db = Db::open_in_memory().unwrap();
        db.sync(
            &Config::parse(
                r#"
                [feeds.shared]
                url = "https://shared.example/"
                [users.chris]
                admin = true
                [users.chris.feeds.mine]
                url = "https://chris.example/"
                [users.anna]
                "#,
            )
            .unwrap(),
        )
        .unwrap();
        let one = |guid: &str| NewItem {
            guid: guid.into(),
            title: guid.into(),
            url: None,
            author: None,
            summary: None,
            content: None,
            published: Some(1),
        };
        db.upsert_items("https://shared.example/", 0, &[one("shared-item")])
            .unwrap();
        db.upsert_items("https://chris.example/", 0, &[one("chris-item")])
            .unwrap();
        let chris = db.user("chris").unwrap().unwrap();
        let anna = db.user("anna").unwrap().unwrap();
        let chris_token = crate::pairing::create_device(&db, &chris, "laptop", 0)
            .unwrap()
            .token;
        let anna_token = crate::pairing::create_device(&db, &anna, "phone", 0)
            .unwrap()
            .token;
        (
            Arc::new(AppState::new(db, None, 500).unwrap()),
            chris_token,
            anna_token,
        )
    }

    #[tokio::test]
    async fn users_are_isolated_over_http() {
        let (s, chris, anna) = household().await;
        let (_, body) = call(&s, "GET", "/api/v1/me", None, Some(&anna)).await;
        let me: Me = serde_json::from_str(&body).unwrap();
        assert_eq!(
            (me.user.as_str(), me.admin, me.device.as_deref()),
            ("anna", false, Some("phone"))
        );

        let items = |token: String| {
            let s = s.clone();
            async move {
                let (_, body) = call(&s, "GET", "/api/v1/items", None, Some(&token)).await;
                serde_json::from_str::<ItemPage>(&body).unwrap().items
            }
        };
        let chris_items = items(chris.clone()).await;
        let anna_items = items(anna.clone()).await;
        assert_eq!(chris_items.len(), 2);
        assert_eq!(
            anna_items
                .iter()
                .map(|i| i.title.as_str())
                .collect::<Vec<_>>(),
            ["shared-item"]
        );

        // anna cannot reach chris's private item, and her reads are hers.
        let private = chris_items
            .iter()
            .find(|i| i.title == "chris-item")
            .unwrap()
            .id;
        let uri = format!("/api/v1/items/{private}");
        assert_eq!(
            call(&s, "GET", &uri, None, Some(&anna)).await.0,
            StatusCode::NOT_FOUND
        );
        let shared = anna_items[0].id;
        let uri = format!("/api/v1/items/{shared}/read");
        call(&s, "PUT", &uri, Some(r#"{"value":true}"#), Some(&anna)).await;
        assert!(items(anna.clone()).await[0].read);
        assert!(
            !items(chris.clone())
                .await
                .iter()
                .find(|i| i.id == shared)
                .unwrap()
                .read
        );
    }

    #[tokio::test]
    async fn only_admins_approve_for_others() {
        let (s, chris, anna) = household().await;
        let pair = |name: &'static str| {
            let s = s.clone();
            async move {
                let body = format!(r#"{{"name":"{name}"}}"#);
                let (_, body) = call(&s, "POST", "/api/v1/pair", Some(&body), None).await;
                serde_json::from_str::<PairStart>(&body).unwrap()
            }
        };

        // anna may approve for herself, but not for chris.
        let p = pair("tablet").await;
        let uri = format!("/api/v1/pairings/{}/approve", p.code);
        let (status, _) = call(&s, "POST", &uri, Some(r#"{"user":"chris"}"#), Some(&anna)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, body) = call(&s, "POST", &uri, None, Some(&anna)).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::OK, r#"{"name":"tablet","user":"anna"}"#)
        );

        // chris, an admin, may approve for anna.
        let p = pair("kindle").await;
        let uri = format!("/api/v1/pairings/{}/approve", p.code);
        let (status, _) = call(&s, "POST", &uri, Some(r#"{"user":"nobody"}"#), Some(&chris)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, body) = call(&s, "POST", &uri, Some(r#"{"user":"anna"}"#), Some(&chris)).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::OK, r#"{"name":"kindle","user":"anna"}"#)
        );
        let (_, body) = call(
            &s,
            "GET",
            &format!("/api/v1/pair/{}", p.poll_id),
            None,
            None,
        )
        .await;
        let token = serde_json::from_str::<PairApproved>(&body).unwrap().token;
        let (_, body) = call(&s, "GET", "/api/v1/me", None, Some(&token)).await;
        assert!(body.contains(r#""user":"anna""#), "{body}");
    }

    #[tokio::test]
    async fn pairing_is_limited_per_client() {
        let pair = |s: &Arc<AppState>, forwarded: &'static str| {
            let s = s.clone();
            async move {
                let req = Request::post("/api/v1/pair")
                    .header("content-type", "application/json")
                    .header("x-forwarded-for", forwarded)
                    .body(Body::from(r#"{"name":"d"}"#))
                    .unwrap();
                router(s).oneshot(req).await.unwrap().status()
            }
        };
        // Behind a proxy, the address the proxy added counts; a forged
        // earlier entry does not help.
        let mut proxied = AppState::new(Db::open_in_memory().unwrap(), None, 500).unwrap();
        proxied.behind_proxy = true;
        let s = Arc::new(proxied);
        for _ in 0..crate::pairing::MAX_PER_CLIENT {
            assert_eq!(pair(&s, "1.2.3.4").await, StatusCode::CREATED);
        }
        assert_eq!(
            pair(&s, "9.9.9.9, 1.2.3.4").await,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(pair(&s, "5.6.7.8").await, StatusCode::CREATED);

        // Not behind a proxy, the header is ignored: it would be trivial to
        // forge, so every request counts as the same (peer) client here.
        let s = Arc::new(AppState::new(Db::open_in_memory().unwrap(), None, 500).unwrap());
        for _ in 0..crate::pairing::MAX_PER_CLIENT {
            assert_eq!(pair(&s, "1.1.1.1").await, StatusCode::CREATED);
        }
        assert_eq!(pair(&s, "2.2.2.2").await, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn version_needs_no_token() {
        let s = Arc::new(AppState::new(Db::open_in_memory().unwrap(), None, 500).unwrap());
        let resp = router(s)
            .oneshot(Request::get("/api/v1/version").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: Version = serde_json::from_slice(&body).unwrap();
        assert_eq!(v.api, 1);
        assert_eq!(v.version, env!("CARGO_PKG_VERSION"));
    }
}
