//! The one row of terminal cells above the picture: what the session is doing.

use std::io;

use katui::present::cells::render_cell;
use katui::scene::CellSize;
use katui::scene::cell::{self, Attrs, Buffer, Color, Position};

/// The rows the status takes from the top of the screen.
pub(crate) const ROWS: u16 = 1;
const TEXT: Color = Color::Indexed(250);
const BAR: Color = Color::Indexed(234);

#[derive(Debug, Default)]
pub(crate) struct Status {
    text: String,
    shown: Option<Buffer>,
}

impl Status {
    /// Show `text` from the next frame on.
    pub(crate) fn set(&mut self, text: impl Into<String>) {
        let text = text.into();
        if self.text != text {
            self.text = text;
            self.shown = None;
        }
    }

    /// Send the row again in full on the next frame, for when a picture covered it.
    pub(crate) fn invalidate(&mut self) {
        self.shown = None;
    }

    /// Write the cells of the row that changed since the last frame.
    pub(crate) fn draw(&mut self, out: &mut Vec<u8>, columns: u16) -> io::Result<()> {
        let mut row = Buffer::blank(CellSize {
            columns,
            rows: ROWS,
        });
        row.set_string(
            Position { x: 0, y: 0 },
            &" ".repeat(usize::from(columns)),
            TEXT,
            BAR,
            Attrs::EMPTY,
        );
        row.set_string(Position { x: 1, y: 0 }, &self.text, TEXT, BAR, Attrs::EMPTY);
        let changed = match &self.shown {
            Some(shown) => cell::diff(shown, &row),
            None => cell::all_cells(&row),
        };
        for (at, cell) in &changed {
            render_cell(out, *at, cell)?;
        }
        if !changed.is_empty() {
            out.extend_from_slice(b"\x1b[0m");
        }
        self.shown = Some(row);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Status;

    #[test]
    fn the_row_writes_once_and_then_only_what_changed() {
        let mut status = Status::default();
        let mut out = Vec::new();
        status.set("connecting");
        status.draw(&mut out, 20).expect("writing to a Vec works");
        let first = out.len();
        assert!(first > 0, "the first frame writes the row");
        status.draw(&mut out, 20).expect("writing to a Vec works");
        assert_eq!(out.len(), first, "an unchanged row writes nothing");
        status.set("live");
        status.draw(&mut out, 20).expect("writing to a Vec works");
        assert!(out.len() > first, "a changed row writes its cells");
        status.invalidate();
        let before = out.len();
        status.draw(&mut out, 20).expect("writing to a Vec works");
        assert!(
            out.len() > before,
            "an invalidated row writes in full again"
        );
    }
}
