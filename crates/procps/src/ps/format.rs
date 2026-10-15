// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2023 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2011-2012 Sami Kerola <kerolasa@iki.fi>
// SPDX-FileCopyrightText: 1998-2004 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/sortformat.c.

use std::{ffi::OsString, os::unix::ffi::OsStringExt};

use crate::{
    c_number::atoi,
    ps::{
        error::{InternalError, SyntaxError, Warning},
        options::{FormatFlags, FormatModifiers, ThreadFlags},
        personality::{self, Personality},
        specifier::{self, Specifier},
    },
};

/// What a column shows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Content {
    /// The value of a format specifier.
    Specifier(&'static Specifier),
    /// Its header on every row: text of an AIX format between descriptors,
    /// or the `:` of IRIX's `-l` format.
    Text,
}

/// A column of `ps`'s output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Column {
    /// What the column shows.
    pub(crate) content: Content,
    /// The header.
    pub(crate) header: Vec<u8>,
    /// The width.
    pub(crate) width: usize,
}

/// The order of a sort key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Order {
    /// Ascending.
    Ascending,
    /// Descending.
    Descending,
    /// Neither. A code of BSD's `O` after the first one, without its own
    /// `+` or `-`, has this order, and procps-ng's sort ignores the key.
    Ignored,
}

/// A key to sort processes by.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SortKey {
    /// The specifier whose value the key compares.
    pub(crate) specifier: &'static Specifier,
    /// The order.
    pub(crate) order: Order,
}

/// The option that a format or sort list came from (`SF_*`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Source {
    /// UNIX's `-O`, a format list after the `pid` column.
    UnixCapitalO,
    /// UNIX's `-o`.
    UnixO,
    /// BSD's `O`, a list of sort codes or a format list after `pid`.
    BsdCapitalO,
    /// BSD's `o`.
    BsdO,
    /// BSD's `m` with the `bsd` personality, which sorts by memory.
    BsdM,
    /// `--sort` and BSD's `k`.
    GnuSort,
    /// `--format`.
    GnuFormat,
}

/// A format or sort list, kept until every option has been read.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Deferred {
    /// The list as the option gave it.
    text: Vec<u8>,
    /// The option.
    source: Source,
    /// The columns that the list gives, in order.
    columns: Vec<Column>,
    /// The sort keys that the list gives, in the order of the list.
    keys: Vec<SortKey>,
}

/// The columns and sort keys that the options give.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Formats {
    /// The columns, in order.
    pub(crate) columns: Vec<Column>,
    /// The sort keys, in the order that procps-ng applies them.
    pub(crate) sort: Vec<SortKey>,
}

/// Why the format and sort lists are not valid.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum FormatError {
    /// The command line has an error.
    Syntax(SyntaxError),
    /// A check that procps-ng reports as an internal error failed.
    Internal(InternalError),
}

impl From<SyntaxError> for FormatError {
    fn from(error: SyntaxError) -> Self {
        Self::Syntax(error)
    }
}

impl From<InternalError> for FormatError {
    fn from(error: InternalError) -> Self {
        Self::Internal(error)
    }
}

/// What the other options decide about the formats.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Settings<'a> {
    /// The predefined formats.
    pub(crate) flags: FormatFlags,
    /// The changes to the predefined formats.
    pub(crate) modifiers: FormatModifiers,
    /// The thread display.
    pub(crate) threads: ThreadFlags,
    /// Whether a process tree is shown.
    pub(crate) forest: bool,
    /// The personality.
    pub(crate) personality: &'a Personality,
    /// Whether the default format is BSD's.
    pub(crate) prefer_bsd_defaults: bool,
    /// The value of `PS_FORMAT`.
    pub(crate) ps_format: Option<&'a [u8]>,
}

/// The state that procps-ng keeps for one parse of the command line, apart
/// from its message for an unknown specifier.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SortFormat {
    /// The format and sort lists, in the order of the command line.
    deferred: Vec<Deferred>,
    /// Whether `--sort` or `k` was given, so that `O` must be a format.
    have_gnu_sort: bool,
    /// Whether a sort list has been read.
    sorted: bool,
    /// Whether a format list has been read.
    formatted: bool,
}

/// What [`format_parse`] needs to report an unknown specifier.
struct Unknown<'a> {
    /// The first unknown specifier, which procps-ng's static buffer keeps
    /// for the rest of the run.
    first: &'a mut Option<OsString>,
}

impl Unknown<'_> {
    /// The error for the unknown specifier `name`.
    fn error(&mut self, name: &[u8]) -> SyntaxError {
        let first = self
            .first
            .get_or_insert_with(|| OsString::from_vec(name.to_vec()));

        SyntaxError::UnknownSpecifier(first.clone())
    }
}

/// The columns for `name`, with `header` as the header of each, as
/// procps-ng's `do_one_spec` finds them: a specifier, or a macro for several
/// specifiers.
fn one_spec(name: &[u8], header: Option<&[u8]>) -> Option<Vec<Column>> {
    if let Some(specifier) = specifier::find(name) {
        let width = specifier.width();
        let column = match header {
            Some(header) => Column {
                content: Content::Specifier(specifier),
                header: header.to_vec(),
                width: width.max(header.len()),
            },
            None => Column {
                content: Content::Specifier(specifier),
                header: specifier.header.as_bytes().to_vec(),
                width,
            },
        };

        return Some(vec![column]);
    }

    let specifiers = specifier::find_macro(name)?;

    Some(
        specifiers
            .split([',', ' '])
            .flat_map(|name| one_spec(name.as_bytes(), header).unwrap_or_default())
            .collect(),
    )
}

/// The single column for the specifier `name`, which is in procps-ng's
/// table.
fn column(name: &str) -> Vec<Column> {
    one_spec(name.as_bytes(), None).unwrap_or_default()
}

/// Whether `byte` separates the elements of a format or sort list.
const fn is_list_separator(byte: u8) -> bool {
    matches!(byte, b' ' | b',' | b'\t' | b'\n')
}

/// The number of elements in `list`, as procps-ng counts them, with one
/// trailing separator removed. Returns `None` if the list is empty or has an
/// empty element.
fn count_items(list: &[u8]) -> Option<(usize, &[u8])> {
    if list.is_empty() || list.first().copied().is_some_and(is_list_separator) {
        return None;
    }

    let mut items = 0;
    let mut need_item = true;

    for &byte in list {
        if is_list_separator(byte) {
            if need_item {
                return None;
            }

            need_item = true;
        } else {
            if need_item {
                items += 1;
            }

            need_item = false;
        }
    }

    let list = if need_item {
        &list[..list.len() - 1]
    } else {
        list
    };

    Some((items, list))
}

/// Whether `width` is a column width that procps-ng accepts: decimal digits
/// without a leading zero, with a positive value as `atoi` reads it.
fn valid_width(width: &[u8]) -> Option<usize> {
    let digits = !width.is_empty() && width.iter().all(u8::is_ascii_digit);

    if !digits || width[0] == b'0' {
        return None;
    }

    usize::try_from(atoi(width)).ok().filter(|&width| width > 0)
}

/// Reads the format list `list` into `columns`, as procps-ng's
/// `format_parse` does. An element is a specifier or a macro, with an
/// optional `:width` and an optional `=header`. When the list has an error
/// and contains a `%`, procps-ng reads it again as an AIX format, which may
/// succeed, unless the list is `%cpu` or starts with `%cpu `, `%cpu,` or
/// `%m`. The columns that the first reading gave stay in `columns`.
fn format_parse(
    list: &[u8],
    columns: &mut Vec<Column>,
    unknown: &mut Unknown<'_>,
) -> Result<(), SyntaxError> {
    let parsed = parse_elements(list, columns, unknown);

    let Err(error) = parsed else {
        return Ok(());
    };

    let aix = list.contains(&b'%')
        && list != b"%cpu"
        && !list.starts_with(b"%cpu ")
        && !list.starts_with(b"%cpu,")
        && !list.starts_with(b"%m");

    if aix {
        return aix_format_parse(list, columns);
    }

    Err(error)
}

/// Reads the elements of the format list `list` into `columns`, until the
/// first error.
fn parse_elements(
    list: &[u8],
    columns: &mut Vec<Column>,
    unknown: &mut Unknown<'_>,
) -> Result<(), SyntaxError> {
    let Some((mut items, list)) = count_items(list) else {
        return Err(SyntaxError::ImproperFormatList);
    };

    if items == 0 {
        return Err(SyntaxError::EmptyFormatList);
    }

    let mut rest = Some(list);

    while items > 0 {
        items -= 1;

        let Some(walk) = rest else {
            break;
        };
        let separator = walk.iter().position(|&byte| is_list_separator(byte));
        let element = separator.map_or(walk, |at| &walk[..at]);
        let (name, header) = match element.iter().position(|&byte| byte == b'=') {
            Some(at) => (&element[..at], Some(&element[at + 1..])),
            None => (element, None),
        };
        let (name, width) = match name.iter().position(|&byte| byte == b':') {
            Some(at) => {
                let width = valid_width(&name[at + 1..]).ok_or(SyntaxError::BadWidth)?;

                (&name[..at], Some(width))
            }
            None => (name, None),
        };

        let Some(mut found) = one_spec(name, header) else {
            return Err(unknown.error(name));
        };

        if let Some(width) = width {
            let [column] = found.as_mut_slice() else {
                return Err(SyntaxError::MacroWidth);
            };

            column.width = width;
        }

        columns.append(&mut found);
        rest = separator.map(|at| &walk[at + 1..]);
    }

    Ok(())
}

/// Reads an AIX format, such as `%p %z`, into `columns`, as procps-ng's
/// `aix_format_parse` does. Each `%` and the descriptor after it give a
/// column, and the text between descriptors gives a column of its own.
fn aix_format_parse(list: &[u8], columns: &mut Vec<Column>) -> Result<(), SyntaxError> {
    check_aix_format(list)?;

    let mut rest = list;

    loop {
        if let [b'%', after @ ..] = rest {
            let [descriptor, after @ ..] = after else {
                return Err(SyntaxError::MissingAixDescriptor);
            };

            if *descriptor == b'%' {
                return Err(SyntaxError::MissingAixDescriptor);
            }

            let (name, header) =
                specifier::aix_descriptor(*descriptor).ok_or(SyntaxError::UnknownAixDescriptor)?;

            columns.extend(one_spec(name.as_bytes(), Some(header.as_bytes())).unwrap_or_default());
            rest = after;
            continue;
        }

        let length = rest
            .iter()
            .position(|&byte| byte == b'%')
            .unwrap_or(rest.len());

        if length == 0 {
            return Ok(());
        }

        columns.push(Column {
            content: Content::Text,
            header: rest[..length].to_vec(),
            width: length,
        });
        rest = &rest[length..];
    }
}

/// Checks an AIX format as procps-ng's state machine does: it starts with
/// `%` or a space, and each `%` is followed by a descriptor.
fn check_aix_format(list: &[u8]) -> Result<(), SyntaxError> {
    enum State {
        Initial,
        Text,
        Descriptor,
    }

    let mut bytes = list.iter().copied().chain(std::iter::repeat(0));
    let mut byte = bytes.next().unwrap_or_default();

    if byte != b'%' && byte != b' ' {
        return Err(SyntaxError::ImproperAixDescriptor);
    }

    let mut state = State::Initial;

    loop {
        state = match state {
            State::Initial if byte == b'%' => State::Descriptor,
            State::Initial if byte == 0 => return Ok(()),
            State::Initial => State::Text,
            State::Text => {
                byte = bytes.next().unwrap_or_default();

                match byte {
                    b'%' => State::Descriptor,
                    0 => return Ok(()),
                    _ => State::Text,
                }
            }
            State::Descriptor => {
                byte = bytes.next().unwrap_or_default();

                if byte == 0 || byte == b' ' {
                    return Err(SyntaxError::MissingAixDescriptor);
                }

                State::Initial
            }
        };
    }
}

/// The sort key for `name`, with an optional leading `-` for descending
/// order or `+` for ascending order, as procps-ng's `do_one_sort_spec` reads
/// it.
fn one_sort_spec(name: &[u8]) -> Option<SortKey> {
    let (order, name) = match name {
        [b'-', rest @ ..] => (Order::Descending, rest),
        [b'+', rest @ ..] => (Order::Ascending, rest),
        _ => (Order::Ascending, name),
    };

    specifier::find(name).map(|specifier| SortKey { specifier, order })
}

/// Reads the sort list of `--sort` or `k`, as procps-ng's `long_sort_parse`
/// does.
fn long_sort_parse(list: &[u8]) -> Result<Vec<SortKey>, SyntaxError> {
    let (items, list) = count_items(list).ok_or(SyntaxError::ImproperSortList)?;

    if items == 0 {
        return Err(SyntaxError::EmptySortList);
    }

    list.split(|&byte| is_list_separator(byte))
        .map(|name| one_sort_spec(name).ok_or(SyntaxError::UnknownSortKey))
        .collect()
}

/// The codes that BSD's `O` accepts as a sort list.
const SORT_CODES: &[u8] = b"CGJKMNPRSTUcfgjkmnoprstuvy+-";

/// Checks that `list` is a list of BSD's sort codes, as procps-ng's
/// `verify_short_sort` does: known codes, each at most once, and a `+` or
/// `-` only before a code.
fn verify_short_sort(list: &[u8], forest: bool) -> Result<(), SyntaxError> {
    if !list.iter().all(|byte| SORT_CODES.contains(byte)) {
        return Err(SyntaxError::BadSortCode);
    }

    let mut seen = [false; 256];

    for (index, &code) in list.iter().enumerate() {
        match code {
            b'+' | b'-' => {
                if matches!(list.get(index + 1), None | Some(b'+' | b'-')) {
                    return Err(SyntaxError::BadSortCode);
                }
            }
            _ => {
                if code == b'P' && forest {
                    return Err(SyntaxError::ParentSortWithForest);
                }

                if seen[usize::from(code)] {
                    return Err(SyntaxError::BadSortCode);
                }

                seen[usize::from(code)] = true;
            }
        }
    }

    Ok(())
}

/// Reads a list of BSD's sort codes, as procps-ng's `short_sort_parse` does.
/// procps-ng ignores the error for a code without a specifier, and keeps the
/// keys before it.
fn short_sort_parse(list: &[u8]) -> Vec<SortKey> {
    let mut order = Order::Ascending;
    let mut keys = Vec::new();

    for &code in list {
        match code {
            b'+' => order = Order::Ascending,
            b'-' => order = Order::Descending,
            _ => {
                let Some(mut key) =
                    specifier::sort_code(code).and_then(|name| one_sort_spec(name.as_bytes()))
                else {
                    break;
                };

                key.order = order;
                keys.push(key);
                order = Order::Ignored;
            }
        }
    }

    keys
}

/// The columns of `-O` or `O`: `pid`, the user's columns, and then the
/// columns of the trailer macro, as procps-ng's `O_wrap` adds them.
fn o_wrap(columns: &mut Vec<Column>, trailer: &str) {
    let mut wrapped = column("pid");

    wrapped.append(columns);
    wrapped.extend(one_spec(trailer.as_bytes(), None).unwrap_or_default());
    *columns = wrapped;
}

/// Inserts `column` after the first column with the header `header`, as
/// procps-ng's `fmt_add_after` does. Returns whether a column has the
/// header.
fn add_after(columns: &mut Vec<Column>, header: &str, column: Vec<Column>) -> bool {
    let Some(at) = columns
        .iter()
        .position(|existing| existing.header == header.as_bytes())
    else {
        return false;
    };

    let after = columns.split_off(at + 1);

    columns.extend(column);
    columns.extend(after);
    true
}

/// Removes the first column with the header `header`, as procps-ng's
/// `fmt_delete` does.
fn delete(columns: &mut Vec<Column>, header: &str) {
    if let Some(at) = columns
        .iter()
        .position(|existing| existing.header == header.as_bytes())
    {
        columns.remove(at);
    }
}

impl SortFormat {
    /// Keeps a format or sort list until every option has been read, as
    /// procps-ng's `defer_sf_option` does.
    pub(crate) fn defer(&mut self, text: &[u8], source: Source) {
        self.deferred.push(Deferred {
            text: text.to_vec(),
            source,
            columns: Vec::new(),
            keys: Vec::new(),
        });

        if source == Source::GnuSort {
            self.have_gnu_sort = true;
        }
    }

    /// Reads each kept list, as procps-ng's `parse_O_option` does, in the
    /// order of the command line.
    fn parse_lists(
        &mut self,
        settings: &Settings<'_>,
        unknown: &mut Unknown<'_>,
    ) -> Result<(), SyntaxError> {
        for index in 0..self.deferred.len() {
            let deferred = &mut self.deferred[index];

            match deferred.source {
                Source::BsdO | Source::GnuFormat | Source::UnixO => {
                    format_parse(&deferred.text, &mut deferred.columns, unknown)?;
                    self.formatted = true;
                }
                Source::UnixCapitalO => {
                    if self.formatted {
                        return Err(SyntaxError::LateCapitalO);
                    }

                    format_parse(&deferred.text, &mut deferred.columns, unknown)?;
                    self.formatted = true;
                    o_wrap(&mut deferred.columns, "END_SYS5");
                }
                Source::BsdCapitalO => {
                    let sort = if self.have_gnu_sort || self.sorted {
                        Err(SyntaxError::MultipleSorts)
                    } else {
                        verify_short_sort(&deferred.text, settings.forest)
                    };

                    let Err(error) = sort else {
                        deferred.keys = short_sort_parse(&deferred.text);
                        self.sorted = true;
                        continue;
                    };

                    if self.formatted {
                        return Err(SyntaxError::LateO);
                    }

                    if format_parse(&deferred.text, &mut deferred.columns, unknown).is_err() {
                        return Err(error);
                    }

                    self.formatted = true;
                    o_wrap(&mut deferred.columns, "END_BSD");
                }
                Source::GnuSort | Source::BsdM => {
                    if self.sorted {
                        return Err(SyntaxError::MultipleSorts);
                    }

                    self.sorted = true;
                    deferred.keys = long_sort_parse(&deferred.text)?;
                }
            }
        }

        Ok(())
    }

    /// Reads the kept lists and decides the columns and sort keys, as
    /// procps-ng's `process_sf_options` does. `first_unknown` is the first
    /// unknown specifier of the run, and `warnings` receives the warning for
    /// a `PS_FORMAT` that procps-ng ignores.
    pub(crate) fn process(
        &mut self,
        settings: &Settings<'_>,
        first_unknown: &mut Option<OsString>,
        warnings: &mut Vec<Warning>,
    ) -> Result<Formats, FormatError> {
        let mut unknown = Unknown {
            first: first_unknown,
        };

        self.parse_lists(settings, &mut unknown)?;

        let mut columns: Vec<Column> = self
            .deferred
            .iter()
            .flat_map(|deferred| deferred.columns.iter().cloned())
            .collect();
        let sort = self
            .deferred
            .iter()
            .flat_map(|deferred| deferred.keys.iter().rev().copied())
            .collect();

        if settings.flags.is_empty()
            && settings.modifiers.is_empty()
            && columns.is_empty()
            && let Some(ps_format) = settings.ps_format.filter(|format| !format.is_empty())
        {
            if settings.threads.contains(ThreadFlags::MUST_USE) {
                return Err(SyntaxError::ThreadsWithPsFormat.into());
            }

            let mut from_environment = Vec::new();

            match format_parse(ps_format, &mut from_environment, &mut unknown) {
                Ok(()) => {
                    return Ok(Formats {
                        columns: from_environment,
                        sort,
                    });
                }
                Err(error) => warnings.push(Warning::PsFormatIgnored(error)),
            }
        }

        if !columns.is_empty() {
            if !settings.flags.is_empty() {
                return Err(SyntaxError::ConflictingFormats.into());
            }

            if !settings.modifiers.is_empty() {
                return Err(SyntaxError::ModifierWithFormatList.into());
            }

            if settings.threads.contains(ThreadFlags::MUST_USE) {
                return Err(SyntaxError::ThreadsWithFormatList.into());
            }

            return Ok(Formats { columns, sort });
        }

        let Some(name) = predefined(settings)? else {
            return Ok(Formats {
                columns: generate_sysv_list(settings)?,
                sort,
            });
        };

        columns = one_spec(name.as_bytes(), None).unwrap_or_default();
        modify(&mut columns, settings)?;

        Ok(Formats { columns, sort })
    }
}

/// The macro for the predefined format that the options select, or `None`
/// for the format that procps-ng's `generate_sysv_list` builds.
fn predefined(settings: &Settings<'_>) -> Result<Option<&'static str>, SyntaxError> {
    let flags = settings.flags;
    let personality = settings.personality;

    if flags.is_empty() {
        return Ok(None);
    }

    let name = if flags == FormatFlags::U_F | FormatFlags::U_L {
        personality.unix.fl
    } else if flags == FormatFlags::U_F {
        personality.unix.f
    } else if flags == FormatFlags::U_L {
        personality.unix.l
    } else if flags == FormatFlags::U_J {
        personality.unix.j
    } else if flags == FormatFlags::U_J | FormatFlags::U_L {
        Some("RD_lj")
    } else if flags == FormatFlags::U_J | FormatFlags::U_F {
        Some("RD_fj")
    } else if flags == FormatFlags::B_J {
        Some(personality.bsd.j)
    } else if flags == FormatFlags::B_L {
        Some(personality.bsd.l)
    } else if flags == FormatFlags::B_S {
        Some(personality.bsd.s)
    } else if flags == FormatFlags::B_U {
        Some(personality.bsd.u)
    } else if flags == FormatFlags::B_V {
        Some(personality.bsd.v)
    } else if flags == FormatFlags::L_X {
        Some("OL_X")
    } else if flags == FormatFlags::L_M {
        Some("OL_m")
    } else if flags == FormatFlags::F_C {
        Some("FLASK_context")
    } else {
        return Err(SyntaxError::ConflictingFormats);
    };

    Ok(name)
}

/// Builds UNIX's default, `-f`, `-F`, `-l` and `-j` formats, as procps-ng's
/// `generate_sysv_list` does.
fn generate_sysv_list(settings: &Settings<'_>) -> Result<Vec<Column>, SyntaxError> {
    let flags = settings.flags;
    let modifiers = settings.modifiers;
    let threads = settings.threads;
    let personality = settings.personality.flags;

    let bsd = settings.prefer_bsd_defaults;
    let full = flags.contains(FormatFlags::U_F);
    let long = flags.contains(FormatFlags::U_L);
    let extra_full = modifiers.contains(FormatModifiers::F);
    let processor = modifiers.contains(FormatModifiers::P);
    let y = modifiers.contains(FormatModifiers::Y);
    let class = modifiers.contains(FormatModifiers::C);
    let lwp = threads.contains(ThreadFlags::U_L);
    let irix = personality.contains(personality::Flags::IRIX_L);
    let zap_address = personality.contains(personality::Flags::ZAP_ADDR);
    let sane_user = personality.contains(personality::Flags::SANE_USER);

    if y && !long {
        return Err(SyntaxError::YWithoutL);
    }

    // procps-ng adds each column whose condition is true, from the last column
    // to the first, in this order. `:` is IRIX's separator between the `SZ`
    // and `RSS` columns.
    let columns = [
        (bsd && flags.is_empty(), "args"),
        (bsd && !flags.is_empty(), "cmd"),
        (bsd, "bsdtime"),
        (bsd && !long, "stat"),
        (!bsd && full, "cmd"),
        (!bsd && !full, "ucmd"),
        (!bsd, "time"),
        (true, "tname"),
        (full, "stime"),
        (extra_full && !processor, "psr"),
        (extra_full && !(long && y), "rss"),
        (long, "wchan"),
        (long && !y && irix, "sgi_rss"),
        (long && !y && irix, ":"),
        (extra_full || long, "sz"),
        (long && y, "rss"),
        (long && !y && (zap_address || irix), "sgi_p"),
        (long && !y && !(zap_address || irix), "addr_1"),
        (class, "pri"),
        (class, "class"),
        (!class && long, "ni"),
        (!class && long && irix, "priority"),
        (!class && long && !irix, "opri"),
        (lwp && full, "nlwp"),
        ((full || long) && !class, "c"),
        (processor, "psr"),
        (lwp, "lwp"),
        (modifiers.contains(FormatModifiers::J), "sid"),
        (modifiers.contains(FormatModifiers::J), "pgid"),
        (full || long, "ppid"),
        (threads.contains(ThreadFlags::U_T), "spid"),
        (true, "pid"),
        (full && sane_user, "user"),
        (full && !sane_user, "uid_hack"),
        (!full && long, "uid"),
        (long, "s"),
        (long && !y, "f"),
        (modifiers.contains(FormatModifiers::M), "label"),
    ];

    Ok(columns
        .iter()
        .rev()
        .filter(|(condition, _)| *condition)
        .flat_map(|&(_, name)| match name {
            ":" => vec![Column {
                content: Content::Text,
                header: b":".to_vec(),
                width: 1,
            }],
            _ => column(name),
        })
        .collect())
}

/// Changes a predefined format for the modifiers, the thread display and the
/// personality, as the end of procps-ng's `process_sf_options` does.
fn modify(columns: &mut Vec<Column>, settings: &Settings<'_>) -> Result<(), FormatError> {
    let modifiers = settings.modifiers;
    let threads = settings.threads;
    let must_use = threads.contains(ThreadFlags::MUST_USE);

    if modifiers.contains(FormatModifiers::J) {
        if !add_after(columns, "PPID", column("pgid")) && !add_after(columns, "PID", column("pgid"))
        {
            return Err(InternalError::NoPidForJ.into());
        }

        if !add_after(columns, "PGID", column("sid")) {
            return Err(SyntaxError::LostPgid.into());
        }
    }

    if modifiers.contains(FormatModifiers::Y) {
        delete(columns, "F");

        if add_after(columns, "ADDR", column("rss")) {
            delete(columns, "ADDR");
        }
    }

    if modifiers.contains(FormatModifiers::C) {
        for header in ["%CPU", "CPU", "CP", "C", "NI"] {
            delete(columns, header);
        }

        if !add_after(columns, "PRI", column("class")) {
            return Err(InternalError::NoPriForC.into());
        }

        delete(columns, "PRI");

        if !add_after(columns, "CLS", column("pri")) {
            return Err(SyntaxError::LostCls.into());
        }
    }

    if threads.contains(ThreadFlags::U_T) && !add_after(columns, "PID", column("spid")) && must_use
    {
        return Err(SyntaxError::SpidWithoutPid.into());
    }

    if threads.contains(ThreadFlags::U_L) {
        let added = ["SID", "SESS", "PGID", "PGRP", "PPID", "PID"]
            .iter()
            .any(|header| add_after(columns, header, column("lwp")));

        if !added && must_use {
            return Err(SyntaxError::LwpWithoutPid.into());
        }

        add_after(columns, "%CPU", column("nlwp"));
    }

    if modifiers.contains(FormatModifiers::M) {
        columns.splice(0..0, column("label"));
    }

    let personality = settings.personality.flags;

    if personality.contains(personality::Flags::ZAP_ADDR)
        && settings.flags.contains(FormatFlags::U_L)
        && add_after(columns, "ADDR", column("sgi_p"))
    {
        delete(columns, "ADDR");
    }

    if personality.contains(personality::Flags::SANE_USER)
        && settings.flags.contains(FormatFlags::U_F)
        && add_after(columns, "UID", column("user"))
    {
        delete(columns, "UID");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{
        Column, Content, FormatError, Formats, Order, Settings, SortFormat, SortKey, Source,
        aix_format_parse, column, modify, verify_short_sort,
    };
    use crate::ps::{
        error::{InternalError, SyntaxError, Warning},
        options::{FormatFlags, FormatModifiers, ThreadFlags},
        personality::Personality,
        specifier,
        system::testing::FakeSystem,
    };

    fn personality() -> Personality {
        Personality::from_environment(&FakeSystem::default())
    }

    fn settings(personality: &Personality) -> Settings<'_> {
        Settings {
            flags: FormatFlags::empty(),
            modifiers: FormatModifiers::empty(),
            threads: ThreadFlags::empty(),
            forest: false,
            personality,
            prefer_bsd_defaults: false,
            ps_format: None,
        }
    }

    /// The column of the specifier `name`, with `header` and `width`.
    fn with(name: &str, header: &str, width: usize) -> Column {
        Column {
            content: Content::Specifier(specifier::find(name.as_bytes()).expect("a specifier")),
            header: header.as_bytes().to_vec(),
            width,
        }
    }

    /// The columns of the specifiers `names`, with their default headers and
    /// widths.
    fn named(names: &[&str]) -> Vec<Column> {
        names.iter().flat_map(|name| column(name)).collect()
    }

    /// A column that shows `text` on every row.
    fn text(text: &str) -> Column {
        Column {
            content: Content::Text,
            header: text.as_bytes().to_vec(),
            width: text.len(),
        }
    }

    fn key(name: &str, order: Order) -> SortKey {
        SortKey {
            specifier: specifier::find(name.as_bytes()).expect("a specifier"),
            order,
        }
    }

    const UNIX_DEFAULT: [&str; 4] = ["pid", "tname", "time", "ucmd"];

    fn processed(
        lists: &[(&str, Source)],
        settings: &Settings<'_>,
    ) -> Result<Formats, FormatError> {
        let mut sort_format = SortFormat::default();

        for (list, source) in lists {
            sort_format.defer(list.as_bytes(), *source);
        }

        sort_format.process(settings, &mut None, &mut Vec::new())
    }

    fn columns_only(columns: Vec<Column>) -> Formats {
        Formats {
            columns,
            sort: Vec::new(),
        }
    }

    #[rstest]
    #[case::list("pid,comm", named(&["pid", "comm"]))]
    #[case::header("pid=Process,tty=T", vec![with("pid", "Process", 7), with("tty", "T", 8)])]
    #[case::empty_header("pid=", vec![with("pid", "", 5)])]
    #[case::width("comm:3", vec![with("comm", "COMMAND", 3)])]
    #[case::width_and_header("comm:3=X", vec![with("comm", "X", 3)])]
    #[case::macro_columns("DefSysV", named(&["pid", "tname", "time", "cmd"]))]
    #[case::macro_header(
        "DefSysV=Z",
        vec![with("pid", "Z", 5), with("tname", "Z", 8), with("time", "Z", 8), with("cmd", "Z", 27)]
    )]
    #[case::trailing_separator("pid,", named(&["pid"]))]
    #[case::blanks("pid comm\tppid\nsid", named(&["pid", "comm", "ppid", "sid"]))]
    #[case::aix("%p %z", vec![with("pid", "PID", 5), text(" "), with("vsz", "VSZ", 6)])]
    #[case::aix_header("%y", vec![with("tty", "TTY", 8)])]
    #[case::aix_after_c("%cpublah", vec![with("comm", "COMMAND", 15), text("publah")])]
    #[case::leading_space(" %p", vec![text(" "), with("pid", "PID", 5)])]
    #[case::aix_after_a_column(
        "%cpu\t%p",
        vec![with("%cpu", "%CPU", 4), with("comm", "COMMAND", 15), text("pu\t"), with("pid", "PID", 5)]
    )]
    fn a_format_list_gives_columns(#[case] list: &str, #[case] expected: Vec<Column>) {
        let personality = personality();

        assert_eq!(
            processed(&[(list, Source::UnixO)], &settings(&personality)),
            Ok(columns_only(expected))
        );
    }

    #[rstest]
    #[case::unknown("foo", SyntaxError::UnknownSpecifier(OsString::from("foo")))]
    #[case::leading_comma(",pid", SyntaxError::ImproperFormatList)]
    #[case::double_comma("pid,,comm", SyntaxError::ImproperFormatList)]
    #[case::zero_width("pid:0", SyntaxError::BadWidth)]
    #[case::leading_zero("pid:05", SyntaxError::BadWidth)]
    #[case::empty_width("pid:", SyntaxError::BadWidth)]
    #[case::overflowing_width("pid:4294967296", SyntaxError::BadWidth)]
    #[case::macro_width("DefBSD:5", SyntaxError::MacroWidth)]
    #[case::aix_missing("%", SyntaxError::MissingAixDescriptor)]
    #[case::aix_double("%%p", SyntaxError::MissingAixDescriptor)]
    #[case::aix_space("% p", SyntaxError::MissingAixDescriptor)]
    #[case::aix_improper("a%", SyntaxError::ImproperAixDescriptor)]
    #[case::aix_unknown("%q", SyntaxError::UnknownAixDescriptor)]
    #[case::cpu_and_comma_not_aix("%cpu,%p", SyntaxError::UnknownSpecifier(OsString::from("%p")))]
    #[case::cpu_and_space_not_aix("%cpu %p", SyntaxError::UnknownSpecifier(OsString::from("%p")))]
    #[case::mem_not_aix("%mem %p", SyntaxError::UnknownSpecifier(OsString::from("%p")))]
    fn a_bad_format_list_is_an_error(#[case] list: &str, #[case] expected: SyntaxError) {
        let personality = personality();

        assert_eq!(
            processed(&[(list, Source::UnixO)], &settings(&personality)),
            Err(FormatError::Syntax(expected))
        );
    }

    #[test]
    fn the_first_unknown_specifier_is_reported_for_every_later_one() {
        let personality = personality();
        let mut first = Some(OsString::from("first"));
        let mut sort_format = SortFormat::default();

        sort_format.defer(b"second", Source::UnixO);

        assert_eq!(
            sort_format.process(&settings(&personality), &mut first, &mut Vec::new()),
            Err(FormatError::Syntax(SyntaxError::UnknownSpecifier(
                OsString::from("first")
            )))
        );
    }

    #[rstest]
    #[case::capital_o(
        &[("comm", Source::UnixCapitalO)],
        Ok(columns_only(named(&["pid", "comm", "state", "tname", "time", "command"])))
    )]
    #[case::capital_o_then_o(
        &[("pid", Source::UnixCapitalO), ("comm", Source::UnixO)],
        Ok(columns_only(named(&["pid", "pid", "state", "tname", "time", "command", "comm"])))
    )]
    #[case::o_then_capital_o(
        &[("comm", Source::UnixO), ("pid", Source::UnixCapitalO)],
        Err(SyntaxError::LateCapitalO)
    )]
    #[case::bsd_format(
        &[("comm", Source::BsdCapitalO)],
        Ok(columns_only(named(&["pid", "comm", "state", "tname", "cputime", "comm"])))
    )]
    #[case::bsd_neither(&[("pid", Source::BsdO), ("zz", Source::BsdCapitalO)], Err(SyntaxError::LateO))]
    #[case::bsd_repeated_code(&[("pp", Source::BsdCapitalO)], Err(SyntaxError::BadSortCode))]
    #[case::sort_twice(
        &[("pid", Source::GnuSort), ("p", Source::BsdCapitalO)],
        Err(SyntaxError::MultipleSorts)
    )]
    #[case::sorts_twice(
        &[("pid", Source::GnuSort), ("ppid", Source::GnuSort)],
        Err(SyntaxError::MultipleSorts)
    )]
    #[case::long_sort(
        &[("-ppid,+pid", Source::GnuSort)],
        Ok(Formats {
            columns: named(&UNIX_DEFAULT),
            sort: vec![key("pid", Order::Ascending), key("ppid", Order::Descending)],
        })
    )]
    #[case::signed_codes(
        &[("p-u", Source::BsdCapitalO)],
        Ok(Formats {
            columns: named(&UNIX_DEFAULT),
            sort: vec![key("user", Order::Descending), key("pid", Order::Ascending)],
        })
    )]
    #[case::unsigned_codes(
        &[("pu", Source::BsdCapitalO)],
        Ok(Formats {
            columns: named(&UNIX_DEFAULT),
            sort: vec![key("user", Order::Ignored), key("pid", Order::Ascending)],
        })
    )]
    #[case::code_without_specifier(
        &[("pKu", Source::BsdCapitalO)],
        Ok(Formats {
            columns: named(&UNIX_DEFAULT),
            sort: vec![key("pid", Order::Ascending)],
        })
    )]
    #[case::format_and_sort(
        &[("pid", Source::UnixO), ("ppid", Source::GnuSort)],
        Ok(Formats {
            columns: named(&["pid"]),
            sort: vec![key("ppid", Order::Ascending)],
        })
    )]
    fn lists_give_columns_and_sort_keys(
        #[case] lists: &[(&str, Source)],
        #[case] expected: Result<Formats, SyntaxError>,
    ) {
        let personality = personality();

        assert_eq!(
            processed(lists, &settings(&personality)),
            expected.map_err(FormatError::Syntax)
        );
    }

    #[rstest]
    #[case::bad_code("pZ", false, Err(SyntaxError::BadSortCode))]
    #[case::sign_at_end("p+", false, Err(SyntaxError::BadSortCode))]
    #[case::two_signs("+-p", false, Err(SyntaxError::BadSortCode))]
    #[case::parent_with_forest("P", true, Err(SyntaxError::ParentSortWithForest))]
    #[case::parent("P", false, Ok(()))]
    #[case::signs("-p+u", false, Ok(()))]
    fn short_sort_codes_are_checked(
        #[case] codes: &str,
        #[case] forest: bool,
        #[case] expected: Result<(), SyntaxError>,
    ) {
        assert_eq!(verify_short_sort(codes.as_bytes(), forest), expected);
    }

    #[rstest]
    #[case::default(FormatFlags::empty(), FormatModifiers::empty(), ThreadFlags::empty(), false, Ok(named(&UNIX_DEFAULT)))]
    #[case::bsd_default(FormatFlags::empty(), FormatModifiers::empty(), ThreadFlags::empty(), true, Ok(named(&["pid", "tname", "stat", "bsdtime", "args"])))]
    #[case::full(FormatFlags::U_F, FormatModifiers::empty(), ThreadFlags::empty(), false, Ok(named(&["uid_hack", "pid", "ppid", "c", "stime", "tname", "time", "cmd"])))]
    #[case::long(FormatFlags::U_L, FormatModifiers::empty(), ThreadFlags::empty(), false, Ok(named(&["f", "s", "uid", "pid", "ppid", "c", "opri", "ni", "addr_1", "sz", "wchan", "tname", "time", "ucmd"])))]
    #[case::long_y(FormatFlags::U_L, FormatModifiers::Y, ThreadFlags::empty(), false, Ok(named(&["s", "uid", "pid", "ppid", "c", "opri", "ni", "rss", "sz", "wchan", "tname", "time", "ucmd"])))]
    #[case::extra_full(FormatFlags::U_F, FormatModifiers::F, ThreadFlags::empty(), false, Ok(named(&["uid_hack", "pid", "ppid", "c", "sz", "rss", "psr", "stime", "tname", "time", "cmd"])))]
    #[case::jobs(FormatFlags::empty(), FormatModifiers::J, ThreadFlags::empty(), false, Ok(named(&["pid", "pgid", "sid", "tname", "time", "ucmd"])))]
    #[case::threads_full(FormatFlags::U_F, FormatModifiers::empty(), ThreadFlags::U_L, false, Ok(named(&["uid_hack", "pid", "ppid", "lwp", "c", "nlwp", "stime", "tname", "time", "cmd"])))]
    #[case::scheduling(FormatFlags::U_L, FormatModifiers::C, ThreadFlags::empty(), false, Ok(named(&["f", "s", "uid", "pid", "ppid", "class", "pri", "addr_1", "sz", "wchan", "tname", "time", "ucmd"])))]
    #[case::label(FormatFlags::empty(), FormatModifiers::M, ThreadFlags::empty(), false, Ok(named(&["label", "pid", "tname", "time", "ucmd"])))]
    #[case::y_without_l(
        FormatFlags::empty(),
        FormatModifiers::Y,
        ThreadFlags::empty(),
        false,
        Err(SyntaxError::YWithoutL)
    )]
    fn unix_formats_are_built_as_procps_ng_builds_them(
        #[case] flags: FormatFlags,
        #[case] modifiers: FormatModifiers,
        #[case] threads: ThreadFlags,
        #[case] prefer_bsd_defaults: bool,
        #[case] expected: Result<Vec<Column>, SyntaxError>,
    ) {
        let personality = personality();
        let settings = Settings {
            flags,
            modifiers,
            threads,
            prefer_bsd_defaults,
            ..settings(&personality)
        };

        assert_eq!(
            processed(&[], &settings),
            expected.map(columns_only).map_err(FormatError::Syntax)
        );
    }

    const OLD_LINUX_USER: [&str; 11] = [
        "user",
        "pid",
        "pcpu",
        "pmem",
        "vsz",
        "rss",
        "tname",
        "stat",
        "start_time",
        "bsdtime",
        "args",
    ];

    #[rstest]
    #[case::user(FormatFlags::B_U, FormatModifiers::empty(), ThreadFlags::empty(), Ok(named(&OLD_LINUX_USER)))]
    #[case::jobs(FormatFlags::B_J, FormatModifiers::empty(), ThreadFlags::empty(), Ok(named(&["ppid", "pid", "pgid", "sid", "tname", "tpgid", "stat", "uid", "bsdtime", "args"])))]
    #[case::long_with_c(FormatFlags::B_L, FormatModifiers::C, ThreadFlags::empty(), Ok(named(&["flags", "uid", "pid", "ppid", "class", "pri", "vsz", "rss", "wchan", "stat", "tname", "bsdtime", "args"])))]
    #[case::long_with_y(FormatFlags::B_L, FormatModifiers::Y, ThreadFlags::empty(), Ok(named(&["uid", "pid", "ppid", "priority", "nice", "vsz", "rss", "wchan", "stat", "tname", "bsdtime", "args"])))]
    #[case::jobs_with_label(FormatFlags::B_J, FormatModifiers::M, ThreadFlags::empty(), Ok(named(&["label", "ppid", "pid", "pgid", "sid", "tname", "tpgid", "stat", "uid", "bsdtime", "args"])))]
    #[case::user_with_threads(
        FormatFlags::B_U,
        FormatModifiers::empty(),
        ThreadFlags::U_L | ThreadFlags::U_M | ThreadFlags::MUST_USE,
        Ok(named(&["user", "pid", "lwp", "pcpu", "nlwp", "pmem", "vsz", "rss", "tname", "stat", "start_time", "bsdtime", "args"]))
    )]
    #[case::two_formats(FormatFlags::B_U | FormatFlags::B_V, FormatModifiers::empty(), ThreadFlags::empty(), Err(FormatError::Syntax(SyntaxError::ConflictingFormats)))]
    #[case::user_with_c(
        FormatFlags::B_U,
        FormatModifiers::C,
        ThreadFlags::empty(),
        Err(FormatError::Internal(InternalError::NoPriForC))
    )]
    fn bsd_formats_come_from_macros(
        #[case] flags: FormatFlags,
        #[case] modifiers: FormatModifiers,
        #[case] threads: ThreadFlags,
        #[case] expected: Result<Vec<Column>, FormatError>,
    ) {
        let personality = personality();
        let settings = Settings {
            flags,
            modifiers,
            threads,
            ..settings(&personality)
        };

        assert_eq!(processed(&[], &settings), expected.map(columns_only));
    }

    #[rstest]
    #[case::flags(
        FormatFlags::U_F,
        FormatModifiers::empty(),
        ThreadFlags::empty(),
        SyntaxError::ConflictingFormats
    )]
    #[case::modifiers(
        FormatFlags::empty(),
        FormatModifiers::C,
        ThreadFlags::empty(),
        SyntaxError::ModifierWithFormatList
    )]
    #[case::threads(
        FormatFlags::empty(),
        FormatModifiers::empty(),
        ThreadFlags::MUST_USE,
        SyntaxError::ThreadsWithFormatList
    )]
    fn a_format_list_cannot_be_combined_with_predefined_formats(
        #[case] flags: FormatFlags,
        #[case] modifiers: FormatModifiers,
        #[case] threads: ThreadFlags,
        #[case] expected: SyntaxError,
    ) {
        let personality = personality();
        let settings = Settings {
            flags,
            modifiers,
            threads,
            ..settings(&personality)
        };

        assert_eq!(
            processed(&[("pid", Source::UnixO)], &settings),
            Err(FormatError::Syntax(expected))
        );
    }

    #[rstest]
    #[case::used(Some(&b"pid,ppid"[..]), named(&["pid", "ppid"]), vec![])]
    #[case::ignored(
        Some(&b"bar"[..]),
        named(&UNIX_DEFAULT),
        vec![Warning::PsFormatIgnored(SyntaxError::UnknownSpecifier(OsString::from("bar")))]
    )]
    #[case::improper(
        Some(&b","[..]),
        named(&UNIX_DEFAULT),
        vec![Warning::PsFormatIgnored(SyntaxError::ImproperFormatList)]
    )]
    #[case::empty(Some(&b""[..]), named(&UNIX_DEFAULT), vec![])]
    fn ps_format_replaces_the_default(
        #[case] ps_format: Option<&[u8]>,
        #[case] columns: Vec<Column>,
        #[case] warnings: Vec<Warning>,
    ) {
        let personality = personality();
        let settings = Settings {
            ps_format,
            ..settings(&personality)
        };
        let mut reported = Vec::new();
        let result = SortFormat::default().process(&settings, &mut None, &mut reported);

        assert_eq!((result, reported), (Ok(columns_only(columns)), warnings));
    }

    #[test]
    fn ps_format_with_a_thread_display_that_needs_a_column_is_an_error() {
        let personality = personality();
        let settings = Settings {
            ps_format: Some(b"pid"),
            threads: ThreadFlags::MUST_USE,
            ..settings(&personality)
        };

        assert_eq!(
            SortFormat::default().process(&settings, &mut None, &mut Vec::new()),
            Err(FormatError::Syntax(SyntaxError::ThreadsWithPsFormat))
        );
    }

    #[rstest]
    #[case::spid(ThreadFlags::U_T | ThreadFlags::MUST_USE, SyntaxError::SpidWithoutPid)]
    #[case::lwp(ThreadFlags::U_L | ThreadFlags::MUST_USE, SyntaxError::LwpWithoutPid)]
    fn a_thread_column_needs_a_column_to_follow(
        #[case] threads: ThreadFlags,
        #[case] expected: SyntaxError,
    ) {
        let personality = personality();
        let settings = Settings {
            threads,
            ..settings(&personality)
        };
        let mut columns = column("comm");

        assert_eq!(
            modify(&mut columns, &settings),
            Err(FormatError::Syntax(expected))
        );
    }

    #[test]
    fn aix_text_is_a_column_of_its_own() {
        let mut columns = Vec::new();

        assert_eq!(
            aix_format_parse(b"%p:", &mut columns).map(|()| columns),
            Ok(vec![with("pid", "PID", 5), text(":")])
        );
    }
}
