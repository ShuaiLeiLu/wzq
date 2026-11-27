use std::{
    fmt::Write as _,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
};

use crate::{
    game::SIZE,
    prediction::{Forecast, Odds, Position},
};

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Rapfi reports expected score (a draw earns half a point) for the side to move.
fn percentages(score: f64, draw: f64, turn: u8) -> Option<Odds> {
    let win = score - 0.5 * draw;
    let loss = 1.0 - score - 0.5 * draw;
    let probabilities = if turn == 1 {
        [win, draw, loss]
    } else {
        [loss, draw, win]
    };
    // The engine prints rounded floats. Allow only protocol rounding error.
    if probabilities
        .iter()
        .any(|v| !(-0.00001..=1.00001).contains(v))
    {
        return None;
    }
    let raw = probabilities.map(|p| 100.0 * p.clamp(0.0, 1.0));
    let mut amounts = raw.map(|p| p.floor() as u8);
    let mut order = [0, 1, 2];
    order.sort_by(|&a, &b| raw[b].fract().total_cmp(&raw[a].fract()));
    let remaining = 100u8.checked_sub(amounts.iter().sum())?;
    for i in order.into_iter().take(usize::from(remaining)) {
        amounts[i] += 1;
    }
    Some(Odds {
        black: amounts[0],
        draw: amounts[1],
        white: amounts[2],
    })
}

fn point(s: &str) -> Option<usize> {
    let (x, y) = s.split_once(',')?;
    let (x, y) = (x.parse::<usize>().ok()?, y.parse::<usize>().ok()?);
    if x < SIZE && y < SIZE {
        Some(y * SIZE + x)
    } else {
        None
    }
}

struct Engine {
    _child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
}

impl Engine {
    async fn start(path: &Path) -> io::Result<Self> {
        let path = path.canonicalize()?;
        let mut child = Command::new(&path)
            .current_dir(path.parent().unwrap())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        input.write_all(b"START 15\nINFO RULE 0\nINFO SHOW_DETAIL 2\nINFO MAX_NODE 2048\nINFO TIMEOUT_TURN 500\n").await?;
        Ok(Self {
            _child: child,
            input,
            output,
        })
    }

    async fn search(&mut self, pos: &Position) -> io::Result<Forecast> {
        let mut board = String::from("BOARD\n");
        let opening = pos.moves.is_empty();
        // Rapfi's empty-board book returns H8 without analysis. Evaluate that
        // continuation from White's turn, keeping H8 as the first move in the PV.
        let turn = if opening { 2 } else { pos.turn };
        if opening {
            board.push_str("7,7,2\n");
        }
        for &i in &pos.moves {
            let side = if pos.board[i] == turn { 1 } else { 2 };
            writeln!(board, "{},{},{side}", i % SIZE, i / SIZE).unwrap();
        }
        board.push_str("DONE\n");
        self.input.write_all(board.as_bytes()).await?;
        let mut first = false;
        let (mut score, mut draw, mut mate, mut nodes) = (None, None, None, 0);
        let mut pv = Vec::new();
        let mut best = None;
        while let Some(line) = self.output.next_line().await? {
            if line.starts_with("ERROR") {
                return Err(invalid("Rapfi rejected the position or evaluator"));
            }
            if let Some(move_index) = point(&line) {
                best = Some(move_index);
                break;
            }
            let Some((field, value)) = line.strip_prefix("INFO ").and_then(|s| s.split_once(' '))
            else {
                continue;
            };
            if field == "PV" {
                first = value == "0";
                if first {
                    score = None;
                    draw = None;
                    mate = None;
                    nodes = 0;
                    pv.clear();
                }
            } else if first {
                match field {
                    "WINRATE" => score = value.parse().ok(),
                    "DRAWRATE" => draw = value.parse().ok(),
                    "TOTALNODES" => nodes = value.parse().unwrap_or(0),
                    "EVAL" if value.starts_with("+M") => mate = Some(turn),
                    "EVAL" if value.starts_with("-M") => mate = Some(3 - turn),
                    "BESTLINE" => {
                        pv = value
                            .split_whitespace()
                            .take(6)
                            .map(point)
                            .collect::<Option<Vec<_>>>()
                            .ok_or_else(|| invalid("invalid Rapfi variation"))?
                    }
                    _ => {}
                }
            }
        }
        let best = best.ok_or_else(|| invalid("Rapfi ended before the final move"))?;
        if pv.is_empty() {
            pv.push(best);
        }
        if pv[0] != best {
            return Err(invalid("Rapfi result and variation disagree"));
        }
        if opening {
            pv.insert(0, SIZE * SIZE / 2);
        }
        if pv
            .iter()
            .enumerate()
            .any(|(n, &i)| pos.board[i] != 0 || pv[..n].contains(&i))
        {
            return Err(invalid("Rapfi variation contains an occupied point"));
        }
        let odds = match mate {
            Some(winner) => Odds::result(winner),
            None => score
                .zip(draw)
                .and_then(|(s, d)| percentages(s, d, turn))
                .ok_or_else(|| invalid("Rapfi did not return a valid WDL distribution"))?,
        };
        Ok(Forecast {
            odds,
            source: "rapfi",
            variation: pv,
            nodes,
            explanation: None,
            tactic: mate.map(|c| {
                if c == 1 {
                    "引擎找到黑棋的必胜变化"
                } else {
                    "引擎找到白棋的必胜变化"
                }
            }),
        })
    }
}

pub struct Rapfi {
    path: PathBuf,
    engine: Option<Engine>,
}

impl Rapfi {
    pub fn installed() -> Option<Self> {
        let path = std::env::var_os("WZQ_RAPFI_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".rapfi/pbrain-rapfi"));
        path.is_file().then_some(Self { path, engine: None })
    }

    pub async fn evaluate(&mut self, pos: &Position) -> io::Result<Forecast> {
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            if self.engine.is_none() {
                self.engine = Some(Engine::start(&self.path).await?);
            }
            self.engine.as_mut().unwrap().search(pos).await
        })
        .await
        .unwrap_or_else(|_| Err(io::Error::new(io::ErrorKind::TimedOut, "Rapfi timed out")));
        if result.is_err() {
            self.engine = None;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires the engine installed by scripts/build-rapfi.sh"]
    async fn real_engine_reuses_process_and_returns_black_draw_white() {
        let mut engine = Rapfi::installed().expect("install the pinned engine");
        let mut position = Position {
            room: "test".into(),
            game_id: 1,
            board: vec![0; SIZE * SIZE],
            turn: 1,
            moves: vec![],
            previous: None,
        };
        let opening = engine.evaluate(&position).await.unwrap();
        assert_eq!(opening.variation[0], 112);
        assert!(opening.nodes > 0);
        for i in [112, 128, 126] {
            position.board[i] = 1;
            position.moves.push(i);
        }
        position.turn = 2;
        let handicap = engine.evaluate(&position).await.unwrap();
        assert_eq!(handicap.source, "rapfi");
        assert!(handicap.odds.black > handicap.odds.white);
        assert_eq!(
            handicap.odds.black + handicap.odds.draw + handicap.odds.white,
            100
        );
        assert!(handicap.nodes > 0 && !handicap.variation.is_empty());
    }

    #[test]
    fn half_draws_perspective_and_rounding() {
        for (turn, expected) in [(1, [60, 20, 20]), (2, [20, 20, 60])] {
            let o = percentages(0.7, 0.2, turn).unwrap();
            assert_eq!([o.black, o.draw, o.white], expected);
        }
        for score in 0..=100 {
            let o = percentages(score as f64 / 100.0, 0.0, 1).unwrap();
            assert_eq!(o.black + o.draw + o.white, 100);
        }
        assert!(percentages(0.2, 0.8, 1).is_none());
        assert!(percentages(f64::NAN, 0.0, 1).is_none());
    }
}
