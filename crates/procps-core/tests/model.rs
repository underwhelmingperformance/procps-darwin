// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Builds snapshot requests and maps field values.

use darwin_proc::Pid;
use pretty_assertions::assert_eq;
use procps_core::{Field, FieldGroup, SnapshotRequest};
use rstest::rstest;

#[rstest]
#[case::available(Field::Available(2), Field::Available(4))]
#[case::denied(Field::Denied, Field::Denied)]
#[case::unsupported(Field::Unsupported, Field::Unsupported)]
#[case::failed(Field::Failed, Field::Failed)]
fn mapping_a_field_keeps_its_outcome(#[case] field: Field<i32>, #[case] expected: Field<i32>) {
    assert_eq!(field.map(|value| value * 2), expected);
}

#[rstest]
#[case::available(Field::Available(2), Some(&2))]
#[case::denied(Field::Denied, None)]
#[case::unsupported(Field::Unsupported, None)]
#[case::failed(Field::Failed, None)]
fn only_an_available_field_has_a_value(#[case] field: Field<i32>, #[case] expected: Option<&i32>) {
    assert_eq!(field.available(), expected);
}

#[test]
fn a_default_request_reads_the_identity_of_every_process() {
    let request = SnapshotRequest::default();
    let groups = FieldGroup::ALL.map(|group| request.wants(group));

    assert_eq!(
        (
            groups,
            request.selects(Pid::from(1)),
            request.wants_system()
        ),
        ([false; FieldGroup::ALL.len()], true, false)
    );
}

#[test]
fn a_request_lists_its_groups_processes_and_system_statistics() {
    let request = SnapshotRequest::default()
        .with(FieldGroup::Threads)
        .with(FieldGroup::Usage)
        .for_processes([Pid::from(1), Pid::from(42)])
        .with_system();

    assert_eq!(
        (
            FieldGroup::ALL.map(|group| request.wants(group)),
            [1, 42, 43].map(|pid| request.selects(Pid::from(pid))),
            request.wants_system()
        ),
        (
            FieldGroup::ALL.map(|group| matches!(group, FieldGroup::Threads | FieldGroup::Usage)),
            [true, true, false],
            true
        )
    );
}

#[test]
fn processes_selected_twice_are_selected_together() {
    let request = SnapshotRequest::default()
        .for_processes([Pid::from(1)])
        .for_processes([Pid::from(2)]);

    assert_eq!(
        [1, 2, 3].map(|pid| request.selects(Pid::from(pid))),
        [true, true, false]
    );
}
