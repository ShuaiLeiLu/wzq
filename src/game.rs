use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::Instant,
};

use rand::{Rng, RngCore, rngs::OsRng};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{mpsc::UnboundedSender, watch};

use crate::{
    ddz::room::DdzRoom,
    history::{Archive, Journal},
    prediction::{Forecast, Position},
    ranking::{ChampionBroadcast, DdzResult, RankedMatch, RankedPlayer, RankedResult},
};

pub(crate) const SIZE: usize = 15;
const TURN_MS: u64 = 30_000;
const DISCONNECT_MS: u64 = 45_000;
const UNDO_MS: u64 = 10_000;
const UNDO_LIMIT: u8 = 3;
/// Black stones placed before a handicap game, indexed by handicap level
/// (0 none, 1 让先, 2 让二子, 3 让三子): H8, I9 and G9.
const HANDICAP_STONES: [&[usize]; 4] = [&[], &[], &[112, 128], &[112, 128, 126]];
const HANDICAP_LABELS: [&str; 4] = ["不让", "让先", "让二子", "让三子"];
pub const ACCOUNT_SECONDS: u64 = 12 * 60 * 60;
type Failure = &'static str;

#[derive(Clone, Serialize)]
pub struct Profile {
    pub id: u64,
    pub username: String,
    pub avatar: String,
}

pub struct Challenge {
    pub token: String,
    pub expires: u64,
    pub attempts: u8,
}

pub struct Player {
    guest_name: String,
    pub(crate) name: String,
    pub(crate) room: Option<String>,
    pub(crate) ready: bool,
    pub(crate) connection: Option<(u64, watch::Sender<String>)>,
    expires: u64,
    last_message: Option<u64>,
    account: Option<String>,
    pub signing_in: bool,
    pub challenge: Option<Challenge>,
}

struct Account {
    profile: Profile,
    expires: u64,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct UndoRequest {
    color: u8,
    deadline: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Match {
    status: &'static str,
    board: Vec<u8>,
    turn: u8,
    moves: usize,
    last: Option<usize>,
    line: Vec<usize>,
    winner: Option<u8>,
    reason: Option<&'static str>,
    turn_deadline: Option<u64>,
    turn_remaining: Option<u64>,
    handicap: u8,
    undo: Option<UndoRequest>,
    undo_left: [u8; 2],
    // Entry zero is the starting board, including any preset handicap stones.
    black_win_history: Vec<Option<u8>>,
    #[serde(skip)]
    prediction_id: u64,
    // Within this match a board uniquely determines the turn, including after an undo.
    #[serde(skip)]
    predictions: HashMap<Vec<u8>, Option<Forecast>>,
    #[serde(skip)]
    history: Vec<usize>,
    #[serde(skip)]
    journal: Journal,
    #[serde(skip)]
    undo_refused: Option<usize>,
    #[serde(skip)]
    ranked: Option<(RankedMatch, UnboundedSender<RankedResult>)>,
}

impl Match {
    fn new(
        now: u64,
        ranked: Option<(RankedMatch, UnboundedSender<RankedResult>)>,
        handicap: u8,
    ) -> Self {
        let stones = HANDICAP_STONES[usize::from(handicap)];
        let mut board = vec![0; SIZE * SIZE];
        for &index in stones {
            board[index] = 1;
        }
        Self {
            status: "playing",
            board,
            turn: if stones.is_empty() { 1 } else { 2 },
            moves: stones.len(),
            last: None,
            line: Vec::new(),
            winner: None,
            reason: None,
            turn_deadline: Some(now + TURN_MS),
            turn_remaining: None,
            handicap,
            undo: None,
            undo_left: [UNDO_LIMIT; 2],
            black_win_history: vec![None],
            prediction_id: OsRng.next_u64(),
            predictions: HashMap::new(),
            history: Vec::new(),
            journal: Journal::default(),
            undo_refused: None,
            ranked,
        }
    }

    fn playing(&self) -> bool {
        self.status == "playing"
    }

    fn finish(&mut self, winner: Option<u8>, reason: &'static str) {
        let result = match winner {
            Some(1) => "黑棋获胜",
            Some(2) => "白棋获胜",
            _ => "和棋",
        };
        self.journal.finish(
            result,
            json!({"reason":reason,"line":self.line,"winner":winner}),
        );
        self.status = "finished";
        self.undo = None;
        self.winner = winner;
        self.reason = Some(reason);
        self.turn_deadline = None;
        self.turn_remaining = None;
        if let Some((game, results)) = self.ranked.take() {
            if results.send(game.finish(winner)).is_err() {
                eprintln!("leaderboard result writer stopped");
            }
        }
    }

    fn expire(&mut self, now: u64) -> bool {
        if self.turn_deadline.is_some_and(|deadline| now >= deadline) {
            self.finish(Some(3 - self.turn), "timeout");
            return true;
        }
        false
    }

    fn set_connected(&mut self, online: bool, now: u64) {
        if !self.playing() || self.expire(now) {
            return;
        }
        // The clock is already paused while an undo request waits. A seated
        // player dropping cancels the request and refunds it.
        if let Some(request) = self.undo {
            if !online {
                self.undo = None;
                self.undo_left[usize::from(request.color - 1)] += 1;
            }
            return;
        }
        match (online, self.turn_deadline) {
            (false, Some(deadline)) => {
                self.turn_remaining = Some(deadline - now);
                self.turn_deadline = None;
            }
            (true, None) => {
                self.turn_deadline = self.turn_remaining.take().map(|remaining| now + remaining);
            }
            _ => {}
        }
    }

    fn play(&mut self, color: u8, index: Option<usize>, now: u64) -> Result<(), Failure> {
        if self.expire(now) {
            return Err("落子时间已用完。");
        }
        if !self.playing() {
            return Err("请等待双方准备后开局。");
        }
        if self.undo.is_some() {
            return Err("正在等待悔棋回应。");
        }
        if self.turn_deadline.is_none() {
            return Err("棋友正在重连，请稍候。");
        }
        if self.turn != color {
            return Err("现在轮到对方落子。");
        }
        let index = index
            .filter(|&i| self.board.get(i) == Some(&0))
            .ok_or("请选择棋盘上的空交叉点。")?;
        self.journal
            .event(Some(usize::from(color - 1)), "落子", json!({"index":index}));
        self.board[index] = color;
        self.last = Some(index);
        self.history.push(index);
        self.black_win_history.push(
            self.predictions
                .get(&self.board)
                .and_then(|f| f.as_ref())
                .map(|f| f.odds.black),
        );
        self.moves += 1;
        self.line = winning_line(&self.board, index, color);
        if !self.line.is_empty() {
            self.finish(Some(color), "line");
        } else if self.moves == self.board.len() {
            self.finish(None, "draw");
        } else {
            self.turn = 3 - color;
            self.turn_deadline = Some(now + TURN_MS);
        }
        Ok(())
    }

    /// The color that may ask to take back its last move right now: the
    /// player who just moved, before the opponent replies.
    fn undo_candidate(&self) -> Option<u8> {
        if !self.playing() || self.undo.is_some() || self.undo_refused == Some(self.history.len()) {
            return None;
        }
        let color = self.board[*self.history.last()?];
        (color != self.turn && self.undo_left[usize::from(color - 1)] > 0).then_some(color)
    }

    fn request_undo(&mut self, color: u8, now: u64) -> Result<(), Failure> {
        if self.expire(now) || !self.playing() {
            return Err("对局已结束，不能悔棋。");
        }
        if self.undo.is_some() {
            return Err("已有悔棋请求在等待回应。");
        }
        let Some(deadline) = self.turn_deadline else {
            return Err("棋友正在重连，请稍候。");
        };
        if self.undo_candidate() != Some(color) {
            return Err(if self.undo_left[usize::from(color - 1)] == 0 {
                "本局悔棋次数已用完。"
            } else if self.undo_refused == Some(self.history.len()) {
                "这一手的悔棋已被拒绝。"
            } else {
                "只能在对方落子前撤回自己的上一手。"
            });
        }
        self.turn_deadline = None;
        self.turn_remaining = Some(deadline - now);
        self.undo_left[usize::from(color - 1)] -= 1;
        self.journal
            .event(Some(usize::from(color - 1)), "请求悔棋", Value::Null);
        self.undo = Some(UndoRequest {
            color,
            deadline: now + UNDO_MS,
        });
        Ok(())
    }

    fn resolve_undo(&mut self, accept: bool, now: u64) {
        let Some(request) = self.undo.take() else {
            return;
        };
        let remaining = self.turn_remaining.take().unwrap_or(TURN_MS);
        self.journal.event(
            None,
            if accept {
                "撤回上一手"
            } else {
                "悔棋未通过"
            },
            json!({"index":self.history.last()}),
        );
        if accept {
            let index = self.history.pop().expect("undo needs a played move");
            self.black_win_history.pop();
            self.board[index] = 0;
            self.moves -= 1;
            self.last = self.history.last().copied();
            self.turn = request.color;
            self.turn_deadline = Some(now + TURN_MS);
            self.undo_refused = None;
        } else {
            self.undo_refused = Some(self.history.len());
            self.turn_deadline = Some(now + remaining);
        }
    }

    fn expire_undo(&mut self, now: u64) -> bool {
        if self.undo.is_some_and(|request| now >= request.deadline) {
            self.resolve_undo(false, now);
            return true;
        }
        false
    }
}

pub(crate) fn winning_line(board: &[u8], index: usize, color: u8) -> Vec<usize> {
    let origin = ((index / SIZE) as isize, (index % SIZE) as isize);
    let bounds = 0..SIZE as isize;
    for (dr, dc) in [(0, 1), (1, 0), (1, 1), (1, -1)] {
        let mut line = VecDeque::from([index]);
        for direction in [-1, 1] {
            let (row_step, col_step) = (dr * direction, dc * direction);
            let (mut r, mut c) = (origin.0 + row_step, origin.1 + col_step);
            while bounds.contains(&r) && bounds.contains(&c) {
                let next = r as usize * SIZE + c as usize;
                if board[next] != color {
                    break;
                }
                if direction < 0 {
                    line.push_front(next);
                } else {
                    line.push_back(next);
                }
                r += row_step;
                c += col_step;
            }
        }
        if line.len() >= 5 {
            return line.into();
        }
    }
    Vec::new()
}

#[derive(Serialize)]
pub(crate) struct Message {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) text: String,
    pub(crate) seat: i8,
}

pub(crate) const SYSTEM_SEAT: i8 = -2;

struct Handicap {
    giver: String,
    level: u8,
}

struct Room {
    id: String,
    name: String,
    theme: &'static str,
    permanent: bool,
    seats: [Option<String>; 2],
    spectators: HashSet<String>,
    game: Option<Match>,
    same_pair_rematch: bool,
    handicap: Option<Handicap>,
    guidance: VecDeque<Message>,
    next_message: u64,
}

impl Room {
    fn new(id: String, name: String, theme: &'static str, permanent: bool) -> Self {
        Self {
            id,
            name,
            theme,
            permanent,
            seats: [None, None],
            spectators: HashSet::new(),
            game: None,
            same_pair_rematch: false,
            handicap: None,
            guidance: VecDeque::new(),
            next_message: 1,
        }
    }
    fn seat(&self, token: &str) -> Option<usize> {
        self.seats
            .iter()
            .position(|seat| seat.as_deref() == Some(token))
    }
    fn count(&self) -> usize {
        self.seats.iter().flatten().count()
    }
    fn playing(&self) -> bool {
        self.game.as_ref().is_some_and(Match::playing)
    }
    fn joinable(&self) -> bool {
        !self.playing() && self.count() < 2
    }
    fn watchable(&self) -> bool {
        self.spectators.len() < 30 && (self.count() > 0 || !self.spectators.is_empty())
    }

    /// Seat 0 plays black. A handicap giver always takes white.
    fn assign_colors(&mut self) {
        if let Some(handicap) = &self.handicap {
            if self.seats[0].as_deref() == Some(handicap.giver.as_str()) {
                self.seats.swap(0, 1);
            }
        } else if self.same_pair_rematch || OsRng.gen_bool(0.5) {
            self.seats.swap(0, 1);
        }
        self.same_pair_rematch = true;
    }

    fn post(&mut self, name: String, text: String, seat: i8) {
        self.guidance.push_back(Message {
            id: self.next_message,
            name,
            text,
            seat,
        });
        self.next_message += 1;
        if self.guidance.len() > 40 {
            self.guidance.pop_front();
        }
    }

    fn announce(&mut self, text: String) {
        self.post("棋室".into(), text, SYSTEM_SEAT);
    }
}

pub struct Store {
    pub(crate) archive: Option<Archive>,
    started: Instant,
    pub players: HashMap<String, Player>,
    rooms: Vec<Room>,
    accounts: HashMap<String, Account>,
    next_connection: u64,
    prediction_cursor: usize,
    ranked_results: UnboundedSender<RankedResult>,
    champion: Option<ChampionBroadcast>,
    pub(crate) tables: Vec<DdzRoom>,
    pub(crate) ddz_results: Option<UnboundedSender<DdzResult>>,
    ddz_champion: Option<ChampionBroadcast>,
}

pub(crate) fn random_token(bytes: usize) -> String {
    let mut data = vec![0; bytes];
    OsRng.fill_bytes(&mut data);
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn clean_name(input: Option<&str>, limit: usize) -> String {
    let clean = input
        .unwrap_or_default()
        .replace(|ch: char| ch.is_ascii_control(), "");
    clean.trim().chars().take(limit).collect()
}

impl Store {
    pub fn new(ranked_results: UnboundedSender<RankedResult>) -> Self {
        let rooms = [
            ("1001", "松风棋室", "pine"),
            ("1002", "听雨棋室", "rain"),
            ("1003", "竹影棋室", "bamboo"),
            ("1004", "青石棋室", "stone"),
        ]
        .into_iter()
        .map(|(id, name, theme)| Room::new(id.into(), name.into(), theme, true))
        .collect();
        Self {
            started: Instant::now(),
            archive: None,
            players: HashMap::new(),
            rooms,
            accounts: HashMap::new(),
            next_connection: 0,
            prediction_cursor: 0,
            ranked_results,
            champion: None,
            tables: DdzRoom::permanent_tables(),
            ddz_results: None,
            ddz_champion: None,
        }
    }

    pub fn now(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn room_index(&self, id: &str) -> Option<usize> {
        self.rooms.iter().position(|room| room.id == id)
    }

    /// Gomoku rooms and Dou Dizhu tables share one set of room numbers.
    pub(crate) fn free_room_id(&self) -> String {
        loop {
            let id = OsRng.gen_range(2000..10000).to_string();
            if self.room_index(&id).is_none() && self.table_index(&id).is_none() {
                break id;
            }
        }
    }

    fn current_room(&self, token: &str) -> Option<usize> {
        self.room_index(self.players.get(token)?.room.as_deref()?)
    }

    pub(crate) fn profile(&self, player: &Player) -> Option<&Profile> {
        self.accounts
            .get(player.account.as_deref()?)
            .map(|account| &account.profile)
    }

    pub fn account_id(&self, cookie: Option<&str>) -> Option<u64> {
        self.accounts.get(cookie?).map(|account| account.profile.id)
    }

    pub fn authorized(&self, token: &str, cookie: Option<&str>) -> bool {
        self.players.get(token).is_some_and(|player| {
            player
                .account
                .as_deref()
                .is_none_or(|id| cookie == Some(id) && self.accounts.contains_key(id))
        })
    }

    pub fn online(&self, token: &str) -> bool {
        self.players
            .get(token)
            .is_some_and(|player| player.connection.is_some())
    }

    fn apply_account(&mut self, token: &str, account_id: &str) {
        let Some(account) = self.accounts.get(account_id) else {
            return;
        };
        let player = self.players.get_mut(token).expect("validated player");
        player.account = Some(account_id.into());
        let name = clean_name(Some(&account.profile.username), 16);
        player.name = if name.is_empty() {
            format!("棋友 {}", account.profile.id)
        } else {
            name
        };
    }

    pub fn session(&mut self, given: Option<&str>, cookie: Option<&str>) -> String {
        let token = match given.filter(|token| self.authorized(token, cookie)) {
            Some(token) => token.to_owned(),
            None => {
                let token = random_token(24);
                let name = format!("访客 {}", OsRng.gen_range(1000..10000));
                self.players.insert(
                    token.clone(),
                    Player {
                        guest_name: name.clone(),
                        name,
                        room: None,
                        ready: false,
                        connection: None,
                        expires: self.now() + DISCONNECT_MS,
                        last_message: None,
                        account: None,
                        signing_in: false,
                        challenge: None,
                    },
                );
                token
            }
        };
        if let Some(account_id) = cookie.filter(|_| self.players[&token].account.is_none()) {
            self.apply_account(&token, account_id);
        }
        token
    }

    pub fn establish_account(
        &mut self,
        player: &str,
        previous: Option<&str>,
        profile: Profile,
    ) -> String {
        if let Some(previous) = previous {
            self.revoke_account(previous);
        }
        let token = random_token(32);
        self.accounts.insert(
            token.clone(),
            Account {
                profile,
                expires: self.now() + ACCOUNT_SECONDS * 1000,
            },
        );
        self.apply_account(player, &token);
        self.publish(None);
        token
    }

    pub fn revoke_account(&mut self, token: &str) -> bool {
        if self.accounts.remove(token).is_none() {
            return false;
        }
        for player in self
            .players
            .values_mut()
            .filter(|p| p.account.as_deref() == Some(token))
        {
            player.account = None;
            player.name.clone_from(&player.guest_name);
        }
        true
    }

    fn snapshot(&self, token: &str, player: &Player) -> Value {
        let room = self.current_room(token).map(|index| &self.rooms[index]);
        json!({
            "me": {"name": player.name, "account": self.profile(player)},
            "serverTime": self.now(),
            "champion": self.champion,
            "ddzChampion": self.ddz_champion,
            "tables": self.table_summaries(),
            "table": self.table_snapshot(token),
            "online": self.players.values().filter(|player| player.connection.is_some()).count(),
            "rooms": self.rooms.iter().map(|room| json!({ "id": room.id, "name": room.name, "theme": room.theme,
                "count": room.count(), "joinable": room.joinable(), "watchable": room.watchable(), "playing": room.playing() })).collect::<Vec<_>>(),
            "room": room.map(|room| json!({
                "id": room.id, "name": room.name, "seat": room.seat(token).map_or(-1, |seat| seat as i8),
                "seats": room.seats.each_ref().map(|seat| seat.as_ref().and_then(|key| self.players.get(key)).map(|person| json!({
                    "name": person.name, "ready": person.ready, "online": person.connection.is_some(),
                    "avatar": self.profile(person).map_or("", |profile| profile.avatar.as_str()) }))),
                "game": room.game, "guidance": room.guidance,
                "prediction": room.game.as_ref().filter(|g| g.playing()).and_then(|g| g.predictions.get(&g.board)?.as_ref()),
                "handicap": room.handicap.as_ref().map(|handicap| json!({ "level": handicap.level, "giver": room.seat(&handicap.giver) })),
                "undoColor": room.game.as_ref().and_then(Match::undo_candidate),
                "spectatorCount": room.spectators.iter().filter(|name| self.online(name)).count()
            }))
        })
    }

    pub fn publish(&self, room: Option<&str>) {
        for (token, player) in &self.players {
            if let Some((_, sender)) = &player.connection {
                if room.is_none() || room == player.room.as_deref() {
                    sender.send_replace(self.snapshot(token, player).to_string());
                }
            }
        }
    }

    pub fn set_champion(&mut self, champion: ChampionBroadcast) {
        if !champion.expired() && self.champion.as_ref() != Some(&champion) {
            self.champion = Some(champion);
            self.publish(None);
        }
    }

    pub fn next_prediction(&mut self) -> Option<Position> {
        // Rotate between rooms so a fast game cannot monopolize the shared engine.
        let count = self.rooms.len();
        for _ in 0..count {
            let index = self.prediction_cursor % count;
            self.prediction_cursor = (index + 1) % count;
            let room = &mut self.rooms[index];
            let Some(game) = room.game.as_mut() else {
                continue;
            };
            if !game.playing() || game.predictions.contains_key(&game.board) {
                continue;
            }
            game.predictions.insert(game.board.clone(), None);
            let previous = game.history.last().and_then(|&i| {
                let mut before = game.board.clone();
                before[i] = 0;
                Some(game.predictions.get(&before)?.as_ref()?.odds)
            });
            return Some(Position {
                room: room.id.clone(),
                game_id: game.prediction_id,
                board: game.board.clone(),
                turn: game.turn,
                moves: [HANDICAP_STONES[usize::from(game.handicap)], &game.history].concat(),
                previous,
            });
        }
        None
    }

    pub fn save_prediction(&mut self, position: &Position, forecast: Forecast) {
        let Some(index) = self.room_index(&position.room) else {
            return;
        };
        let Some(game) = self.rooms[index].game.as_mut() else {
            return;
        };
        if !game.playing() || game.prediction_id != position.game_id {
            return;
        }
        let current = game.board == position.board && game.turn == position.turn;
        let moves = &position.moves[HANDICAP_STONES[usize::from(game.handicap)].len()..];
        let mut trend_changed = false;
        if game.history.starts_with(moves) {
            let value = &mut game.black_win_history[moves.len()];
            trend_changed = *value != Some(forecast.odds.black);
            *value = Some(forecast.odds.black);
        }
        game.predictions
            .insert(position.board.clone(), Some(forecast));
        if current || trend_changed {
            self.publish(Some(&position.room));
        }
    }

    pub fn set_ddz_champion(&mut self, champion: ChampionBroadcast) {
        if !champion.expired() && self.ddz_champion.as_ref() != Some(&champion) {
            self.ddz_champion = Some(champion);
            self.publish(None);
        }
    }

    pub fn attach_ddz_results(&mut self, results: UnboundedSender<DdzResult>) {
        self.ddz_results = Some(results);
    }

    pub fn connect(&mut self, token: &str) -> (u64, watch::Receiver<String>) {
        self.next_connection += 1;
        let connection = self.next_connection;
        let (sender, receiver) = watch::channel(String::new());
        self.players
            .get_mut(token)
            .expect("validated player")
            .connection = Some((connection, sender));
        if let Some(index) = self.current_room(token) {
            self.sync_room(index);
        }
        if let Some(index) = self.current_table(token) {
            self.sync_table(index);
        }
        self.publish(None);
        (connection, receiver)
    }

    pub fn disconnect(&mut self, token: &str, connection: u64) {
        let expires = self.now() + DISCONNECT_MS;
        let Some(player) = self.players.get_mut(token) else {
            return;
        };
        if player.connection.as_ref().map(|pair| pair.0) != Some(connection) {
            return;
        }
        player.connection = None;
        player.expires = expires;
        if let Some(index) = self.current_room(token) {
            self.sync_room(index);
        }
        self.publish(None);
    }

    fn sync_room(&mut self, index: usize) {
        let now = self.now();
        let room = &self.rooms[index];
        let online = room.seats.iter().all(|seat| {
            seat.as_ref()
                .is_some_and(|key| self.players[key].connection.is_some())
        });
        if room.playing() {
            if let Some(game) = &mut self.rooms[index].game {
                game.set_connected(online, now);
            }
        } else if online
            && room
                .seats
                .iter()
                .flatten()
                .all(|key| self.players[key].ready)
        {
            self.rooms[index].assign_colors();
            let handicap = self.rooms[index]
                .handicap
                .as_ref()
                .map_or(0, |handicap| handicap.level);
            let players = self.rooms[index].seats.each_ref().map(|seat| {
                let profile = self.profile(&self.players[seat.as_ref()?])?;
                Some(RankedPlayer {
                    id: profile.id,
                    name: clean_name(Some(&profile.username), 32),
                })
            });
            // Handicap games stay off the daily ranking.
            let match_id = random_token(16);
            let ranked = (handicap == 0 && players.iter().any(Option::is_some)).then(|| {
                (
                    RankedMatch {
                        id: match_id.clone(),
                        players,
                    },
                    self.ranked_results.clone(),
                )
            });
            let room = &mut self.rooms[index];
            let mut game = Match::new(now, ranked, handicap);
            let names = room
                .seats
                .iter()
                .flatten()
                .map(|key| self.players[key].name.clone())
                .collect();
            game.journal = Journal::start(
                self.archive.as_ref(),
                &match_id,
                "gomoku",
                &room.id,
                names,
                HANDICAP_LABELS[usize::from(handicap)],
            );
            for &index in HANDICAP_STONES[usize::from(handicap)] {
                game.journal
                    .event(Some(0), "预置黑子", json!({"index":index}));
            }
            room.game = Some(game);
            for key in room.seats.iter().flatten() {
                self.players.get_mut(key).expect("occupied seat").ready = false;
            }
        }
    }

    pub fn tick(&mut self) {
        let now = self.now();
        let mut dirty = self
            .champion
            .as_ref()
            .is_some_and(ChampionBroadcast::expired);
        if dirty {
            self.champion = None;
        }
        if self
            .ddz_champion
            .as_ref()
            .is_some_and(ChampionBroadcast::expired)
        {
            self.ddz_champion = None;
            dirty = true;
        }
        dirty |= self.tick_tables(now);
        for room in &mut self.rooms {
            if let Some(game) = &mut room.game {
                dirty |= game.expire(now);
                if game.expire_undo(now) {
                    room.announce("悔棋请求超时，已自动拒绝。".into());
                    dirty = true;
                }
            }
        }
        let expired: Vec<_> = self
            .players
            .iter()
            .filter(|(_, player)| player.connection.is_none() && now >= player.expires)
            .map(|(token, _)| token.clone())
            .collect();
        for token in expired {
            self.leave(&token);
            self.players.remove(&token);
            dirty = true;
        }
        let expired: Vec<_> = self
            .accounts
            .iter()
            .filter(|(_, account)| now >= account.expires)
            .map(|(token, _)| token.clone())
            .collect();
        for token in expired {
            dirty |= self.revoke_account(&token);
        }
        if dirty {
            self.publish(None);
        }
    }

    pub(crate) fn leave(&mut self, token: &str) {
        if let Some(index) = self.current_table(token) {
            self.leave_table(token, index);
            return;
        }
        let Some(index) = self.current_room(token) else {
            return;
        };
        let now = self.now();
        let room = &mut self.rooms[index];
        if let Some(seat) = room.seat(token) {
            room.seats[seat] = None;
            room.same_pair_rematch = false;
            room.handicap = None;
            if let Some(game) = &mut room.game {
                if !game.expire(now) && game.playing() {
                    game.finish(Some(2 - seat as u8), "left");
                }
            }
            for key in room.seats.iter().flatten() {
                self.players.get_mut(key).expect("occupied seat").ready = false;
            }
        } else {
            room.spectators.remove(token);
        }
        let player = self.players.get_mut(token).expect("validated player");
        player.room = None;
        player.ready = false;
        if room.count() == 0 {
            room.game = None;
            if room.spectators.is_empty() {
                if room.permanent {
                    room.guidance.clear();
                } else {
                    self.rooms.remove(index);
                }
            }
        }
    }

    fn join(&mut self, token: &str, index: usize, watching: bool) -> Result<(), Failure> {
        let current = self.current_room(token);
        let room = &mut self.rooms[index];
        if watching {
            if room.spectators.contains(token) {
                return Ok(());
            }
            if current.is_some() || self.players[token].room.is_some() {
                return Err("请先退出当前房间。");
            }
            if !room.watchable() {
                return Err("这个房间当前无法观战。");
            }
            room.spectators.insert(token.into());
        } else {
            if room.seat(token).is_some() {
                return Ok(());
            }
            let elsewhere = self.players[token].room.as_deref() != Some(room.id.as_str());
            if elsewhere && self.players[token].room.is_some() {
                return Err("请先退出当前房间。");
            }
            if !room.joinable() {
                return Err("房间暂时无法加入，请换一个房间。");
            }
            room.spectators.remove(token);
            *room
                .seats
                .iter_mut()
                .find(|seat| seat.is_none())
                .expect("available seat") = Some(token.into());
            room.game = None;
            room.same_pair_rematch = false;
            room.handicap = None;
        }
        let player = self.players.get_mut(token).expect("validated player");
        player.room = Some(room.id.clone());
        player.ready = false;
        Ok(())
    }

    /// Checks a chat line and the sender's rate limit; returns the sender's
    /// name and the cleaned text.
    pub(crate) fn chat_line(
        &mut self,
        token: &str,
        text: Option<&str>,
    ) -> Result<(String, String), Failure> {
        let now = self.now();
        let text = text
            .unwrap_or_default()
            .replace(|ch: char| ch.is_ascii_control(), " ");
        let text = text.trim();
        if text.is_empty() || text.chars().count() > 160 {
            return Err("请输入 1 至 160 个字符。");
        }
        let player = self.players.get_mut(token).expect("validated player");
        if player.last_message.is_some_and(|sent| now - sent < 1000) {
            return Err("请稍等一秒再发送。");
        }
        player.last_message = Some(now);
        Ok((player.name.clone(), text.into()))
    }

    fn send_message(
        &mut self,
        token: &str,
        index: usize,
        text: Option<&str>,
    ) -> Result<String, Failure> {
        let (name, text) = self.chat_line(token, text)?;
        let room = &mut self.rooms[index];
        let seat = room.seat(token).map_or(-1, |seat| seat as i8);
        room.post(name, text, seat);
        Ok(room.id.clone())
    }

    pub fn command(&mut self, token: &str, data: &Value) -> Result<Option<String>, Failure> {
        if let Some(result) = self.ddz_command(token, data) {
            return result;
        }
        let current = self.current_room(token);
        let action = data["action"].as_str();
        match action {
            Some("rename") => {
                let name = clean_name(data["name"].as_str(), 16);
                if name.is_empty() {
                    return Err("请输入昵称。");
                }
                let player = self.players.get_mut(token).expect("validated player");
                if player.account.is_none() {
                    player.guest_name.clone_from(&name);
                }
                player.name = name;
            }
            Some("create") => {
                if current.is_some() {
                    return Err("请先退出当前房间。");
                }
                if self.rooms.len() >= 100 {
                    return Err("房间数量已达上限，请稍后再试。");
                }
                let id = self.free_room_id();
                let name = clean_name(data["name"].as_str(), 20);
                let name = if name.is_empty() {
                    format!("{}的棋室", self.players[token].name)
                } else {
                    name
                };
                self.rooms.push(Room::new(id, name, "pine", false));
                self.join(token, self.rooms.len() - 1, false)?;
            }
            Some("join" | "watch") => {
                let id = data["id"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| data["id"].as_u64().map(|id| id.to_string()));
                let index = id
                    .and_then(|id| self.room_index(&id))
                    .ok_or("没有找到这个房间，请核对房间号。")?;
                self.join(token, index, action == Some("watch"))?;
            }
            Some("quick") => {
                let index = self
                    .rooms
                    .iter()
                    .position(|room| room.joinable() && room.count() == 1)
                    .or_else(|| self.rooms.iter().position(Room::joinable))
                    .ok_or("暂时没有可加入的房间，可以创建一间新棋室。")?;
                self.join(token, index, false)?;
            }
            Some("leave") => self.leave(token),
            Some("guidance") => {
                return self
                    .send_message(
                        token,
                        current.ok_or("请先加入棋室再发言。")?,
                        data["text"].as_str(),
                    )
                    .map(Some);
            }
            Some("ready") => {
                let index = current
                    .filter(|&i| self.rooms[i].seat(token).is_some() && !self.rooms[i].playing())
                    .ok_or("当前无法切换准备状态。")?;
                self.players.get_mut(token).expect("validated player").ready =
                    data["ready"].as_bool().unwrap_or(false);
                self.sync_room(index);
            }
            Some("handicap") => {
                let index = current
                    .filter(|&i| self.rooms[i].seat(token).is_some())
                    .ok_or("只有入座棋手可以设置让子。")?;
                let level = data["level"]
                    .as_u64()
                    .and_then(|level| u8::try_from(level).ok())
                    .filter(|&level| usize::from(level) < HANDICAP_LABELS.len())
                    .ok_or("请选择让子档位。")?;
                let name = self.players[token].name.clone();
                let room = &mut self.rooms[index];
                if room.playing() {
                    return Err("对局进行中，无法修改让子。");
                }
                let unchanged = match &room.handicap {
                    Some(handicap) => handicap.giver == token && handicap.level == level,
                    None => level == 0,
                };
                if unchanged {
                    return Ok(None);
                }
                room.handicap = (level > 0).then(|| Handicap {
                    giver: token.into(),
                    level,
                });
                room.announce(if level == 0 {
                    format!("{name} 取消了让子。")
                } else {
                    format!(
                        "{name} 设置{}，双方重新准备后生效。",
                        HANDICAP_LABELS[usize::from(level)]
                    )
                });
                for key in room.seats.iter().flatten() {
                    self.players.get_mut(key).expect("occupied seat").ready = false;
                }
            }
            Some("undo") => {
                let now = self.now();
                let index = current.ok_or("只有入座棋手可以悔棋。")?;
                let name = self.players[token].name.clone();
                let room = &mut self.rooms[index];
                let color = room.seat(token).ok_or("只有入座棋手可以悔棋。")? as u8 + 1;
                room.game
                    .as_mut()
                    .ok_or("对局尚未开始。")?
                    .request_undo(color, now)?;
                room.announce(format!("{name} 请求悔棋，等待对方回应。"));
            }
            Some("undo-reply") => {
                let now = self.now();
                let index = current.ok_or("没有需要回应的悔棋请求。")?;
                let name = self.players[token].name.clone();
                let room = &mut self.rooms[index];
                let color = room.seat(token).ok_or("没有需要回应的悔棋请求。")? as u8 + 1;
                let game = room
                    .game
                    .as_mut()
                    .filter(|game| game.undo.is_some_and(|request| request.color != color))
                    .ok_or("没有需要回应的悔棋请求。")?;
                let accept = data["accept"].as_bool().unwrap_or(false);
                game.resolve_undo(accept, now);
                room.announce(if accept {
                    format!("{name} 同意悔棋，已撤回一步。")
                } else {
                    format!("{name} 拒绝了悔棋。")
                });
            }
            Some("move") => {
                let now = self.now();
                let room = &mut self.rooms[current.ok_or("只有入座棋手可以落子。")?];
                let color = room.seat(token).ok_or("只有入座棋手可以落子。")? as u8 + 1;
                let index = data["index"]
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok());
                room.game
                    .as_mut()
                    .ok_or("请等待双方准备后开局。")?
                    .play(color, index, now)?;
            }
            _ => return Err("无法执行此操作。"),
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rematch_swaps_colors_and_keeps_ranked_identities_with_the_players() {
        let (scores, _results) = tokio::sync::mpsc::unbounded_channel();
        let mut store = Store::new(scores);
        let first = store.session(None, None);
        let second = store.session(None, None);
        let newcomer = store.session(None, None);
        for (token, id) in [(&first, 30), (&second, 40)] {
            store.establish_account(
                token,
                None,
                Profile {
                    id,
                    username: format!("棋手 {id}"),
                    avatar: String::new(),
                },
            );
            store.connect(token);
            store.join(token, 0, false).unwrap();
        }
        store.connect(&newcomer);
        let ready = |store: &mut Store| {
            for token in [&first, &second] {
                store
                    .command(token, &json!({"action": "ready", "ready": true}))
                    .unwrap();
            }
        };
        ready(&mut store);
        let opening = store.rooms[0].seats.clone();
        store.rooms[0]
            .game
            .as_mut()
            .unwrap()
            .finish(Some(1), "line");
        ready(&mut store);
        let room = &store.rooms[0];
        assert_eq!(room.seats, [opening[1].clone(), opening[0].clone()]);
        let (ranked, _) = room.game.as_ref().unwrap().ranked.as_ref().unwrap();
        for (seat, profile) in room.seats.iter().zip(&ranked.players) {
            let player = &store.players[seat.as_ref().unwrap()];
            assert_eq!(
                store.profile(player).unwrap().id,
                profile.as_ref().unwrap().id
            );
        }
        store.leave(&second);
        store.join(&newcomer, 0, false).unwrap();
        assert!(!store.rooms[0].same_pair_rematch);
    }

    /// Two connected guests seated in room 0.
    fn seated_pair() -> (Store, String, String) {
        let (scores, _results) = tokio::sync::mpsc::unbounded_channel();
        let mut store = Store::new(scores);
        let first = store.session(None, None);
        let second = store.session(None, None);
        for token in [&first, &second] {
            store.connect(token);
            store.join(token, 0, false).unwrap();
        }
        (store, first, second)
    }

    fn start(store: &mut Store, players: [&String; 2]) {
        for token in players {
            store
                .command(token, &json!({"action": "ready", "ready": true}))
                .unwrap();
        }
        assert!(store.rooms[0].playing());
    }

    /// Returns (black, white) tokens for the running game.
    fn colors(store: &Store) -> (String, String) {
        let seats = &store.rooms[0].seats;
        (seats[0].clone().unwrap(), seats[1].clone().unwrap())
    }

    fn play(store: &mut Store, token: &str, index: usize) {
        store
            .command(token, &json!({"action": "move", "index": index}))
            .unwrap();
    }

    #[test]
    fn model_result_follows_the_board_through_moves_and_undo() {
        let (mut store, first, second) = seated_pair();
        start(&mut store, [&first, &second]);
        let (black, white) = colors(&store);
        let opening = store.next_prediction().unwrap();
        assert!(store.next_prediction().is_none());
        play(&mut store, &black, 112);
        let after_move = store.next_prediction().unwrap();
        assert_eq!(after_move.turn, 2);
        let forecast = Forecast {
            odds: crate::prediction::Odds {
                black: 60,
                draw: 5,
                white: 35,
            },
            source: "rapfi",
            variation: vec![112],
            nodes: 256,
            tactic: None,
            explanation: None,
        };
        store.save_prediction(&opening, forecast.clone());
        assert_eq!(
            store.rooms[0].game.as_ref().unwrap().black_win_history,
            [Some(60), None]
        );
        let visible =
            |s: &Store| s.snapshot(&black, &s.players[&black])["room"]["prediction"].clone();
        assert!(visible(&store).is_null());
        store.command(&black, &json!({"action":"undo"})).unwrap();
        store
            .command(&white, &json!({"action":"undo-reply","accept":true}))
            .unwrap();
        assert_eq!(visible(&store)["black"], 60);
        assert_eq!(
            store.rooms[0].game.as_ref().unwrap().black_win_history,
            [Some(60)]
        );
        assert!(store.next_prediction().is_none());
        store.rooms[0]
            .game
            .as_mut()
            .unwrap()
            .finish(Some(1), "timeout");
        start(&mut store, [&first, &second]);
        store.save_prediction(&after_move, forecast);
        let (new_black, _) = colors(&store);
        play(&mut store, &new_black, 112);
        assert!(visible(&store).is_null());
    }

    #[test]
    fn handicap_gives_the_receiver_black_preset_stones_and_skips_ranking() {
        let (mut store, giver, receiver) = seated_pair();
        store
            .command(&giver, &json!({"action": "ready", "ready": true}))
            .unwrap();
        store
            .command(&giver, &json!({"action": "handicap", "level": 3}))
            .unwrap();
        assert!(
            !store.players[&giver].ready,
            "changing handicap resets ready"
        );
        for _ in 0..2 {
            start(&mut store, [&giver, &receiver]);
            let room = &store.rooms[0];
            assert_eq!(room.seats[1].as_deref(), Some(giver.as_str()));
            let game = room.game.as_ref().unwrap();
            assert_eq!((game.turn, game.moves, game.handicap), (2, 3, 3));
            for index in [112, 128, 126] {
                assert_eq!(game.board[index], 1);
            }
            assert!(game.ranked.is_none());
            store.rooms[0]
                .game
                .as_mut()
                .unwrap()
                .finish(Some(1), "line");
        }
        store
            .command(&receiver, &json!({"action": "handicap", "level": 0}))
            .unwrap();
        assert!(store.rooms[0].handicap.is_none());
        store.leave(&receiver);
        store
            .command(&giver, &json!({"action": "handicap", "level": 1}))
            .unwrap();
        store.join(&receiver, 0, false).unwrap();
        assert!(store.rooms[0].handicap.is_none(), "a seat change clears it");
    }

    #[test]
    fn undo_needs_consent_and_only_takes_back_the_requesters_last_move() {
        let (mut store, first, second) = seated_pair();
        start(&mut store, [&first, &second]);
        let (black, white) = colors(&store);
        let undo = json!({"action": "undo"});
        assert!(
            store.command(&black, &undo).is_err(),
            "nothing to take back"
        );
        play(&mut store, &black, 112);
        assert!(store.command(&white, &undo).is_err(), "only the mover asks");
        store.command(&black, &undo).unwrap();
        assert!(
            store
                .command(&white, &json!({"action": "move", "index": 0}))
                .is_err()
        );
        assert!(
            store
                .command(&black, &json!({"action": "undo-reply", "accept": true}))
                .is_err(),
            "the requester cannot answer"
        );
        store
            .command(&white, &json!({"action": "undo-reply", "accept": true}))
            .unwrap();
        let game = store.rooms[0].game.as_ref().unwrap();
        assert_eq!((game.board[112], game.moves, game.turn), (0, 0, 1));
        assert_eq!(game.last, None);
        assert_eq!(game.undo_left, [2, 3]);

        play(&mut store, &black, 112);
        store.command(&black, &undo).unwrap();
        store
            .command(&white, &json!({"action": "undo-reply", "accept": false}))
            .unwrap();
        let game = store.rooms[0].game.as_ref().unwrap();
        assert_eq!((game.board[112], game.turn), (1, 2));
        assert!(game.turn_deadline.is_some());
        assert!(store.command(&black, &undo).is_err(), "refused move stays");
        play(&mut store, &white, 0);
        play(&mut store, &black, 1);
        assert_eq!(store.rooms[0].game.as_ref().unwrap().undo_left, [1, 3]);
        store.command(&black, &undo).unwrap();
        store
            .command(&white, &json!({"action": "undo-reply", "accept": true}))
            .unwrap();
        play(&mut store, &black, 2);
        assert!(store.command(&black, &undo).is_err(), "limit of three");
    }

    #[test]
    fn unanswered_undo_expires_and_disconnect_cancels_it() {
        let (mut store, first, second) = seated_pair();
        start(&mut store, [&first, &second]);
        let (black, white) = colors(&store);
        play(&mut store, &black, 112);
        store.command(&black, &json!({"action": "undo"})).unwrap();
        let now = store.now();
        let game = store.rooms[0].game.as_mut().unwrap();
        assert!(game.expire_undo(now + UNDO_MS));
        assert!(game.undo.is_none() && game.board[112] == 1 && game.turn == 2);
        assert!(game.turn_deadline.is_some());

        play(&mut store, &white, 0);
        play(&mut store, &black, 1);
        store.command(&black, &json!({"action": "undo"})).unwrap();
        let connection = store.players[&white].connection.as_ref().unwrap().0;
        store.disconnect(&white, connection);
        let game = store.rooms[0].game.as_ref().unwrap();
        assert!(game.undo.is_none());
        assert_eq!(game.undo_left[0], 2, "a cancelled request is refunded");
        assert!(game.turn_deadline.is_none() && game.turn_remaining.is_some());
    }
}
