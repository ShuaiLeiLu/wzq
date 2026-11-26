//! Cards, hand types and how one play beats another.
//!
//! A card is a number in `0..54`. The 52 suited cards are grouped by rank,
//! four per rank, from 3 (ranks 0) up to 2 (rank 12). Cards 52 and 53 are the
//! small and big jokers, ranks 13 and 14.

use serde::Serialize;

pub type Card = u8;
pub const DECK: u8 = 54;
pub const TWO: u8 = 12;
pub const SMALL_JOKER: u8 = 13;
pub const BIG_JOKER: u8 = 14;
/// Sequences (straights, consecutive pairs, planes) stop at A.
const TOP_SEQUENCE: u8 = 11;

pub type Counts = [u8; 15];

pub fn rank(card: Card) -> u8 {
    if card < 52 { card / 4 } else { card - 52 + 13 }
}

pub fn counts(cards: &[Card]) -> Counts {
    let mut counts = [0; 15];
    for &card in cards {
        counts[usize::from(rank(card))] += 1;
    }
    counts
}

/// Highest rank first, the order a hand is shown in.
pub fn sort_hand(cards: &mut [Card]) {
    cards.sort_unstable_by(|a, b| b.cmp(a));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Single,
    Pair,
    Triple,
    TripleSingle,
    TriplePair,
    Straight,
    PairStraight,
    Plane,
    PlaneSingles,
    PlanePairs,
    FourSingles,
    FourPairs,
    Bomb,
    Rocket,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Single => "单张",
            Kind::Pair => "对子",
            Kind::Triple => "三张",
            Kind::TripleSingle => "三带一",
            Kind::TriplePair => "三带一对",
            Kind::Straight => "顺子",
            Kind::PairStraight => "连对",
            Kind::Plane => "飞机",
            Kind::PlaneSingles | Kind::PlanePairs => "飞机带翅膀",
            Kind::FourSingles => "四带二",
            Kind::FourPairs => "四带两对",
            Kind::Bomb => "炸弹",
            Kind::Rocket => "王炸",
        }
    }
}

/// When a lead could be read more than one way, the first kind listed wins.
const LEAD_PREFERENCE: [Kind; 14] = [
    Kind::Rocket,
    Kind::Bomb,
    Kind::Straight,
    Kind::PairStraight,
    Kind::PlanePairs,
    Kind::PlaneSingles,
    Kind::Plane,
    Kind::FourPairs,
    Kind::FourSingles,
    Kind::TriplePair,
    Kind::TripleSingle,
    Kind::Triple,
    Kind::Pair,
    Kind::Single,
];

/// A recognized play. `rank` is the deciding rank (the lowest rank of a
/// sequence) and `len` the number of links in a sequence (1 otherwise).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Combo {
    pub kind: Kind,
    pub rank: u8,
    pub len: u8,
}

impl Combo {
    fn new(kind: Kind, rank: u8, len: u8) -> Self {
        Self { kind, rank, len }
    }

    pub fn is_bomb(&self) -> bool {
        matches!(self.kind, Kind::Bomb | Kind::Rocket)
    }

    pub fn beats(&self, other: &Combo) -> bool {
        match (self.kind, other.kind) {
            (Kind::Rocket, _) => true,
            (_, Kind::Rocket) => false,
            (Kind::Bomb, Kind::Bomb) => self.rank > other.rank,
            (Kind::Bomb, _) => true,
            (_, Kind::Bomb) => false,
            _ => self.kind == other.kind && self.len == other.len && self.rank > other.rank,
        }
    }
}

fn run_of(counts: &Counts, need: u8, total: usize, min_len: usize) -> Option<Combo> {
    let used: Vec<u8> = (0..15u8).filter(|&r| counts[usize::from(r)] > 0).collect();
    let low = *used.first()?;
    let high = *used.last()?;
    let len = usize::from(high - low) + 1;
    let fits = high <= TOP_SEQUENCE
        && len >= min_len
        && len * usize::from(need) == total
        && used.iter().all(|&r| counts[usize::from(r)] == need);
    fits.then(|| {
        let kind = if need == 1 {
            Kind::Straight
        } else {
            Kind::PairStraight
        };
        Combo::new(kind, low, len as u8)
    })
}

/// Every way the cards can be read as one legal play.
pub fn interpretations(cards: &[Card]) -> Vec<Combo> {
    let counts = counts(cards);
    let n = cards.len();
    let mut out = Vec::new();
    let ranks_with = |count: u8| (0..15u8).filter(move |&r| counts[usize::from(r)] == count);
    let distinct = (0..15).filter(|&r| counts[r] > 0).count();
    if n == 0 {
        return out;
    }
    if distinct == 1 {
        let r = rank(cards[0]);
        let kind = match n {
            1 => Some(Kind::Single),
            2 => Some(Kind::Pair),
            3 => Some(Kind::Triple),
            4 => Some(Kind::Bomb),
            _ => None,
        };
        out.extend(kind.map(|kind| Combo::new(kind, r, 1)));
    }
    if n == 2 && counts[13] == 1 && counts[14] == 1 {
        out.push(Combo::new(Kind::Rocket, BIG_JOKER, 1));
    }
    if n == 4 && distinct == 2 {
        out.extend(ranks_with(3).map(|r| Combo::new(Kind::TripleSingle, r, 1)));
    }
    if n == 5 && distinct == 2 && ranks_with(2).next().is_some() {
        out.extend(ranks_with(3).map(|r| Combo::new(Kind::TriplePair, r, 1)));
    }
    if n >= 5 {
        out.extend(run_of(&counts, 1, n, 5));
    }
    if n >= 6 && n.is_multiple_of(2) {
        out.extend(run_of(&counts, 2, n, 3));
    }
    for quad in ranks_with(4) {
        let mut rest = counts;
        rest[usize::from(quad)] = 0;
        // The rocket is never split up as two kickers.
        let rocket = rest[13] == 1 && rest[14] == 1;
        if n == 6 && !rocket {
            out.push(Combo::new(Kind::FourSingles, quad, 1));
        }
        if n == 8 && rest.iter().all(|&c| c % 2 == 0) {
            out.push(Combo::new(Kind::FourPairs, quad, 1));
        }
    }
    for start in 0..TOP_SEQUENCE {
        let mut rest = counts;
        for end in start..=TOP_SEQUENCE {
            if counts[usize::from(end)] < 3 {
                break;
            }
            rest[usize::from(end)] -= 3;
            let len = usize::from(end - start) + 1;
            if len < 2 {
                continue;
            }
            let wings = n as isize - 3 * len as isize;
            let combo = |kind| Combo::new(kind, start, len as u8);
            if wings == 0 {
                out.push(combo(Kind::Plane));
            } else if wings == len as isize && !(rest[13] == 1 && rest[14] == 1) {
                out.push(combo(Kind::PlaneSingles));
            } else if wings == 2 * len as isize && rest.iter().all(|&c| c % 2 == 0) {
                out.push(combo(Kind::PlanePairs));
            }
        }
    }
    out.dedup();
    out
}

/// Reads the cards as a play. Without `previous` this is a lead; with it, the
/// play must beat `previous`, and a reading of the same kind is preferred so
/// that ambiguous cards follow the table.
pub fn parse(cards: &[Card], previous: Option<&Combo>) -> Option<Combo> {
    let readings = interpretations(cards);
    match previous {
        None => LEAD_PREFERENCE
            .iter()
            .find_map(|&kind| {
                readings
                    .iter()
                    .filter(|c| c.kind == kind)
                    .max_by_key(|c| c.rank)
            })
            .copied(),
        Some(previous) => {
            let beating = || readings.iter().filter(|c| c.beats(previous));
            beating()
                .find(|c| c.kind == previous.kind)
                .or_else(|| beating().find(|c| c.kind == Kind::Bomb))
                .or_else(|| beating().next())
                .copied()
        }
    }
}

/// Kicker ranks for a triple, plane or four: lone cards first, then cards
/// from pairs and triples, smallest first, never from bombs or the rocket.
fn kickers(counts: &Counts, exclude: &[u8], amount: usize, pairs: bool) -> Option<Vec<u8>> {
    let rocket = counts[13] == 1 && counts[14] == 1;
    let mut pool: Vec<u8> = (0..15u8)
        .filter(|r| !exclude.contains(r) && counts[usize::from(*r)] > 0)
        .filter(|&r| counts[usize::from(r)] < 4 && !(rocket && r >= SMALL_JOKER))
        .filter(|&r| !pairs || counts[usize::from(r)] >= 2)
        .collect();
    pool.sort_by_key(|&r| (counts[usize::from(r)], r));
    let mut out = Vec::new();
    for r in pool {
        let take = if pairs {
            2
        } else {
            usize::from(counts[usize::from(r)]).min(amount - out.len())
        };
        out.extend(std::iter::repeat_n(r, take));
        if out.len() == amount * if pairs { 2 } else { 1 } {
            return Some(out);
        }
    }
    None
}

/// Plays available from `counts`, as rank lists. With `previous`, only plays
/// that beat it. Each shape appears once, with the cheapest kickers.
pub fn rank_plays(counts: &Counts, previous: Option<&Combo>) -> Vec<(Combo, Vec<u8>)> {
    let mut out = Vec::new();
    let c = |r: u8| counts[usize::from(r)];
    let wants = |kind: Kind, len: u8| match previous {
        None => true,
        Some(p) => p.kind == kind && p.len == len,
    };
    let mut push = |combo: Combo, ranks: Vec<u8>| {
        if previous.is_none_or(|p| combo.beats(p)) {
            out.push((combo, ranks));
        }
    };
    let simple = [(Kind::Single, 1u8), (Kind::Pair, 2), (Kind::Triple, 3)];
    for (kind, need) in simple {
        if wants(kind, 1) {
            for r in 0..15u8 {
                if c(r) >= need {
                    push(Combo::new(kind, r, 1), vec![r; usize::from(need)]);
                }
            }
        }
    }
    for (kind, pairs) in [(Kind::TripleSingle, false), (Kind::TriplePair, true)] {
        if wants(kind, 1) {
            for r in (0..=TWO).filter(|&r| c(r) >= 3) {
                if let Some(extra) = kickers(counts, &[r], 1, pairs) {
                    let mut ranks = vec![r; 3];
                    ranks.extend(extra);
                    push(Combo::new(kind, r, 1), ranks);
                }
            }
        }
    }
    let sequences = [
        (Kind::Straight, 1u8, 5u8, 12u8),
        (Kind::PairStraight, 2, 3, 10),
        (Kind::Plane, 3, 2, 6),
        (Kind::PlaneSingles, 3, 2, 5),
        (Kind::PlanePairs, 3, 2, 4),
    ];
    for (kind, need, min_len, max_len) in sequences {
        for len in min_len..=max_len {
            if !wants(kind, len) {
                continue;
            }
            for start in 0..=TOP_SEQUENCE.saturating_sub(len - 1) {
                let run: Vec<u8> = (start..start + len).collect();
                if run.iter().any(|&r| c(r) < need) {
                    continue;
                }
                let mut ranks: Vec<u8> = run
                    .iter()
                    .flat_map(|&r| vec![r; usize::from(need)])
                    .collect();
                let wings = match kind {
                    Kind::PlaneSingles => kickers(counts, &run, usize::from(len), false),
                    Kind::PlanePairs => kickers(counts, &run, usize::from(len), true),
                    _ => Some(Vec::new()),
                };
                if let Some(wings) = wings {
                    ranks.extend(wings);
                    push(Combo::new(kind, start, len), ranks);
                }
            }
        }
    }
    for r in (0..=TWO).filter(|&r| c(r) == 4) {
        for (kind, pairs) in [(Kind::FourSingles, false), (Kind::FourPairs, true)] {
            if wants(kind, 1)
                && let Some(extra) = kickers(counts, &[r], 2, pairs)
            {
                let mut ranks = vec![r; 4];
                ranks.extend(extra);
                push(Combo::new(kind, r, 1), ranks);
            }
        }
        push(Combo::new(Kind::Bomb, r, 1), vec![r; 4]);
    }
    if c(SMALL_JOKER) == 1 && c(BIG_JOKER) == 1 {
        push(
            Combo::new(Kind::Rocket, BIG_JOKER, 1),
            vec![SMALL_JOKER, BIG_JOKER],
        );
    }
    out
}

/// Picks cards from `hand` matching the rank list, lowest suit first.
pub fn take_ranks(hand: &[Card], ranks: &[u8]) -> Vec<Card> {
    let mut pool: Vec<Card> = hand.to_vec();
    pool.sort_unstable();
    let mut out = Vec::with_capacity(ranks.len());
    for &r in ranks {
        let index = pool
            .iter()
            .position(|&card| rank(card) == r)
            .expect("rank list drawn from this hand");
        out.push(pool.remove(index));
    }
    out
}

/// Card plays that beat `previous` (or any lead), ordered for the hint
/// button: same kind from smallest, then bombs, then the rocket.
pub fn hints(hand: &[Card], previous: Option<&Combo>) -> Vec<Vec<Card>> {
    let mut plays = rank_plays(&counts(hand), previous);
    plays.sort_by_key(|(combo, ranks)| {
        (
            combo.is_bomb(),
            combo.kind == Kind::Rocket,
            previous.is_none() && combo.kind != Kind::Single,
            combo.rank,
            std::cmp::Reverse(ranks.len()),
        )
    });
    plays
        .into_iter()
        .map(|(_, ranks)| take_ranks(hand, &ranks))
        .collect()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Cards from rank names, e.g. "3 3 4 4 5 5 X D" (X small, D big joker).
    pub fn cards(text: &str) -> Vec<Card> {
        let mut used = [0u8; 15];
        text.split_whitespace()
            .map(|name| {
                let r = match name {
                    "J" => 8,
                    "Q" => 9,
                    "K" => 10,
                    "A" => 11,
                    "2" => 12,
                    "X" => 13,
                    "D" => 14,
                    "10" | "T" => 7,
                    digit => digit.parse::<u8>().unwrap() - 3,
                };
                let suit = used[usize::from(r)];
                used[usize::from(r)] += 1;
                if r >= 13 { 52 + r - 13 } else { r * 4 + suit }
            })
            .collect()
    }

    fn lead(text: &str) -> Option<(Kind, u8, u8)> {
        parse(&cards(text), None).map(|c| (c.kind, c.rank, c.len))
    }

    #[test]
    fn recognizes_every_kind() {
        assert_eq!(lead("7"), Some((Kind::Single, 4, 1)));
        assert_eq!(lead("2 2"), Some((Kind::Pair, 12, 1)));
        assert_eq!(lead("X D"), Some((Kind::Rocket, 14, 1)));
        assert_eq!(lead("9 9 9"), Some((Kind::Triple, 6, 1)));
        assert_eq!(lead("9 9 9 3"), Some((Kind::TripleSingle, 6, 1)));
        assert_eq!(lead("9 9 9 3 3"), Some((Kind::TriplePair, 6, 1)));
        assert_eq!(lead("3 4 5 6 7"), Some((Kind::Straight, 0, 5)));
        assert_eq!(lead("10 J Q K A"), Some((Kind::Straight, 7, 5)));
        assert_eq!(lead("3 3 4 4 5 5"), Some((Kind::PairStraight, 0, 3)));
        assert_eq!(lead("3 3 3 4 4 4"), Some((Kind::Plane, 0, 2)));
        assert_eq!(lead("3 3 3 4 4 4 9 J"), Some((Kind::PlaneSingles, 0, 2)));
        assert_eq!(lead("3 3 3 4 4 4 9 9 J J"), Some((Kind::PlanePairs, 0, 2)));
        assert_eq!(lead("5 5 5 5 3 9"), Some((Kind::FourSingles, 2, 1)));
        assert_eq!(lead("5 5 5 5 3 3 9 9"), Some((Kind::FourPairs, 2, 1)));
        assert_eq!(lead("5 5 5 5"), Some((Kind::Bomb, 2, 1)));
    }

    #[test]
    fn rejects_illegal_shapes() {
        for text in [
            "J Q K A 2",
            "3 4 5 6",
            "3 3 4 4",
            "K K A A 2 2",
            "2 2 2 A A A",
            "3 3 3 4 4 4 X D",
            "5 5 5 5 3",
            "3 4",
            "9 9 9 3 X",
            "5 5 5 5 X D",
        ] {
            assert_eq!(lead(text), None, "{text}");
        }
    }

    #[test]
    fn the_rocket_is_never_split_into_wings() {
        assert_eq!(lead("3 3 3 4 4 4 X D").map(|c| c.0), None);
        assert_eq!(
            lead("3 3 3 4 4 4 9 X").map(|c| c.0),
            Some(Kind::PlaneSingles)
        );
    }

    #[test]
    fn ambiguous_cards_follow_the_table() {
        // 3333 4444 reads as a plane with wings or four with two pairs.
        let both = cards("3 3 3 3 4 4 4 4");
        let plane = parse(&cards("10 10 10 9 9 9 J Q"), None).unwrap();
        assert!(parse(&both, Some(&plane)).is_none());
        assert_eq!(
            parse(&both, Some(&Combo::new(Kind::FourPairs, 0, 1))),
            Some(Combo::new(Kind::FourPairs, 1, 1))
        );
        // Twelve cards: four linked triples, or three with wings.
        let readings = interpretations(&cards("3 3 3 4 4 4 5 5 5 6 6 6"));
        assert!(readings.contains(&Combo::new(Kind::Plane, 0, 4)));
        assert!(readings.contains(&Combo::new(Kind::PlaneSingles, 0, 3)));
        assert!(readings.contains(&Combo::new(Kind::PlaneSingles, 1, 3)));
    }

    #[test]
    fn beating_rules() {
        let single_two = parse(&cards("2"), None).unwrap();
        let joker = parse(&cards("X"), None).unwrap();
        let bomb = parse(&cards("3 3 3 3"), None).unwrap();
        let big_bomb = parse(&cards("2 2 2 2"), None).unwrap();
        let rocket = parse(&cards("X D"), None).unwrap();
        let straight = parse(&cards("3 4 5 6 7"), None).unwrap();
        assert!(joker.beats(&single_two));
        assert!(bomb.beats(&straight) && !straight.beats(&bomb));
        assert!(big_bomb.beats(&bomb) && !bomb.beats(&big_bomb));
        assert!(rocket.beats(&big_bomb) && !big_bomb.beats(&rocket));
        let longer = parse(&cards("4 5 6 7 8 9"), None).unwrap();
        assert!(!longer.beats(&straight));
        assert_eq!(
            parse(&cards("4 5 6 7 8"), Some(&straight)).map(|c| c.rank),
            Some(1)
        );
        assert!(parse(&cards("3 4 5 6 7"), Some(&straight)).is_none());
        let four_two = parse(&cards("9 9 9 9 3 4"), None).unwrap();
        assert!(!four_two.is_bomb() && bomb.beats(&four_two));
    }

    #[test]
    fn hints_beat_the_table_smallest_first() {
        let hand = cards("3 5 5 8 8 8 K 2 2 2 2 X D");
        let pair = parse(&cards("4 4"), None).unwrap();
        let options: Vec<_> = hints(&hand, Some(&pair))
            .into_iter()
            .map(|cards| parse(&cards, Some(&pair)).unwrap())
            .collect();
        assert_eq!(options[0], Combo::new(Kind::Pair, 2, 1));
        assert_eq!(options[1], Combo::new(Kind::Pair, 5, 1));
        assert!(options.iter().any(|c| c.kind == Kind::Bomb));
        assert_eq!(options.last().unwrap().kind, Kind::Rocket);
        // Triple with one: the kicker is the lone 3, not a 5 from the pair.
        let triple = parse(&cards("4 4 4 6"), None).unwrap();
        let first = &hints(&hand, Some(&triple))[0];
        assert_eq!(parse(first, None).unwrap().kind, Kind::TripleSingle);
        assert!(first.iter().any(|&card| rank(card) == 0));
    }

    #[test]
    fn every_generated_play_parses_as_itself() {
        let hand = cards("3 3 3 4 4 4 5 5 5 6 7 8 9 10 J J Q Q 2 D");
        for (combo, ranks) in rank_plays(&counts(&hand), None) {
            let play = take_ranks(&hand, &ranks);
            assert!(
                interpretations(&play).contains(&combo),
                "{combo:?} from {ranks:?}"
            );
        }
    }
}
