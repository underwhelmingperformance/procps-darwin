// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Declarations from the macOS SDK that the `libc` crate lacks.
//!
//! The items are `pub` so that public types can convert from them, but the
//! module is private.

use libc::{c_char, c_int, c_short, c_uchar, c_ushort, dev_t, gid_t, pid_t, uid_t};

/// `struct kinfo_proc` from `<sys/sysctl.h>`.
#[repr(C)]
pub struct KinfoProc {
    pub kp_proc: ExternProc,
    pub kp_eproc: Eproc,
}

/// `struct extern_proc` from `<sys/proc.h>`. Only `p_starttime` of the leading
/// union is read.
#[repr(C)]
pub struct ExternProc {
    pub p_starttime: libc::timeval,
    pub p_vmspace: *mut libc::c_void,
    pub p_sigacts: *mut libc::c_void,
    pub p_flag: c_int,
    pub p_stat: c_char,
    pub p_pid: pid_t,
    pub p_oppid: pid_t,
    pub p_dupfd: c_int,
    pub user_stack: *mut c_char,
    pub exit_thread: *mut libc::c_void,
    pub p_debugger: c_int,
    pub sigwait: c_int,
    pub p_estcpu: u32,
    pub p_cpticks: c_int,
    pub p_pctcpu: u32,
    pub p_wchan: *mut libc::c_void,
    pub p_wmesg: *mut c_char,
    pub p_swtime: u32,
    pub p_slptime: u32,
    pub p_realtimer: libc::itimerval,
    pub p_rtime: libc::timeval,
    pub p_uticks: u64,
    pub p_sticks: u64,
    pub p_iticks: u64,
    pub p_traceflag: c_int,
    pub p_tracep: *mut libc::c_void,
    pub p_siglist: c_int,
    pub p_textvp: *mut libc::c_void,
    pub p_holdcnt: c_int,
    pub p_sigmask: libc::sigset_t,
    pub p_sigignore: libc::sigset_t,
    pub p_sigcatch: libc::sigset_t,
    pub p_priority: c_uchar,
    pub p_usrpri: c_uchar,
    pub p_nice: c_char,
    pub p_comm: [c_char; MAXCOMLEN + 1],
    pub p_pgrp: *mut libc::c_void,
    pub p_addr: *mut libc::c_void,
    pub p_xstat: c_ushort,
    pub p_acflag: c_ushort,
    pub p_ru: *mut libc::c_void,
}

/// `struct eproc` from `<sys/sysctl.h>`.
#[repr(C)]
#[expect(
    clippy::struct_field_names,
    reason = "the fields keep their names from the SDK header"
)]
pub struct Eproc {
    pub e_paddr: *mut libc::c_void,
    pub e_sess: *mut libc::c_void,
    pub e_pcred: Pcred,
    pub e_ucred: Ucred,
    pub e_vm: Vmspace,
    pub e_ppid: pid_t,
    pub e_pgid: pid_t,
    pub e_jobc: c_short,
    pub e_tdev: dev_t,
    pub e_tpgid: pid_t,
    pub e_tsess: *mut libc::c_void,
    pub e_wmesg: [c_char; 8],
    pub e_xsize: i32,
    pub e_xrssize: c_short,
    pub e_xccount: c_short,
    pub e_xswrss: c_short,
    pub e_flag: i32,
    pub e_login: [c_char; 12],
    pub e_spare: [i32; 4],
}

/// `struct _pcred` from `<sys/sysctl.h>`.
#[repr(C)]
pub struct Pcred {
    pub pc_lock: [c_char; 72],
    pub pc_ucred: *mut libc::c_void,
    pub p_ruid: uid_t,
    pub p_svuid: uid_t,
    pub p_rgid: gid_t,
    pub p_svgid: gid_t,
    pub p_refcnt: c_int,
}

/// `struct _ucred` from `<sys/sysctl.h>`.
#[repr(C)]
#[expect(
    clippy::struct_field_names,
    reason = "the fields keep their names from the SDK header"
)]
pub struct Ucred {
    pub cr_ref: i32,
    pub cr_uid: uid_t,
    pub cr_ngroups: c_short,
    pub cr_groups: [gid_t; NGROUPS],
}

/// `struct vmspace` from `<sys/vm.h>`.
#[repr(C)]
pub struct Vmspace {
    pub dummy: i32,
    pub dummy2: *mut c_char,
    pub dummy3: [i32; 5],
    pub dummy4: [*mut c_char; 3],
}

pub const MAXCOMLEN: usize = 16;
pub const NGROUPS: usize = 16;

/// The terminal device of a process that has no controlling terminal.
pub const NODEV: dev_t = -1;

pub const SIDL: c_char = 1;
pub const SRUN: c_char = 2;
pub const SSLEEP: c_char = 3;
pub const SSTOP: c_char = 4;
pub const SZOMB: c_char = 5;

pub const P_LP64: c_int = 0x4;
pub const P_SUGID: c_int = 0x100;
pub const P_SYSTEM: c_int = 0x200;
pub const P_TRACED: c_int = 0x800;
pub const P_WEXIT: c_int = 0x2000;
pub const P_EXEC: c_int = 0x4000;
pub const P_TRANSLATED: c_int = 0x20000;

pub const EPROC_SLEADER: i32 = 0x2;

#[cfg(test)]
mod tests {
    use std::mem::{offset_of, size_of};

    use pretty_assertions::assert_eq;

    use super::{Eproc, ExternProc, KinfoProc, Pcred, Ucred};

    // The expected values come from a C program built against the macOS 27 SDK
    // that prints `sizeof` and `offsetof` for each struct.

    #[test]
    fn kinfo_proc_matches_the_sdk() {
        assert_eq!(
            [
                size_of::<KinfoProc>(),
                offset_of!(KinfoProc, kp_eproc),
                size_of::<ExternProc>(),
                size_of::<Eproc>(),
                size_of::<Pcred>(),
                size_of::<Ucred>(),
            ],
            [648, 296, 296, 352, 104, 76]
        );
    }

    #[test]
    fn extern_proc_fields_match_the_sdk() {
        assert_eq!(
            [
                offset_of!(ExternProc, p_starttime),
                offset_of!(ExternProc, p_flag),
                offset_of!(ExternProc, p_stat),
                offset_of!(ExternProc, p_pid),
                offset_of!(ExternProc, p_sigignore),
                offset_of!(ExternProc, p_sigcatch),
                offset_of!(ExternProc, p_priority),
                offset_of!(ExternProc, p_nice),
                offset_of!(ExternProc, p_comm),
                offset_of!(ExternProc, p_xstat),
            ],
            [0, 32, 36, 40, 232, 236, 240, 242, 243, 280]
        );
    }

    #[test]
    fn eproc_fields_match_the_sdk() {
        assert_eq!(
            [
                offset_of!(Eproc, e_pcred),
                offset_of!(Eproc, e_ucred),
                offset_of!(Eproc, e_ppid),
                offset_of!(Eproc, e_pgid),
                offset_of!(Eproc, e_tdev),
                offset_of!(Eproc, e_tpgid),
                offset_of!(Eproc, e_flag),
                offset_of!(Pcred, p_ruid),
                offset_of!(Pcred, p_svuid),
                offset_of!(Pcred, p_rgid),
                offset_of!(Pcred, p_svgid),
                offset_of!(Ucred, cr_uid),
                offset_of!(Ucred, cr_ngroups),
                offset_of!(Ucred, cr_groups),
            ],
            [16, 120, 264, 268, 276, 280, 316, 80, 84, 88, 92, 4, 8, 12]
        );
    }
}
