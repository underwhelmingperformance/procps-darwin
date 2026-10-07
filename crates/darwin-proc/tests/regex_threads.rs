// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Matches one compiled expression on several threads at once.

use std::{sync::Barrier, thread};

use darwin_proc::{Regex, RegexError};
use pretty_assertions::assert_eq;

const THREADS: usize = 12;
const ROUNDS: usize = 300;
const TEXTS: usize = 300;

/// The text with the number `index` in a round. It has the letters `a` and
/// `b` in the order of the bits of `index`, then the round's number, then `cd`
/// or `ce`. The text matches the round's expression only if `index` is even.
fn text(index: usize, round: usize) -> String {
    let letters: String = (0..10)
        .map(|bit| if index >> bit & 1 == 0 { 'a' } else { 'b' })
        .collect();
    let end = if index.is_multiple_of(2) { "cd" } else { "ce" };

    format!("{letters}{round}{end}")
}

// `regexec` adds states to a cache in the compiled expression, and it locks a
// mutex in the expression first. Each round compiles a new expression, so the
// cache is empty when the threads start matching together.
#[test]
fn threads_can_match_one_expression_at_the_same_time() -> Result<(), RegexError> {
    for round in 0..ROUNDS {
        let regex = Regex::new(format!("^(a|b)*{round}(c|d)+$").as_ref(), false)?;
        let barrier = Barrier::new(THREADS);

        let matches = thread::scope(|scope| {
            let threads: Vec<_> = (0..THREADS)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();

                        (0..TEXTS)
                            .filter(|&index| regex.is_match(text(index, round).as_ref()))
                            .count()
                    })
                })
                .collect();

            threads
                .into_iter()
                .map(|thread| thread.join().ok())
                .collect::<Vec<_>>()
        });

        assert_eq!(matches, vec![Some(TEXTS / 2); THREADS]);
    }

    Ok(())
}
