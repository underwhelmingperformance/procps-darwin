// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2024 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2012-2014 Jaromir Capik <jcapik@redhat.com>
// SPDX-FileCopyrightText: 2011 Lukas Nykryn <lnykryn@redhat.com>
// SPDX-FileCopyrightText: 1998-2004 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/output.c and src/ps/display.c.

use std::io::Write;

use crate::ps::{
    format::{Column, Content},
    options::{Header, Modifiers, Screen},
    print::{Cell, OUTBUF_SIZE, Room},
    specifier::{Justification, Specifier},
};

/// The most padding that procps-ng writes before a column
/// (`SPACE_AMOUNT`).
const SPACE_AMOUNT: i64 = 144;

/// How procps-ng aligns a column, from its `CF_JUST_MASK` flags. Text
/// between the descriptors of an AIX format has no flags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Alignment {
    /// A specifier's alignment.
    Specifier(Justification),
    /// Text of an AIX format, which procps-ng writes as it is.
    Text,
}

impl Alignment {
    fn of(column: &Column) -> Self {
        match column.content {
            Content::Specifier(specifier) => Self::Specifier(specifier.justification),
            Content::Text => Self::Text,
        }
    }
}

/// The settings of a [`Layout`] that come from the options.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Settings {
    /// Where the header line is printed.
    pub(crate) header: Header,
    /// The screen size.
    pub(crate) screen: Screen,
    /// `--delimiter`: the byte between columns.
    pub(crate) delimiter: Option<u8>,
    /// The options that change how columns show their values.
    pub(crate) modifiers: Modifiers,
}

/// procps-ng's layout of `ps`'s lines: the width of a line, where each
/// column starts, and when the header is printed.
#[derive(Debug)]
pub(crate) struct Layout<'c> {
    columns: &'c [Column],
    /// The width of a line, a multiple of the screen's width
    /// (`active_cols`).
    active_cols: u32,
    /// Whether a line has room for signal masks of 16 digits.
    wide_signals: bool,
    delimiter: Option<u8>,
    /// BSD's `n`: user IDs and wait channels are numbers, aligned on the
    /// right (`user_is_number` and `wchan_is_number`).
    numeric: bool,
    /// The rows until the next header (`lines_to_next_header`). A header is
    /// printed when the count reaches 0.
    lines_to_next_header: i64,
    /// The rows between headers (`header_gap`), or -1 for no further header.
    header_gap: i64,
    /// Whether any line has been printed (`did_stuff`).
    did_stuff: bool,
}

impl<'c> Layout<'c> {
    /// The layout of `columns`, as procps-ng's `check_headers` and
    /// `check_header_width` decide it.
    pub(crate) fn new(columns: &'c [Column], settings: Settings) -> Self {
        let mut lines_to_next_header = 1;
        let mut header_gap = -1;

        match settings.header {
            Header::Multiple => header_gap = settings.screen.rows - 1,
            Header::None => lines_to_next_header = -1,
            Header::Single => {
                let has_header = columns.iter().any(|column| {
                    !column.header.is_empty() && matches!(column.content, Content::Specifier(_))
                });

                if !has_header {
                    lines_to_next_header = -1;
                }
            }
        }

        let (active_cols, wide_signals) = check_header_width(columns, settings.screen);

        Self {
            columns,
            active_cols,
            wide_signals,
            delimiter: settings.delimiter,
            numeric: settings.modifiers.contains(Modifiers::NUMERIC),
            lines_to_next_header,
            header_gap,
            did_stuff: false,
        }
    }

    /// Prints a row, after a header if one is due, as procps-ng's
    /// `show_one_proc` does. `print` gives the text of each column that shows
    /// a specifier, with the room that the column has.
    pub(crate) fn row(
        &mut self,
        mut print: impl FnMut(&'static Specifier, Room) -> Cell,
        stdout: &mut impl Write,
        stderr: &mut impl Write,
    ) {
        self.lines_to_next_header -= 1;

        if self.lines_to_next_header == 0 {
            self.lines_to_next_header = self.header_gap;
            self.line(None, stdout, stderr);
        }

        self.line(Some(&mut print), stdout, stderr);
    }

    /// Prints the header if no line has been printed yet and a header is
    /// due, as procps-ng's last call of `show_one_proc` does. Returns whether
    /// a row for a process was printed. procps-ng exits with status 1 when
    /// none was.
    pub(crate) fn finish(&mut self, stdout: &mut impl Write, stderr: &mut impl Write) -> bool {
        if self.did_stuff {
            return true;
        }

        self.lines_to_next_header -= 1;

        if self.lines_to_next_header == 0 {
            self.lines_to_next_header = self.header_gap;
            self.line(None, stdout, stderr);
        }

        false
    }

    /// The room of a column (`max_rightward`), and the padding that the
    /// last column may need (`tmpspace`), as procps-ng computes them. A
    /// column before the last has its width. The last column has the rest of
    /// the line. procps-ng computes the rest in unsigned arithmetic, so if
    /// the last column starts beyond the line's width, the subtraction wraps
    /// and the room is capped at one byte less than the output buffer.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "procps-ng mixes int and unsigned arithmetic, which wraps"
    )]
    fn room(&self, width: i64, last: bool, position: Position) -> (u32, i64) {
        let Position {
            correct,
            actual,
            dospace,
        } = position;
        let (room, tmpspace) = if !last {
            (width as u32, 0)
        } else if correct - actual < 1 {
            (
                self.active_cols
                    .wrapping_sub(actual as u32)
                    .wrapping_sub(dospace as u32),
                dospace,
            )
        } else {
            (
                self.active_cols.wrapping_sub(correct as u32),
                correct - actual,
            )
        };

        (room.min(OUTBUF_SIZE as u32 - 1), tmpspace)
    }

    /// The padding before `cell` in `column`, and the cells that a wide
    /// signal mask takes beyond its width (`legit`), as procps-ng computes
    /// them. A column without a limit, or a wait channel, that starts at the
    /// end of the line keeps only its first byte. `remaining` is the room
    /// that the line has left, in procps-ng's unsigned arithmetic.
    fn leftpad(&self, column: &Column, cell: &mut Cell, remaining: u32) -> (i64, i64) {
        let width = int(column.width);
        let fill = |columns: i64| (columns - cell.amount).max(0);

        match Alignment::of(column) {
            Alignment::Specifier(Justification::User | Justification::Wchan) if self.numeric => {
                (fill(width), 0)
            }
            Alignment::Text | Alignment::Specifier(Justification::Left | Justification::User) => {
                (0, 0)
            }
            Alignment::Specifier(Justification::Right) => (fill(width), 0),
            Alignment::Specifier(Justification::Signal) if self.wide_signals => (fill(16), 7),
            Alignment::Specifier(Justification::Signal) => (fill(9), 0),
            Alignment::Specifier(Justification::Wchan | Justification::Unlimited) => {
                if remaining == 0 {
                    cell.bytes.truncate(1);
                }

                (0, 0)
            }
        }
    }

    /// Prints one line: the header without `print`, or a row.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "procps-ng mixes int and unsigned arithmetic, which wraps"
    )]
    fn line(
        &mut self,
        mut print: Option<&mut dyn FnMut(&'static Specifier, Room) -> Cell>,
        stdout: &mut impl Write,
        stderr: &mut impl Write,
    ) {
        // A failed write is reported when `ps` closes its output, as
        // procps-ng's `close_stdout` reports it.
        let mut write = |bytes: &[u8]| {
            let _ = stdout.write_all(bytes);
        };
        let pad = self.delimiter.unwrap_or(b' ');
        let mut position = Position::default();

        self.did_stuff = true;

        if self.active_cols > OUTBUF_SIZE as u32 {
            let _ = stderr.write_all(b"fix bigness error\n");
        }

        for (index, column) in self.columns.iter().enumerate() {
            let next = self.columns.get(index + 1);
            let (room, tmpspace) = self.room(int(column.width), next.is_none(), position);
            let room = Room {
                cells: room as usize,
                wide_signals: self.wide_signals,
                lines_to_next_header: self.lines_to_next_header,
            };
            let mut cell = match (print.as_mut(), column.content) {
                (Some(print), Content::Specifier(specifier)) => print(specifier, room),
                _ => Cell::text(column.header.clone()),
            };

            cell.amount = cell.amount.clamp(0, int(OUTBUF_SIZE) - 1);
            cell.bytes.truncate(OUTBUF_SIZE - 1);

            let remaining = self
                .active_cols
                .wrapping_sub(position.actual as u32)
                .wrapping_sub(tmpspace as u32);
            let (leftpad, legit) = self.leftpad(column, &mut cell, remaining);
            let mut space = position.correct - position.actual + leftpad;

            if space < 1 || self.delimiter.is_some() {
                space = position.dospace;
            }

            write(&vec![pad; space.clamp(0, SPACE_AMOUNT) as usize]);
            write(&cell.bytes);

            let Some(next) = next else {
                write(b"\n");
                break;
            };
            let both_specifiers = matches!(column.content, Content::Specifier(_))
                && matches!(next.content, Content::Specifier(_));

            position = Position {
                correct: position.correct + int(column.width) + legit + i64::from(both_specifiers),
                actual: position.actual + space.min(SPACE_AMOUNT) + cell.amount,
                dospace: i64::from(both_specifiers),
            };
        }
    }
}

/// Where procps-ng is on a line.
#[derive(Clone, Copy, Debug, Default)]
struct Position {
    /// Where the next column should start (`correct`).
    correct: i64,
    /// Where the next column would start without padding (`actual`).
    actual: i64,
    /// Whether the next column needs a space before it (`dospace`).
    dospace: i64,
}

/// `value` as procps-ng's `int`.
fn int(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The width of a line and whether it has room for wide signal masks, as
/// procps-ng's `check_header_width` decides them. procps-ng tries the
/// screen's width and then each multiple of it, and stops at the first
/// multiple that fits every column or that reaches half of its output
/// buffer, 65536 columns. A screen that is already wider than that gives a
/// wider line.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "procps-ng keeps the width in an unsigned int"
)]
fn check_header_width(columns: &[Column], screen: Screen) -> (u32, bool) {
    let mut total: i64 = 0;
    let mut was_normal = 0;
    let mut signals = 0;

    for (index, column) in columns.iter().enumerate() {
        let width = int(column.width);
        let last = index + 1 == columns.len();

        match Alignment::of(column) {
            Alignment::Text => {
                total += width;
                was_normal = 0;
                continue;
            }
            Alignment::Specifier(Justification::Signal) => signals += 1,
            Alignment::Specifier(_) => {}
        }

        total += if Alignment::of(column) == Alignment::Specifier(Justification::Unlimited) && last
        {
            3
        } else {
            width
        };
        total += was_normal;
        was_normal = 1;
    }

    let columns = screen.columns;
    let mut multiple = 0;
    let active_cols = loop {
        multiple += 1;

        let active = columns * multiple;

        if active >= total || active >= int(OUTBUF_SIZE) / 2 {
            break active;
        }
    };

    (active_cols as u32, total + signals * 7 <= active_cols)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{Layout, Position, Settings, check_header_width};
    use crate::ps::{
        format::{Column, Content},
        options::{Header, Modifiers, Screen},
        print::{Cell, Room},
        specifier::{self, Justification},
    };

    /// The column of the specifier `name`, with its default header and width.
    fn column(name: &str) -> Column {
        let specifier = specifier::find(name.as_bytes()).expect("a specifier");

        Column {
            content: Content::Specifier(specifier),
            header: specifier.header.as_bytes().to_vec(),
            width: specifier.width(),
        }
    }

    /// The column of the specifier `name` with `header`.
    fn headed(name: &str, header: &str) -> Column {
        Column {
            header: header.as_bytes().to_vec(),
            ..column(name)
        }
    }

    /// A column that shows `text` on every line.
    fn text(text: &str) -> Column {
        Column {
            content: Content::Text,
            header: text.as_bytes().to_vec(),
            width: text.len(),
        }
    }

    fn settings(header: Header, columns: i64) -> Settings {
        Settings {
            header,
            screen: Screen { columns, rows: 24 },
            delimiter: None,
            modifiers: Modifiers::empty(),
        }
    }

    /// What `ps` writes for `rows`, each of which gives the text of each
    /// specifier by name, and whether it printed anything. The text of a
    /// column without a limit is cut to the room that the column has, as the
    /// printers cut it.
    fn laid_out(
        columns: &[Column],
        settings: Settings,
        rows: &[&[(&str, &str)]],
    ) -> (String, String, bool) {
        let mut layout = Layout::new(columns, settings);
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());

        for row in rows {
            layout.row(
                |specifier, room: Room| {
                    let value = row
                        .iter()
                        .find(|(name, _)| *name == specifier.name)
                        .map_or("", |(_, value)| *value)
                        .as_bytes();

                    if specifier.justification == Justification::Unlimited {
                        return Cell::text(&value[..value.len().min(room.cells)]);
                    }

                    Cell::text(value)
                },
                &mut stdout,
                &mut stderr,
            );
        }

        let printed = layout.finish(&mut stdout, &mut stderr);

        (
            String::from_utf8(stdout).expect("the output is UTF-8"),
            String::from_utf8(stderr).expect("the output is UTF-8"),
            printed,
        )
    }

    const SLEEP: &[(&str, &str)] = &[("pid", "10"), ("comm", "sleep"), ("args", "sleep 100")];
    const CAT: &[(&str, &str)] = &[("pid", "200"), ("comm", "cat"), ("args", "cat")];
    const LONG: &[(&str, &str)] = &[
        ("args", "a command line that is longer than its column"),
        ("pid", "1"),
    ];

    #[rstest]
    #[case::header_and_rows(
        vec![column("pid"), column("comm")],
        Header::Single,
        &[SLEEP, CAT],
        ("  PID COMMAND\n   10 sleep\n  200 cat\n", true)
    )]
    #[case::header_without_rows(
        vec![column("pid"), column("comm")],
        Header::Single,
        &[],
        ("  PID COMMAND\n", false)
    )]
    #[case::no_header(
        vec![column("pid"), column("comm")],
        Header::None,
        &[SLEEP],
        ("   10 sleep\n", true)
    )]
    #[case::nothing(vec![column("pid"), column("comm")], Header::None, &[], ("", false))]
    #[case::empty_headers(
        vec![headed("pid", ""), headed("comm", "")],
        Header::Single,
        &[SLEEP],
        ("   10 sleep\n", true)
    )]
    #[case::one_empty_header(
        vec![headed("pid", ""), column("comm")],
        Header::Single,
        &[SLEEP],
        ("      COMMAND\n   10 sleep\n", true)
    )]
    #[case::aix_text(
        vec![column("pid"), text(":"), column("comm")],
        Header::Single,
        &[SLEEP],
        ("  PID:COMMAND\n   10:sleep\n", true)
    )]
    #[case::left_column_not_last(
        vec![column("comm"), column("pid")],
        Header::Single,
        &[SLEEP],
        ("COMMAND           PID\nsleep              10\n", true)
    )]
    #[case::unlimited_not_last_is_cut(
        vec![headed("args", "A"), column("pid")],
        Header::Single,
        &[LONG],
        ("A                             PID\na command line that is long     1\n", true)
    )]
    fn columns_are_laid_out_as_procps_ng_lays_them_out(
        #[case] columns: Vec<Column>,
        #[case] header: Header,
        #[case] rows: &[&[(&str, &str)]],
        #[case] expected: (&str, bool),
    ) {
        let (stdout, stderr, printed) = laid_out(&columns, settings(header, 131_072), rows);

        assert_eq!(
            (stdout.as_str(), stderr.as_str(), printed),
            (expected.0, "", expected.1)
        );
    }

    #[test]
    fn the_header_repeats_at_the_top_of_each_screen() {
        let settings = Settings {
            screen: Screen {
                columns: 80,
                rows: 3,
            },
            ..settings(Header::Multiple, 80)
        };
        let (stdout, _, _) = laid_out(
            &[column("pid"), column("comm")],
            settings,
            &[SLEEP, CAT, SLEEP],
        );

        assert_eq!(
            stdout,
            "  PID COMMAND\n   10 sleep\n  200 cat\n  PID COMMAND\n   10 sleep\n"
        );
    }

    #[test]
    fn a_delimiter_replaces_the_padding() {
        let settings = Settings {
            delimiter: Some(b','),
            ..settings(Header::Single, 131_072)
        };
        let (stdout, _, _) = laid_out(&[column("pid"), column("comm")], settings, &[SLEEP]);

        assert_eq!(stdout, "PID,COMMAND\n10,sleep\n");
    }

    #[test]
    fn the_last_column_has_the_rest_of_the_line() {
        let (stdout, _, _) = laid_out(
            &[column("pid"), column("args")],
            settings(Header::Single, 12),
            &[SLEEP],
        );

        assert_eq!(stdout, "  PID COMMAND\n   10 sleep \n");
    }

    #[test]
    fn a_last_column_without_room_keeps_one_byte() {
        let pid = Column {
            width: 1,
            ..headed("pid", "PIDXX")
        };
        let (stdout, _, _) = laid_out(
            &[pid, column("comm")],
            settings(Header::Single, 6),
            &[&[("pid", "12"), ("comm", "sleep")]],
        );

        assert_eq!(stdout, "PIDXX C\n12 sle\n");
    }

    #[test]
    fn numeric_users_are_aligned_on_the_right() {
        let settings = Settings {
            modifiers: Modifiers::NUMERIC,
            ..settings(Header::Single, 131_072)
        };
        let (stdout, _, _) = laid_out(
            &[column("user"), column("pid")],
            settings,
            &[&[("user", "501"), ("pid", "10")]],
        );

        assert_eq!(stdout, "    USER   PID\n     501    10\n");
    }

    #[rstest]
    #[case::wide(131_072, "          CAUGHT COMMAND\n0000000000004001 sleep\n")]
    #[case::narrow(19, "   CAUGHT COMMAND\n 00004001 sleep\n")]
    fn signal_masks_take_16_cells_on_a_wide_line(#[case] columns: i64, #[case] expected: &str) {
        let wide = &[("caught", "0000000000004001"), ("comm", "sleep")];
        let narrow = &[("caught", "00004001"), ("comm", "sleep")];
        let row: &[(&str, &str)] = if columns > 100 { wide } else { narrow };
        let (stdout, _, _) = laid_out(
            &[column("caught"), column("comm")],
            settings(Header::Single, columns),
            &[row],
        );

        assert_eq!(stdout, expected);
    }

    #[test]
    fn a_line_wider_than_the_output_buffer_is_reported() {
        let (stdout, stderr, _) = laid_out(
            &[column("pid"), column("comm")],
            settings(Header::Single, 200_000),
            &[SLEEP],
        );

        assert_eq!(
            (stdout.as_str(), stderr.as_str()),
            (
                "  PID COMMAND\n   10 sleep\n",
                "fix bigness error\nfix bigness error\n"
            )
        );
    }

    #[rstest]
    #[case::before_the_last(false, Position { correct: 30, actual: 40, dospace: 1 }, (8, 0))]
    #[case::last(true, Position { correct: 9, actual: 9, dospace: 1 }, (10, 1))]
    #[case::last_behind(true, Position { correct: 12, actual: 9, dospace: 1 }, (8, 3))]
    #[case::last_past_the_end(true, Position { correct: 9, actual: 25, dospace: 1 }, (131_071, 1))]
    #[case::last_behind_past_the_end(
        true,
        Position { correct: 30, actual: 25, dospace: 1 },
        (131_071, 5)
    )]
    fn the_room_of_a_column_wraps_as_procps_ng_s_unsigned_arithmetic_does(
        #[case] last: bool,
        #[case] position: Position,
        #[case] expected: (u32, i64),
    ) {
        let columns = [column("pid"), column("comm")];
        let layout = Layout::new(&columns, settings(Header::Single, 20));

        assert_eq!(layout.room(8, last, position), expected);
    }

    #[test]
    fn the_room_of_the_last_column_is_limited_to_the_output_buffer() {
        let columns = [column("pid"), column("comm")];
        let layout = Layout::new(&columns, settings(Header::Single, 200_000));
        let position = Position {
            correct: 0,
            actual: 0,
            dospace: 0,
        };

        assert_eq!(layout.room(8, true, position), (131_071, 0));
    }

    #[rstest]
    // The text resets the separation, so the signal column adds only its
    // width: 5 + 1 + 9 + 1 + 3 = 19, and 19 + 7 fits in 26.
    #[case::text_before_a_signal_mask(
        vec![column("pid"), text(":"), column("caught"), column("comm")],
        26,
        (26, true)
    )]
    #[case::multiple_of_the_screen(vec![column("pid"), column("comm")], 4, (12, true))]
    #[case::wider_than_half_the_buffer(
        vec![Column { width: 70_000, ..column("pid") }, column("comm")],
        80,
        (65_600, false)
    )]
    fn the_line_is_a_multiple_of_the_screen_s_width(
        #[case] columns: Vec<Column>,
        #[case] screen: i64,
        #[case] expected: (u32, bool),
    ) {
        let screen = Screen {
            columns: screen,
            rows: 24,
        };

        assert_eq!(check_header_width(&columns, screen), expected);
    }

    #[test]
    fn a_header_of_text_alone_is_left_out() {
        let (stdout, _, _) = laid_out(
            &[headed("pid", ""), text(":"), headed("comm", "")],
            settings(Header::Single, 131_072),
            &[SLEEP],
        );

        assert_eq!(stdout, "   10:sleep\n");
    }

    #[test]
    fn a_wait_channel_header_at_the_end_of_the_line_keeps_one_byte() {
        let pid = Column {
            width: 1,
            ..headed("pid", "PIDXXXXXXXX")
        };
        let (stdout, _, _) = laid_out(
            &[pid, column("wchan")],
            settings(Header::Single, 6),
            &[&[("pid", "12"), ("wchan", "-")]],
        );

        assert_eq!(stdout, "PIDXXXXXXXX W\n12 -\n");
    }

    #[test]
    fn padding_is_limited_to_144_bytes() {
        let pid = Column {
            width: 200,
            ..column("pid")
        };
        let (stdout, _, _) = laid_out(
            &[pid, column("comm")],
            settings(Header::Single, 131_072),
            &[SLEEP],
        );
        let expected = format!(
            "{}PID{}COMMAND\n{}10{}sleep\n",
            " ".repeat(144),
            " ".repeat(54),
            " ".repeat(144),
            " ".repeat(55)
        );

        assert_eq!(stdout, expected);
    }
}
