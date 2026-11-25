mod auth;
mod game;
mod ranking;

use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, DefaultBodyLimit, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use auth::{ApiError, AuthResult, Sub2Api};
use game::{ACCOUNT_SECONDS, Challenge, Store};
use ranking::{Leaderboard, Standings};

const COOKIE: &str = "__Host-yijian-account";
const EXPIRED: ApiError = ApiError(StatusCode::UNAUTHORIZED, "连接已过期，请刷新后重试。");

#[derive(Clone)]
struct App {
    store: Arc<Mutex<Store>>,
    auth: Sub2Api,
    leaderboard: Leaderboard,
}

fn cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|value| {
            let (name, value) = value.trim().split_once('=')?;
            (name == COOKIE).then_some(value)
        })
}

fn player_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn account_response(token: &str, seconds: u64) -> Response {
    let mut response = Json(json!({"ok": true})).into_response();
    let cookie =
        format!("{COOKIE}={token}; Path=/; Max-Age={seconds}; HttpOnly; Secure; SameSite=Lax");
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("valid cookie"),
    );
    response
}

async fn request_headers(request: Request, next: Next) -> Response {
    let api = request.uri().path().starts_with("/api/");
    if api
        && request
            .headers()
            .get("sec-fetch-site")
            .is_some_and(|value| value == "cross-site")
    {
        return ApiError(StatusCode::FORBIDDEN, "请直接打开游戏页面。").into_response();
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    if api && !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

async fn session(
    State(app): State<App>,
    headers: HeaderMap,
    Json(data): Json<Value>,
) -> Json<Value> {
    let token = app
        .store
        .lock()
        .unwrap()
        .session(data["token"].as_str(), cookie(&headers));
    Json(json!({ "token": token }))
}

async fn leaderboard(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<Standings>, ApiError> {
    let viewer = app.store.lock().unwrap().account_id(cookie(&headers));
    app.leaderboard
        .standings(viewer)
        .await
        .map(Json)
        .map_err(|error| {
            eprintln!("leaderboard read failed: {error}");
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "暂时无法读取今日排行榜，请稍后再试。",
            )
        })
}

async fn action(
    State(app): State<App>,
    headers: HeaderMap,
    Json(data): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let token = player_token(&headers).ok_or(EXPIRED)?;
    let mut store = app.store.lock().unwrap();
    if !store.authorized(token, cookie(&headers)) {
        return Err(EXPIRED);
    }
    if !store.online(token) {
        return Err(ApiError(StatusCode::CONFLICT, "正在重新连接棋室，请稍候。"));
    }
    match store.command(token, &data) {
        Ok(room) => store.publish(room.as_deref()),
        Err(message) => {
            if message == "落子时间已用完。" {
                store.publish(None);
            }
            return Err(ApiError(StatusCode::BAD_REQUEST, message));
        }
    }
    Ok(Json(json!({"ok": true})))
}

struct Connection {
    app: App,
    player: String,
    id: u64,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.app
            .store
            .lock()
            .unwrap()
            .disconnect(&self.player, self.id);
    }
}

#[derive(Deserialize)]
struct Subscriber {
    token: Option<String>,
}

async fn events(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<Subscriber>,
) -> Response {
    let token = query.token.unwrap_or_default();
    let connection = {
        let mut store = app.store.lock().unwrap();
        store
            .authorized(&token, cookie(&headers))
            .then(|| store.connect(&token))
    };
    let connection = connection.map(|(id, receiver)| {
        (
            Connection {
                app,
                player: token,
                id,
            },
            receiver,
        )
    });
    let stream = async_stream::stream! {
        if let Some((_connection, mut receiver)) = connection {
            loop {
                let data = receiver.borrow_and_update().clone();
                yield Ok::<_, Infallible>(Event::default().data(data));
                if receiver.changed().await.is_err() { break; }
            }
        } else {
            yield Ok(Event::default().event("expired").data("{}"));
        }
    };
    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

enum SignIn {
    Password(String, String),
    Totp(String),
}

fn can_sign_in(peer: SocketAddr, headers: &HeaderMap) -> bool {
    let local = peer.ip().is_loopback()
        || matches!(peer.ip(), std::net::IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback()));
    let loopback_host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<axum::http::uri::Authority>().ok())
        .is_some_and(|host| matches!(host.host(), "localhost" | "127.0.0.1" | "[::1]"));
    local
        && (loopback_host
            || headers
                .get("x-forwarded-proto")
                .is_some_and(|proto| proto == "https"))
}

async fn login(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(data): Json<Value>,
) -> Result<Response, ApiError> {
    let email = data["email"].as_str().unwrap_or_default().trim();
    let password = data["password"].as_str().unwrap_or_default();
    let valid_email = email
        .split_once('@')
        .is_some_and(|(left, right)| !left.is_empty() && !right.is_empty() && !right.contains('@'));
    if !valid_email
        || email.len() > 254
        || email.chars().any(char::is_whitespace)
        || password.is_empty()
        || password.len() > 4000
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "请输入有效的邮箱和密码。",
        ));
    }
    sign_in(
        app,
        peer,
        headers,
        SignIn::Password(email.into(), password.into()),
    )
    .await
}

async fn totp(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(data): Json<Value>,
) -> Result<Response, ApiError> {
    let code = data["code"]
        .as_str()
        .filter(|value| value.len() == 6 && value.bytes().all(|b| b.is_ascii_digit()))
        .ok_or(ApiError(StatusCode::BAD_REQUEST, "请输入 6 位验证码。"))?;
    sign_in(app, peer, headers, SignIn::Totp(code.into())).await
}

async fn sign_in(
    app: App,
    peer: SocketAddr,
    headers: HeaderMap,
    input: SignIn,
) -> Result<Response, ApiError> {
    if !can_sign_in(peer, &headers) {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "请通过线上 HTTPS 地址登录。",
        ));
    }
    let token = player_token(&headers).ok_or(EXPIRED)?;
    let cookie = cookie(&headers);
    let challenge = {
        let mut store = app.store.lock().unwrap();
        if !store.authorized(token, cookie) || !store.online(token) {
            return Err(EXPIRED);
        }
        let now = store.now();
        let player = store.players.get_mut(token).expect("validated player");
        if player.signing_in {
            return Err(ApiError(StatusCode::CONFLICT, "正在验证，请稍候。"));
        }
        let challenge = match &input {
            SignIn::Password(..) => {
                player.challenge = None;
                None
            }
            SignIn::Totp(_) => {
                let Some(challenge) = player
                    .challenge
                    .as_mut()
                    .filter(|value| now < value.expires && value.attempts < 5)
                else {
                    player.challenge = None;
                    return Err(ApiError(StatusCode::GONE, "二次验证已过期，请重新登录。"));
                };
                challenge.attempts += 1;
                Some(challenge.token.clone())
            }
        };
        player.signing_in = true;
        challenge
    };

    struct SignInGuard<'a> {
        store: &'a Mutex<Store>,
        player: &'a str,
    }
    impl Drop for SignInGuard<'_> {
        fn drop(&mut self) {
            if let Some(player) = self.store.lock().unwrap().players.get_mut(self.player) {
                player.signing_in = false;
            }
        }
    }
    let _guard = SignInGuard {
        store: &app.store,
        player: token,
    };
    let result = match input {
        SignIn::Password(email, password) => app.auth.login(&email, &password).await,
        SignIn::Totp(code) => {
            app.auth
                .totp(challenge.as_deref().expect("validated challenge"), &code)
                .await
        }
    };
    let mut store = app.store.lock().unwrap();
    if !store.authorized(token, cookie) {
        return Err(EXPIRED);
    }
    match result? {
        AuthResult::Challenge(challenge) => {
            let expiry = store.now() + 180_000;
            store
                .players
                .get_mut(token)
                .expect("validated player")
                .challenge = Some(Challenge {
                token: challenge,
                expires: expiry,
                attempts: 0,
            });
            Ok(Json(json!({"requiresTotp": true})).into_response())
        }
        AuthResult::Account(profile) => {
            store
                .players
                .get_mut(token)
                .expect("validated player")
                .challenge = None;
            let account = store.establish_account(token, cookie, profile);
            Ok(account_response(&account, ACCOUNT_SECONDS))
        }
    }
}

async fn logout(State(app): State<App>, headers: HeaderMap) -> Result<Response, ApiError> {
    let token = player_token(&headers).ok_or(EXPIRED)?;
    let mut store = app.store.lock().unwrap();
    let cookie = cookie(&headers);
    if !store.authorized(token, cookie) {
        return Err(EXPIRED);
    }
    let player = store.players.get_mut(token).expect("validated player");
    if player.signing_in {
        return Err(ApiError(StatusCode::CONFLICT, "正在验证，请稍候。"));
    }
    player.challenge = None;
    if cookie.is_some_and(|value| store.revoke_account(value)) {
        store.publish(None);
    }
    Ok(account_response("", 0))
}

fn asset(content_type: &'static str, body: &'static [u8]) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        Body::from(body),
    )
        .into_response()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let address = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "3000".into())
        .parse::<u16>()?;
    let (ranked_results, mut results) = tokio::sync::mpsc::unbounded_channel();
    let app = App {
        store: Arc::new(Mutex::new(Store::new(ranked_results))),
        auth: Sub2Api::new()?,
        leaderboard: Leaderboard::new()?,
    };
    let champion_app = app.clone();
    tokio::spawn(async move {
        let mut refresh = tokio::time::interval(Duration::from_secs(15));
        refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut unavailable = false;
        loop {
            refresh.tick().await;
            match champion_app.leaderboard.yesterday_champion().await {
                Ok(champion) => {
                    champion_app.store.lock().unwrap().set_champion(champion);
                    unavailable = false;
                }
                Err(error) => {
                    if !unavailable {
                        eprintln!("yesterday's champion is unavailable: {error}");
                    }
                    unavailable = true;
                }
            }
        }
    });
    let ranking_writer = app.leaderboard.clone();
    tokio::spawn(async move {
        while let Some(result) = results.recv().await {
            loop {
                match ranking_writer.record(&result).await {
                    Ok(()) => break,
                    Err(error) => {
                        eprintln!("leaderboard write failed: {error}");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        }
    });
    let store = app.store.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            store.lock().unwrap().tick();
        }
    });
    let routes = Router::new()
        .route(
            "/",
            get(|| async {
                asset(
                    "text/html; charset=utf-8",
                    include_bytes!("../public/index.html"),
                )
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                asset(
                    "text/javascript; charset=utf-8",
                    include_bytes!("../public/app.js"),
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async {
                asset(
                    "text/css; charset=utf-8",
                    include_bytes!("../public/style.css"),
                )
            }),
        )
        .route(
            "/chessroom.webp",
            get(|| async { asset("image/webp", include_bytes!("../public/chessroom.webp")) }),
        )
        .route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }))
        .route("/healthz", get(|| async { Json(json!({"ok": true})) }))
        .route("/api/session", post(session))
        .route("/api/leaderboard", get(leaderboard))
        .route("/api/events", get(events))
        .route("/api/action", post(action))
        .route("/api/auth/login", post(login))
        .route("/api/auth/totp", post(totp))
        .route("/api/auth/logout", post(logout))
        .fallback(|| async { ApiError(StatusCode::NOT_FOUND, "页面不存在。") })
        .layer(DefaultBodyLimit::max(8192))
        .layer(middleware::from_fn(request_headers))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind((address.as_str(), port)).await?;
    println!(
        "弈间五子棋 · http://localhost:{}",
        listener.local_addr()?.port()
    );
    axum::serve(
        listener,
        routes.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
