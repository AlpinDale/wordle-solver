use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::ffi::CStr;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use darwin_kperf_sys::{
    kperf::{self, KPC_CLASS_FIXED_MASK, KPC_MAX_COUNTERS},
    load::LibraryHandle,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PerfClock {
    DarwinFixedCounters,
    StdInstant,
}

#[derive(Clone, Copy, Debug)]
pub struct PerfMeasurement {
    nanos: u64,
    cycles: Option<u64>,
    instructions: Option<u64>,
    clock: PerfClock,
}

impl PerfMeasurement {
    pub fn ticks(self) -> u64 {
        self.cycles.unwrap_or(self.nanos)
    }

    pub fn nanos(self) -> u64 {
        self.nanos
    }

    pub fn cycles(self) -> Option<u64> {
        self.cycles
    }

    pub fn instructions(self) -> Option<u64> {
        self.instructions
    }

    pub fn duration(self) -> Duration {
        Duration::from_nanos(self.nanos)
    }

    pub fn clock(self) -> PerfClock {
        self.clock
    }

    pub fn tick_label(self) -> &'static str {
        match self.clock {
            PerfClock::DarwinFixedCounters => "cycles",
            PerfClock::StdInstant => "ticks",
        }
    }

    fn from_instant(start: Instant) -> Self {
        Self {
            nanos: elapsed_nanos(start),
            cycles: None,
            instructions: None,
            clock: PerfClock::StdInstant,
        }
    }
}

#[derive(Debug)]
pub struct PerfTimer {
    start: PerfTimerStart,
}

#[derive(Debug)]
enum PerfTimerStart {
    StdInstant {
        wall_start: Instant,
    },
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    DarwinFixedCounters {
        wall_start: Instant,
        start_cycles: u64,
        start_instructions: u64,
        _guard: std::sync::MutexGuard<'static, ()>,
    },
}

impl PerfTimer {
    pub fn start() -> Self {
        Self {
            start: PerfTimerStart::new(),
        }
    }

    pub fn stop(self) -> PerfMeasurement {
        self.start.stop()
    }

    pub fn measure<F, T>(f: F) -> (PerfMeasurement, T)
    where
        F: FnOnce() -> T,
    {
        let timer = Self::start();
        let value = f();
        (timer.stop(), value)
    }

    pub fn hardware_cycles_supported() -> bool {
        PerfTimerStart::hardware_cycles_supported()
    }

    pub fn hardware_cycles_status() -> &'static str {
        PerfTimerStart::hardware_cycles_status()
    }
}

impl PerfTimerStart {
    fn new() -> Self {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if let Some(timer) = Self::try_fixed_counters() {
            return timer;
        }

        Self::StdInstant {
            wall_start: Instant::now(),
        }
    }

    fn stop(self) -> PerfMeasurement {
        match self {
            Self::StdInstant { wall_start } => PerfMeasurement::from_instant(wall_start),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Self::DarwinFixedCounters {
                wall_start,
                start_cycles,
                start_instructions,
                _guard,
            } => {
                let nanos = elapsed_nanos(wall_start);
                let sample = read_fixed_counters();
                let _ = stop_fixed_counting();
                match sample {
                    Some((end_instructions, end_cycles)) => PerfMeasurement {
                        nanos,
                        cycles: Some(end_cycles.saturating_sub(start_cycles)),
                        instructions: Some(end_instructions.saturating_sub(start_instructions)),
                        clock: PerfClock::DarwinFixedCounters,
                    },
                    None => PerfMeasurement::from_instant(wall_start),
                }
            }
        }
    }

    fn hardware_cycles_supported() -> bool {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            kperf_state().is_some()
        }

        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            false
        }
    }

    fn hardware_cycles_status() -> &'static str {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            match kperf_state_result() {
                Ok(_) => "available",
                Err(reason) => reason,
            }
        }

        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            "unsupported platform"
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn try_fixed_counters() -> Option<Self> {
        let _state = kperf_state()?;
        let guard = kperf_lock().lock().ok()?;
        start_fixed_counting()?;
        let (start_instructions, start_cycles) = read_fixed_counters()?;
        Some(Self::DarwinFixedCounters {
            wall_start: Instant::now(),
            start_cycles,
            start_instructions,
            _guard: guard,
        })
    }
}

fn elapsed_nanos(start: Instant) -> u64 {
    let nanos = start.elapsed().as_nanos();
    u64::try_from(nanos).unwrap_or(u64::MAX)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn kperf_lock() -> &'static Mutex<()> {
    static KPERF_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    KPERF_LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn kperf_state() -> Option<&'static KperfState> {
    kperf_state_result().ok()
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn kperf_state_result() -> Result<&'static KperfState, &'static str> {
    static KPERF_STATE: OnceLock<Result<KperfState, &'static str>> = OnceLock::new();
    KPERF_STATE
        .get_or_init(KperfState::new)
        .as_ref()
        .map_err(|err| *err)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn start_fixed_counting() -> Option<()> {
    let state = kperf_state()?;
    // SAFETY: the vtable function pointer was resolved from the loaded
    // framework and `KPC_CLASS_FIXED_MASK` is the documented fixed-counter
    // class mask.
    let start_counting = unsafe { (state.vtable.kpc_set_counting)(KPC_CLASS_FIXED_MASK) };
    if start_counting != 0 {
        return None;
    }

    // SAFETY: same as above; enabling thread counting with the fixed counter
    // class is the documented usage for per-thread fixed counters.
    let start_thread = unsafe { (state.vtable.kpc_set_thread_counting)(KPC_CLASS_FIXED_MASK) };
    if start_thread != 0 {
        // SAFETY: disabling counting by passing 0 is the documented shutdown
        // path for the kperf API.
        let _ = unsafe { (state.vtable.kpc_set_counting)(0) };
        return None;
    }

    Some(())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn stop_fixed_counting() -> Option<()> {
    let state = kperf_state()?;
    // SAFETY: disabling thread counting by passing 0 is the documented API
    // contract.
    let stop_thread = unsafe { (state.vtable.kpc_set_thread_counting)(0) };
    // SAFETY: disabling counting by passing 0 is the documented API contract.
    let stop_counting = unsafe { (state.vtable.kpc_set_counting)(0) };
    if stop_thread == 0 && stop_counting == 0 {
        Some(())
    } else {
        None
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn read_fixed_counters() -> Option<(u64, u64)> {
    let state = kperf_state()?;
    let mut counters = [0_u64; KPC_MAX_COUNTERS];
    // SAFETY: `counters` points to `KPC_MAX_COUNTERS` contiguous `u64`s, which
    // satisfies the API requirement for `buf_count`, and `tid=0` reads the
    // current thread.
    let result = unsafe {
        (state.vtable.kpc_get_thread_counters)(0, KPC_MAX_COUNTERS as u32, counters.as_mut_ptr())
    };
    if result != 0 {
        return None;
    }

    // On Apple Silicon, fixed counter slot 0 is retired instructions and
    // fixed counter slot 1 is elapsed cycles.
    Some((counters[0], counters[1]))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[derive(Debug)]
struct KperfState {
    _handle: LibraryHandle,
    vtable: kperf::VTable,
    saved_force_all: i32,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl KperfState {
    fn new() -> Result<Self, &'static str> {
        let handle = LibraryHandle::open(kperf_framework_path())
            .map_err(|_| "failed to load kperf.framework")?;
        let vtable = kperf::VTable::load(&handle).map_err(|_| "failed to resolve kperf symbols")?;

        let mut saved_force_all = 0_i32;
        // SAFETY: `saved_force_all` is a valid out-pointer for the sysctl read.
        let get_result = unsafe { (vtable.kpc_force_all_ctrs_get)(&mut saved_force_all) };
        if get_result != 0 {
            return Err("missing privileges for hardware counters");
        }

        // SAFETY: passing 1 requests force-acquisition of all counters, which
        // is the documented enable path for privileged callers.
        let set_result = unsafe { (vtable.kpc_force_all_ctrs_set)(1) };
        if set_result != 0 {
            return Err("failed to force-acquire hardware counters");
        }

        // SAFETY: the loaded function pointer is valid and the fixed class mask
        // is the documented selector for fixed counters.
        let fixed_count = unsafe { (vtable.kpc_get_counter_count)(KPC_CLASS_FIXED_MASK) };
        if fixed_count < 2 {
            // SAFETY: restores the previously observed force-all state.
            let _ = unsafe { (vtable.kpc_force_all_ctrs_set)(saved_force_all) };
            return Err("fixed cycle counters are unavailable");
        }

        Ok(Self {
            _handle: handle,
            vtable,
            saved_force_all,
        })
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl Drop for KperfState {
    fn drop(&mut self) {
        // SAFETY: restores the saved force-all state captured during init.
        let _ = unsafe { (self.vtable.kpc_force_all_ctrs_set)(self.saved_force_all) };
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn kperf_framework_path() -> &'static CStr {
    c"/System/Library/PrivateFrameworks/kperf.framework/kperf"
}
