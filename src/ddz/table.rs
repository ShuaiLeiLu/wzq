//! One Dou Dizhu deal: bidding, doubling, play and settlement.
//!
//! Seats are `0..3` and play passes to `(seat + 1) % 3`. The table knows
//! nothing about players; the room tells it each tick which seats are bots
//! and which humans are online.

use std::collections::VecDeque;

use rand::{Rng, seq::SliceRandom};
use serde::Serialize;
use serde_json::{Value, json};

use crate::history::Journal;

use super::{
    bot,
    cards::{self, Card, Combo, DECK, Kind},
};

pub const BID_MS: u64 = 15_000;
pub const DOUBLE_MS: u64 = 15_000;
pub const PLAY_MS: u64 = 25_000;
/// Time the clients spend animating the deal before bidding starts.
pub const DEAL_MS: u64 = 2_200;
/// How long a bot "thinks" before acting.
const BOT_MS: u64 = 1_000;
/// How long 托管 waits before acting for a human.
const TRUSTEE_MS: u64 = 800;
/// A disconnected player's turn is played for them after this long.
const OFFLINE_MS: u64 = 3_000;
/// The most a single farmer can win or lose in one deal.
pub const CAP: i64 = 64;
type Failure = &'static str;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Bidding,
    Doubling,
    Playing,
    Finished,
}

/// What a seat did most recently, shown beside it on the table.
#[derive(Clone, Debug, Serialize)]
pub struct Act {
    pub text: &'static str,
    pub cards: Vec<Card>,
    pub kind: Option<Kind>,
}

impl Act {
    fn say(text: &'static str) -> Self {
        Self {
            text,
            cards: Vec::new(),
            kind: None,
        }
    }
}

#[derive(Clone, Debug)]
struct Play {
    seat: usize,
    combo: Combo,
}

#[derive(Debug)]
struct Bidding {
    caller: Option<usize>,
    declined: [bool; 3],
    queue: VecDeque<usize>,
    last_robber: Option<usize>,
    caller_final: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub winner: usize,
    pub landlord_won: bool,
    /// "spring", "antiSpring" or none.
    pub spring: Option<&'static str>,
    pub multiplier: i64,
    pub deltas: [i64; 3],
}

/// Things the room announces in the chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Redeal,
    Landlord(usize),
    Bomb(usize, bool),
    Finished,
}

pub struct Table {
    pub(crate) journal: Journal,
    rejected_this_turn: bool,
    pub phase: Phase,
    hands: [Vec<Card>; 3],
    bottom: Vec<Card>,
    pub landlord: Option<usize>,
    pub turn: usize,
    pub deadline: u64,
    turn_started: u64,
    bidding: Bidding,
    pub acts: [Option<Act>; 3],
    pub robs: u8,
    pub doubled: [Option<bool>; 3],
    pub shown: bool,
    pub bombs: u8,
    last: Option<Play>,
    played: [u16; 3],
    played_cards: Vec<Card>,
    pub trustee: [bool; 3],
    pub outcome: Option<Outcome>,
    pub events: Vec<Event>,
    /// Counts deals, redeals included, so clients can animate each one.
    deals: u32,
}

fn next(seat: usize) -> usize {
    (seat + 1) % 3
}

impl Table {
    pub fn new(now: u64) -> Self {
        let mut table = Self {
            journal: Journal::default(),
            rejected_this_turn: false,
            phase: Phase::Bidding,
            hands: Default::default(),
            bottom: Vec::new(),
            landlord: None,
            turn: 0,
            deadline: 0,
            turn_started: 0,
            bidding: Bidding {
                caller: None,
                declined: [false; 3],
                queue: VecDeque::new(),
                last_robber: None,
                caller_final: false,
            },
            acts: Default::default(),
            robs: 0,
            doubled: [None; 3],
            shown: false,
            bombs: 0,
            last: None,
            played: [0; 3],
            played_cards: Vec::new(),
            trustee: [false; 3],
            outcome: None,
            events: Vec::new(),
            deals: 0,
        };
        table.deal(now);
        table
    }

    fn deal(&mut self, now: u64) {
        let mut rng = rand::thread_rng();
        let mut deck: Vec<Card> = (0..DECK).collect();
        deck.shuffle(&mut rng);
        for (seat, hand) in self.hands.iter_mut().enumerate() {
            *hand = deck[seat * 17..seat * 17 + 17].to_vec();
            cards::sort_hand(hand);
        }
        self.bottom = deck[51..].to_vec();
        cards::sort_hand(&mut self.bottom);
        let start = rng.gen_range(0..3);
        self.bidding = Bidding {
            caller: None,
            declined: [false; 3],
            queue: VecDeque::from([start, next(start), next(next(start))]),
            last_robber: None,
            caller_final: false,
        };
        self.robs = 0;
        self.acts = Default::default();
        self.deals += 1;
        // Bidding (and its clock) starts once the cards have been dealt out.
        self.start_turn(start, now + DEAL_MS, BID_MS);
    }

    fn start_turn(&mut self, seat: usize, now: u64, limit: u64) {
        self.rejected_this_turn = false;
        self.turn = seat;
        self.turn_started = now;
        self.deadline = now + limit;
    }

    /// The common multiplier: robbing, bombs, spring and 明牌 each double it.
    pub fn multiplier(&self) -> i64 {
        let spring = self.outcome.as_ref().is_some_and(|o| o.spring.is_some());
        1 << (u32::from(self.robs)
            + u32::from(self.bombs)
            + u32::from(spring)
            + u32::from(self.shown))
    }

    pub fn playing(&self) -> bool {
        self.phase != Phase::Finished
    }

    fn robbing(&self) -> bool {
        self.bidding.caller.is_some()
    }

    pub fn bid(&mut self, seat: usize, yes: bool, now: u64) -> Result<(), Failure> {
        if self.phase != Phase::Bidding {
            return Err("现在不是叫地主的时候。");
        }
        if self.turn != seat {
            return Err("还没轮到你叫地主。");
        }
        let label = match (self.robbing(), yes) {
            (false, true) => "叫地主",
            (false, false) => "不叫",
            (true, true) => "抢地主",
            (true, false) => "不抢",
        };
        self.journal.event(Some(seat), label, Value::Null);
        let bidding = &mut self.bidding;
        bidding.queue.pop_front();
        match bidding.caller {
            None if yes => {
                bidding.caller = Some(seat);
                bidding.queue = [next(seat), next(next(seat))]
                    .into_iter()
                    .filter(|&other| !bidding.declined[other])
                    .collect();
                self.acts[seat] = Some(Act::say("叫地主"));
            }
            None => {
                bidding.declined[seat] = true;
                self.acts[seat] = Some(Act::say("不叫"));
                if bidding.queue.is_empty() {
                    self.journal.event(None, "重新发牌", Value::Null);
                    self.events.push(Event::Redeal);
                    self.deal(now);
                    return Ok(());
                }
            }
            Some(caller) => {
                if yes {
                    self.robs += 1;
                    bidding.last_robber = Some(seat);
                }
                self.acts[seat] = Some(Act::say(if yes { "抢地主" } else { "不抢" }));
                if seat == caller {
                    let landlord = if yes {
                        caller
                    } else {
                        bidding.last_robber.unwrap_or(caller)
                    };
                    self.crown(landlord, now);
                    return Ok(());
                }
                if bidding.queue.is_empty()
                    && bidding.last_robber.is_some()
                    && !bidding.caller_final
                {
                    bidding.caller_final = true;
                    bidding.queue.push_back(caller);
                }
            }
        }
        match self.bidding.queue.front() {
            Some(&seat) => self.start_turn(seat, now, BID_MS),
            None => {
                let landlord = self
                    .bidding
                    .last_robber
                    .or(self.bidding.caller)
                    .expect("bidding ends with a landlord");
                self.crown(landlord, now);
            }
        }
        Ok(())
    }

    fn crown(&mut self, landlord: usize, now: u64) {
        self.journal
            .event(Some(landlord), "成为地主", json!({"cards":self.bottom}));
        self.landlord = Some(landlord);
        let bottom = self.bottom.clone();
        self.hands[landlord].extend(bottom);
        cards::sort_hand(&mut self.hands[landlord]);
        self.phase = Phase::Doubling;
        self.doubled = [None; 3];
        self.events.push(Event::Landlord(landlord));
        self.turn_started = now;
        self.deadline = now + DOUBLE_MS;
    }

    pub fn double(&mut self, seat: usize, yes: bool, now: u64) -> Result<(), Failure> {
        if self.phase != Phase::Doubling {
            return Err("现在不能选择加倍。");
        }
        if self.doubled[seat].is_some() {
            return Err("你已经选择过了。");
        }
        self.journal
            .event(Some(seat), if yes { "加倍" } else { "不加倍" }, Value::Null);
        self.doubled[seat] = Some(yes);
        self.acts[seat] = Some(Act::say(if yes { "加倍" } else { "不加倍" }));
        if self.doubled.iter().all(Option::is_some) {
            self.phase = Phase::Playing;
            self.acts = Default::default();
            let landlord = self.landlord.expect("landlord chosen");
            self.start_turn(landlord, now, PLAY_MS);
        }
        Ok(())
    }

    /// 明牌: the landlord shows the hand to everyone, doubling the stakes.
    pub fn show(&mut self, seat: usize) -> Result<(), Failure> {
        let before_first_play = self.phase == Phase::Doubling
            || (self.phase == Phase::Playing && self.played_cards.is_empty());
        if self.landlord != Some(seat) || self.shown || !before_first_play {
            return Err("只有地主能在出第一手前明牌。");
        }
        self.journal.event(Some(seat), "明牌", Value::Null);
        self.shown = true;
        Ok(())
    }

    /// The play the seat to move must beat, or `None` when it leads.
    pub fn to_beat(&self) -> Option<&Combo> {
        self.last
            .as_ref()
            .filter(|play| play.seat != self.turn)
            .map(|play| &play.combo)
    }

    pub fn play(&mut self, seat: usize, chosen: &[Card], now: u64) -> Result<(), Failure> {
        if self.phase != Phase::Playing {
            return Err("现在还不能出牌。");
        }
        if self.turn != seat {
            return Err("还没轮到你出牌。");
        }
        let mut sorted = chosen.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        if sorted.len() != chosen.len()
            || sorted.iter().any(|card| !self.hands[seat].contains(card))
        {
            return Err("请选择自己手里的牌。");
        }
        let previous = self.to_beat();
        let Some(combo) = cards::parse(&sorted, previous) else {
            let reason = if previous.is_some() {
                "所选的牌压不过上家。"
            } else {
                "所选的牌不符合出牌规则。"
            };
            if !self.rejected_this_turn {
                self.journal
                    .event(Some(seat), "出牌未通过", json!({"message":reason}));
                self.rejected_this_turn = true;
            }
            return Err(reason);
        };
        self.hands[seat].retain(|card| !sorted.contains(card));
        cards::sort_hand(&mut sorted);
        self.journal.event(
            Some(seat),
            combo.kind.label(),
            json!({"cards":sorted,"left":self.hands[seat].len()}),
        );
        self.played_cards.extend(&sorted);
        self.played[seat] += 1;
        if combo.is_bomb() {
            self.bombs += 1;
            self.events
                .push(Event::Bomb(seat, combo.kind == Kind::Rocket));
        }
        self.acts[seat] = Some(Act {
            text: combo.kind.label(),
            cards: sorted,
            kind: Some(combo.kind),
        });
        self.last = Some(Play { seat, combo });
        if self.hands[seat].is_empty() {
            self.finish(seat);
        } else {
            self.advance(now);
        }
        Ok(())
    }

    pub fn pass(&mut self, seat: usize, now: u64) -> Result<(), Failure> {
        if self.phase != Phase::Playing || self.turn != seat {
            return Err("还没轮到你出牌。");
        }
        if self.to_beat().is_none() {
            return Err("这一轮由你先出，不能不出。");
        }
        self.journal.event(Some(seat), "不出", Value::Null);
        self.acts[seat] = Some(Act::say("不出"));
        self.advance(now);
        Ok(())
    }

    fn advance(&mut self, now: u64) {
        let seat = next(self.turn);
        // A new round: the leader's table clears.
        if self.last.as_ref().is_some_and(|play| play.seat == seat) {
            self.acts = Default::default();
        }
        self.acts[seat] = None;
        self.start_turn(seat, now, PLAY_MS);
    }

    fn finish(&mut self, winner: usize) {
        let landlord = self.landlord.expect("landlord chosen");
        let landlord_won = winner == landlord;
        let farmer_plays: u16 = (0..3)
            .filter(|&s| s != landlord)
            .map(|s| self.played[s])
            .sum();
        let spring = if landlord_won && farmer_plays == 0 {
            Some("spring")
        } else if !landlord_won && self.played[landlord] == 1 {
            Some("antiSpring")
        } else {
            None
        };
        self.phase = Phase::Finished;
        self.outcome = Some(Outcome {
            winner,
            landlord_won,
            spring,
            multiplier: 0,
            deltas: [0; 3],
        });
        let common = self.multiplier();
        let factor = |seat: usize| {
            if self.doubled[seat] == Some(true) {
                2
            } else {
                1
            }
        };
        let mut deltas = [0i64; 3];
        for farmer in (0..3).filter(|&s| s != landlord) {
            let amount = (common * factor(landlord) * factor(farmer)).min(CAP);
            let signed = if landlord_won { amount } else { -amount };
            deltas[landlord] += signed;
            deltas[farmer] -= signed;
        }
        let outcome = self.outcome.as_mut().expect("just set");
        outcome.multiplier = common;
        outcome.deltas = deltas;
        self.journal.finish(if landlord_won {"地主获胜"} else {"农民获胜"}, json!({"winnerSeat":winner,"landlord":landlord,"counts": self.hands.each_ref().map(Vec::len), "multiplier":common,"deltas":deltas,"spring":spring}));
        self.events.push(Event::Finished);
    }

    pub fn set_trustee(&mut self, seat: usize, on: bool) -> Result<(), Failure> {
        if !self.playing() {
            return Err("对局已结束。");
        }
        if self.trustee[seat] != on {
            self.journal.event(
                Some(seat),
                if on { "开启托管" } else { "取消托管" },
                Value::Null,
            );
        }
        self.trustee[seat] = on;
        Ok(())
    }

    /// Acts for bots, 托管 and disconnected players, and enforces the clock.
    /// Returns whether anything changed.
    pub fn tick(&mut self, now: u64, bots: [bool; 3], online: [bool; 3]) -> bool {
        let delay = |table: &Self, seat: usize| {
            if bots[seat] {
                Some(BOT_MS + (seat as u64 * 211 + table.played_cards.len() as u64 * 97) % 700)
            } else if table.trustee[seat] {
                Some(TRUSTEE_MS)
            } else if !online[seat] {
                Some(OFFLINE_MS)
            } else {
                None
            }
        };
        match self.phase {
            Phase::Finished => false,
            Phase::Doubling => {
                let mut changed = false;
                for seat in 0..3 {
                    if self.doubled[seat].is_some()
                        || !self.playing()
                        || self.phase != Phase::Doubling
                    {
                        continue;
                    }
                    let due = delay(self, seat).is_some_and(|wait| now >= self.turn_started + wait);
                    if due || now >= self.deadline {
                        let yes = bots[seat]
                            && bot::wants_double(&self.hands[seat], self.landlord == Some(seat));
                        self.double(seat, yes, now)
                            .expect("undecided seat may double");
                        changed = true;
                    }
                }
                changed
            }
            Phase::Bidding | Phase::Playing => {
                let seat = self.turn;
                let timed_out = now >= self.deadline;
                let due = delay(self, seat).is_some_and(|wait| now >= self.turn_started + wait);
                if !due && !timed_out {
                    return false;
                }
                if timed_out && !bots[seat] && !self.trustee[seat] {
                    self.journal.event(Some(seat), "超时托管", Value::Null);
                    self.trustee[seat] = true;
                }
                self.act_for(seat, bots[seat], now);
                true
            }
        }
    }

    fn act_for(&mut self, seat: usize, bot_seat: bool, now: u64) {
        if self.phase == Phase::Bidding {
            // 托管 never bids for a human.
            let yes = bot_seat && bot::wants_landlord(&self.hands[seat], self.robbing());
            self.bid(seat, yes, now).expect("seat to move may bid");
            return;
        }
        let landlord = self.landlord.expect("landlord chosen");
        let previous = self.to_beat().copied();
        let mut unseen = bot::deck_counts();
        let own = cards::counts(&self.hands[seat]);
        let played = cards::counts(&self.played_cards);
        for (rank, count) in unseen.iter_mut().enumerate() {
            *count -= own[rank] + played[rank];
        }
        let view_of = |other: usize| bot::Seat {
            cards: self.hands[other].len(),
            partner: other != landlord && seat != landlord,
        };
        let (after, before) = (next(seat), next(next(seat)));
        let choice = bot::choose(&bot::View {
            hand: &self.hands[seat],
            previous: previous.as_ref(),
            unseen,
            next: view_of(after),
            before: view_of(before),
            played_by_next: self.last.as_ref().is_some_and(|play| play.seat == after),
        });
        let result = match choice {
            Some(chosen) => self.play(seat, &chosen, now),
            None if previous.is_some() => self.pass(seat, now),
            // The bot always finds a lead; fall back to the smallest card.
            None => {
                let smallest = *self.hands[seat].last().expect("a hand still in play");
                self.play(seat, &[smallest], now)
            }
        };
        result.expect("the bot only picks legal plays");
    }

    /// What `viewer` may see. Hands stay private until the deal ends, except
    /// the landlord's after 明牌.
    pub fn view(&self, viewer: Option<usize>) -> Value {
        let finished = self.phase == Phase::Finished;
        let hands: Vec<Option<&Vec<Card>>> = (0..3)
            .map(|seat| {
                let open =
                    finished || viewer == Some(seat) || (self.shown && self.landlord == Some(seat));
                open.then(|| &self.hands[seat])
            })
            .collect();
        let my_turn = viewer.is_some_and(|seat| seat == self.turn) && self.phase == Phase::Playing;
        let hints: Vec<Vec<Card>> = match viewer {
            Some(seat) if my_turn => cards::hints(&self.hands[seat], self.to_beat())
                .into_iter()
                .take(40)
                .collect(),
            _ => Vec::new(),
        };
        json!({
            "phase": self.phase,
            "deal": self.deals,
            "landlord": self.landlord,
            "turn": self.turn,
            "deadline": self.deadline,
            "bottom": if self.landlord.is_some() { self.bottom.clone() } else { Vec::new() },
            "hands": hands,
            "counts": self.hands.each_ref().map(Vec::len),
            "acts": self.acts,
            "robbing": self.robbing(),
            "robs": self.robs,
            "bombs": self.bombs,
            "shown": self.shown,
            "doubled": self.doubled,
            "multiplier": self.multiplier(),
            "trustee": self.trustee,
            "lead": my_turn && self.to_beat().is_none(),
            "toBeat": self.to_beat(),
            "lastSeat": self.last.as_ref().map(|play| play.seat),
            "hints": hints,
            "played": self.played_cards,
            "playedCount": self.played,
            "outcome": self.outcome,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::cards::tests::cards;
    use super::*;

    fn rigged(hands: [&str; 3], bottom: &str, start: usize) -> Table {
        let mut table = Table::new(0);
        // One call, so every card in the deal gets its own suit.
        let mut deck =
            cards(&format!("{} {} {} {bottom}", hands[0], hands[1], hands[2])).into_iter();
        for (seat, text) in hands.iter().enumerate() {
            table.hands[seat] = deck
                .by_ref()
                .take(text.split_whitespace().count())
                .collect();
        }
        table.bottom = deck.collect();
        assert_eq!(table.bottom.len(), 3);
        table.bidding.queue = VecDeque::from([start, next(start), next(next(start))]);
        table.start_turn(start, 0, BID_MS);
        table
    }

    fn run_bots(table: &mut Table) -> u64 {
        let mut now = 0;
        while table.playing() && now < 10_000_000 {
            now += 100;
            table.tick(now, [true; 3], [true; 3]);
        }
        now
    }

    #[test]
    fn bots_finish_many_deals_with_balanced_scores() {
        for _ in 0..300 {
            let mut table = Table::new(0);
            assert_eq!(table.hands.iter().map(Vec::len).sum::<usize>(), 51);
            run_bots(&mut table);
            assert_eq!(table.phase, Phase::Finished);
            let outcome = table.outcome.as_ref().unwrap();
            assert_eq!(outcome.deltas.iter().sum::<i64>(), 0);
            assert!(table.hands[outcome.winner].is_empty());
            let landlord = table.landlord.unwrap();
            assert_eq!(outcome.landlord_won, outcome.winner == landlord);
            assert!(outcome.deltas.iter().all(|d| d.abs() <= 2 * CAP));
            assert_eq!(
                table.played_cards.len() + table.hands.iter().map(Vec::len).sum::<usize>(),
                54
            );
        }
    }

    #[test]
    fn robbing_doubles_and_the_caller_gets_the_last_word() {
        let mut table = Table::new(0);
        let start = table.turn;
        table.bid(start, true, 1).unwrap();
        assert!(table.bid(start, true, 1).is_err(), "out of turn");
        table.bid(next(start), true, 2).unwrap();
        table.bid(next(next(start)), false, 3).unwrap();
        assert_eq!(table.turn, start, "the caller may rob back");
        table.bid(start, true, 4).unwrap();
        assert_eq!(table.landlord, Some(start));
        assert_eq!(table.robs, 2);
        assert_eq!(table.multiplier(), 4);
        assert_eq!(table.hands[start].len(), 20);
        assert_eq!(table.phase, Phase::Doubling);
    }

    #[test]
    fn decliners_cannot_rob_and_a_lone_call_wins() {
        let mut table = Table::new(0);
        let start = table.turn;
        table.bid(start, false, 1).unwrap();
        table.bid(next(start), true, 2).unwrap();
        // Only the third seat may still rob.
        assert_eq!(table.turn, next(next(start)));
        table.bid(next(next(start)), false, 3).unwrap();
        assert_eq!(table.landlord, Some(next(start)));
        assert_eq!(table.robs, 0);
    }

    #[test]
    fn three_passes_redeal() {
        let mut table = Table::new(0);
        let before = table.hands.clone();
        let start = table.turn;
        for seat in [start, next(start), next(next(start))] {
            table.bid(seat, false, 1).unwrap();
        }
        assert_eq!(table.phase, Phase::Bidding);
        assert!(table.landlord.is_none());
        assert!(table.events.contains(&Event::Redeal));
        assert_ne!(table.hands, before);
    }

    #[test]
    fn spring_and_doubling_settle_per_farmer() {
        let mut table = rigged(
            [
                "3 3 3 3 4 4 4 4 5 5 5 5 6 6 6 6 7",
                "8 8 8 8 9 9 9 9 10 10 10 10 J J J J Q",
                "Q Q Q K K K K A A A A 2 2 2 2 X D",
            ],
            "7 7 7",
            0,
        );
        table.bid(0, true, 0).unwrap();
        table.bid(1, false, 0).unwrap();
        table.bid(2, false, 0).unwrap();
        assert_eq!(table.landlord, Some(0));
        table.double(0, true, 0).unwrap();
        table.double(1, true, 0).unwrap();
        table.double(2, false, 0).unwrap();
        table.show(0).unwrap();
        // The landlord plays out while both farmers pass every round.
        for text in ["3 3 3 4 4 4 5 5 5 6 6 6", "3 4 5 6 7", "7 7 7"] {
            let chosen = cards(text);
            let mut picked = Vec::new();
            for card in chosen {
                let wanted = cards::rank(card);
                let card = *table.hands[0]
                    .iter()
                    .find(|c| cards::rank(**c) == wanted && !picked.contains(*c))
                    .unwrap();
                picked.push(card);
            }
            table.play(0, &picked, 0).unwrap();
            if table.playing() {
                table.pass(1, 0).unwrap();
                table.pass(2, 0).unwrap();
            }
        }
        let outcome = table.outcome.as_ref().unwrap();
        assert_eq!(outcome.spring, Some("spring"));
        // Common ×4 (明牌, spring); landlord doubled ×2; farmer 1 doubled ×2.
        assert_eq!(outcome.multiplier, 4);
        assert_eq!(outcome.deltas, [16 + 8, -16, -8]);
    }

    #[test]
    fn leading_cannot_pass_and_plays_must_beat() {
        let mut table = rigged(
            [
                "3 4 5 6 7 8 9 10 J Q K A 2 X 3 4 5",
                "6 6 7 7 8 8 9 9 10 10 J J Q Q K K A",
                "3 3 4 4 5 5 6 7 8 9 10 J Q K A 2 2",
            ],
            "A 2 D",
            0,
        );
        table.bid(0, true, 0).unwrap();
        table.bid(1, false, 0).unwrap();
        table.bid(2, false, 0).unwrap();
        for seat in 0..3 {
            table.double(seat, false, 0).unwrap();
        }
        assert!(table.pass(0, 0).is_err());
        let three = *table.hands[0]
            .iter()
            .find(|c| cards::rank(**c) == 0)
            .unwrap();
        table.play(0, &[three], 0).unwrap();
        let four_pair: Vec<Card> = table.hands[1]
            .iter()
            .copied()
            .filter(|c| cards::rank(*c) == 3)
            .collect();
        assert!(
            table.play(1, &four_pair, 0).is_err(),
            "a pair cannot answer a single"
        );
        let big_joker = 53;
        assert!(
            table.play(1, &[big_joker], 0).is_err(),
            "a card from another hand"
        );
    }

    #[test]
    fn timeouts_turn_on_trustee_and_hands_stay_private() {
        let mut table = Table::new(0);
        let start = table.turn;
        table.tick(DEAL_MS + BID_MS - 1, [false; 3], [true; 3]);
        assert!(
            !table.trustee[start],
            "the deal animation does not eat the clock"
        );
        table.tick(DEAL_MS + BID_MS, [false; 3], [true; 3]);
        assert!(table.trustee[start]);
        assert_ne!(table.turn, start);
        let view = table.view(Some(next(start)));
        let hands = view["hands"].as_array().unwrap();
        assert!(hands[next(start)].is_array());
        assert!(hands[start].is_null() && hands[next(next(start))].is_null());
        assert_eq!(view["bottom"].as_array().unwrap().len(), 0);
        let spectator = table.view(None);
        assert!(
            spectator["hands"]
                .as_array()
                .unwrap()
                .iter()
                .all(Value::is_null)
        );
    }
}
