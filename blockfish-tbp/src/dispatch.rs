//! Per-message dispatcher for the TBP protocol.
//!
//! `handle(&mut Option<Bot>, &ShapeTable, FrontendMessage) -> Option<BotMessage>`
//! is the single entry point for inbound TBP messages from the host. Returns
//! the optional reply to send back. The wasm entry point in `wasm.rs` calls
//! this from `self.onmessage` and posts the reply (if any) back to the host.

use crate::bot::Bot;
use crate::coords::place_to_tbp_move;
use blockfish::{ai, ShapeTable};

/// Handle one inbound TBP message, returning at most one outbound message.
pub fn handle(
    bot: &mut Option<Bot>,
    shtb: &ShapeTable,
    msg: tbp::FrontendMessage,
) -> Option<tbp::BotMessage> {
    match msg {
        tbp::FrontendMessage::Rules { .. } => Some(tbp::BotMessage::Ready),

        tbp::FrontendMessage::Start {
            hold,
            queue,
            combo,
            back_to_back,
            board,
            ..
        } => {
            *bot = Some(Bot::from_start(&board, hold, &queue, combo, back_to_back));
            None
        }

        tbp::FrontendMessage::Stop => {
            *bot = None;
            None
        }

        tbp::FrontendMessage::Suggest => {
            let bot_ref = match bot.as_ref() {
                Some(b) => b,
                None => {
                    return Some(empty_suggestion());
                }
            };
            // Run the search to completion. Synchronous on the wasm worker
            // thread; bounded by `cfg.search_limit`
            let snapshot = bot_ref.to_snapshot();
            let state = ai::State::from(snapshot.clone());
            let best = ai::search_sync(shtb, &bot_ref.cfg, state);

            let move_opt = best.and_then(|(_rating, trace)| {
                if trace.is_empty() {
                    return None;
                }
                let placements =
                    ai::reconstruct_placements(shtb, ai::State::from(snapshot), &trace);
                let (color, tf, _did_hold) = *placements.first()?;
                place_to_tbp_move(shtb, color, tf)
            });

            Some(match move_opt {
                Some(m) => tbp::BotMessage::Suggestion {
                    moves: vec![m],
                    #[cfg(feature = "move_info")]
                    move_info: Default::default(),
                },
                None => empty_suggestion(),
            })
        }

        tbp::FrontendMessage::Play { mv } => {
            if let Some(b) = bot.as_mut() {
                if let Err(reason) = b.apply_play(shtb, &mv) {
                    log_error(reason);
                    *bot = None;
                }
            }
            None
        }

        tbp::FrontendMessage::NewPiece { piece } => {
            if let Some(b) = bot.as_mut() {
                b.push_new_piece(piece);
            }
            None
        }

        tbp::FrontendMessage::Quit => {
            *bot = None;
            None
        }
    }
}

/// Build the initial `info` greeting the bot must post once at startup.
pub fn make_info() -> tbp::BotMessage {
    tbp::BotMessage::Info {
        name: "Blockfish".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        author: "iitalics, mystery".into(),
        features: tbp::Feature::enabled(),
    }
}

fn empty_suggestion() -> tbp::BotMessage {
    tbp::BotMessage::Suggestion {
        moves: vec![],
        #[cfg(feature = "move_info")]
        move_info: Default::default(),
    }
}

#[cfg(target_arch = "wasm32")]
fn log_error(msg: &str) {
    web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(msg));
}

#[cfg(not(target_arch = "wasm32"))]
fn log_error(msg: &str) {
    eprintln!("[blockfish-tbp] {}", msg);
}

#[cfg(test)]
mod test {
    use super::*;
    use blockfish::srs;

    fn empty_board() -> [[Option<char>; 10]; 40] {
        [[None; 10]; 40]
    }

    #[test]
    fn test_full_sequence() {
        let shtb = srs();
        let mut bot: Option<Bot> = None;

        // 1. rules - ready
        let r = handle(
            &mut bot,
            &shtb,
            tbp::FrontendMessage::Rules {
                randomizer: Default::default(),
            },
        );
        assert!(matches!(r, Some(tbp::BotMessage::Ready)));

        // 2. start - none, bot session created
        let r = handle(
            &mut bot,
            &shtb,
            tbp::FrontendMessage::Start {
                hold: None,
                queue: vec![
                    tbp::Piece::T,
                    tbp::Piece::I,
                    tbp::Piece::O,
                    tbp::Piece::S,
                    tbp::Piece::Z,
                    tbp::Piece::L,
                    tbp::Piece::J,
                ],
                combo: 0,
                back_to_back: false,
                board: empty_board(),
                randomizer: Default::default(),
            },
        );
        assert!(r.is_none());
        assert!(bot.is_some());

        // 3. suggest - suggestion with at least one move
        let r = handle(&mut bot, &shtb, tbp::FrontendMessage::Suggest);
        let mv = match r {
            Some(tbp::BotMessage::Suggestion { moves, .. }) => {
                assert!(!moves.is_empty(), "bot should suggest at least one move");
                moves[0]
            }
            other => panic!("expected Suggestion, got {:?}", other),
        };

        // 4. play - none (bot mutates internal state)
        let r = handle(&mut bot, &shtb, tbp::FrontendMessage::Play { mv });
        assert!(r.is_none());
        assert!(bot.is_some(), "bot should still be alive after play");

        // 5. new_piece - none, queue grows
        let r = handle(
            &mut bot,
            &shtb,
            tbp::FrontendMessage::NewPiece {
                piece: tbp::Piece::T,
            },
        );
        assert!(r.is_none());

        // 6. suggest again - another suggestion
        let r = handle(&mut bot, &shtb, tbp::FrontendMessage::Suggest);
        assert!(matches!(r, Some(tbp::BotMessage::Suggestion { .. })));

        // 7. stop - none, bot dropped
        let r = handle(&mut bot, &shtb, tbp::FrontendMessage::Stop);
        assert!(r.is_none());
        assert!(bot.is_none());
    }

    #[test]
    fn test_suggest_without_session_returns_empty() {
        let shtb = srs();
        let mut bot: Option<Bot> = None;
        let r = handle(&mut bot, &shtb, tbp::FrontendMessage::Suggest);
        match r {
            Some(tbp::BotMessage::Suggestion { moves, .. }) => assert!(moves.is_empty()),
            other => panic!("expected empty Suggestion, got {:?}", other),
        }
    }

    #[test]
    fn test_make_info_has_expected_shape() {
        match make_info() {
            tbp::BotMessage::Info {
                name,
                version,
                author,
                features,
            } => {
                assert_eq!(name, "Blockfish");
                assert!(!version.is_empty());
                assert!(!author.is_empty());
                assert_eq!(features, tbp::Feature::enabled());
            }
            other => panic!("expected Info, got {:?}", other),
        }
    }

    /// Verify start JSON
    #[test]
    fn test_start_json_deserializes_to_frontend_message() {
        let json = serde_json::json!({
            "type": "start",
            "hold": null,
            "queue": ["T", "L", "J", "S", "Z", "O", "I"],
            "combo": 0,
            "back_to_back": false,
            "board": vec![vec![serde_json::Value::Null; 10]; 40]
        });
        let msg: tbp::FrontendMessage = serde_json::from_value(json).expect("valid TBP start");
        match msg {
            tbp::FrontendMessage::Start {
                hold,
                queue,
                combo,
                back_to_back,
                board,
                ..
            } => {
                assert!(hold.is_none());
                assert_eq!(queue.len(), 7);
                assert_eq!(queue[0], tbp::Piece::T);
                assert_eq!(combo, 0);
                assert!(!back_to_back);
                // board is 40×10 of None
                for row in board.iter() {
                    for cell in row.iter() {
                        assert!(cell.is_none());
                    }
                }
            }
            other => panic!("expected Start, got {:?}", other),
        }
    }
}
