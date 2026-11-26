use std::{
    fs::{self, File},
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json,
    extract::{Path as Param, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{App, auth::ApiError, game::random_token, ranking::Leaderboard};

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Clone)]
pub struct Archive {
    dir: Arc<PathBuf>,
    pub instance: String,
    unsaved: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Archive {
    pub fn open() -> io::Result<Self> {
        let dir = PathBuf::from(
            std::env::var_os("WZQ_HISTORY_DIR").unwrap_or_else(|| "data/history-outbox".into()),
        );
        Self::open_in(dir)
    }

    fn open_in(dir: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        let archive = Self {
            dir: Arc::new(dir),
            instance: random_token(16),
            unsaved: Default::default(),
        };
        let probe = archive.dir.join(format!("{}.probe", archive.instance));
        File::create(&probe)?.sync_all()?;
        fs::remove_file(probe)?;
        Ok(archive)
    }

    fn save(&self, name: &str, event: &Value) -> io::Result<()> {
        let path = self.dir.join(name);
        let temp = path.with_extension("tmp");
        let mut file = File::create(&temp)?;
        serde_json::to_writer(&mut file, event)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::rename(temp, path)?;
        #[cfg(unix)]
        File::open(self.dir.as_ref())?.sync_all()?;
        Ok(())
    }

    fn batch(&self) -> io::Result<Vec<(PathBuf, Value)>> {
        self.unsaved
            .lock()
            .unwrap()
            .retain(|(name, event)| self.save(name, event).is_err());
        let mut batch = Vec::new();
        for entry in fs::read_dir(self.dir.as_ref())? {
            let path = entry?.path();
            if path.extension() != Some("json".as_ref()) {
                continue;
            }
            let event = serde_json::from_reader(File::open(&path)?)?;
            batch.push((path, event));
            if batch.len() == 100 {
                break;
            }
        }
        Ok(batch)
    }

    pub async fn sync(self, db: Leaderboard) {
        let mut retry = 1;
        loop {
            let result: Result<bool, String> = async {
                let archive = self.clone();
                let batch = tokio::task::spawn_blocking(move || archive.batch())
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?;
                if batch.is_empty() {
                    return Ok(false);
                }
                let events: Vec<_> = batch.iter().map(|(_, e)| e).collect();
                db.rpc("/rpc/save_game_events", json!({"p_events": events}))
                    .await?;
                for (path, _) in batch {
                    fs::remove_file(path).map_err(|e| e.to_string())?;
                }
                Ok(true)
            }
            .await;
            match result {
                Ok(true) => {
                    retry = 1;
                    continue;
                }
                Ok(false) => retry = 1,
                Err(error) => {
                    if retry == 1 {
                        eprintln!("game history sync: {error}");
                    }
                    retry = (retry * 2).min(30);
                }
            }
            tokio::time::sleep(Duration::from_secs(retry)).await;
        }
    }
}

#[derive(Default)]
pub struct Journal(Option<GameLog>);

struct GameLog {
    archive: Archive,
    id: String,
    meta: Value,
    seq: u64,
}

impl Journal {
    pub fn start(
        archive: Option<&Archive>,
        id: &str,
        game: &str,
        room: &str,
        names: Vec<String>,
        mode: &str,
    ) -> Self {
        let mut journal = Self(archive.map(|a| GameLog {
            archive: a.clone(), id: id.into(), seq: 0,
            meta: json!({"id":id,"game":game,"room":room,"players":names,"mode":mode,"start":timestamp(),"instance":a.instance}),
        }));
        journal.event(None, "开局", Value::Null);
        journal
    }

    pub fn event(&mut self, seat: Option<usize>, action: &str, detail: Value) {
        self.append(seat, action, detail, None);
    }

    pub fn finish(&mut self, result: &str, detail: Value) {
        self.append(None, result, detail, Some("finished"));
        self.0 = None;
    }

    pub fn abandon(&mut self) {
        self.append(
            None,
            "玩家全部离桌，牌局中止",
            Value::Null,
            Some("interrupted"),
        );
        self.0 = None;
    }

    fn append(&mut self, seat: Option<usize>, action: &str, detail: Value, status: Option<&str>) {
        let Some(log) = &mut self.0 else {
            return;
        };
        let event = json!({"match":log.meta,"seq":log.seq,"at":timestamp(),"seat":seat,"action":action,"detail":detail,"status":status});
        let name = format!("{}-{:06}.json", log.id, log.seq);
        log.seq += 1;
        if let Err(error) = log.archive.save(&name, &event) {
            eprintln!("game history spool: {error}");
            log.archive.unsaved.lock().unwrap().push((name, event));
        }
    }
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub game: Option<String>,
    pub room: Option<String>,
    pub day: Option<String>,
    pub before: Option<String>,
}

fn valid_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn valid_day(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return false;
    }
    let year = value[..4].parse::<u32>().unwrap();
    let month = value[5..7].parse::<u8>().unwrap();
    let day = value[8..].parse::<u8>().unwrap();
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let last = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    year > 0 && day > 0 && day <= last
}

async fn query(app: &App, path: &str, args: Value) -> Result<Value, ApiError> {
    let unavailable = || {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "暂时无法读取对局记录，请稍后重试。",
        )
    };
    app.leaderboard
        .rpc(path, args)
        .await
        .map_err(|e| {
            eprintln!("history read: {e}");
            unavailable()
        })?
        .json()
        .await
        .map_err(|_| unavailable())
}

pub(crate) async fn list(
    State(app): State<App>,
    Query(filter): Query<HistoryQuery>,
) -> Result<Json<Value>, ApiError> {
    if filter
        .game
        .as_deref()
        .is_some_and(|g| !matches!(g, "gomoku" | "ddz"))
        || filter
            .room
            .as_deref()
            .is_some_and(|r| r.len() != 4 || !r.bytes().all(|c| c.is_ascii_digit()))
        || filter.day.as_deref().is_some_and(|d| !valid_day(d))
        || filter.before.as_deref().is_some_and(|id| !valid_id(id))
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "查询条件有误。"));
    }
    let args = json!({
        "p_instance": app.archive.instance, "p_game": filter.game,
        "p_room": filter.room, "p_day": filter.day, "p_before": filter.before,
    });
    let mut data = query(&app, "/rpc/list_game_history", args).await?;
    let rows = data
        .as_array_mut()
        .ok_or(ApiError(StatusCode::BAD_GATEWAY, "记录格式有误。"))?;
    let more = rows.len() > 25;
    rows.truncate(25);
    Ok(Json(
        json!({"matches":rows,"next":if more {rows.last().and_then(|r|r["id"].as_str())} else {None}}),
    ))
}

pub(crate) async fn detail(
    State(app): State<App>,
    Param(id): Param<String>,
) -> Result<Json<Value>, ApiError> {
    let missing = || ApiError(StatusCode::NOT_FOUND, "没有找到这局记录。");
    if !valid_id(&id) {
        return Err(missing());
    }
    let data = query(
        &app,
        "/rpc/game_history_detail",
        json!({"p_instance":app.archive.instance,"p_id":id}),
    )
    .await?;
    if data.is_null() {
        return Err(missing());
    }
    Ok(Json(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_events_survive_restart_and_finish_only_once() {
        let dir = std::env::temp_dir().join(format!("wzq-history-{}", random_token(16)));
        let archive = Archive::open_in(dir.clone()).unwrap();
        let id = random_token(16);
        let mut journal = Journal::start(
            Some(&archive),
            &id,
            "ddz",
            "1101",
            vec!["访客".into()],
            "测试",
        );
        journal.event(Some(0), "对子", json!({"cards":[0,1]}));
        journal.finish("地主获胜", json!({"winnerSeat":0}));
        journal.abandon();
        let restarted = Archive::open_in(dir.clone()).unwrap();
        let mut batch = restarted.batch().unwrap();
        batch.sort_by_key(|(_, event)| event["seq"].as_u64().unwrap());
        assert_eq!(batch.len(), 3);
        assert_eq!(batch[1].1["detail"]["cards"], json!([0, 1]));
        assert_eq!(batch[2].1["status"], "finished");
        for (path, event) in batch {
            assert_eq!(event["match"]["id"], id);
            assert_eq!(event["match"]["instance"], archive.instance);
            fs::remove_file(path).unwrap();
        }
        fs::remove_dir(dir).unwrap();
    }
}
