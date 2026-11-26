//! The player behind bot seats and 托管 (auto-play).
//!
//! It splits its hand into the fewest plays, keeps track of which cards
//! are still out (neither in its hand nor played) to tell which of its
//! plays nobody can beat, saves those for the endgame, and plays as a
//! team when it is a farmer: it never overtakes a teammate, feeds one who
//! is about to go out, and blocks a landlord who is.

use super::cards::{
    BIG_JOKER, Card, Combo, Counts, Kind, SMALL_JOKER, TWO, counts, rank_plays, take_ranks,
};

/// Another seat, as this one sees it.
#[derive(Clone, Copy)]
pub struct Seat {
    pub cards: usize,
    pub partner: bool,
}

/// What the bot knows when it must act.
pub struct View<'a> {
    pub hand: &'a [Card],
    /// The play to beat, or `None` when leading.
    pub previous: Option<&'a Combo>,
    /// Cards neither in this hand nor played yet.
    pub unseen: Counts,
    /// The seat that plays next, and the one that played just before.
    pub next: Seat,
    pub before: Seat,
    /// `previous` came from the next seat (the seat before passed).
    pub played_by_next: bool,
}

/// Every card in the deck, by rank.
pub fn deck_counts() -> Counts {
    let mut all = [4; 15];
    all[usize::from(SMALL_JOKER)] = 1;
    all[usize::from(BIG_JOKER)] = 1;
    all
}

/* ---------- Splitting a hand ---------- */

#[derive(Clone, Copy, Debug)]
struct Group {
    kind: Kind,
    rank: u8,
    len: u8,
}

/// The best way found to play a hand out.
#[derive(Clone, Debug)]
struct Plan {
    /// Plays needed, with kickers already attached to triples and planes.
    turns: i32,
    /// `turns` scaled up, plus a penalty for low cards nobody wants.
    score: i32,
    groups: Vec<Group>,
}

/// Search budget per hand; real hands finish far below it.
const SEARCH_LIMIT: u32 = 60_000;

fn evaluate(groups: &[Group]) -> Plan {
    let mut singles: Vec<u8> = Vec::new();
    let mut pairs: Vec<u8> = Vec::new();
    let mut slots = 0usize;
    for group in groups {
        match group.kind {
            Kind::Single => singles.push(group.rank),
            Kind::Pair => pairs.push(group.rank),
            Kind::Triple => slots += 1,
            Kind::Plane => slots += usize::from(group.len),
            _ => {}
        }
    }
    singles.sort_unstable();
    pairs.sort_unstable();
    // Triples and planes carry the smallest loose cards, never a 2 or a joker.
    let carry = |cards: &[u8], slots: usize| cards.iter().take(slots).filter(|&&r| r < TWO).count();
    let single_wings = carry(&singles, slots);
    let pair_wings = carry(&pairs, slots - single_wings);
    let turns = groups.len() - single_wings - pair_wings;
    let weak: i32 = singles[single_wings..]
        .iter()
        .map(|&r| match r {
            0..=4 => 3,
            5..=8 => 2,
            9..=10 => 1,
            _ => 0,
        })
        .sum::<i32>()
        + pairs[pair_wings..].iter().filter(|&&r| r <= 6).count() as i32;
    Plan {
        turns: turns as i32,
        score: turns as i32 * 10 + weak,
        groups: groups.to_vec(),
    }
}

fn search(c: &mut Counts, groups: &mut Vec<Group>, best: &mut Option<Plan>, budget: &mut u32) {
    if *budget == 0 {
        return;
    }
    *budget -= 1;
    let Some(r) = (0..15).find(|&r| c[r] > 0) else {
        let plan = evaluate(groups);
        if best.as_ref().is_none_or(|b| plan.score < b.score) {
            *best = Some(plan);
        }
        return;
    };
    let rank = r as u8;
    // Use every card of this rank as one group.
    let n = c[r];
    let kind = match n {
        1 => Kind::Single,
        2 => Kind::Pair,
        3 => Kind::Triple,
        _ => Kind::Bomb,
    };
    c[r] = 0;
    groups.push(Group { kind, rank, len: 1 });
    search(c, groups, best, budget);
    groups.pop();
    c[r] = n;
    // Or start a sequence here.
    if r <= 11 {
        for (need, min_len, kind) in [
            (1u8, 5usize, Kind::Straight),
            (2, 3, Kind::PairStraight),
            (3, 2, Kind::Plane),
        ] {
            let mut longest = 0;
            while r + longest <= 11 && c[r + longest] >= need {
                longest += 1;
            }
            for len in min_len..=longest {
                for count in &mut c[r..r + len] {
                    *count -= need;
                }
                groups.push(Group {
                    kind,
                    rank,
                    len: len as u8,
                });
                search(c, groups, best, budget);
                groups.pop();
                for count in &mut c[r..r + len] {
                    *count += need;
                }
            }
        }
    }
}

fn plan(hand: &Counts) -> Plan {
    let mut c = *hand;
    let mut groups = Vec::new();
    // The rocket stays together.
    if c[13] == 1 && c[14] == 1 {
        c[13] = 0;
        c[14] = 0;
        groups.push(Group {
            kind: Kind::Rocket,
            rank: BIG_JOKER,
            len: 1,
        });
    }
    let mut best: Option<Plan> = None;
    let mut budget = SEARCH_LIMIT;
    search(&mut c, &mut groups, &mut best, &mut budget);
    let mut best = best.expect("a split always exists");
    // Big cards win tricks back; spending one has a price.
    best.score -= control_cards(hand) * CONTROL_WEIGHT;
    best
}

const CONTROL_WEIGHT: i32 = 2;

/* ---------- Reading the table ---------- */

/// Nothing still out can beat this play except a bomb.
fn boss(unseen: &Counts, combo: &Combo) -> bool {
    rank_plays(unseen, Some(combo))
        .iter()
        .all(|(other, _)| other.is_bomb())
}

/// Plays in the plan that nothing still out can beat.
fn sure_plays(plan: &Plan, unseen: &Counts) -> i32 {
    plan.groups
        .iter()
        .filter(|group| {
            let combo = Combo {
                kind: group.kind,
                rank: group.rank,
                len: group.len,
            };
            match group.kind {
                Kind::Rocket => true,
                Kind::Bomb => rank_plays(unseen, Some(&combo)).is_empty(),
                _ => boss(unseen, &combo),
            }
        })
        .count() as i32
}

fn control_cards(c: &Counts) -> i32 {
    let quads = (0..=TWO).filter(|&r| c[usize::from(r)] == 4).count() as i32;
    4 * i32::from(c[14])
        + 3 * i32::from(c[13])
        + 2 * i32::from(c[12])
        + 3 * quads
        + i32::from(c[11] >= 3)
}

pub fn wants_landlord(hand: &[Card], robbing: bool) -> bool {
    let c = counts(hand);
    let control = control_cards(&c);
    let turns = plan(&c).turns;
    if robbing {
        control >= 10 || (control >= 7 && turns <= 6)
    } else {
        control >= 8 || (control >= 5 && turns <= 6)
    }
}

pub fn wants_double(hand: &[Card], landlord: bool) -> bool {
    let c = counts(hand);
    let control = control_cards(&c);
    let turns = plan(&c).turns;
    if landlord {
        control >= 9 && turns <= 6
    } else {
        control >= 10 && turns <= 6
    }
}

struct Candidate<'a> {
    combo: &'a Combo,
    ranks: &'a [u8],
    after: Plan,
    boss: bool,
    /// Plays left after this one that could still lose the lead.
    loose: i32,
}

/// The cards to play, or `None` to pass.
pub fn choose(view: &View) -> Option<Vec<Card>> {
    let hand = counts(view.hand);
    let plays = rank_plays(&hand, view.previous);
    if let Some((_, ranks)) = plays
        .iter()
        .find(|(_, ranks)| ranks.len() == view.hand.len())
    {
        return Some(take_ranks(view.hand, ranks));
    }
    if plays.is_empty() {
        return None;
    }
    let options: Vec<Candidate> = plays
        .iter()
        .map(|(combo, ranks)| {
            let mut after = hand;
            for &r in ranks {
                after[usize::from(r)] -= 1;
            }
            let after = plan(&after);
            Candidate {
                combo,
                ranks,
                loose: after.turns - sure_plays(&after, &view.unseen),
                after,
                boss: boss(&view.unseen, combo),
            }
        })
        .collect();
    let chosen = match view.previous {
        None => lead(view, &options),
        Some(previous) => follow(view, &hand, previous, &options),
    };
    chosen.map(|option| take_ranks(view.hand, option.ranks))
}

fn opponents<'a>(view: &'a View) -> impl Iterator<Item = &'a Seat> {
    [&view.next, &view.before]
        .into_iter()
        .filter(|seat| !seat.partner)
}

fn lead<'a>(view: &View, options: &'a [Candidate<'a>]) -> Option<&'a Candidate<'a>> {
    let opponent_on = |cards: usize| opponents(view).any(|seat| seat.cards == cards);
    options.iter().min_by_key(|option| {
        let combo = option.combo;
        let mut value = option.after.score * 4;
        if option.boss && option.loose <= 1 {
            // Win the lead with this, cash the other sure plays, then the last one.
            value -= 10_000 + option.after.turns * 10;
        } else if option.boss || combo.is_bomb() {
            // Keep sure winners for the endgame.
            value += if combo.is_bomb() { 120 } else { 30 };
        }
        // Never hand a nearly-out opponent the play they need.
        if combo.kind == Kind::Single && !option.boss && opponent_on(1) {
            value += 400 + (15 - i32::from(combo.rank)) * 4;
        }
        if combo.kind == Kind::Pair && !option.boss && opponent_on(2) {
            value += 300;
        }
        // Against a short hand, lead shapes it is unlikely to match.
        let fewest = opponents(view).map(|seat| seat.cards).min().unwrap_or(20);
        if fewest <= 5 && matches!(combo.kind, Kind::Single | Kind::Pair) && !option.boss {
            value += 40;
        }
        // Feed a teammate who is about to go out.
        if view.next.partner {
            if view.next.cards == 1 && combo.kind == Kind::Single {
                value -= 2_000 - i32::from(combo.rank) * 10;
            }
            if view.next.cards == 2 && combo.kind == Kind::Pair {
                value -= 1_500 - i32::from(combo.rank) * 10;
            }
        }
        // Among equals, shed more cards and start low.
        (value, -(option.ranks.len() as i32), combo.rank)
    })
}

fn follow<'a>(
    view: &View,
    hand: &Counts,
    previous: &Combo,
    options: &'a [Candidate<'a>],
) -> Option<&'a Candidate<'a>> {
    let played_by = if view.played_by_next {
        view.next
    } else {
        view.before
    };
    let landlord_next = !view.next.partner && view.before.partner;
    if played_by.partner {
        // Leave a teammate's lead alone, unless the landlord sits next,
        // is about to go out, and could beat it.
        if !(landlord_next && view.next.cards <= 2 && !boss(&view.unseen, previous)) {
            return None;
        }
        return options
            .iter()
            .filter(|option| option.boss)
            .min_by_key(|option| (option.combo.is_bomb(), option.combo.rank));
    }
    let fewest = opponents(view).map(|seat| seat.cards).min().unwrap_or(20);
    let urgent = played_by.cards <= 2 || fewest <= 2;
    if urgent {
        // Stop them: the highest plain answer, else the smallest bomb.
        return options
            .iter()
            .filter(|option| !option.combo.is_bomb())
            .max_by_key(|option| option.combo.rank)
            .or_else(|| options.iter().min_by_key(|option| option.combo.rank));
    }
    let before = plan(hand);
    let cost = |option: &Candidate| {
        let mut cost = option.after.score - before.score;
        if option.boss && option.loose <= 1 {
            cost -= 1_000;
        }
        if option.combo.is_bomb() {
            cost += 30;
        }
        cost
    };
    let best = options
        .iter()
        .min_by_key(|option| (cost(option), option.combo.rank))?;
    // The farmer right after the landlord tops a low single or pair with
    // something from J up, so the landlord has to spend a big card.
    let topping = !view.before.partner
        && view.next.partner
        && matches!(previous.kind, Kind::Single | Kind::Pair)
        && previous.rank < 8;
    let best = if topping {
        options
            .iter()
            .filter(|option| {
                !option.combo.is_bomb() && option.combo.rank >= 8 && cost(option) <= cost(best) + 6
            })
            .min_by_key(|option| option.combo.rank)
            .unwrap_or(best)
    } else {
        best
    };
    if best.combo.is_bomb() {
        return (best.loose <= 1 || fewest <= 4).then_some(best);
    }
    // How much a play may cost the hand before it is better to pass.
    let allowance = if fewest <= 6 { 22 } else { 9 };
    (cost(best) <= allowance).then_some(best)
}

#[cfg(test)]
mod tests {
    use super::super::cards::{parse, tests::cards};
    use super::*;

    fn unseen_without(hand: &[Card]) -> Counts {
        let mut unseen = deck_counts();
        for (count, own) in unseen.iter_mut().zip(counts(hand)) {
            *count -= own;
        }
        unseen
    }

    fn view<'a>(hand: &'a [Card], previous: Option<&'a Combo>) -> View<'a> {
        View {
            hand,
            previous,
            unseen: unseen_without(hand),
            next: Seat {
                cards: 17,
                partner: false,
            },
            before: Seat {
                cards: 17,
                partner: false,
            },
            played_by_next: false,
        }
    }

    fn kind_of(cards: &[Card]) -> Kind {
        parse(cards, None).unwrap().kind
    }

    #[test]
    fn splits_into_the_fewest_plays() {
        assert_eq!(plan(&counts(&cards("3 4 5 6 7 8 9 9 9 J"))).turns, 2);
        assert_eq!(plan(&counts(&cards("3 3 4 4 5 5 7 7 7 8 8 8 K"))).turns, 2);
        assert_eq!(plan(&counts(&cards("3 4 5 6 7 7 8 9 10 J"))).turns, 2);
        assert_eq!(plan(&counts(&cards("X D 2 2 2 2"))).turns, 2);
    }

    #[test]
    fn plays_out_a_finishing_hand() {
        let hand = cards("3 4 5 6 7");
        assert_eq!(choose(&view(&hand, None)).unwrap().len(), 5);
    }

    #[test]
    fn plays_the_sure_winner_first_in_the_endgame() {
        // Big joker is unbeatable; play it, win the lead, then the pair.
        let hand = cards("D 5 5");
        let lead = choose(&view(&hand, None)).unwrap();
        assert_eq!(lead, cards("D"));
    }

    #[test]
    fn leads_low_groups_and_keeps_control_cards() {
        let hand = cards("3 4 5 6 7 9 9 K 2 D");
        let lead = choose(&view(&hand, None)).unwrap();
        assert!(
            matches!(kind_of(&lead), Kind::Straight | Kind::Pair),
            "{lead:?}"
        );
    }

    #[test]
    fn does_not_overtake_a_teammate_or_waste_bombs_early() {
        let hand = cards("3 5 5 8 9 9 9 9 J Q K");
        let single = parse(&cards("4"), None).unwrap();
        let mut partner = view(&hand, Some(&single));
        partner.before.partner = true;
        assert!(choose(&partner).is_none());
        let pair = parse(&cards("A A"), None).unwrap();
        assert!(choose(&view(&hand, Some(&pair))).is_none());
        let mut urgent = view(&hand, Some(&pair));
        urgent.before.cards = 2;
        assert_eq!(kind_of(&choose(&urgent).unwrap()), Kind::Bomb);
    }

    #[test]
    fn guards_against_an_opponent_on_one_card() {
        let hand = cards("3 6 6 K 2 9 10 J");
        let mut guarded = view(&hand, None);
        guarded.next.cards = 1;
        let lead = parse(&choose(&guarded).unwrap(), None).unwrap();
        assert!(lead.kind != Kind::Single, "{lead:?}");
        // With only singles left, it leads the highest one.
        let singles = cards("3 7 K");
        let mut forced = view(&singles, None);
        forced.next.cards = 1;
        assert_eq!(choose(&forced).unwrap(), cards("K"));
    }

    #[test]
    fn feeds_a_teammate_on_one_card() {
        let hand = cards("3 8 8 9 9 10 10 K K");
        let mut feeding = view(&hand, None);
        feeding.next = Seat {
            cards: 1,
            partner: true,
        };
        assert_eq!(choose(&feeding).unwrap(), cards("3"));
    }

    #[test]
    fn stops_an_opponent_about_to_go_out() {
        let hand = cards("4 9 Q 2 5 5");
        let single = parse(&cards("8"), None).unwrap();
        let mut urgent = view(&hand, Some(&single));
        urgent.before.cards = 1;
        assert_eq!(choose(&urgent).unwrap(), cards("2"));
    }

    #[test]
    fn bids_on_strong_hands_only() {
        let strong = cards("X D 2 2 2 A A 3 3 3 3 5 6 7 8 9 J");
        let weak = cards("3 4 5 6 8 9 10 J J Q 4 6 8 K 5 7 9");
        assert!(wants_landlord(&strong, false) && wants_landlord(&strong, true));
        assert!(!wants_landlord(&weak, false));
    }
}
