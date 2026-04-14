//! Conversion helpers between the external `tbp` crate types and blockfish's
//! internal types.

use blockfish::{BasicMatrix, Color, Orientation};
use std::convert::TryFrom;

/// Convert a `tbp::Piece` into a blockfish `Color`.
pub fn from_tbp_piece(p: tbp::Piece) -> Color {
    let ch = match p {
        tbp::Piece::I => 'I',
        tbp::Piece::O => 'O',
        tbp::Piece::T => 'T',
        tbp::Piece::L => 'L',
        tbp::Piece::J => 'J',
        tbp::Piece::S => 'S',
        tbp::Piece::Z => 'Z',
    };
    Color::try_from(ch).expect("tbp piece letter is always alphabetic")
}

/// Convert a blockfish `Color` back into a `tbp::Piece`.
pub fn to_tbp_piece(c: Color) -> Option<tbp::Piece> {
    match c.as_char() {
        'I' => Some(tbp::Piece::I),
        'O' => Some(tbp::Piece::O),
        'T' => Some(tbp::Piece::T),
        'L' => Some(tbp::Piece::L),
        'J' => Some(tbp::Piece::J),
        'S' => Some(tbp::Piece::S),
        'Z' => Some(tbp::Piece::Z),
        _ => None,
    }
}

/// Map a blockfish `Orientation` to the TBP string-tagged enum.
/// Blockfish's `R0..R3` is CW from spawn (`common.rs::Orientation::cw`), which
/// matches TBP's `North, East, South, West` clockwise cycle.
pub fn to_tbp_orientation(r: Orientation) -> tbp::Orientation {
    match r {
        Orientation::R0 => tbp::Orientation::North,
        Orientation::R1 => tbp::Orientation::East,
        Orientation::R2 => tbp::Orientation::South,
        Orientation::R3 => tbp::Orientation::West,
    }
}

pub fn from_tbp_orientation(r: tbp::Orientation) -> Orientation {
    match r {
        tbp::Orientation::North => Orientation::R0,
        tbp::Orientation::East => Orientation::R1,
        tbp::Orientation::South => Orientation::R2,
        tbp::Orientation::West => Orientation::R3,
    }
}

/// Ingest a TBP `start` board into a fresh `BasicMatrix`.
pub fn board_to_matrix(board: &[[Option<char>; 10]; 40]) -> BasicMatrix {
    let mut mat = BasicMatrix::with_cols(10);
    for (row, cells) in board.iter().enumerate() {
        for (col, cell) in cells.iter().enumerate() {
            if cell.is_some() {
                mat.set((row as u16, col as u16));
            }
        }
    }
    mat
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_piece_round_trip() {
        for ch in "IOTLJSZ".chars() {
            let color = Color::try_from(ch).unwrap();
            let p = to_tbp_piece(color).expect("tetromino letter should map");
            let back = from_tbp_piece(p);
            assert_eq!(back.as_char(), ch);
        }
    }

    #[test]
    fn test_garbage_does_not_map_to_piece() {
        let g = Color::try_from('G').unwrap();
        assert_eq!(to_tbp_piece(g), None);
    }

    #[test]
    fn test_orientation_round_trip() {
        for &r in &[
            Orientation::R0,
            Orientation::R1,
            Orientation::R2,
            Orientation::R3,
        ] {
            let t = to_tbp_orientation(r);
            assert_eq!(from_tbp_orientation(t), r);
        }
        // spot-check specific names
        assert!(matches!(
            to_tbp_orientation(Orientation::R0),
            tbp::Orientation::North
        ));
        assert!(matches!(
            to_tbp_orientation(Orientation::R1),
            tbp::Orientation::East
        ));
        assert!(matches!(
            to_tbp_orientation(Orientation::R2),
            tbp::Orientation::South
        ));
        assert!(matches!(
            to_tbp_orientation(Orientation::R3),
            tbp::Orientation::West
        ));
    }

    #[test]
    fn test_board_ingestion_bottom_row_first() {
        let mut board: [[Option<char>; 10]; 40] = [[None; 10]; 40];
        // Put a garbage cell at TBP (x=4, y=0)
        board[0][4] = Some('G');
        // Another at TBP (x=7, y=3).
        board[3][7] = Some('G');
        let mat = board_to_matrix(&board);
        // Matrix row 0 should have col 4 filled (and nothing else)
        assert!(mat.get((0, 4)));
        assert!(!mat.get((0, 3)));
        assert!(mat.get((3, 7)));
        assert!(!mat.get((2, 7)));
    }
}
