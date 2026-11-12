//! Own-process memory figures.

use hc_core::platform::{MemoryProbe, MemoryUsage};
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::GetCurrentProcess;

pub struct WinMemoryProbe;

impl MemoryProbe for WinMemoryProbe {
    fn current_process_memory(&self) -> Option<MemoryUsage> {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        counters.cb = size;
        // SAFETY: `counters` is a valid, correctly sized PROCESS_MEMORY_COUNTERS_EX;
        // the _EX struct begins with a PROCESS_MEMORY_COUNTERS, so the cast is the
        // documented way to request the extended data. The pseudo-handle needs no close.
        let result = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX)
                    .cast::<PROCESS_MEMORY_COUNTERS>(),
                size,
            )
        };
        result.ok()?;
        Some(MemoryUsage {
            private_kb: counters.PrivateUsage as u64 / 1024,
            working_set_kb: counters.WorkingSetSize as u64 / 1024,
        })
    }
}
