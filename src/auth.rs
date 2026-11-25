use std::time::Duration;

use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use reqwest::{Client, Url};
use serde_json::{Value, json};

use crate::game::Profile;

pub struct ApiError(pub StatusCode, pub &'static str);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

pub enum AuthResult {
    Challenge(String),
    Account(Profile),
}

#[derive(Clone)]
pub struct Sub2Api {
    origin: Url,
    client: Client,
}

impl Sub2Api {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let origin = Url::parse(
            &std::env::var("SUB2API_ORIGIN").unwrap_or_else(|_| "https://zhisuanapi.cn".into()),
        )?;
        let local = matches!(origin.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if origin.scheme() != "https" && !(origin.scheme() == "http" && local) {
            return Err("SUB2API_ORIGIN must use HTTPS outside loopback development".into());
        }
        let client = Client::builder()
            .timeout(Duration::from_millis(4500))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { origin, client })
    }

    async fn request(
        &self,
        path: &str,
        payload: Option<Value>,
        token: Option<&str>,
    ) -> Result<Value, ApiError> {
        let url = self
            .origin
            .join(&format!("/api/v1/auth/{path}"))
            .expect("valid auth path");
        let mut request = match payload {
            Some(data) => self.client.post(url).json(&data),
            None => self.client.get(url),
        }
        .header(header::ACCEPT, "application/json");
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let unavailable = || {
            ApiError(
                StatusCode::BAD_GATEWAY,
                "暂时无法连接智算登录服务，请稍后重试。",
            )
        };
        let response = request.send().await.map_err(|_| unavailable())?;
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(ApiError(status, "登录请求过于频繁，请稍后重试。"));
        }
        if status.is_redirection() || status.is_server_error() {
            return Err(unavailable());
        }
        let mut result: Value = response.json().await.map_err(|_| unavailable())?;
        if !status.is_success() || result["code"].as_i64() != Some(0) || result["data"].is_null() {
            return Err(match path {
                "login/2fa" => ApiError(StatusCode::UNAUTHORIZED, "验证码无效或已过期，请重试。"),
                "login" => ApiError(
                    StatusCode::UNAUTHORIZED,
                    "智算账号登录未通过，请核对邮箱、密码及账号状态。",
                ),
                _ => ApiError(StatusCode::BAD_GATEWAY, "暂时无法读取智算账号资料。"),
            });
        }
        Ok(result["data"].take())
    }

    async fn profile(&self, result: Value) -> Result<Profile, ApiError> {
        let token = result["access_token"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or(ApiError(
                StatusCode::BAD_GATEWAY,
                "智算登录服务未返回有效的登录凭据。",
            ))?;
        let profile = self.request("me", None, Some(token)).await?;
        let id = profile["id"]
            .as_u64()
            .filter(|id| *id > 0 && *id <= 9_007_199_254_740_991)
            .ok_or(ApiError(
                StatusCode::BAD_GATEWAY,
                "智算登录服务未返回有效的用户 ID。",
            ))?;
        let name: String = profile["username"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .chars()
            .take(64)
            .collect();
        let avatar = profile["avatar_url"]
            .as_str()
            .filter(|value| !value.is_empty() && value.len() <= 2048)
            .and_then(|value| self.origin.join(value).ok())
            .filter(|url| {
                url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
            })
            .map(|url| url.to_string())
            .unwrap_or_default();
        Ok(Profile {
            id,
            username: if name.is_empty() {
                format!("棋友 {id}")
            } else {
                name
            },
            avatar,
        })
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<AuthResult, ApiError> {
        let result = self
            .request(
                "login",
                Some(json!({"email": email, "password": password})),
                None,
            )
            .await?;
        if result["requires_2fa"].as_bool() == Some(true) {
            let token = result["temp_token"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or(ApiError(
                    StatusCode::BAD_GATEWAY,
                    "智算登录服务未返回有效的二次验证凭据。",
                ))?;
            return Ok(AuthResult::Challenge(token.into()));
        }
        self.profile(result).await.map(AuthResult::Account)
    }

    pub async fn totp(&self, token: &str, code: &str) -> Result<AuthResult, ApiError> {
        let result = self
            .request(
                "login/2fa",
                Some(json!({"temp_token": token, "totp_code": code})),
                None,
            )
            .await?;
        self.profile(result).await.map(AuthResult::Account)
    }
}
