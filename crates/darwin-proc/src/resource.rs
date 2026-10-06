// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{io, time::Duration};

use crate::{
    Call, Error, Pid,
    ffi::{self, RusageInfoV6},
    sysctl::zeroed,
    time::Timebase,
};

/// A process's resource usage, from `proc_pid_rusage` with `RUSAGE_INFO_V6`.
///
/// ```
/// use darwin_proc::Pid;
///
/// let usage = Pid::current().resource_usage()?;
///
/// assert!(usage.physical_footprint > 0);
/// # Ok::<(), darwin_proc::Error>(())
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceUsage {
    /// The user CPU time.
    pub user_time: Duration,
    /// The system CPU time.
    pub system_time: Duration,
    /// The time that the process's threads spent runnable but waiting for a
    /// processor.
    pub runnable_time: Duration,
    /// The number of wakeups from idle to run this process's threads.
    pub idle_wakeups: u64,
    /// The number of wakeups by interrupts.
    pub interrupt_wakeups: u64,
    /// The number of page faults that read from disk.
    pub pageins: u64,
    /// The wired memory size in bytes.
    pub wired_size: u64,
    /// The resident memory size in bytes.
    pub resident_size: u64,
    /// The physical footprint in bytes, which Activity Monitor shows as
    /// "Memory".
    pub physical_footprint: u64,
    /// The largest physical footprint in bytes since the process started.
    pub peak_physical_footprint: u64,
    /// The bytes read from disk.
    pub disk_bytes_read: u64,
    /// The bytes written to disk.
    pub disk_bytes_written: u64,
    /// The bytes written by the process, including those still in caches.
    pub logical_writes: u64,
    /// The number of instructions retired. The kernel reads it from the CPU's
    /// performance counters, and reports 0 where it cannot read them, as on
    /// the macOS virtual machines that CI uses.
    pub instructions: u64,
    /// The number of CPU cycles, from the same counters as `instructions`.
    pub cycles: u64,
    /// The energy used, in nanojoules.
    pub energy_nanojoules: u64,
}

impl ResourceUsage {
    /// Decodes `raw`, whose times are in Mach absolute time.
    pub(crate) fn decode(raw: &RusageInfoV6, timebase: Timebase) -> Self {
        Self {
            user_time: timebase.duration(raw.ri_user_time),
            system_time: timebase.duration(raw.ri_system_time),
            runnable_time: timebase.duration(raw.ri_runnable_time),
            idle_wakeups: raw.ri_pkg_idle_wkups,
            interrupt_wakeups: raw.ri_interrupt_wkups,
            pageins: raw.ri_pageins,
            wired_size: raw.ri_wired_size,
            resident_size: raw.ri_resident_size,
            physical_footprint: raw.ri_phys_footprint,
            peak_physical_footprint: raw.ri_lifetime_max_phys_footprint,
            disk_bytes_read: raw.ri_diskio_bytesread,
            disk_bytes_written: raw.ri_diskio_byteswritten,
            logical_writes: raw.ri_logical_writes,
            instructions: raw.ri_instructions,
            cycles: raw.ri_cycles,
            energy_nanojoules: raw.ri_energy_nj,
        }
    }
}

impl Pid {
    /// Reads the process's [`ResourceUsage`]. For a zombie, it reads the usage
    /// at the time that the process exited.
    ///
    /// ```
    /// use darwin_proc::Pid;
    ///
    /// let usage = Pid::current().resource_usage()?;
    ///
    /// assert!(usage.peak_physical_footprint >= usage.physical_footprint);
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Exited`] if the process does not exist,
    /// [`Error::Denied`] for another user's process, or another error if
    /// `proc_pid_rusage` fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn resource_usage(self) -> Result<ResourceUsage, Error> {
        let mut raw = zeroed::<RusageInfoV6>();

        // SAFETY: the kernel writes a `rusage_info_v6` for `RUSAGE_INFO_V6`,
        // and `raw` is one. Every bit pattern is a valid `RusageInfoV6`.
        let result = unsafe {
            libc::proc_pid_rusage(
                self.as_raw(),
                ffi::RUSAGE_INFO_V6,
                (&raw mut raw).cast::<libc::rusage_info_t>(),
            )
        };

        if result != 0 {
            return Err(self.live_error(Call::ResourceUsage, io::Error::last_os_error()));
        }

        let timebase = Timebase::current().map_err(|source| Error::Os {
            call: Call::ResourceUsage,
            source,
        })?;

        Ok(ResourceUsage::decode(&raw, timebase))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pretty_assertions::assert_eq;

    use super::ResourceUsage;
    use crate::{ffi::RusageInfoV6, sysctl::zeroed, time::Timebase};

    #[test]
    fn every_field_is_decoded() {
        let mut raw = zeroed::<RusageInfoV6>();
        raw.ri_user_time = 24_000_000;
        raw.ri_system_time = 2_400_000;
        raw.ri_runnable_time = 240_000;
        raw.ri_pkg_idle_wkups = 1;
        raw.ri_interrupt_wkups = 2;
        raw.ri_pageins = 3;
        raw.ri_wired_size = 4;
        raw.ri_resident_size = 5;
        raw.ri_phys_footprint = 6;
        raw.ri_lifetime_max_phys_footprint = 7;
        raw.ri_diskio_bytesread = 8;
        raw.ri_diskio_byteswritten = 9;
        raw.ri_logical_writes = 10;
        raw.ri_instructions = 11;
        raw.ri_cycles = 12;
        raw.ri_energy_nj = 13;

        assert_eq!(
            ResourceUsage::decode(&raw, Timebase::new(125, 3)),
            ResourceUsage {
                user_time: Duration::from_secs(1),
                system_time: Duration::from_millis(100),
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
        );
    }
}
