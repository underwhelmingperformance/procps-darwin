// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2024 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2012-2014 Jaromir Capik <jcapik@redhat.com>
// SPDX-FileCopyrightText: 2011 Lukas Nykryn <lnykryn@redhat.com>
// SPDX-FileCopyrightText: 1998-2004 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/display.c and src/ps/output.c.

use darwin_proc::Pid;

use crate::ps::{
    format::{Order, SortKey},
    options::Forest,
    specifier,
};

/// The size of procps-ng's buffer for the prefix of a row in a process tree
/// (`sizeof(forest_prefix)`). The buffer ends with a NUL, so a prefix has at
/// most `LEVELS - 1` levels, and a deeper process has the level of its
/// parent.
const LEVELS: usize = 4 * 32 * 1024 + 100;

/// One level of the prefix of a row in a process tree, as procps-ng's
/// `forest_prefix` encodes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Branch {
    /// The process is its parent's last child (`L`).
    Last,
    /// The process has a later sibling (`+`).
    Middle,
    /// An ancestor at this level has a later sibling (`|`).
    Through,
    /// An ancestor at this level is its parent's last child (` `).
    Blank,
}

/// The sort keys that procps-ng's `prep_forest_sort` gives a process tree:
/// `keys`, or the start time if `keys` is empty, and then the parent's pid.
/// procps-ng applies the keys in order, so the parent's pid is the most
/// significant, and the children of each process are next to each other.
pub(crate) fn sort_keys(keys: &[SortKey]) -> Vec<SortKey> {
    let key = |name: &[u8]| {
        specifier::find(name).map(|specifier| SortKey {
            specifier,
            order: Order::Ascending,
        })
    };
    let first = if keys.is_empty() {
        key(b"start_time").into_iter().collect()
    } else {
        keys.to_vec()
    };

    first.into_iter().chain(key(b"ppid")).collect()
}

/// The rows of the process trees in the order of procps-ng's `show_forest`.
/// `processes` has the pid and the parent's pid of each process, sorted with
/// the keys from [`sort_keys`]. Each row has an index into `processes` and
/// the row's prefix. A process whose parent is not in `processes` is the
/// root of a tree. procps-ng looks for roots from the last process to the
/// first, so the trees are in the reverse of the order that the sort gives
/// their roots.
pub(crate) fn rows(processes: &[(Pid, Pid)]) -> Vec<(usize, Vec<Branch>)> {
    let mut walk = Walk {
        processes,
        prefix: Vec::new(),
        rows: Vec::new(),
    };

    for index in (0..processes.len()).rev() {
        let (_, parent) = processes[index];

        if processes.iter().any(|&(pid, _)| pid == parent) {
            continue;
        }

        walk.show_tree(index, 0, false);
    }

    walk.rows
}

/// The state of procps-ng's walk of a process tree.
struct Walk<'a> {
    /// The pid and the parent's pid of each process.
    processes: &'a [(Pid, Pid)],
    /// The prefix of the current row (`forest_prefix`).
    prefix: Vec<Branch>,
    /// The rows so far.
    rows: Vec<(usize, Vec<Branch>)>,
}

impl Walk<'_> {
    /// Adds the row of the process at `index`, at `level`, and then the rows
    /// of its children, as procps-ng's `show_tree` does. `sibling` says
    /// whether the process has a later sibling. The children of a process
    /// follow its first child in the sorted processes.
    fn show_tree(&mut self, index: usize, level: usize, sibling: bool) {
        let (pid, _) = self.processes[index];

        if level > 0 {
            self.set(
                level,
                if sibling {
                    Branch::Middle
                } else {
                    Branch::Last
                },
            );
        }

        self.prefix.truncate(level);
        self.rows.push((index, self.prefix.clone()));

        let Some(first) = self.processes.iter().position(|&(_, parent)| parent == pid) else {
            return;
        };

        if level > 0 {
            self.set(
                level,
                if sibling {
                    Branch::Through
                } else {
                    Branch::Blank
                },
            );
        }

        self.prefix.truncate(level);

        let child_level = if level + 1 < LEVELS { level + 1 } else { level };
        let mut child = first;
        let mut more = true;

        while more && child < self.processes.len() {
            more = self
                .processes
                .get(child + 1)
                .is_some_and(|&(_, parent)| parent == pid);
            self.show_tree(child, child_level, more);
            child += 1;
        }

        self.prefix.truncate(level);
    }

    /// Sets the branch of `level`, whose prefix has at least `level - 1`
    /// branches.
    fn set(&mut self, level: usize, branch: Branch) {
        self.prefix.truncate(level - 1);
        self.prefix.push(branch);
    }
}

/// The text before a command in a row with `prefix`, as procps-ng's
/// `forest_helper` writes it into a column with `cells` of room. The prefix
/// stops before a level that does not fit. UNIX's `-H` indents each level
/// by two spaces, and BSD's `f` and `--forest` draw the branches.
pub(crate) fn text(forest: Forest, prefix: &[Branch], cells: usize) -> Vec<u8> {
    let width = if forest == Forest::Unix { 2 } else { 4 };

    prefix
        .iter()
        .take(cells / width)
        .flat_map(|branch| match (forest, branch) {
            (Forest::Unix, _) => b"  ".as_slice(),
            (_, Branch::Blank) => b"    ",
            (_, Branch::Last | Branch::Middle) => b" \\_ ",
            (_, Branch::Through) => b" |  ",
        })
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use darwin_proc::Pid;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{
        Branch::{self, Blank, Last, Middle, Through},
        rows, sort_keys, text,
    };
    use crate::ps::{
        format::{Order, SortKey},
        options::Forest,
        specifier,
    };

    fn key(name: &str, order: Order) -> SortKey {
        SortKey {
            specifier: specifier::find(name.as_bytes()).expect("a specifier"),
            order,
        }
    }

    #[rstest]
    #[case::start_time(&[], vec![key("start_time", Order::Ascending), key("ppid", Order::Ascending)])]
    #[case::given(
        &[key("pid", Order::Descending)],
        vec![key("pid", Order::Descending), key("ppid", Order::Ascending)]
    )]
    fn a_tree_sorts_by_parent_last(#[case] keys: &[SortKey], #[case] expected: Vec<SortKey>) {
        assert_eq!(sort_keys(keys), expected);
    }

    /// The pids and parents' pids of `processes`.
    fn pids(processes: &[(i32, i32)]) -> Vec<(Pid, Pid)> {
        processes
            .iter()
            .map(|&(pid, parent)| (Pid::from(pid), Pid::from(parent)))
            .collect()
    }

    // The processes are sorted by their parents' pids: 1 has the children 10
    // and 20, 10 has 11 and 12, 12 has 13, and 20 has 21. 2 is the root of a
    // second tree, and 99's parent is missing.
    #[test]
    fn the_tree_shows_each_child_after_its_parent() {
        let processes = pids(&[
            (1, 0),
            (2, 0),
            (10, 1),
            (20, 1),
            (11, 10),
            (12, 10),
            (13, 12),
            (21, 20),
            (99, 98),
        ]);
        let expected: Vec<(usize, Vec<Branch>)> = vec![
            (8, vec![]),
            (1, vec![]),
            (0, vec![]),
            (2, vec![Middle]),
            (4, vec![Through, Middle]),
            (5, vec![Through, Last]),
            (6, vec![Through, Blank, Last]),
            (3, vec![Last]),
            (7, vec![Blank, Last]),
        ];

        assert_eq!(rows(&processes), expected);
    }

    #[rstest]
    #[case::gnu(Forest::Gnu, 100, " |       \\_ ")]
    #[case::bsd(Forest::Bsd, 100, " |       \\_ ")]
    #[case::unix(Forest::Unix, 100, "      ")]
    #[case::cut(Forest::Gnu, 11, " |      ")]
    #[case::unix_cut(Forest::Unix, 5, "    ")]
    fn the_prefix_is_drawn_as_procps_ng_draws_it(
        #[case] forest: Forest,
        #[case] cells: usize,
        #[case] expected: &str,
    ) {
        let drawn = text(forest, &[Through, Blank, Last], cells);

        assert_eq!(String::from_utf8(drawn), Ok(expected.to_owned()));
    }
}
