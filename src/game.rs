use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::Instant,
};

use rand::{Rng, RngCore, rngs::OsRng};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{mpsc::UnboundedSender, watch};

use crate::ranking::{RankedMatch, RankedPlayer, RankedResult};

const SIZE: usize = 15;
const TURN_MS: u64 = 30_000;
const DISCONNECT_MS: u64 = 45_000;
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
    name: String,
    room: Option<String>,
    ready: bool,
    connection: Option<(u64, watch::Sender<String>)>,
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
    #[serde(skip)]
    ranked: Option<(RankedMatch, UnboundedSender<RankedResult>)>,
}

impl Match {
    fn new(now: u64, ranked: Option<(RankedMatch, UnboundedSender<RankedResult>)>) -> Self {
        Self {
            status: "playing",
            board: vec![0; SIZE * SIZE],
            turn: 1,
            moves: 0,
            last: None,
            line: Vec::new(),
            winner: None,
            reason: None,
            turn_deadline: Some(now + TURN_MS),
            turn_remaining: None,
            ranked,
        }
    }

    fn playing(&self) -> bool {
        self.status == "playing"
    }

    fn finish(&mut self, winner: Option<u8>, reason: &'static str) {
        self.status = "finished";
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
        if self.turn_deadline.is_none() {
            return Err("棋友正在重连，请稍候。");
        }
        if self.turn != color {
            return Err("现在轮到对方落子。");
        }
        let index = index
            .filter(|&i| self.board.get(i) == Some(&0))
            .ok_or("请选择棋盘上的空交叉点。")?;
        self.board[index] = color;
        self.last = Some(index);
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
}

fn winning_line(board: &[u8], index: usize, color: u8) -> Vec<usize> {
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
struct Message {
    id: u64,
    name: String,
    text: String,
    seat: i8,
}

struct Room {
    id: String,
    name: String,
    theme: &'static str,
    permanent: bool,
    seats: [Option<String>; 2],
    spectators: HashSet<String>,
    game: Option<Match>,
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
}

pub struct Store {
    started: Instant,
    pub players: HashMap<String, Player>,
    rooms: Vec<Room>,
    accounts: HashMap<String, Account>,
    next_connection: u64,
    ranked_results: UnboundedSender<RankedResult>,
}

fn random_token(bytes: usize) -> String {
    let mut data = vec![0; bytes];
    OsRng.fill_bytes(&mut data);
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn clean_name(input: Option<&str>, limit: usize) -> String {
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
            players: HashMap::new(),
            rooms,
            accounts: HashMap::new(),
            next_connection: 0,
            ranked_results,
        }
    }

    pub fn now(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn room_index(&self, id: &str) -> Option<usize> {
        self.rooms.iter().position(|room| room.id == id)
    }

    fn current_room(&self, token: &str) -> Option<usize> {
        self.room_index(self.players.get(token)?.room.as_deref()?)
    }

    fn profile(&self, player: &Player) -> Option<&Profile> {
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
            "online": self.players.values().filter(|player| player.connection.is_some()).count(),
            "rooms": self.rooms.iter().map(|room| json!({ "id": room.id, "name": room.name, "theme": room.theme,
                "count": room.count(), "joinable": room.joinable(), "watchable": room.watchable(), "playing": room.playing() })).collect::<Vec<_>>(),
            "room": room.map(|room| json!({
                "id": room.id, "name": room.name, "seat": room.seat(token).map_or(-1, |seat| seat as i8),
                "seats": room.seats.each_ref().map(|seat| seat.as_ref().and_then(|key| self.players.get(key)).map(|person| json!({
                    "name": person.name, "ready": person.ready, "online": person.connection.is_some(),
                    "avatar": self.profile(person).map_or("", |profile| profile.avatar.as_str()) }))),
                "game": room.game, "guidance": room.guidance,
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
            let players = room.seats.each_ref().map(|seat| {
                let profile = self.profile(&self.players[seat.as_ref()?])?;
                Some(RankedPlayer {
                    id: profile.id,
                    name: clean_name(Some(&profile.username), 32),
                })
            });
            let ranked = players.iter().any(Option::is_some).then(|| {
                (
                    RankedMatch {
                        id: random_token(16),
                        players,
                    },
                    self.ranked_results.clone(),
                )
            });
            let room = &mut self.rooms[index];
            room.game = Some(Match::new(now, ranked));
            for key in room.seats.iter().flatten() {
                self.players.get_mut(key).expect("occupied seat").ready = false;
            }
        }
    }

    pub fn tick(&mut self) {
        let now = self.now();
        let mut dirty = false;
        for room in &mut self.rooms {
            if let Some(game) = &mut room.game {
                dirty |= game.expire(now);
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

    fn leave(&mut self, token: &str) {
        let Some(index) = self.current_room(token) else {
            return;
        };
        let now = self.now();
        let room = &mut self.rooms[index];
        if let Some(seat) = room.seat(token) {
            room.seats[seat] = None;
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
            if current.is_some() {
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
            if current.is_some_and(|current| current != index) {
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
        }
        let player = self.players.get_mut(token).expect("validated player");
        player.room = Some(room.id.clone());
        player.ready = false;
        Ok(())
    }

    fn send_message(
        &mut self,
        token: &str,
        index: usize,
        text: Option<&str>,
    ) -> Result<String, Failure> {
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
        let room = &mut self.rooms[index];
        room.guidance.push_back(Message {
            id: room.next_message,
            name: player.name.clone(),
            text: text.into(),
            seat: room.seat(token).map_or(-1, |seat| seat as i8),
        });
        room.next_message += 1;
        if room.guidance.len() > 40 {
            room.guidance.pop_front();
        }
        Ok(room.id.clone())
    }

    pub fn command(&mut self, token: &str, data: &Value) -> Result<Option<String>, Failure> {
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
                let id = loop {
                    let id = OsRng.gen_range(2000..10000).to_string();
                    if self.room_index(&id).is_none() {
                        break id;
                    }
                };
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
