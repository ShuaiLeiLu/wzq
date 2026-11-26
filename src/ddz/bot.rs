//! A heuristic player used for bot seats and for 托管 (auto-play).
//!
//! It scores a hand by how many plays it would take to empty it, minus a
//! bonus for control cards (2s, jokers, bombs), and picks the play that
//! leaves the best hand behind.

use super::cards::{
    BIG_JOKER, Card, Combo, Counts, Kind, SMALL_JOKER, TWO, counts, rank_plays, take_ranks,
};

/// What the bot knows when it must act.
pub struct View<'a> {
    pub hand: &'a [Card],
    /// The play to beat, or `None` when leading.
    pub previous: Option<&'a Combo>,
    /// The play to beat came from a teammate.
    pub partner_played: bool,
    /// Cards left in the smallest opposing hand.
    pub fewest_opponent_cards: usize,
    /// Cards left in the next player's hand when that player is an opponent.
    pub next_opponent_cards: Option<usize>,
}

fn quads(c: &Counts) -> i32 {
    (0..=TWO).filter(|&r| c[usize::from(r)] == 4).count() as i32
}

fn rocket(c: &Counts) -> bool {
    c[usize::from(SMALL_JOKER)] == 1 && c[usize::from(BIG_JOKER)] == 1
}

/// Plays needed for the hand without breaking out straights.
fn basic(c: &Counts) -> i32 {
    let mut c = *c;
    let mut plays = 0;
    if rocket(&c) {
        plays += 1;
        c[13] = 0;
        c[14] = 0;
    }
    for count in c.iter_mut().take(13) {
        if *count == 4 {
            plays += 1;
            *count = 0;
        }
    }
    let mut slots = 0;
    for need in [3u8, 2] {
        let min_len = if need == 3 { 2 } else { 3 };
        let mut r = 0;
        while r <= 11 {
            if c[r] < need || (need == 2 && c[r] != 2) {
                r += 1;
                continue;
            }
            let mut end = r;
            while end < 11 && c[end + 1] >= need && (need == 3 || c[end + 1] == 2) {
                end += 1;
            }
            let len = end - r + 1;
            if len >= min_len {
                plays += 1;
                if need == 3 {
                    slots += len as i32;
                }
                for count in &mut c[r..=end] {
                    *count -= need;
                }
            }
            r = end + 1;
        }
    }
    for count in c.iter_mut().take(13) {
        if *count == 3 {
            plays += 1;
            slots += 1;
            *count = 0;
        }
    }
    let singles = c.iter().filter(|&&n| n == 1).count() as i32;
    let pairs = c.iter().filter(|&&n| n == 2).count() as i32;
    let single_wings = slots.min(singles);
    let pair_wings = (slots - single_wings).min(pairs);
    plays + singles - single_wings + pairs - pair_wings
}

fn search(c: &Counts, from: usize, depth: u8) -> i32 {
    let mut best = basic(c);
    if depth == 0 {
        return best;
    }
    for start in from..=7 {
        if c[start..start + 5].contains(&0) {
            continue;
        }
        let mut rest = *c;
        for end in start..=11 {
            if c[end] == 0 {
                break;
            }
            rest[end] -= 1;
            if end - start + 1 >= 5 {
                best = best.min(1 + search(&rest, start, depth - 1));
            }
        }
    }
    best
}

fn plays_needed(c: &Counts) -> i32 {
    search(c, 0, 3)
}

/// Lower is better.
fn cost(c: &Counts) -> i32 {
    let control = 3 * i32::from(c[12])
        + 3 * i32::from(c[13])
        + 4 * i32::from(c[14])
        + 14 * quads(c)
        + if rocket(c) { 10 } else { 0 };
    plays_needed(c) * 10 - control
}

/// Hand strength for bidding and doubling.
fn strength(hand: &[Card]) -> i32 {
    let c = counts(hand);
    4 * i32::from(c[14])
        + 3 * i32::from(c[13])
        + 2 * i32::from(c[12])
        + 4 * quads(&c)
        + i32::from(c[11] >= 3)
        - (plays_needed(&c) - 7).max(0)
}

pub fn wants_landlord(hand: &[Card], robbing: bool) -> bool {
    strength(hand) >= if robbing { 10 } else { 7 }
}

pub fn wants_double(hand: &[Card], landlord: bool) -> bool {
    strength(hand) >= if landlord { 9 } else { 10 }
}

/// The cards to play, or `None` to pass.
pub fn choose(view: &View) -> Option<Vec<Card>> {
    let hand = counts(view.hand);
    let options = rank_plays(&hand, view.previous);
    if let Some((_, ranks)) = options
        .iter()
        .find(|(_, ranks)| ranks.len() == view.hand.len())
    {
        return Some(take_ranks(view.hand, ranks));
    }
    let highest = (0..15).rev().find(|&r| hand[r] > 0).unwrap_or(0) as u8;
    let guard_single = view.next_opponent_cards == Some(1);
    let scored = options.iter().map(|(combo, ranks)| {
        let mut after = hand;
        for &r in ranks {
            after[usize::from(r)] -= 1;
        }
        let mut score = cost(&after);
        if view.previous.is_none() {
            // Lead with more cards and lower ranks when it costs the same.
            score -= ranks.len() as i32;
            if guard_single && combo.kind == Kind::Single && combo.rank != highest {
                score += 40;
            }
        }
        (score, combo.rank, combo, ranks, after)
    });
    let (score, _, combo, ranks, after) = scored.min_by_key(|item| (item.0, item.1))?;
    if view.previous.is_some() {
        if view.partner_played {
            return None;
        }
        let urgent = view.fewest_opponent_cards <= 4;
        if combo.is_bomb() && !urgent && plays_needed(&after) > 2 {
            return None;
        }
        if !urgent && score - cost(&hand) > 5 {
            return None;
        }
    }
    Some(take_ranks(view.hand, ranks))
}

#[cfg(test)]
mod tests {
    use super::super::cards::{parse, tests::cards};
    use super::*;

    fn view<'a>(hand: &'a [Card], previous: Option<&'a Combo>) -> View<'a> {
        View {
            hand,
            previous,
            partner_played: false,
            fewest_opponent_cards: 17,
            next_opponent_cards: Some(17),
        }
    }

    #[test]
    fn plays_out_a_finishing_hand() {
        let hand = cards("3 4 5 6 7");
        assert_eq!(choose(&view(&hand, None)).unwrap().len(), 5);
    }

    #[test]
    fn keeps_straights_together_and_leads_low() {
        let hand = cards("3 4 5 6 7 9 9 K");
        let lead = choose(&view(&hand, None)).unwrap();
        let combo = parse(&lead, None).unwrap();
        assert!(
            matches!(combo.kind, Kind::Straight | Kind::Pair),
            "{combo:?}"
        );
        let single = parse(&cards("4"), None).unwrap();
        let answer = choose(&view(&hand, Some(&single))).unwrap();
        assert_eq!(parse(&answer, None).unwrap().rank, 10, "answers with the K");
    }

    #[test]
    fn does_not_beat_a_partner_or_waste_bombs_early() {
        let hand = cards("3 5 5 8 9 9 9 9 J Q K");
        let single = parse(&cards("4"), None).unwrap();
        let mut partner = view(&hand, Some(&single));
        partner.partner_played = true;
        assert!(choose(&partner).is_none());
        let pair = parse(&cards("A A"), None).unwrap();
        assert!(choose(&view(&hand, Some(&pair))).is_none());
        let mut urgent = view(&hand, Some(&pair));
        urgent.fewest_opponent_cards = 2;
        assert_eq!(
            parse(&choose(&urgent).unwrap(), None).unwrap().kind,
            Kind::Bomb
        );
    }

    #[test]
    fn guards_against_an_opponent_on_one_card() {
        let hand = cards("3 6 6 K 2");
        let mut guarded = view(&hand, None);
        guarded.next_opponent_cards = Some(1);
        let lead = parse(&choose(&guarded).unwrap(), None).unwrap();
        assert!(lead.kind != Kind::Single || lead.rank == 12, "{lead:?}");
    }

    #[test]
    fn bids_on_strong_hands_only() {
        let strong = cards("X D 2 2 2 A A 3 3 3 3 5 6 7 8 9 J");
        let weak = cards("3 4 5 6 8 9 10 J J Q 4 6 8 K 5 7 9");
        assert!(wants_landlord(&strong, false) && wants_landlord(&strong, true));
        assert!(!wants_landlord(&weak, false));
    }
}
