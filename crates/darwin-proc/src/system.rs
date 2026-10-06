// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    io,
    sync::Mutex,
    time::{Duration, SystemTime},
};

use crate::{
    Call, Error, ffi,
    sysctl::{page_size, sysctl_value, zeroed},
    time::Timebase,
};

/// The system as a whole. Any user can read its statistics.
#[derive(Clone, Copy, Debug)]
pub struct Host;

/// The load averages over the last one, five and fifteen minutes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoadAverage {
    /// The average over the last minute.
    pub one_minute: f64,
    /// The average over the last five minutes.
    pub five_minutes: f64,
    /// The average over the last fifteen minutes.
    pub fifteen_minutes: f64,
}

/// The system's physical memory, in bytes. `total` comes from `hw.memsize`
/// and the other fields from `HOST_VM_INFO64`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Memory {
    /// The installed memory, from `hw.memsize`.
    pub total: u64,
    /// Memory that nothing uses. As in `vm_stat`, this excludes the
    /// speculative pages, which the kernel's free count includes.
    pub free: u64,
    /// Memory in use that was accessed recently.
    pub active: u64,
    /// Memory in use that was not accessed recently.
    pub inactive: u64,
    /// Memory that the kernel read ahead of use. The kernel can free it at
    /// once.
    pub speculative: u64,
    /// Memory on the kernel's throttled page queue.
    pub throttled: u64,
    /// Memory that the kernel cannot page out.
    pub wired: u64,
    /// Memory with contents that its owners have marked as discardable.
    pub purgeable: u64,
    /// Memory that caches files.
    pub file_backed: u64,
    /// Memory that does not cache a file, such as heaps and stacks.
    pub anonymous: u64,
    /// Memory that the compressor occupies.
    pub compressor: u64,
    /// The size of the data in the compressor before compression.
    pub compressed: u64,
}

/// The swap space, in bytes, from `vm.swapusage`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Swap {
    /// The size of the swap files.
    pub total: u64,
    /// The part of the swap files in use.
    pub used: u64,
    /// The part of the swap files not in use.
    pub available: u64,
    /// Whether the swap files are encrypted.
    pub encrypted: bool,
}

/// The time that one processor has spent in each state since boot, in
/// scheduler ticks. The kernel keeps 32-bit counters, which wrap.
///
/// Darwin counts all user-mode time as `user`, whatever the thread's
/// priority, and reports 0 for `nice`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessorTicks {
    /// Running user code.
    pub user: u32,
    /// Running kernel code.
    pub system: u32,
    /// Idle.
    pub idle: u32,
    /// Running user code at lowered priority. Darwin always reports 0.
    pub nice: u32,
}

/// The numbers of tasks and threads in the system, from
/// `PROCESSOR_SET_LOAD_INFO`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskTotals {
    /// The number of tasks, including `kernel_task`.
    pub tasks: usize,
    /// The number of threads in all tasks.
    pub threads: usize,
}

/// One user reference to a send right. Dropping the value releases the
/// reference.
struct SendRight(libc::mach_port_t);

impl SendRight {
    /// A send right to the host port.
    fn host() -> Self {
        // SAFETY: `mach_host_self` takes no arguments and returns a send right
        // that the caller owns.
        Self(unsafe { ffi::mach_host_self() })
    }
}

impl Drop for SendRight {
    fn drop(&mut self) {
        // SAFETY: this value owns one reference to the send right, and
        // `task_self` returns this task's own port.
        unsafe { ffi::mach_port_deallocate(task_self(), self.0) };
    }
}

/// The port of the calling task.
fn task_self() -> libc::mach_port_t {
    // SAFETY: the C library sets `mach_task_self_` before `main` runs and
    // again in a forked child, and reading it is what the
    // `mach_task_self()` macro does.
    unsafe { ffi::mach_task_self_ }
}

/// Converts a `kern_return_t` from `call` into a result.
fn mach_result(call: Call, code: libc::kern_return_t) -> Result<(), Error> {
    if code == libc::KERN_SUCCESS {
        return Ok(());
    }

    Err(Error::Mach { call, code })
}

impl Memory {
    /// Decodes `raw`, whose counts are in pages of `page_size` bytes. `total`
    /// is the installed memory.
    pub(crate) fn decode(raw: &libc::vm_statistics64, total: u64, page_size: u64) -> Self {
        let bytes = |pages: u64| pages * page_size;

        Self {
            total,
            free: bytes(raw.free_count.saturating_sub(raw.speculative_count).into()),
            active: bytes(raw.active_count.into()),
            inactive: bytes(raw.inactive_count.into()),
            speculative: bytes(raw.speculative_count.into()),
            throttled: bytes(raw.throttled_count.into()),
            wired: bytes(raw.wire_count.into()),
            purgeable: bytes(raw.purgeable_count.into()),
            file_backed: bytes(raw.external_page_count.into()),
            anonymous: bytes(raw.internal_page_count.into()),
            compressor: bytes(raw.compressor_page_count.into()),
            compressed: bytes(raw.total_uncompressed_pages_in_compressor),
        }
    }
}

impl From<&libc::xsw_usage> for Swap {
    fn from(raw: &libc::xsw_usage) -> Self {
        Self {
            total: raw.xsu_total,
            used: raw.xsu_used,
            available: raw.xsu_avail,
            encrypted: raw.xsu_encrypted != 0,
        }
    }
}

impl From<[libc::integer_t; libc::CPU_STATE_MAX as usize]> for ProcessorTicks {
    /// Converts the ticks in the order of the `CPU_STATE_*` constants.
    fn from(ticks: [libc::integer_t; libc::CPU_STATE_MAX as usize]) -> Self {
        let [user, system, idle, nice] = ticks.map(libc::integer_t::cast_unsigned);

        Self {
            user,
            system,
            idle,
            nice,
        }
    }
}

impl From<&libc::processor_set_load_info> for TaskTotals {
    fn from(raw: &libc::processor_set_load_info) -> Self {
        Self {
            tasks: usize::try_from(raw.task_count).unwrap_or(0),
            threads: usize::try_from(raw.thread_count).unwrap_or(0),
        }
    }
}

/// Serialises the `getutxent` calls, which share the C library's position in
/// the login records.
static LOGIN_RECORDS: Mutex<()> = Mutex::new(());

impl Host {
    /// The time that the system booted.
    ///
    /// ```
    /// use std::time::SystemTime;
    ///
    /// use darwin_proc::Host;
    ///
    /// assert!(Host::boot_time()? < SystemTime::now());
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Os`] if the `kern.boottime` sysctl fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn boot_time() -> Result<SystemTime, Error> {
        let boot = sysctl_value::<libc::timeval>(c"kern.boottime").map_err(|source| Error::Os {
            call: Call::BootTime,
            source,
        })?;

        Ok(SystemTime::UNIX_EPOCH
            + Duration::from_secs(boot.tv_sec.unsigned_abs())
            + Duration::from_micros(boot.tv_usec.unsigned_abs().into()))
    }

    /// The time since the system booted, including time asleep, from
    /// `mach_continuous_time`. It never decreases, even when the clock is set,
    /// so the difference between two readings is the time between them.
    ///
    /// ```
    /// use darwin_proc::Host;
    ///
    /// assert!(Host::uptime()? <= Host::uptime()?);
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Os`] if `mach_timebase_info` fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn uptime() -> Result<Duration, Error> {
        let timebase = Timebase::current().map_err(|source| Error::Os {
            call: Call::Uptime,
            source,
        })?;

        // SAFETY: `mach_continuous_time` takes no arguments and only returns a
        // count.
        let ticks = unsafe { ffi::mach_continuous_time() };

        Ok(timebase.duration(ticks))
    }

    /// The load averages.
    ///
    /// ```
    /// use darwin_proc::Host;
    ///
    /// assert!(Host::load_average()?.one_minute >= 0.0);
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Os`] if `getloadavg` fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn load_average() -> Result<LoadAverage, Error> {
        let mut averages = [0.0; 3];

        // SAFETY: `averages` has room for the 3 values that the call asks for.
        let count = unsafe { libc::getloadavg(averages.as_mut_ptr(), 3) };

        if count != 3 {
            return Err(Error::Os {
                call: Call::LoadAverage,
                source: io::Error::other(format!("returned {count} averages, not 3")),
            });
        }

        let [one_minute, five_minutes, fifteen_minutes] = averages;

        Ok(LoadAverage {
            one_minute,
            five_minutes,
            fifteen_minutes,
        })
    }

    /// The physical memory.
    ///
    /// The kernel shares a budget of 10 requests a second among all the
    /// programs that are not part of macOS, and answers any further requests
    /// in the same second from a cache, so the counts can be up to a second
    /// old.
    ///
    /// ```
    /// use darwin_proc::Host;
    ///
    /// let memory = Host::memory()?;
    ///
    /// assert!(memory.free < memory.total);
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Os`] if the `hw.memsize` or `hw.pagesize` sysctl
    /// fails, or [`Error::Mach`] if `host_statistics64` fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn memory() -> Result<Memory, Error> {
        let total = sysctl_value::<u64>(c"hw.memsize").map_err(|source| Error::Os {
            call: Call::MemorySize,
            source,
        })?;
        let page_size = page_size().map_err(|source| Error::Os {
            call: Call::PageSize,
            source,
        })?;
        let host = SendRight::host();
        let mut statistics = zeroed::<libc::vm_statistics64>();
        let mut count = libc::HOST_VM_INFO64_COUNT;

        // SAFETY: `statistics` has room for `count` integers, and the kernel
        // writes at most that many and sets `count` to the number of integers
        // that it wrote.
        let code = unsafe {
            libc::host_statistics64(
                host.0,
                libc::HOST_VM_INFO64,
                (&raw mut statistics).cast(),
                &raw mut count,
            )
        };

        mach_result(Call::MemoryStatistics, code)?;

        Ok(Memory::decode(&statistics, total, page_size))
    }

    /// The swap space.
    ///
    /// ```
    /// use darwin_proc::Host;
    ///
    /// let swap = Host::swap()?;
    ///
    /// assert!(swap.used <= swap.total);
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Os`] if the `vm.swapusage` sysctl fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn swap() -> Result<Swap, Error> {
        let usage =
            sysctl_value::<libc::xsw_usage>(c"vm.swapusage").map_err(|source| Error::Os {
                call: Call::Swap,
                source,
            })?;

        Ok(Swap::from(&usage))
    }

    /// The ticks of each processor, in the order of the processor numbers.
    ///
    /// ```
    /// use darwin_proc::Host;
    ///
    /// assert!(!Host::processors()?.is_empty());
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Mach`] if `host_processor_info` fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn processors() -> Result<Vec<ProcessorTicks>, Error> {
        let host = SendRight::host();
        let mut processors: libc::natural_t = 0;
        let mut info: libc::processor_info_array_t = std::ptr::null_mut();
        let mut count: libc::mach_msg_type_number_t = 0;

        // SAFETY: the three pointers refer to writable locals. On success the
        // kernel maps an array of `count` integers into this task and stores
        // its address in `info`.
        let code = unsafe {
            libc::host_processor_info(
                host.0,
                libc::PROCESSOR_CPU_LOAD_INFO,
                &raw mut processors,
                &raw mut info,
                &raw mut count,
            )
        };

        mach_result(Call::Processors, code)?;

        let length = usize::try_from(count).unwrap_or(0);

        // SAFETY: `host_processor_info` succeeded, so `info` points to `count`
        // initialised integers, which stay mapped until the `vm_deallocate`
        // below.
        let integers = unsafe { std::slice::from_raw_parts(info, length) };
        let (processors, _) = integers.as_chunks::<{ libc::CPU_STATE_MAX as usize }>();
        let ticks = processors
            .iter()
            .copied()
            .map(ProcessorTicks::from)
            .collect();

        // SAFETY: the kernel mapped `info` into this task for this call, and
        // nothing refers to it after `ticks` is built.
        unsafe {
            libc::vm_deallocate(
                task_self(),
                info as libc::vm_address_t,
                std::mem::size_of_val(integers),
            );
        }

        Ok(ticks)
    }

    /// The numbers of tasks and threads.
    ///
    /// ```
    /// use darwin_proc::Host;
    ///
    /// let totals = Host::task_totals()?;
    ///
    /// assert!(totals.threads >= totals.tasks);
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Mach`] if `processor_set_default` or
    /// `processor_set_statistics` fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn task_totals() -> Result<TaskTotals, Error> {
        let host = SendRight::host();
        let mut set: libc::mach_port_t = 0;

        // SAFETY: `set` is a writable local for the port name.
        let code = unsafe { ffi::processor_set_default(host.0, &raw mut set) };

        mach_result(Call::DefaultProcessorSet, code)?;

        let set = SendRight(set);
        let mut load = zeroed::<libc::processor_set_load_info>();
        let mut count = ffi::PROCESSOR_SET_LOAD_INFO_COUNT;

        // SAFETY: `load` has room for `count` integers, and the kernel writes
        // at most that many.
        let code = unsafe {
            ffi::processor_set_statistics(
                set.0,
                libc::PROCESSOR_SET_LOAD_INFO,
                (&raw mut load).cast(),
                &raw mut count,
            )
        };

        mach_result(Call::TaskTotals, code)?;

        Ok(TaskTotals::from(&load))
    }

    /// The number of login sessions in the login records, as `who` lists
    /// them.
    ///
    /// ```
    /// use darwin_proc::Host;
    ///
    /// let sessions = Host::logged_in_users();
    /// ```
    #[must_use]
    #[tracing::instrument(level = "debug")]
    pub fn logged_in_users() -> usize {
        let _guard = LOGIN_RECORDS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut sessions = 0;

        // SAFETY: `setutxent` rewinds the login records, and the lock keeps
        // other threads in this crate from moving the position.
        unsafe { libc::setutxent() };

        loop {
            // SAFETY: `getutxent` returns null at the end of the records, or a
            // pointer to a record that stays valid until the next call.
            let record = unsafe { libc::getutxent() };

            if record.is_null() {
                break;
            }

            // SAFETY: `record` is not null, so it points to a valid record.
            let record = unsafe { &*record };

            if record.ut_type == libc::USER_PROCESS && record.ut_user[0] != 0 {
                sessions += 1;
            }
        }

        // SAFETY: `endutxent` closes the login records.
        unsafe { libc::endutxent() };

        sessions
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::{Memory, ProcessorTicks, Swap, TaskTotals};
    use crate::sysctl::zeroed;

    #[test]
    fn every_memory_field_is_decoded() {
        let mut raw = zeroed::<libc::vm_statistics64>();
        raw.free_count = 5;
        raw.active_count = 2;
        raw.inactive_count = 3;
        raw.speculative_count = 4;
        raw.throttled_count = 5;
        raw.wire_count = 6;
        raw.purgeable_count = 7;
        raw.external_page_count = 8;
        raw.internal_page_count = 9;
        raw.compressor_page_count = 10;
        raw.total_uncompressed_pages_in_compressor = 11;

        assert_eq!(
            Memory::decode(&raw, 1 << 30, 16_384),
            Memory {
                total: 1 << 30,
                free: 16_384,
                active: 32_768,
                inactive: 49_152,
                speculative: 65_536,
                throttled: 81_920,
                wired: 98_304,
                purgeable: 114_688,
                file_backed: 131_072,
                anonymous: 147_456,
                compressor: 163_840,
                compressed: 180_224,
            }
        );
    }

    #[test]
    fn every_swap_field_is_decoded() {
        let mut raw = zeroed::<libc::xsw_usage>();
        raw.xsu_total = 3 << 30;
        raw.xsu_used = 1 << 30;
        raw.xsu_avail = 2 << 30;
        raw.xsu_encrypted = 1;

        assert_eq!(
            Swap::from(&raw),
            Swap {
                total: 3 << 30,
                used: 1 << 30,
                available: 2 << 30,
                encrypted: true,
            }
        );
    }

    #[test]
    fn ticks_follow_the_cpu_state_order() {
        assert_eq!(
            ProcessorTicks::from([1, 2, 3, -1]),
            ProcessorTicks {
                user: 1,
                system: 2,
                idle: 3,
                nice: u32::MAX,
            }
        );
    }

    #[test]
    fn negative_task_totals_become_zero() {
        let mut raw = zeroed::<libc::processor_set_load_info>();
        raw.task_count = -1;
        raw.thread_count = 7;

        assert_eq!(
            TaskTotals::from(&raw),
            TaskTotals {
                tasks: 0,
                threads: 7,
            }
        );
    }
}
