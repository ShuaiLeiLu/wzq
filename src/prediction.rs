use std::{
    env,
    sync::{Arc, Mutex},
    time::Duration,
};

use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::game::{SIZE, Store, winning_line};
use crate::rapfi::Rapfi;

pub struct Position {
    pub room: String,
    pub game_id: u64,
    pub board: Vec<u8>,
    pub turn: u8,
    pub moves: Vec<usize>,
    pub previous: Option<Odds>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
pub struct Odds {
    pub black: u8,
    pub draw: u8,
    pub white: u8,
}

impl Odds {
    pub fn result(winner: u8) -> Self {
        Self {
            black: if winner == 1 { 100 } else { 0 },
            draw: if winner == 0 { 100 } else { 0 },
            white: if winner == 2 { 100 } else { 0 },
        }
    }

    fn parse(content: &str) -> Option<Self> {
        let content = content.trim();
        let json = content
            .strip_prefix("```json")
            .and_then(|s| s.strip_suffix("```"))
            .unwrap_or(content);
        let odds: Self = serde_json::from_str(json).ok()?;
        (u16::from(odds.black) + u16::from(odds.draw) + u16::from(odds.white) == 100)
            .then_some(odds)
    }
}

#[derive(Clone, Serialize)]
pub struct Forecast {
    #[serde(flatten)]
    pub odds: Odds,
    pub source: &'static str,
    pub variation: Vec<usize>,
    pub nodes: u32,
    pub tactic: Option<&'static str>,
    pub explanation: Option<String>,
}

impl Position {
    fn winning_points(&self) -> [Vec<usize>; 2] {
        let mut board = self.board.clone();
        let mut winning_points = [Vec::new(), Vec::new()];
        for (i, stone) in self.board.iter().enumerate() {
            if *stone != 0 {
                continue;
            }
            for color in [1, 2] {
                board[i] = color;
                if !winning_line(&board, i, color).is_empty() {
                    winning_points[(color - 1) as usize].push(i);
                }
            }
            board[i] = 0;
        }
        winning_points
    }

    fn forced_result(&self, wins: &[Vec<usize>; 2]) -> Option<Forecast> {
        let own = &wins[(self.turn - 1) as usize];
        let other = &wins[(2 - self.turn) as usize];
        let (winner, line, reason) = if let Some(&i) = own.first() {
            (self.turn, vec![i], "行棋方下一手可以连五")
        } else if other.len() >= 2 {
            (
                3 - self.turn,
                vec![other[0], other[1]],
                "对方有两个连五点，这一手最多挡住一个",
            )
        } else if self.moves.len() + 1 == self.board.len() {
            (
                0,
                vec![self.board.iter().position(|&s| s == 0)?],
                "最后一手无法连五，棋盘将下满",
            )
        } else {
            return None;
        };
        Some(Forecast {
            odds: Odds::result(winner),
            source: "tactic",
            variation: line,
            nodes: 0,
            tactic: Some(reason),
            explanation: None,
        })
    }

    fn input(&self, wins: &[Vec<usize>; 2], forecast: Option<&Forecast>) -> Value {
        let coord = |&i: &usize| format!("{}{}", char::from(b'A' + (i % SIZE) as u8), i / SIZE + 1);
        let names = |indices: &[usize]| indices.iter().map(coord).collect::<Vec<_>>();
        let rows: Vec<String> = self
            .board
            .chunks_exact(SIZE)
            .map(|row| {
                row.iter()
                    .map(|s| match s {
                        1 => 'X',
                        2 => 'O',
                        _ => '.',
                    })
                    .collect()
            })
            .collect();
        json!({
            "rows": rows, "turn": if self.turn == 1 { "black" } else { "white" },
            "blackWinningPoints": names(&wins[0]), "whiteWinningPoints": names(&wins[1]),
            "previousOdds": self.previous, "lastStone": self.moves.last().map(coord),
            "assessment": forecast.map(|f| json!({"odds":f.odds, "variation":names(&f.variation), "tactic":f.tactic}))
        })
    }
}

#[derive(Clone)]
pub struct Predictor {
    client: Client,
    endpoint: Url,
    model: String,
    key: Option<String>,
}

impl Predictor {
    pub fn from_env() -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let setting = |name| env::var(name).ok().filter(|v| !v.trim().is_empty());
        let model = setting("WZQ_LLM_MODEL");
        let base = setting("WZQ_LLM_BASE_URL");
        let key = setting("WZQ_LLM_API_KEY");
        if model.is_none() && base.is_none() && key.is_none() {
            return Ok(None);
        }
        let model = model.ok_or("set WZQ_LLM_MODEL for win-rate predictions")?;
        let base = base.ok_or("set WZQ_LLM_BASE_URL for win-rate predictions")?;
        let endpoint = Url::parse(&format!("{}/chat/completions", base.trim_end_matches('/')))?;
        let local = matches!(
            endpoint.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]")
        );
        if !(endpoint.scheme() == "https"
            || endpoint.scheme() == "http" && (local || key.is_none()))
        {
            return Err("use HTTPS for a remote WZQ_LLM_BASE_URL with an API key".into());
        }
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(8))
            .build()?;
        Ok(Some(Self {
            client,
            endpoint,
            model,
            key,
        }))
    }

    async fn predict(
        &self,
        pos: &Position,
        wins: &[Vec<usize>; 2],
        forecast: Option<Forecast>,
    ) -> Result<Forecast, Box<dyn std::error::Error + Send + Sync>> {
        let mut request = self.client.post(self.endpoint.clone());
        if let Some(key) = &self.key {
            request = request.bearer_auth(key);
        }
        let task = if forecast.is_some() {
            "Explain the supplied engine assessment in one short Chinese sentence for the players. Use only supplied tactical evidence and principal variation. If previousOdds exists you may compare it. Do not invent a forced win or change the odds. Return JSON with a single string field explanation."
        } else {
            "Estimate outcome percentages for evenly matched human players. Return only JSON with integer percentages black, draw, white summing to 100."
        };
        let reply: Value = request.json(&json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": format!("15x15 freestyle Gomoku. Five or more wins; a full board without five draws. X=black, O=white, .=empty. Coordinates A1 through O15, A1 is top left. Respect turn including handicap starts. {task}")},
                {"role": "user", "content": pos.input(wins, forecast.as_ref()).to_string()}
            ]
        })).send().await?.error_for_status()?.json().await?;
        let content = reply
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or("missing prediction content")?
            .trim();
        if let Some(mut f) = forecast {
            let json = content
                .strip_prefix("```json")
                .and_then(|s| s.strip_suffix("```"))
                .unwrap_or(content);
            let message: Value = serde_json::from_str(json)?;
            let note = message["explanation"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or("missing position explanation")?;
            f.explanation = Some(note.chars().take(160).collect());
            Ok(f)
        } else {
            Ok(Forecast {
                odds: Odds::parse(content).ok_or("invalid prediction percentages")?,
                source: "model",
                variation: vec![],
                nodes: 0,
                explanation: None,
                tactic: None,
            })
        }
    }
}

pub async fn run(store: Arc<Mutex<Store>>, model: Option<Predictor>, mut engine: Option<Rapfi>) {
    let slots = Arc::new(Semaphore::new(4));
    let mut tick = tokio::time::interval(Duration::from_millis(75));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        if engine.is_none() && slots.available_permits() == 0 {
            continue;
        }
        let Some(position) = store.lock().unwrap().next_prediction() else {
            continue;
        };
        let wins = position.winning_points();
        let mut forecast = position.forced_result(&wins);
        if forecast.is_none()
            && let Some(engine) = &mut engine
        {
            match engine.evaluate(&position).await {
                Ok(value) => forecast = Some(value),
                Err(error) => eprintln!("Rapfi unavailable: {error}"),
            }
        }
        if let Some(f) = &forecast {
            store.lock().unwrap().save_prediction(&position, f.clone());
        }
        if let Some(model) = model.clone()
            && let Ok(permit) = slots.clone().try_acquire_owned()
        {
            let store = store.clone();
            tokio::spawn(async move {
                let _permit = permit;
                match model.predict(&position, &wins, forecast).await {
                    Ok(f) => store.lock().unwrap().save_prediction(&position, f),
                    Err(error) => eprintln!("gomoku model unavailable: {error}"),
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immediate_wins_override_blocks_and_a_full_board_can_draw() {
        let mut p = Position {
            room: "test".into(),
            game_id: 1,
            board: vec![0; SIZE * SIZE],
            turn: 2,
            moves: vec![110, 0, 111, 1, 112, 2, 113, 3],
            previous: None,
        };
        for (n, &i) in p.moves.iter().enumerate() {
            p.board[i] = (1 + n % 2) as u8;
        }
        let forecast = p.forced_result(&p.winning_points()).unwrap();
        assert_eq!((forecast.odds.white, forecast.variation), (100, vec![4]));
        p.board[3] = 0;
        let forecast = p.forced_result(&p.winning_points()).unwrap();
        assert_eq!(
            (forecast.odds.black, forecast.variation),
            (100, vec![109, 114])
        );
        for (i, s) in p.board.iter_mut().enumerate() {
            *s = if (i % SIZE + 2 * (i / SIZE)) % 4 < 2 {
                1
            } else {
                2
            };
        }
        p.moves = (0..224).collect();
        p.board[224] = 0;
        assert_eq!(p.forced_result(&p.winning_points()).unwrap().odds.draw, 100);
    }

    #[test]
    fn validates_percentage_distribution() {
        assert_eq!(
            Odds::parse(r#"{"black":72,"white":25,"draw":3}"#)
                .unwrap()
                .white,
            25
        );
        for text in [
            r#"{"black":72,"white":35,"draw":3}"#,
            r#"{"black":100}"#,
            "no idea",
        ] {
            assert!(Odds::parse(text).is_none());
        }
    }
}
