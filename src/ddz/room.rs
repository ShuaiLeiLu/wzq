//! Dou Dizhu tables in the shared store: seats, bots, chat, and passing
//! player actions to the deal.

use std::collections::{HashSet, VecDeque};

use serde_json::{Value, json};

use super::table::{Event, Table};
use crate::{
    game::{Message, SYSTEM_SEAT, Store, clean_name, random_token},
    ranking::{DdzMatch, RankedPlayer},
};

type Failure = &'static str;
const TABLE_LIMIT: usize = 100;
const BOT_NAMES: [&str; 6] = ["阿福", "小满", "团团", "豆包", "十三", "阿宝"];
const BOT_CHEERS: [&str; 4] = [
    "承让承让～",
    "这把手气不错！",
    "下一局再来！",
    "牌好不如打得巧。",
];

pub enum Occupant {
    Human(String),
    /// A player who left mid-deal; 托管 plays the seat until the deal ends.
    Departed(String),
    Bot(String),
}

pub struct DdzRoom {
    pub id: String,
    name: String,
    permanent: bool,
    practice: bool,
    seats: [Option<Occupant>; 3],
    spectators: HashSet<String>,
    table: Option<Table>,
    ranked: Option<DdzMatch>,
    guidance: VecDeque<Message>,
    next_message: u64,
    cheers: usize,
}

impl DdzRoom {
    fn new(id: String, name: String, permanent: bool, practice: bool) -> Self {
        Self {
            id,
            name,
            permanent,
            practice,
            seats: [None, None, None],
            spectators: HashSet::new(),
            table: None,
            ranked: None,
            guidance: VecDeque::new(),
            next_message: 1,
            cheers: 0,
        }
    }

    pub fn permanent_tables() -> Vec<Self> {
        [
            ("1101", "春茗牌桌"),
            ("1102", "夏荷牌桌"),
            ("1103", "秋枫牌桌"),
            ("1104", "冬梅牌桌"),
        ]
        .into_iter()
        .map(|(id, name)| Self::new(id.into(), name.into(), true, false))
        .collect()
    }

    fn seat(&self, token: &str) -> Option<usize> {
        self.seats
            .iter()
            .position(|seat| matches!(seat, Some(Occupant::Human(t)) if t == token))
    }

    fn humans(&self) -> impl Iterator<Item = &String> {
        self.seats.iter().filter_map(|seat| match seat {
            Some(Occupant::Human(token)) => Some(token),
            _ => None,
        })
    }

    fn bots(&self) -> usize {
        self.seats
            .iter()
            .filter(|seat| matches!(seat, Some(Occupant::Bot(_))))
            .count()
    }

    fn occupied(&self) -> usize {
        self.seats.iter().flatten().count()
    }

    fn playing(&self) -> bool {
        self.table.as_ref().is_some_and(Table::playing)
    }

    fn joinable(&self) -> bool {
        !self.practice
            && !self.playing()
            && self
                .seats
                .iter()
                .any(|seat| matches!(seat, None | Some(Occupant::Bot(_))))
    }

    fn watchable(&self) -> bool {
        !self.practice
            && self.spectators.len() < 30
            && (self.humans().next().is_some() || !self.spectators.is_empty())
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
        self.post("牌桌".into(), text, SYSTEM_SEAT);
    }

    fn free_bot_name(&self) -> String {
        BOT_NAMES
            .iter()
            .map(|name| format!("机器人·{name}"))
            .find(|name| {
                !self
                    .seats
                    .iter()
                    .any(|seat| matches!(seat, Some(Occupant::Bot(n)) if n == name))
            })
            .expect("more bot names than seats")
    }

    /// Clears what only made sense while people sat here.
    fn reset_if_deserted(&mut self) {
        if self.humans().next().is_some() {
            return;
        }
        self.table = None;
        self.ranked = None;
        self.seats = [None, None, None];
        if self.spectators.is_empty() {
            self.guidance.clear();
        }
    }
}

fn card_list(value: &Value) -> Option<Vec<u8>> {
    let list = value.as_array()?;
    if list.is_empty() || list.len() > 20 {
        return None;
    }
    list.iter()
        .map(|card| card.as_u64().filter(|&c| c < 54).map(|c| c as u8))
        .collect()
}

fn room_id(data: &Value) -> Option<String> {
    data["id"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| data["id"].as_u64().map(|id| id.to_string()))
}

impl Store {
    pub(crate) fn table_index(&self, id: &str) -> Option<usize> {
        self.tables.iter().position(|room| room.id == id)
    }

    pub(crate) fn current_table(&self, token: &str) -> Option<usize> {
        self.table_index(self.players.get(token)?.room.as_deref()?)
    }

    fn seat_names(&self, index: usize) -> [String; 3] {
        self.tables[index].seats.each_ref().map(|seat| match seat {
            Some(Occupant::Human(token)) => self.players[token].name.clone(),
            Some(Occupant::Departed(name) | Occupant::Bot(name)) => name.clone(),
            None => "空位".into(),
        })
    }

    pub(crate) fn table_summaries(&self) -> Vec<Value> {
        self.tables
            .iter()
            .filter(|room| !room.practice)
            .map(|room| {
                json!({
                    "id": room.id, "name": room.name, "permanent": room.permanent,
                    "humans": room.humans().count(), "bots": room.bots(), "count": room.occupied(),
                    "joinable": room.joinable(), "watchable": room.watchable(), "playing": room.playing(),
                })
            })
            .collect()
    }

    pub(crate) fn table_snapshot(&self, token: &str) -> Option<Value> {
        let room = &self.tables[self.current_table(token)?];
        let seat = room.seat(token);
        let seats = room.seats.each_ref().map(|occupant| {
            occupant.as_ref().map(|occupant| match occupant {
                Occupant::Human(key) => {
                    let person = &self.players[key];
                    json!({
                        "name": person.name, "ready": person.ready, "online": person.connection.is_some(),
                        "avatar": self.profile(person).map_or("", |profile| profile.avatar.as_str()),
                        "bot": false, "departed": false,
                    })
                }
                Occupant::Departed(name) => json!({
                    "name": name, "ready": true, "online": false, "avatar": "", "bot": false, "departed": true,
                }),
                Occupant::Bot(name) => json!({
                    "name": name, "ready": true, "online": true, "avatar": "", "bot": true, "departed": false,
                }),
            })
        });
        Some(json!({
            "id": room.id, "name": room.name, "practice": room.practice,
            "seat": seat.map_or(-1, |seat| seat as i8),
            "seats": seats,
            "game": room.table.as_ref().map(|table| table.view(seat)),
            "ranked": room.ranked.is_some(),
            "guidance": room.guidance,
            "spectatorCount": room.spectators.iter().filter(|key| self.online(key)).count(),
        }))
    }

    /// Starts a deal once every seat is filled and every human is ready.
    pub(crate) fn sync_table(&mut self, index: usize) {
        let room = &self.tables[index];
        if room.playing() {
            return;
        }
        let ready = room.seats.iter().all(|seat| match seat {
            Some(Occupant::Bot(_)) => true,
            Some(Occupant::Human(key)) => self
                .players
                .get(key)
                .is_some_and(|person| person.ready && person.connection.is_some()),
            _ => false,
        });
        if !ready {
            return;
        }
        let players = room.seats.each_ref().map(|seat| match seat {
            Some(Occupant::Human(key)) => {
                self.profile(&self.players[key])
                    .map(|profile| RankedPlayer {
                        id: profile.id,
                        name: clean_name(Some(&profile.username), 32),
                    })
            }
            _ => None,
        });
        // Deals with bots stay off the daily ranking.
        let all_human = room.humans().count() == 3;
        let ranked = (all_human && players.iter().any(Option::is_some)).then(|| DdzMatch {
            id: random_token(16),
            players,
        });
        let humans: Vec<String> = room.humans().cloned().collect();
        let now = self.now();
        for key in &humans {
            self.players.get_mut(key).expect("seated player").ready = false;
        }
        let room = &mut self.tables[index];
        room.table = Some(Table::new(now));
        room.ranked = ranked;
    }

    /// Bots, 托管 and the clock. Returns whether anything changed.
    pub(crate) fn tick_tables(&mut self, now: u64) -> bool {
        let mut dirty = false;
        for index in 0..self.tables.len() {
            let room = &self.tables[index];
            if !room.playing() {
                continue;
            }
            let bots = room
                .seats
                .each_ref()
                .map(|seat| !matches!(seat, Some(Occupant::Human(_))));
            let online = room.seats.each_ref().map(|seat| match seat {
                Some(Occupant::Human(key)) => self.online(key),
                _ => true,
            });
            let table = self.tables[index].table.as_mut().expect("deal in progress");
            if table.tick(now, bots, online) {
                self.after_deal_action(index);
                dirty = true;
            }
        }
        dirty
    }

    /// Announces what just happened and settles a finished deal.
    fn after_deal_action(&mut self, index: usize) {
        let names = self.seat_names(index);
        let room = &mut self.tables[index];
        let Some(table) = &mut room.table else {
            return;
        };
        let events = std::mem::take(&mut table.events);
        let landlord = table.landlord;
        let outcome = table.outcome.clone();
        let multiplier = table.multiplier();
        for event in events {
            match event {
                Event::Redeal => room.announce("三家都不叫，重新发牌。".into()),
                Event::Landlord(seat) => {
                    room.announce(format!("{} 成为地主，底牌已亮出。", names[seat]));
                }
                Event::Bomb(seat, rocket) => room.announce(format!(
                    "{} 打出{}，倍数翻倍！",
                    names[seat],
                    if rocket { "王炸" } else { "炸弹" }
                )),
                Event::Finished => {
                    let outcome = outcome.as_ref().expect("finished deal has an outcome");
                    let landlord = landlord.expect("finished deal has a landlord");
                    let side = if outcome.landlord_won {
                        format!("地主 {} 获胜", names[landlord])
                    } else {
                        let farmers: Vec<&str> = (0..3)
                            .filter(|&s| s != landlord)
                            .map(|s| names[s].as_str())
                            .collect();
                        format!("农民 {} 获胜", farmers.join("、"))
                    };
                    let spring = match outcome.spring {
                        Some("spring") => "，春天！",
                        Some(_) => "，反春！",
                        None => "",
                    };
                    room.announce(format!("{side}{spring}本局 {multiplier} 倍。"));
                    let winners = [0, 1, 2].map(|s| (s == landlord) == outcome.landlord_won);
                    if let Some(game) = room.ranked.take()
                        && let Some(results) = &self.ddz_results
                        && results.send(game.finish(outcome.deltas, winners)).is_err()
                    {
                        eprintln!("dou dizhu result writer stopped");
                    }
                    if let Some(Occupant::Bot(name)) = &room.seats[outcome.winner] {
                        let cheer = BOT_CHEERS[room.cheers % BOT_CHEERS.len()];
                        let (name, seat) = (name.clone(), outcome.winner as i8);
                        room.cheers += 1;
                        room.post(name, cheer.into(), seat);
                    }
                    for seat in &mut room.seats {
                        if matches!(seat, Some(Occupant::Departed(_))) {
                            *seat = None;
                        }
                    }
                    room.reset_if_deserted();
                }
            }
        }
        self.remove_deserted();
    }

    fn remove_deserted(&mut self) {
        self.tables.retain(|room| {
            room.permanent || room.humans().next().is_some() || !room.spectators.is_empty()
        });
    }

    pub(crate) fn leave_table(&mut self, token: &str, index: usize) {
        let name = self.players[token].name.clone();
        let room = &mut self.tables[index];
        if let Some(seat) = room.seat(token) {
            if room.playing() {
                room.seats[seat] = Some(Occupant::Departed(name.clone()));
                if let Some(table) = &mut room.table {
                    table.trustee[seat] = true;
                }
                room.announce(format!("{name} 离开了牌桌，本局由托管代打。"));
            } else {
                room.seats[seat] = None;
            }
        } else {
            room.spectators.remove(token);
        }
        room.reset_if_deserted();
        let player = self.players.get_mut(token).expect("validated player");
        player.room = None;
        player.ready = false;
        self.remove_deserted();
    }

    fn join_table(&mut self, token: &str, index: usize, watching: bool) -> Result<(), Failure> {
        let elsewhere = self.players[token].room.is_some();
        let room = &mut self.tables[index];
        if watching {
            if room.spectators.contains(token) {
                return Ok(());
            }
            if elsewhere {
                return Err("请先退出当前房间。");
            }
            if !room.watchable() {
                return Err("这张牌桌当前无法观战。");
            }
            room.spectators.insert(token.into());
        } else {
            if room.seat(token).is_some() {
                return Ok(());
            }
            if elsewhere && !room.spectators.contains(token) {
                return Err("请先退出当前房间。");
            }
            if room.practice {
                return Err("这是人机练习桌，不能加入。");
            }
            if room.playing() {
                return Err("牌局进行中，可以先观战，下一局再入座。");
            }
            let slot = room
                .seats
                .iter()
                .position(Option::is_none)
                .or_else(|| {
                    room.seats
                        .iter()
                        .position(|seat| matches!(seat, Some(Occupant::Bot(_))))
                })
                .ok_or("牌桌已满，请换一张牌桌。")?;
            room.spectators.remove(token);
            room.seats[slot] = Some(Occupant::Human(token.into()));
            room.table = None;
        }
        let player = self.players.get_mut(token).expect("validated player");
        player.room = Some(room.id.clone());
        player.ready = false;
        Ok(())
    }

    fn create_table(&mut self, token: &str, data: &Value, practice: bool) -> Result<(), Failure> {
        if self.players[token].room.is_some() {
            return Err("请先退出当前房间。");
        }
        if self.tables.len() >= TABLE_LIMIT {
            return Err("牌桌数量已达上限，请稍后再试。");
        }
        let id = self.free_room_id();
        let player_name = self.players[token].name.clone();
        let name = clean_name(data["name"].as_str(), 20);
        let name = if practice {
            format!("{player_name}的练习桌")
        } else if name.is_empty() {
            format!("{player_name}的牌桌")
        } else {
            name
        };
        let mut room = DdzRoom::new(id.clone(), name, false, practice);
        room.seats[0] = Some(Occupant::Human(token.into()));
        if practice {
            for seat in 1..3 {
                room.seats[seat] = Some(Occupant::Bot(room.free_bot_name()));
            }
            room.announce("人机练习不计排行，祝你玩得开心。".into());
        }
        self.tables.push(room);
        let player = self.players.get_mut(token).expect("validated player");
        player.room = Some(id);
        player.ready = practice;
        if practice {
            self.sync_table(self.tables.len() - 1);
        }
        Ok(())
    }

    fn quick_table(&mut self, token: &str) -> Result<(), Failure> {
        if self.players[token].room.is_some() {
            return Err("请先退出当前房间。");
        }
        let open = |room: &DdzRoom| room.joinable() && room.seats.iter().any(Option::is_none);
        let waiting = (1..=2).rev().find_map(|humans| {
            self.tables
                .iter()
                .position(|room| open(room) && room.humans().count() == humans)
        });
        let index = waiting.or_else(|| {
            self.tables
                .iter()
                .position(|room| open(room) && room.permanent)
        });
        match index {
            Some(index) => self.join_table(token, index, false),
            None => self.create_table(token, &Value::Null, false),
        }
    }

    fn deal_action(
        &mut self,
        token: &str,
        index: usize,
        action: &str,
        data: &Value,
    ) -> Result<Option<String>, Failure> {
        let now = self.now();
        let name = self.players[token].name.clone();
        let room = &mut self.tables[index];
        let seat = room.seat(token).ok_or("只有入座的玩家可以操作。")?;
        let id = room.id.clone();
        if action == "ddz-bot" {
            if room.playing() {
                return Err("牌局进行中，不能补机器人。");
            }
            let slot = room
                .seats
                .iter()
                .position(Option::is_none)
                .ok_or("牌桌已经坐满了。")?;
            let bot = room.free_bot_name();
            room.announce(format!("{name} 请来了 {bot}。"));
            room.seats[slot] = Some(Occupant::Bot(bot));
            room.table = None;
            self.sync_table(index);
            return Ok(None);
        }
        let table = room
            .table
            .as_mut()
            .filter(|table| table.playing())
            .ok_or("牌局还没开始。")?;
        let yes = data["yes"].as_bool().unwrap_or(false);
        match action {
            "ddz-bid" => table.bid(seat, yes, now)?,
            "ddz-double" => table.double(seat, yes, now)?,
            "ddz-show" => {
                table.show(seat)?;
                room.announce(format!("{name} 明牌，倍数翻倍！"));
            }
            "ddz-play" => {
                let cards = card_list(&data["cards"]).ok_or("请选择要出的牌。")?;
                table.play(seat, &cards, now)?;
            }
            "ddz-pass" => table.pass(seat, now)?,
            "ddz-trustee" => table.set_trustee(seat, yes)?,
            _ => return Err("无法执行此操作。"),
        }
        self.after_deal_action(index);
        // A finished deal changes the lobby too.
        let still_playing = self
            .table_index(&id)
            .is_some_and(|i| self.tables[i].playing());
        Ok(still_playing.then_some(id))
    }

    /// Handles table actions, or returns `None` for the Gomoku rooms.
    pub(crate) fn ddz_command(
        &mut self,
        token: &str,
        data: &Value,
    ) -> Option<Result<Option<String>, Failure>> {
        let action = data["action"].as_str()?;
        let at_table = self.current_table(token);
        let result = match action {
            "ddz-create" => self.create_table(token, data, false).map(|()| None),
            "ddz-practice" => self.create_table(token, data, true).map(|()| None),
            "ddz-quick" => self.quick_table(token).map(|()| None),
            action if action.starts_with("ddz-") => match at_table {
                Some(index) => self.deal_action(token, index, action, data),
                None => Err("请先入座牌桌。"),
            },
            "join" | "watch" => match room_id(data).and_then(|id| self.table_index(&id)) {
                Some(index) => self
                    .join_table(token, index, action == "watch")
                    .map(|()| None),
                None if at_table.is_some() => Err("请先离开当前牌桌。"),
                None => return None,
            },
            "create" | "quick" | "move" | "handicap" | "undo" | "undo-reply"
                if at_table.is_some() =>
            {
                Err("请先离开当前牌桌。")
            }
            "guidance" => {
                let index = at_table?;
                self.chat_line(token, data["text"].as_str())
                    .map(|(name, text)| {
                        let room = &mut self.tables[index];
                        let seat = room.seat(token).map_or(-1, |seat| seat as i8);
                        room.post(name, text, seat);
                        Some(room.id.clone())
                    })
            }
            "ready" => {
                let index = at_table?;
                let room = &self.tables[index];
                if room.seat(token).is_none() || room.playing() {
                    Err("当前无法切换准备状态。")
                } else {
                    self.players.get_mut(token).expect("validated player").ready =
                        data["ready"].as_bool().unwrap_or(false);
                    self.sync_table(index);
                    Ok(None)
                }
            }
            _ => return None,
        };
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Profile;

    fn store() -> Store {
        let (scores, _) = tokio::sync::mpsc::unbounded_channel();
        Store::new(scores)
    }

    fn guest(store: &mut Store) -> String {
        let token = store.session(None, None);
        store.connect(&token);
        token
    }

    fn act(store: &mut Store, token: &str, data: Value) -> Result<Option<String>, Failure> {
        store.command(token, &data)
    }

    #[test]
    fn practice_starts_at_once_and_bots_finish_the_deal() {
        let mut store = store();
        let me = guest(&mut store);
        act(&mut store, &me, json!({"action": "ddz-practice"})).unwrap();
        let index = store.current_table(&me).unwrap();
        assert!(store.tables[index].playing());
        assert!(
            !store
                .table_summaries()
                .iter()
                .any(|t| t["id"] == store.tables[index].id.as_str())
        );
        act(
            &mut store,
            &me,
            json!({"action": "ddz-trustee", "yes": true}),
        )
        .unwrap();
        let mut now = store.now();
        while store.tables[index].playing() {
            now += 100;
            store.tick_tables(now);
        }
        let snapshot = store.table_snapshot(&me).unwrap();
        assert_eq!(snapshot["game"]["phase"], "finished");
        assert!(
            store.tables[index]
                .guidance
                .iter()
                .any(|m| m.text.contains("获胜"))
        );
        act(&mut store, &me, json!({"action": "leave"})).unwrap();
        assert!(
            store
                .table_index(&snapshot["id"].as_str().unwrap().to_string())
                .is_none()
        );
    }

    #[test]
    fn three_humans_rank_and_leavers_are_played_by_trustee() {
        let (scores, _) = tokio::sync::mpsc::unbounded_channel();
        let mut store = Store::new(scores);
        let (ddz, mut results) = tokio::sync::mpsc::unbounded_channel();
        store.attach_ddz_results(ddz);
        let players: Vec<String> = (0..3).map(|_| guest(&mut store)).collect();
        store.establish_account(
            &players[0],
            None,
            Profile {
                id: 7,
                username: "牌友 7".into(),
                avatar: String::new(),
            },
        );
        for token in &players {
            act(&mut store, token, json!({"action": "ddz-quick"})).unwrap();
        }
        let index = store.current_table(&players[0]).unwrap();
        assert_eq!(
            store.current_table(&players[2]),
            Some(index),
            "quick fills a waiting table"
        );
        for token in &players {
            act(&mut store, token, json!({"action": "ready", "ready": true})).unwrap();
        }
        assert!(store.tables[index].ranked.is_some());
        // Everyone else may not act out of turn, and a gomoku room is off limits.
        assert!(
            act(
                &mut store,
                &players[1],
                json!({"action": "join", "id": "1001"})
            )
            .is_err()
        );
        act(&mut store, &players[1], json!({"action": "leave"})).unwrap();
        assert!(matches!(
            store.tables[index].seats[1],
            Some(Occupant::Departed(_))
        ));
        let mut now = store.now();
        while store.tables[index].playing() {
            now += 100;
            store.tick_tables(now);
        }
        let result = results.try_recv().expect("ranked deal reported");
        drop(result);
        assert!(
            store.tables[index].seats[1].is_none(),
            "the leaver's seat opens after the deal"
        );
    }

    #[test]
    fn bots_fill_seats_and_humans_replace_them_between_deals() {
        let mut store = store();
        let host = guest(&mut store);
        let friend = guest(&mut store);
        act(&mut store, &host, json!({"action": "ddz-create"})).unwrap();
        let id = store.tables.last().unwrap().id.clone();
        act(&mut store, &host, json!({"action": "ddz-bot"})).unwrap();
        act(&mut store, &host, json!({"action": "ddz-bot"})).unwrap();
        assert!(act(&mut store, &host, json!({"action": "ddz-bot"})).is_err());
        act(&mut store, &friend, json!({"action": "join", "id": id})).unwrap();
        let index = store.table_index(&id).unwrap();
        assert_eq!(store.tables[index].bots(), 1);
        assert!(store.free_room_id() != id);
        let view = store.table_snapshot(&friend).unwrap();
        assert_eq!(
            view["seats"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|s| s["bot"] == true)
                .count(),
            1
        );
    }
}
