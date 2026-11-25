use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub struct RankedMatch {
    pub id: String,
    pub players: [Option<RankedPlayer>; 2],
}

#[derive(Serialize)]
pub struct RankedPlayer {
    pub id: u64,
    pub name: String,
}

pub struct RankedResult {
    game: RankedMatch,
    finished_at: u64,
    winner: Option<u8>,
}

impl RankedMatch {
    pub fn finish(self, winner: Option<u8>) -> RankedResult {
        RankedResult {
            game: self,
            finished_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            winner,
        }
    }
}

#[derive(Clone)]
pub struct Leaderboard {
    origin: Url,
    client: Client,
    token: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct Standings {
    day: String,
    entries: Vec<Standing>,
    you: Option<Standing>,
}

#[derive(Deserialize, Serialize)]
struct Standing {
    rank: u64,
    name: String,
    wins: u64,
    losses: u64,
    points: i64,
}

impl Leaderboard {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let origin = Url::parse(
            &std::env::var("WZQ_POSTGREST_ORIGIN")
                .unwrap_or_else(|_| "http://127.0.0.1:3042".into()),
        )?;
        let local = matches!(origin.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if origin.scheme() != "https" && !(origin.scheme() == "http" && local) {
            return Err("WZQ_POSTGREST_ORIGIN must use HTTPS outside loopback".into());
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(4))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            origin,
            client,
            token: std::env::var("WZQ_POSTGREST_TOKEN").ok(),
        })
    }

    async fn rpc(&self, path: &str, body: Value) -> Result<reqwest::Response, String> {
        let url = self.origin.join(path).expect("valid PostgREST RPC path");
        let mut request = self.client.post(url).json(&body);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(format!(
                "PostgREST returned {} for {path}",
                response.status()
            ))
        }
    }

    pub async fn record(&self, result: &RankedResult) -> Result<(), String> {
        self.rpc(
            "/rpc/record_ranked_match",
            json!({
                "p_id": result.game.id,
                "p_finished_at": result.finished_at,
                "p_winner": result.winner,
                "p_players": result.game.players,
            }),
        )
        .await?;
        Ok(())
    }

    pub async fn standings(&self, viewer: Option<u64>) -> Result<Standings, String> {
        self.rpc("/rpc/daily_leaderboard", json!({"p_viewer": viewer}))
            .await?
            .json()
            .await
            .map_err(|error| error.to_string())
    }
}
