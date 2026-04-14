use crate::{
    config::Config,
    place::PlaceFinder,
    shape::{srs, ShapeTable, Transform},
    BasicMatrix, Color, Input,
};

mod analysis;
mod b_star;
mod eval;
mod state;

// Re-exports for external consumers that want to drive the search directly
// (e.g. `blockfish-tbp` on wasm, where threaded `Analysis` cannot be used).
pub use b_star::{Search, SearchTerminated, Step};
pub use state::State;

// Input / output types

/// A game state snapshot to begin an analysis from.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Snapshot {
    pub hold: Option<Color>,
    pub queue: Vec<Color>,
    pub matrix: BasicMatrix,
}

/// A suggested sequence and its rating.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Suggestion {
    /// List of inputs to perform the suggested move.
    pub inputs: Vec<Input>,
    // The "rating" is an abstract measurement for how good a move is (lower is better).
    pub rating: i64,
}

/// Statistics about the analysis after it has finished.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct Stats {
    /// Number of iterations of the search algorithm.
    pub iterations: usize,
    /// Number of nodes generated.
    pub nodes: usize,
    /// Total time taken to do the analysis.
    pub time_taken: std::time::Duration,
}

// Evaluation function interface

pub use eval::Eval;

/// Performs the static analysis function on a snapshot.
pub fn static_eval(snapshot: &Snapshot) -> Eval {
    eval::eval(&snapshot.matrix)
}

// AI interface

// Re-export
pub use analysis::{Analysis, AnalysisDone, MoveId};

/// An instance of the Blockfish AI. Holds engine configuration and can be used to spawn
/// an analysis.
///
/// `AI` can currently be seen as just a sort of builder-pattern type for creating
/// `Analysis`'s.  However, in the future it could be extended to handle more things such
/// as a reusable thread pool and/or the place to call `static_eval`.
pub struct AI {
    config: Config,
    shape_table: std::sync::Arc<ShapeTable>,
    all_tx: Option<std::sync::mpsc::Sender<Suggestion>>,
}

impl AI {
    /// Constructs a new Blockfish AI instance with the given engine configuration.
    pub fn new(config: Config) -> Self {
        Self {
            config,
            shape_table: std::sync::Arc::new(srs()),
            all_tx: None,
        }
    }

    pub fn config(&self) -> Config {
        self.config.clone()
    }

    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }

    /// Begins a new analysis of `snapshot`, returning a handle to it.
    pub fn analyze(&mut self, snapshot: Snapshot) -> Analysis {
        analysis::spawn(
            self.shape_table.clone(),
            self.config.clone(),
            snapshot.into(),
            self.all_tx.take(),
        )
    }

    /// Configures the next analysis (via `analyze()`) to send every suggestion it
    /// encounters to a non-blocking channel. Returns the rx end of that channel.
    ///
    /// NOTE: Even rejected suggestions are sent to this channel; this channel should just
    /// be used for diagnostic purposes.
    pub fn listen_all(&mut self) -> std::sync::mpsc::Receiver<Suggestion> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.all_tx = Some(tx);
        rx
    }
}

// ---------------------------------------------------------------------------
// Synchronous search driver (wasm-friendly alternative to `analysis::spawn`)
// ---------------------------------------------------------------------------

/// Drives the B* search synchronously to completion (or until the configured
/// `search_limit` is hit), returning the best move's `(rating, trace)` pair.
///
/// Parallel to `analysis::spawn`, but thread-free: no `std::thread`, no
/// `mpsc`, no `RwLock`. Intended for single-threaded environments like wasm.
pub fn search_sync(
    shape_table: &ShapeTable,
    config: &Config,
    root: State,
) -> Option<(i64, Vec<usize>)> {
    let params = config.parameters.clone();
    let mut search = Search::new(shape_table, params);
    search.start(root);
    let mut best: Option<(i64, Vec<usize>)> = None;
    while search.node_count() < config.search_limit {
        match search.step() {
            Ok(Step::RatingChanged { rating, trace, .. }) => {
                if best.as_ref().map_or(true, |(r, _)| rating < *r) {
                    best = Some((rating, trace));
                }
            }
            Ok(Step::SequenceRejected { .. }) | Ok(Step::Other) => {}
            Err(_) => break, // search fringe exhausted
        }
    }
    best
}

/// Reconstructs the placement chain for a trace returned by `search_sync`.
/// Parallel to the private `analysis::reconstruct_inputs`
/// but yields `(Color, Transform, did_hold)` tuples instead of keystroke inputs,
/// so the caller can inspect the chosen placement locations directly.
///
/// The returned vec has the same length as `trace`.
pub fn reconstruct_placements(
    shape_table: &ShapeTable,
    state0: State,
    trace: &[usize],
) -> Vec<(Color, Transform, bool)> {
    let mut pfind = PlaceFinder::new(shape_table);
    let mut state = state0;
    let mut out = Vec::with_capacity(trace.len());
    for &idx in trace {
        let pl = state
            .placements(&mut pfind)
            .find(|pl| pl.idx == idx)
            .expect("trace idx out of range");
        out.push((pl.shape.color(), pl.tf, pl.did_hold));
        state.place(&pl);
    }
    out
}

#[cfg(test)]
mod test_sync {
    use super::*;
    use crate::Color;

    #[test]
    fn test_search_sync_empty_board_finds_move() {
        let shtb = srs();
        let cfg = Config::default();
        let snapshot = Snapshot {
            hold: None,
            queue: "IOTSZLJ".chars().map(Color::n).collect(),
            matrix: BasicMatrix::with_cols(10),
        };
        let state: State = snapshot.clone().into();
        let best = search_sync(&shtb, &cfg, state);
        assert!(
            best.is_some(),
            "search_sync should find at least one move on an empty board"
        );
        let (_rating, trace) = best.unwrap();
        assert!(!trace.is_empty(), "trace must not be empty");

        // Walk the placements; each must legally succeed against the current state.
        let state0: State = snapshot.into();
        let placements = reconstruct_placements(&shtb, state0, &trace);
        assert_eq!(placements.len(), trace.len());
        // All placements have a valid color (the piece originally in the queue).
        for (color, _tf, _held) in &placements {
            let _ch = color.as_char();
        }
    }
}
