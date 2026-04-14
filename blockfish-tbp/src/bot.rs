//! In-memory bot session state: board, hold, queue, search config

use crate::adapter::{board_to_matrix, from_tbp_piece};
use crate::coords::tbp_move_to_blockfish;
use blockfish::{ai::Snapshot, BasicMatrix, Color, Config, Place, ShapeTable};
use std::collections::VecDeque;

/// Bot-side mirror of the TBP-controller's board state.
pub struct Bot {
    /// Current playfield. Row 0 is the bottom, same convention as TBP.
    pub matrix: BasicMatrix,
    /// Currently held piece, if any.
    pub hold: Option<Color>,
    /// Queue of upcoming pieces. `queue[0]` is the active piece (about to
    /// be placed); `queue[1..]` is the preview.
    pub queue: VecDeque<Color>,
    /// Combo counter from the host
    pub combo: u32,
    /// Back-to-back flag
    pub b2b: bool,
    /// Search configuration
    pub cfg: Config,
}

impl Bot {
    /// Build a fresh bot session from the contents of a TBP `start` message.
    pub fn from_start(
        board: &[[Option<char>; 10]; 40],
        hold: Option<tbp::Piece>,
        queue: &[tbp::Piece],
        combo: u32,
        b2b: bool,
    ) -> Self {
        Self {
            matrix: board_to_matrix(board),
            hold: hold.map(from_tbp_piece),
            queue: queue.iter().copied().map(from_tbp_piece).collect(),
            combo,
            b2b,
            cfg: Config::default(),
        }
    }

    /// Pack the current state into the format the search expects.
    pub fn to_snapshot(&self) -> Snapshot {
        Snapshot {
            hold: self.hold,
            queue: self.queue.iter().copied().collect(),
            matrix: self.matrix.clone(),
        }
    }

    /// Append a freshly-revealed preview piece to the queue. Sent by the host
    /// after each `play` (one event for normal play, two for first-time hold).
    pub fn push_new_piece(&mut self, piece: tbp::Piece) {
        self.queue.push_back(from_tbp_piece(piece));
    }

    /// Commit a host-acknowledged move into our internal state. Mirrors the
    /// queue/hold/board mutations the host performs after `play`.
    pub fn apply_play(&mut self, shtb: &ShapeTable, mv: &tbp::Move) -> Result<(), &'static str> {
        let active = match self.queue.front() {
            Some(&c) => c,
            None => return Err("queue desync: empty queue on play"),
        };
        let move_color = from_tbp_piece(mv.location.kind);
        let did_hold = move_color != active;

        // Validate hold swap
        if did_hold {
            let next_preview = self.queue.get(1).copied();
            let hold_match = self.hold == Some(move_color);
            let preview_match = self.hold.is_none() && next_preview == Some(move_color);
            if !hold_match && !preview_match {
                return Err(
                    "queue desync: TBP move piece is neither active, hold, nor next preview",
                );
            }
        }

        // Resolve the move to a concrete blockfish Place.
        let (_color, tf, _resolved_did_hold) =
            tbp_move_to_blockfish(shtb, &self.matrix, mv, active, self.hold)
                .ok_or("queue desync: no blockfish placement matches the TBP move")?;

        // Set the piece onto our board and clear lines.
        let shape = shtb
            .shape(move_color)
            .ok_or("queue desync: unknown piece color")?;
        let pl = Place::new(shape, tf, did_hold);
        pl.shape.blit_to(&mut self.matrix, pl.tf);
        self.matrix.sift_rows();

        // Mutate hold/queue per the host's hold-inference rules.
        if !did_hold {
            // Plain placement: the active piece is consumed.
            self.queue.pop_front();
        } else if self.hold.is_some() {
            // Hold swap with a non-empty hold slot
            let new_hold = self
                .queue
                .pop_front()
                .ok_or("queue desync: queue empty during hold swap")?;
            self.hold = Some(new_hold);
        } else {
            // First-time hold
            let new_hold = self
                .queue
                .pop_front()
                .ok_or("queue desync: queue empty during first hold")?;
            self.hold = Some(new_hold);
            self.queue
                .pop_front()
                .ok_or("queue desync: only one piece left during first hold")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use blockfish::srs;

    fn empty_board() -> [[Option<char>; 10]; 40] {
        [[None; 10]; 40]
    }

    fn make_bot_with_queue(pieces: &[tbp::Piece]) -> Bot {
        Bot::from_start(&empty_board(), None, pieces, 0, false)
    }

    #[test]
    fn test_bot_from_start_empty_board() {
        let bot = make_bot_with_queue(&[tbp::Piece::T, tbp::Piece::I, tbp::Piece::O]);
        assert_eq!(bot.matrix.rows(), 0);
        assert_eq!(bot.queue.len(), 3);
        assert_eq!(bot.queue[0].as_char(), 'T');
        assert!(bot.hold.is_none());
    }

    #[test]
    fn test_to_snapshot_round_trips_active_piece() {
        let bot = make_bot_with_queue(&[
            tbp::Piece::T,
            tbp::Piece::S,
            tbp::Piece::Z,
            tbp::Piece::J,
            tbp::Piece::L,
            tbp::Piece::I,
            tbp::Piece::O,
        ]);
        let snap = bot.to_snapshot();
        assert_eq!(snap.queue.len(), 7);
        assert_eq!(snap.queue[0].as_char(), 'T');
        assert!(snap.hold.is_none());
    }

    #[test]
    fn test_apply_play_no_hold_consumes_active_piece() {
        let shtb = srs();
        let mut bot =
            make_bot_with_queue(&[tbp::Piece::T, tbp::Piece::I, tbp::Piece::O, tbp::Piece::S]);
        let snap = bot.to_snapshot();
        let state = blockfish::ai::State::from(snap);
        let (_rating, trace) =
            blockfish::ai::search_sync(&shtb, &bot.cfg, state).expect("search should find a move");
        let placements = blockfish::ai::reconstruct_placements(
            &shtb,
            blockfish::ai::State::from(bot.to_snapshot()),
            &trace,
        );
        let (color, tf, _) = placements[0];
        let (used_color, used_tf) = if color.as_char() == 'T' {
            (color, tf)
        } else {
            let mut pfind = blockfish::PlaceFinder::new(&shtb);
            pfind.reset_matrix(&bot.matrix);
            pfind.push_shape(blockfish::Color::try_from('T').unwrap(), false);
            let pl = pfind.next().expect("T has placements");
            (pl.shape.color(), pl.tf)
        };
        assert_eq!(used_color.as_char(), 'T');
        let mv = crate::coords::place_to_tbp_move(&shtb, used_color, used_tf)
            .expect("place_to_tbp_move should succeed for T");
        let queue_len_before = bot.queue.len();
        bot.apply_play(&shtb, &mv)
            .expect("apply_play should succeed");
        assert_eq!(bot.queue.len(), queue_len_before - 1);
        assert_eq!(bot.queue[0].as_char(), 'I');
        assert!(bot.hold.is_none());
        let mut filled = 0;
        for i in 0..bot.matrix.rows() {
            for j in 0..bot.matrix.cols() {
                if bot.matrix.get((i, j)) {
                    filled += 1;
                }
            }
        }
        assert_eq!(filled, 4, "T piece should add exactly 4 filled cells");
    }

    #[test]
    fn test_apply_play_first_hold_consumes_two_queue_slots() {
        let shtb = srs();
        let mut bot =
            make_bot_with_queue(&[tbp::Piece::T, tbp::Piece::I, tbp::Piece::O, tbp::Piece::S]);
        let mut pfind = blockfish::PlaceFinder::new(&shtb);
        pfind.reset_matrix(&bot.matrix);
        pfind.push_shape(blockfish::Color::try_from('I').unwrap(), false);
        let pl = pfind.next().expect("I has placements");
        let mv = crate::coords::place_to_tbp_move(&shtb, pl.shape.color(), pl.tf)
            .expect("I move conversion should succeed");
        bot.apply_play(&shtb, &mv)
            .expect("first-hold play should succeed");
        assert_eq!(bot.queue.len(), 2);
        assert_eq!(bot.queue[0].as_char(), 'O');
        assert_eq!(bot.hold.map(|c| c.as_char()), Some('T'));
    }

    #[test]
    fn test_push_new_piece_appends() {
        let mut bot = make_bot_with_queue(&[tbp::Piece::T]);
        bot.push_new_piece(tbp::Piece::I);
        bot.push_new_piece(tbp::Piece::O);
        assert_eq!(bot.queue.len(), 3);
        assert_eq!(bot.queue[1].as_char(), 'I');
        assert_eq!(bot.queue[2].as_char(), 'O');
    }
}
