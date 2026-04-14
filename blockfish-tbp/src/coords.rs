//! Coordinate conversion between blockfish's `Place::tf = (row, col, orientation)`
//! (bounding-box origin, row 0 = bottom) and TBP's `location.{x, y, orientation}`
//! (piece center cell, y = bottom-up).
//!
//! ## Why libtetris?
//!
//! The `tbp` crate defines piece locations in libtetris's `(x, y, orientation)`
//! axes, with the pivot at the piece's rotation center. Routing blockfish
//! placements through libtetris's reference cells lets every TBP host interpret
//! our moves identically, with no per-(piece, rotation) mapping table to
//! maintain and no assumption about how blockfish's `R0` lines up with TBP's
//! `North`.
//!
//! ## Strategy
//!
//! 1. Blit the blockfish placement onto a temporary `BasicMatrix` to get its
//!    absolute cells (`ShapeRef::blit_to` is the only public way in).
//! 2. Read the filled cells as libtetris `(x, y)` pairs (note the row/col ↔
//!    y/x swap).
//! 3. For each of the four TBP orientations of the same piece, translate the
//!    libtetris reference cells and check for set equality against step 2.
//! 4. The winning translation offset *is* the TBP `(x, y)` center, since
//!    libtetris centers each rotation around `(0, 0)`.

use crate::adapter::{from_tbp_piece, to_tbp_piece};
use blockfish::{BasicMatrix, Color, PlaceFinder, ShapeRef, ShapeTable, Transform};

// ---------------------------------------------------------------------------
// libtetris reference cell tables
//
// The `LIBTETRIS_CELLS` table and the `lt!` rotation macro below are derived
// from libtetris's `Piece::cells()` implementation:
//
//   Source:  https://github.com/MinusKelvin/cold-clear
//   File:    libtetris/src/piece.rs (lines 298-320, `gen_cells!` macro)
//   Author:  MinusKelvin
//   License: Mozilla Public License, v. 2.0
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.
// ---------------------------------------------------------------------------

/// Index = `piece as usize * 4 + orientation as usize` where
/// `Piece` order is I, O, T, L, J, S, Z and
/// `Orientation` order is North=0, South=1, East=2, West=3.
type CellArray = [(i32, i32); 4];

#[allow(clippy::double_neg)]
const LIBTETRIS_CELLS: [[CellArray; 4]; 7] = {
    // Each row is the 4 rotations of one piece, ordered N, S, E, W.
    // Per libtetris's gen_cells! macro:
    //   N: (x, y), S: (-x, -y), E: (y, -x), W: (-y, x)
    macro_rules! lt {
        ([$(($x:expr, $y:expr)),*]) => {
            [
                [$(($x, $y)),*],     // North
                [$((-$x, -$y)),*],   // South
                [$(($y, -$x)),*],    // East
                [$((-$y, $x)),*],    // West
            ]
        };
    }
    [
        lt!([(-1, 0), (0, 0), (1, 0), (2, 0)]),  // I
        lt!([(0, 0), (1, 0), (0, 1), (1, 1)]),   // O
        lt!([(-1, 0), (0, 0), (1, 0), (0, 1)]),  // T
        lt!([(-1, 0), (0, 0), (1, 0), (1, 1)]),  // L
        lt!([(-1, 0), (0, 0), (1, 0), (-1, 1)]), // J
        lt!([(-1, 0), (0, 0), (0, 1), (1, 1)]),  // S
        lt!([(-1, 1), (0, 1), (0, 0), (1, 0)]),  // Z
    ]
};

/// Map a blockfish color (single letter) to its index in `LIBTETRIS_CELLS`.
fn piece_index(c: Color) -> Option<usize> {
    match c.as_char() {
        'I' => Some(0),
        'O' => Some(1),
        'T' => Some(2),
        'L' => Some(3),
        'J' => Some(4),
        'S' => Some(5),
        'Z' => Some(6),
        _ => None,
    }
}

/// Convert a `tbp::Orientation` into the index used by `LIBTETRIS_CELLS` rows.
fn lt_orientation_index(o: tbp::Orientation) -> usize {
    match o {
        tbp::Orientation::North => 0,
        tbp::Orientation::South => 1,
        tbp::Orientation::East => 2,
        tbp::Orientation::West => 3,
    }
}

const ALL_TBP_ORIENTATIONS: [tbp::Orientation; 4] = [
    tbp::Orientation::North,
    tbp::Orientation::South,
    tbp::Orientation::East,
    tbp::Orientation::West,
];

// ---------------------------------------------------------------------------
// Cell extraction from blockfish placements
// ---------------------------------------------------------------------------

/// Scratch-matrix width for `cells_for_placement`. **Must be 15, not 16**:
/// `BasicMatrix` uses `u16` row bitmasks and `empty_row_bits(16)` overflows
/// the shift count, making empty rows read as fully filled.
const WIDTH: u16 = 15;
/// Border padding so negative `tf` components (blockfish placements can go
/// down to `tf.1 = -2` on a 10-col board) still land inside the scratch
/// matrix after the `(+PAD, +PAD)` shift.
const PAD: i16 = 5;

/// Extract a blockfish placement's 4 filled cells as libtetris `(x, y)` pairs
/// (`x` = col from left, `y` = row from bottom — note the swap vs blockfish
/// `(i, j)`). The returned cells are in the original un-padded frame.
pub fn cells_for_placement(shape: ShapeRef, tf: Transform) -> Vec<(i32, i32)> {
    let mut mat = BasicMatrix::with_cols(WIDTH);
    let padded_tf: Transform = (tf.0 + PAD, tf.1 + PAD, tf.2);
    shape.blit_to(&mut mat, padded_tf);
    let mut cells = Vec::with_capacity(4);
    for i in 0..mat.rows() {
        for j in 0..mat.cols() {
            if mat.get((i, j)) {
                let x = j as i32 - PAD as i32;
                let y = i as i32 - PAD as i32;
                cells.push((x, y));
            }
        }
    }
    cells
}

// ---------------------------------------------------------------------------
// Public conversion API
// ---------------------------------------------------------------------------

/// Convert a blockfish placement into a TBP `Move`. Returns `None` for
/// non-tetromino colors (e.g. garbage) or if no libtetris orientation matches
/// the blockfish cell set (would indicate a conversion bug).
pub fn place_to_tbp_move(
    shape_table: &ShapeTable,
    piece: Color,
    tf: Transform,
) -> Option<tbp::Move> {
    let tbp_piece = to_tbp_piece(piece)?;
    let pidx = piece_index(piece)?;
    let shape = shape_table.shape(piece)?;
    let bf_cells = cells_for_placement(shape, tf);
    if bf_cells.len() != 4 {
        return None;
    }
    let bf_min_x = bf_cells.iter().map(|c| c.0).min()?;
    let bf_min_y = bf_cells.iter().map(|c| c.1).min()?;
    let bf_set: std::collections::HashSet<(i32, i32)> = bf_cells.iter().copied().collect();

    for orientation in ALL_TBP_ORIENTATIONS {
        let lt_offsets = LIBTETRIS_CELLS[pidx][lt_orientation_index(orientation)];
        let lt_min_x = lt_offsets.iter().map(|c| c.0).min()?;
        let lt_min_y = lt_offsets.iter().map(|c| c.1).min()?;
        let dx = bf_min_x - lt_min_x;
        let dy = bf_min_y - lt_min_y;
        let aligned: std::collections::HashSet<(i32, i32)> =
            lt_offsets.iter().map(|&(x, y)| (x + dx, y + dy)).collect();
        if aligned == bf_set {
            // libtetris centers each rotation at (0, 0), so (dx, dy) IS the
            // pivot in TBP coordinates.
            return Some(tbp::Move {
                location: tbp::PieceLocation {
                    kind: tbp_piece,
                    orientation,
                    x: dx,
                    y: dy,
                },
                spin: tbp::Spin::None, // T-spin detection not implemented.
            });
        }
    }
    None
}

/// Inverse of `place_to_tbp_move`: find a blockfish `Place` whose cells match
/// the given TBP move. Returns `(color, tf, did_hold)`, where `did_hold` is
/// `true` if the move's piece differs from the current active piece (the host
/// inferred a hold swap).
pub fn tbp_move_to_blockfish(
    shape_table: &ShapeTable,
    matrix: &BasicMatrix,
    mv: &tbp::Move,
    active_piece: Color,
    hold_piece: Option<Color>,
) -> Option<(Color, Transform, bool)> {
    let target_color = from_tbp_piece(mv.location.kind);
    let did_hold = target_color != active_piece;
    if did_hold {
        // If hold was swapped, the target must match the hold slot. When
        // `hold_piece` is `None` the host inferred the swap from the next
        // preview piece, which we can't validate here.
        if let Some(h) = hold_piece {
            if h != target_color {
                return None;
            }
        }
    }

    let mut pfind = PlaceFinder::new(shape_table);
    pfind.reset_matrix(matrix);
    pfind.push_shape(target_color, did_hold);

    for pl in pfind {
        if let Some(candidate) = place_to_tbp_move(shape_table, target_color, pl.tf) {
            if candidate.location == mv.location {
                return Some((target_color, pl.tf, did_hold));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod test {
    use super::*;
    use blockfish::{srs, Orientation};
    use std::convert::TryFrom;

    /// Every (piece, blockfish rotation) must map to *some* TBP orientation.
    /// A failure means blockfish and libtetris disagree on a piece's shape
    /// — a deep convention mismatch.
    #[test]
    fn test_every_piece_rotation_finds_a_libtetris_match() {
        let shtb = srs();
        let mut total = 0;
        let mut matched = 0;
        for ch in "IOTLJSZ".chars() {
            let color = Color::try_from(ch).unwrap();
            for &rot in &[
                Orientation::R0,
                Orientation::R1,
                Orientation::R2,
                Orientation::R3,
            ] {
                total += 1;
                let tf: Transform = (3, 3, rot);
                let shape = shtb.shape(color).unwrap();
                let cells = cells_for_placement(shape, tf);
                let mv = place_to_tbp_move(&shtb, color, tf);
                assert!(
                    mv.is_some(),
                    "no libtetris match for piece={} rotation={:?}; bf_cells={:?}",
                    ch,
                    rot,
                    cells
                );
                matched += 1;
            }
        }
        assert_eq!(matched, total);
        assert_eq!(total, 28);
    }

    /// Round-trip: libtetris cells reconstructed from a TBP move must equal
    /// the original blockfish cells. Catches axis-swap and orientation
    /// table errors.
    #[test]
    fn test_round_trip_via_libtetris_table() {
        let shtb = srs();
        for ch in "IOTLJSZ".chars() {
            let color = Color::try_from(ch).unwrap();
            let pidx = piece_index(color).unwrap();
            for &rot in &[
                Orientation::R0,
                Orientation::R1,
                Orientation::R2,
                Orientation::R3,
            ] {
                let tf: Transform = (3, 3, rot);
                let shape = shtb.shape(color).unwrap();
                let bf_cells: std::collections::HashSet<(i32, i32)> =
                    cells_for_placement(shape, tf).into_iter().collect();
                let mv = place_to_tbp_move(&shtb, color, tf)
                    .unwrap_or_else(|| panic!("no match for {} {:?}", ch, rot));
                let lt_orient_idx = lt_orientation_index(mv.location.orientation);
                let reconstructed: std::collections::HashSet<(i32, i32)> = LIBTETRIS_CELLS[pidx]
                    [lt_orient_idx]
                    .iter()
                    .map(|&(x, y)| (x + mv.location.x, y + mv.location.y))
                    .collect();
                assert_eq!(
                    bf_cells, reconstructed,
                    "cell set mismatch for {} {:?}: bf={:?} reconstructed={:?}",
                    ch, rot, bf_cells, reconstructed
                );
            }
        }
    }

    /// Full `tf → TBP → tf'` round trip via `tbp_move_to_blockfish` must
    /// recover the same placement on an empty board.
    #[test]
    fn test_tbp_round_trip_no_hold() {
        let shtb = srs();
        let board = BasicMatrix::with_cols(10);
        for ch in "IOTLJSZ".chars() {
            let color = Color::try_from(ch).unwrap();
            let mut pfind = PlaceFinder::new(&shtb);
            pfind.reset_matrix(&board);
            pfind.push_shape(color, false);
            let pl = pfind
                .next()
                .unwrap_or_else(|| panic!("no placement for {}", ch));
            let original_tf = pl.tf;
            let mv = place_to_tbp_move(&shtb, color, original_tf).unwrap();
            let (back_color, back_tf, back_held) =
                tbp_move_to_blockfish(&shtb, &board, &mv, color, None)
                    .unwrap_or_else(|| panic!("inverse failed for {}", ch));
            assert_eq!(back_color, color);
            assert!(!back_held);
            // Compare cells, not `tf` — symmetric pieces like O have multiple
            // valid tfs that produce the same footprint.
            let original_cells: std::collections::HashSet<(i32, i32)> =
                cells_for_placement(shtb.shape(color).unwrap(), original_tf)
                    .into_iter()
                    .collect();
            let recovered_cells: std::collections::HashSet<(i32, i32)> =
                cells_for_placement(shtb.shape(color).unwrap(), back_tf)
                    .into_iter()
                    .collect();
            assert_eq!(
                original_cells, recovered_cells,
                "round-trip cells mismatch for {}",
                ch
            );
        }
    }

    #[test]
    fn test_natural_placements_below_topout() {
        let shtb = srs();
        let board = BasicMatrix::with_cols(10);
        for ch in "IOTLJSZ".chars() {
            let color = Color::try_from(ch).unwrap();
            let mut pfind = PlaceFinder::new(&shtb);
            pfind.reset_matrix(&board);
            pfind.push_shape(color, false);
            for pl in pfind {
                let mv = place_to_tbp_move(&shtb, color, pl.tf).unwrap();
                assert!(
                    mv.location.y < 21,
                    "topout for {} at tf={:?}: y={}",
                    ch,
                    pl.tf,
                    mv.location.y
                );
                assert!(
                    mv.location.y >= 0,
                    "negative y for {}: {}",
                    ch,
                    mv.location.y
                );
            }
        }
    }
}
