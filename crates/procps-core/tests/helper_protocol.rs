// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sends requests and responses through the helper protocol's framing.

use std::{
    ffi::OsString,
    io::{Cursor, ErrorKind},
    os::unix::ffi::OsStringExt,
    path::PathBuf,
    time::{Duration, SystemTime},
};

use darwin_proc::{
    AccountingFlags, Credentials, Gid, LoadAverage, Memory, Pid, ProcessFlags, ProcessInfo,
    ProcessorTicks, Protection, Region, ResourceUsage, RunState, SchedulingPolicy, ShareMode,
    SignalSet, Status, Swap, TaskInfo, TaskTotals, Terminal, ThreadInfo, Uid,
};
use pretty_assertions::assert_eq;
use procps_core::{
    Field, FieldGroup, LocalSource, Process, ProcessSource, Snapshot, SnapshotRequest, System,
    Usage,
    helper::{
        Message, ProtocolError, ReadMessage, Refusal, Request, Response, VERSION, WriteMessage,
    },
};
use rstest::rstest;

/// `message` after a trip through the framing.
fn round_trip<M: Message>(message: &M) -> Result<M, ProtocolError> {
    let mut wire = Vec::new();
    wire.write_message(message)?;

    Cursor::new(wire).read_message()
}

/// A header with `version` and `length`, followed by `body`, which can differ
/// in length from `length`.
fn frame(version: u16, length: u32, body: &[u8]) -> Vec<u8> {
    [&version.to_be_bytes()[..], &length.to_be_bytes(), body].concat()
}

/// A request for every field group of this process, and for the system
/// statistics.
fn everything() -> SnapshotRequest {
    FieldGroup::ALL
        .into_iter()
        .fold(SnapshotRequest::default(), SnapshotRequest::with)
        .for_processes([Pid::current()])
        .with_system()
}

#[rstest]
#[case::default(SnapshotRequest::default())]
#[case::everything(everything())]
fn a_request_survives_the_framing(
    #[case] request: SnapshotRequest,
) -> Result<(), Box<dyn std::error::Error>> {
    let request = Request::Snapshot(request);

    assert_eq!(round_trip(&request)?, request);

    Ok(())
}

#[test]
fn a_snapshot_of_this_process_survives_the_framing() -> Result<(), Box<dyn std::error::Error>> {
    let response = Response::Snapshot(Box::new(LocalSource.snapshot(&everything())?));

    assert_eq!(round_trip(&response)?, response);

    Ok(())
}

#[test]
fn paths_and_arguments_need_not_be_utf8() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = || OsString::from_vec(vec![b'a', 0xff, b'b']);
    let info = Pid::current().info()?;
    let process = Process {
        arguments: Some(Field::Available(vec![bytes()])),
        environment: Some(Field::Denied),
        usage: Some(Usage {
            task: Field::Unsupported,
            resources: Field::Failed,
        }),
        working_directory: Some(Field::Available(PathBuf::from(bytes()))),
        ..Process::from_identity(
            info,
            Field::Available(PathBuf::from(bytes())),
            Field::Available(Pid::from(1)),
        )
    };
    let response = Response::Snapshot(Box::new(Snapshot {
        taken: SystemTime::UNIX_EPOCH,
        uptime: Duration::from_secs(1),
        processes: vec![process],
        system: None,
    }));

    assert_eq!(round_trip(&response)?, response);

    Ok(())
}

#[rstest]
#[case::refusal(Response::Refused(Refusal::Version))]
#[case::every_variant(fixed_response())]
fn a_response_survives_the_framing(
    #[case] response: Response,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(round_trip(&response)?, response);

    Ok(())
}

/// A [`ProtocolError`] that tests can compare.
#[derive(Debug, PartialEq)]
enum Rejection {
    Io(ErrorKind),
    Version(u16),
    TooLarge { length: u64, limit: u32 },
    Encoding,
    TrailingBytes(usize),
}

impl From<&ProtocolError> for Rejection {
    fn from(error: &ProtocolError) -> Self {
        match error {
            ProtocolError::Io(error) => Self::Io(error.kind()),
            ProtocolError::Version { peer } => Self::Version(*peer),
            ProtocolError::TooLarge { length, limit } => Self::TooLarge {
                length: *length,
                limit: *limit,
            },
            ProtocolError::Encoding(_) => Self::Encoding,
            ProtocolError::TrailingBytes(count) => Self::TrailingBytes(*count),
        }
    }
}

/// The rejection, if any, from reading a message of type `M` from `wire`, and
/// the number of bytes that the reader consumed.
fn rejection<M: Message>(wire: Vec<u8>) -> (Option<Rejection>, u64) {
    let mut cursor = Cursor::new(wire);
    let rejection = cursor
        .read_message::<M>()
        .err()
        .as_ref()
        .map(Rejection::from);

    (rejection, cursor.position())
}

/// A body that declares a set of `u64::MAX` process IDs and contains none.
const ENDLESS_SELECTION: [u8; 13] = [
    0, 0, 1, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01,
];

#[rstest]
#[case::other_version(frame(VERSION + 1, 1, &[0]), Rejection::Version(VERSION + 1), 2)]
#[case::above_the_limit(
    frame(VERSION, Request::LIMIT + 1, &[]),
    Rejection::TooLarge { length: u64::from(Request::LIMIT) + 1, limit: Request::LIMIT },
    6,
)]
#[case::largest_length(
    frame(VERSION, u32::MAX, &[]),
    Rejection::TooLarge { length: u64::from(u32::MAX), limit: Request::LIMIT },
    6,
)]
#[case::empty(Vec::new(), Rejection::Io(ErrorKind::UnexpectedEof), 0)]
#[case::truncated_version(vec![0], Rejection::Io(ErrorKind::UnexpectedEof), 1)]
#[case::truncated_length([&VERSION.to_be_bytes()[..], &[0, 0]].concat(), Rejection::Io(ErrorKind::UnexpectedEof), 4)]
#[case::truncated_body(frame(VERSION, 3, &[0]), Rejection::Io(ErrorKind::UnexpectedEof), 7)]
#[case::at_the_limit(frame(VERSION, Request::LIMIT, &[]), Rejection::Io(ErrorKind::UnexpectedEof), 6)]
#[case::unknown_variant(frame(VERSION, 1, &[0x7f]), Rejection::Encoding, 7)]
#[case::endless_selection(frame(VERSION, 13, &ENDLESS_SELECTION), Rejection::Encoding, 19)]
#[case::trailing_bytes(frame(VERSION, 5, &[0, 0, 0, 0, 0]), Rejection::TrailingBytes(1), 11)]
fn a_bad_request_is_rejected(
    #[case] wire: Vec<u8>,
    #[case] expected: Rejection,
    #[case] consumed: u64,
) {
    assert_eq!(rejection::<Request>(wire), (Some(expected), consumed));
}

#[test]
fn a_response_has_its_own_limit() {
    let rejections = (
        rejection::<Response>(frame(VERSION, Request::LIMIT + 1, &[])),
        rejection::<Response>(frame(VERSION, Response::LIMIT + 1, &[])),
    );

    assert_eq!(
        rejections,
        (
            (Some(Rejection::Io(ErrorKind::UnexpectedEof)), 6),
            (
                Some(Rejection::TooLarge {
                    length: u64::from(Response::LIMIT) + 1,
                    limit: Response::LIMIT
                }),
                6
            ),
        )
    );
}

#[test]
fn a_message_above_the_limit_is_not_sent() {
    let request =
        Request::Snapshot(SnapshotRequest::default().for_processes((0..400_000).map(Pid::from)));
    let mut wire = Vec::new();

    let rejection = wire
        .write_message(&request)
        .err()
        .as_ref()
        .map(Rejection::from);

    assert_eq!(
        (rejection, wire),
        (
            Some(Rejection::TooLarge {
                length: 1_191_751,
                limit: Request::LIMIT
            }),
            Vec::new()
        )
    );
}

// The fixtures from here on pin the encoding of version 1 of the protocol.
// Together they use every enum variant, and within each struct, fields of the
// same type have different values. Reordering variants or fields therefore
// changes the frames of the fixed messages.

/// A request for every field group of two processes, and for the system
/// statistics.
fn request_for_everything() -> Request {
    Request::Snapshot(
        FieldGroup::ALL
            .into_iter()
            .fold(SnapshotRequest::default(), SnapshotRequest::with)
            .for_processes([Pid::from(1), Pid::from(99_999)])
            .with_system(),
    )
}

/// The `kinfo_proc` record of a process with `pid` and `status`.
fn fixed_info(pid: i32, status: Status) -> ProcessInfo {
    ProcessInfo {
        pid: Pid::from(pid),
        ppid: Pid::from(1),
        pgid: Pid::from(pid + 1),
        terminal: Some(Terminal {
            device: 0x1000_0002,
            foreground_group: Pid::from(pid + 2),
        }),
        credentials: Credentials {
            ruid: Uid::from(501),
            euid: Uid::from(0),
            svuid: Uid::from(502),
            rgid: Gid::from(20),
            egid: Gid::from(21),
            svgid: Gid::from(22),
            groups: vec![Gid::from(23), Gid::from(12)],
        },
        nice: -5,
        priority: 31,
        status,
        flags: ProcessFlags::from(0x4004),
        accounting: AccountingFlags::from(0x3),
        session_leader: true,
        start_time: SystemTime::UNIX_EPOCH + Duration::new(1_790_000_000, 500_000),
        comm: "launchd".to_owned(),
        ignored_signals: SignalSet::from(0x1),
        caught_signals: SignalSet::from(0x4000),
    }
}

/// The task information of [`fixed_process`].
const fn fixed_task() -> TaskInfo {
    TaskInfo {
        virtual_size: 1 << 40,
        resident_size: 1 << 24,
        user_time: Duration::from_millis(1500),
        system_time: Duration::from_millis(250),
        policy: SchedulingPolicy::Timeshare,
        faults: 1,
        pageins: 2,
        copy_on_write_faults: 3,
        messages_sent: 4,
        messages_received: 5,
        mach_system_calls: 6,
        unix_system_calls: 7,
        context_switches: 8,
        threads: 9,
        running_threads: 10,
        priority: 31,
    }
}

/// The resource usage of [`fixed_process`].
const fn fixed_resources() -> ResourceUsage {
    ResourceUsage {
        user_time: Duration::from_millis(1500),
        system_time: Duration::from_millis(250),
        runnable_time: Duration::from_millis(10),
        idle_wakeups: 1,
        interrupt_wakeups: 2,
        pageins: 3,
        wired_size: 4,
        resident_size: 5,
        physical_footprint: 6,
        peak_physical_footprint: 7,
        disk_bytes_read: 8,
        disk_bytes_written: 9,
        logical_writes: 10,
        instructions: 11,
        cycles: 12,
        energy_nanojoules: 13,
    }
}

/// A thread in each run state, together using every scheduling policy.
fn fixed_threads() -> Vec<ThreadInfo> {
    let states = [
        RunState::Running,
        RunState::Stopped,
        RunState::Waiting,
        RunState::Uninterruptible,
        RunState::Halted,
        RunState::Other(9),
    ];
    let policies = [
        SchedulingPolicy::Timeshare,
        SchedulingPolicy::RoundRobin,
        SchedulingPolicy::Fifo,
        SchedulingPolicy::Other(7),
    ];

    states
        .into_iter()
        .zip(policies.into_iter().cycle())
        .zip(1_u64..)
        .map(|((run_state, policy), id)| ThreadInfo {
            id,
            user_time: Duration::from_millis(100),
            system_time: Duration::from_millis(20),
            cpu_usage: 500,
            policy,
            run_state,
            swapped: false,
            idle: true,
            current_priority: 31,
            base_priority: 32,
            max_priority: 63,
            name: format!("thread {id}"),
        })
        .collect()
}

/// A memory region in each share mode.
fn fixed_regions() -> Vec<Region> {
    let modes = [
        ShareMode::CopyOnWrite,
        ShareMode::Private,
        ShareMode::Empty,
        ShareMode::Shared,
        ShareMode::TrueShared,
        ShareMode::PrivateAliased,
        ShareMode::SharedAliased,
        ShareMode::LargePage,
        ShareMode::Other(99),
    ];

    modes
        .into_iter()
        .zip(1_u64..)
        .map(|(share_mode, index)| Region {
            address: index << 32,
            size: 0x4000,
            protection: Protection::from(0x5),
            max_protection: Protection::from(0x7),
            share_mode,
            user_tag: 1,
            resident: 0x3000,
            private_resident: 0x2000,
            shared_resident: 0x1000,
            shared_now_private: 0x800,
            swapped_out: 0x400,
            dirtied: 0x200,
            reference_count: 2,
            object_id: std::num::NonZeroU32::new(77),
            is_submap: false,
            is_shared: true,
        })
        .collect()
}

/// A process with every field group. Its fields use each variant of `Field`.
fn fixed_process() -> Process {
    Process {
        arguments: Some(Field::Available(vec![OsString::from("launchd")])),
        environment: Some(Field::Denied),
        usage: Some(Usage {
            task: Field::Available(fixed_task()),
            resources: Field::Available(fixed_resources()),
        }),
        threads: Some(Field::Available(fixed_threads())),
        regions: Some(Field::Available(fixed_regions())),
        file_descriptors: Some(Field::Failed),
        working_directory: Some(Field::Available(PathBuf::from("/"))),
        ..Process::from_identity(
            fixed_info(400, Status::Running),
            Field::Available(PathBuf::from("/sbin/launchd")),
            Field::Unsupported,
        )
    }
}

/// A response with [`fixed_process`], one process with only its identity for
/// each status except `Running`, and the system statistics.
fn fixed_response() -> Response {
    let uptime = Duration::from_secs(100);
    let system = System {
        boot_time: SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000),
        load_average: LoadAverage {
            one_minute: 1.5,
            five_minutes: 1.25,
            fifteen_minutes: 1.0,
        },
        memory: Memory {
            total: 1 << 34,
            free: 1,
            active: 2,
            inactive: 3,
            speculative: 4,
            throttled: 5,
            wired: 6,
            purgeable: 7,
            file_backed: 8,
            anonymous: 9,
            compressor: 10,
            compressed: 11,
        },
        swap: Swap {
            total: 1 << 30,
            used: 1 << 20,
            available: (1 << 30) - (1 << 20),
            encrypted: true,
        },
        processors: vec![ProcessorTicks {
            user: 1,
            system: 2,
            idle: 3,
            nice: 0,
        }],
        tasks: TaskTotals {
            tasks: 600,
            threads: 3000,
        },
        users: 2,
    };
    let others = [
        Status::Idle,
        Status::Sleeping,
        Status::Stopped,
        Status::Zombie,
        Status::Other(-1),
    ]
    .into_iter()
    .zip((500..).step_by(10))
    .map(|(status, pid)| {
        Process::from_identity(fixed_info(pid, status), Field::Denied, Field::Failed)
    });

    Response::Snapshot(Box::new(Snapshot {
        taken: system.boot_time + uptime,
        uptime,
        processes: std::iter::once(fixed_process()).chain(others).collect(),
        system: Some(system),
    }))
}

/// The hexadecimal digits of `message`'s frame.
fn hex<M: Message>(message: &M) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";

    let mut wire = Vec::new();
    wire.write_message(message)
        .expect("a fixed message fits in a frame");

    wire.iter()
        .flat_map(|byte| {
            [
                DIGITS[usize::from(byte >> 4)],
                DIGITS[usize::from(byte & 0xf)],
            ]
        })
        .map(char::from)
        .collect()
}

/// The frame of [`fixed_response`] in version 1 of the protocol.
const RESPONSE_V1: &str = concat!(
    "0001000003dc00e4f7c4d50600640006a00602a206018480808002a406f50300",
    "f60314151602170cfb1f01848001030180f7c4d506a0c21e076c61756e636864",
    "0180800100000d2f7362696e2f6c61756e6368640201000100076c61756e6368",
    "6401010100808080808020808080080180cab5ee010080e59a77000102030405",
    "060708090a3e000180cab5ee010080e59a770080ade204010203040506070809",
    "0a0b0c0d010006010080c2d72f0080dac409f403000000013e407e0874687265",
    "61642031020080c2d72f0080dac409f403010100013e407e0874687265616420",
    "32030080c2d72f0080dac409f403020200013e407e0874687265616420330400",
    "80c2d72f0080dac409f403030e0300013e407e087468726561642034050080c2",
    "d72f0080dac409f403000400013e407e087468726561642035060080c2d72f00",
    "80dac409f40301051200013e407e087468726561642036010009808080801080",
    "80010507000180608040802080108008800402014d0001808080802080800105",
    "07010180608040802080108008800402014d0001808080803080800105070201",
    "80608040802080108008800402014d0001808080804080800105070301806080",
    "40802080108008800402014d0001808080805080800105070401806080408020",
    "80108008800402014d0001808080806080800105070501806080408020801080",
    "08800402014d0001808080807080800105070601806080408020801080088004",
    "02014d0001808080808001808001050707018060804080208010800880040201",
    "4d0001808080809001808001050708630180608040802080108008800402014d",
    "00010103010000012fe80702ea07018480808002ec07f50300f6031415160217",
    "0cfb1f00848001030180f7c4d506a0c21e076c61756e63686401808001010300",
    "000000000000fc0702fe070184808080028008f50300f60314151602170cfb1f",
    "02848001030180f7c4d506a0c21e076c61756e63686401808001010300000000",
    "00000090080292080184808080029408f50300f60314151602170cfb1f038480",
    "01030180f7c4d506a0c21e076c61756e63686401808001010300000000000000",
    "a40802a608018480808002a808f50300f60314151602170cfb1f048480010301",
    "80f7c4d506a0c21e076c61756e63686401808001010300000000000000b80802",
    "ba08018480808002bc08f50300f60314151602170cfb1f05ff848001030180f7",
    "c4d506a0c21e076c61756e636864018080010103000000000000000180f7c4d5",
    "0600000000000000f83f000000000000f43f000000000000f03f808080804001",
    "02030405060708090a0b80808080048080408080c0ff03010101020300d804b8",
    "1702",
);

#[rstest]
#[case::request_for_identities(hex(&Request::Snapshot(SnapshotRequest::default())), "00010000000400000000")]
#[case::request_for_everything(hex(&request_for_everything()), "000100000010000700010203040506010202be9a0c01")]
#[case::response(hex(&fixed_response()), RESPONSE_V1)]
#[case::version_refusal(hex(&Response::Refused(Refusal::Version)), "0001000000020100")]
#[case::request_too_large_refusal(hex(&Response::Refused(Refusal::RequestTooLarge)), "0001000000020101")]
#[case::malformed_refusal(hex(&Response::Refused(Refusal::Malformed)), "0001000000020102")]
#[case::failed_refusal(hex(&Response::Refused(Refusal::Failed)), "0001000000020103")]
#[case::timed_out_refusal(hex(&Response::Refused(Refusal::TimedOut)), "0001000000020104")]
#[case::response_too_large_refusal(hex(&Response::Refused(Refusal::ResponseTooLarge)), "0001000000020105")]
fn the_encoding_matches_protocol_version_1(#[case] frame: String, #[case] expected: &str) {
    assert_eq!(
        (VERSION, frame.as_str()),
        (1, expected),
        "the encoding has changed: increase VERSION and replace the expected frames"
    );
}
