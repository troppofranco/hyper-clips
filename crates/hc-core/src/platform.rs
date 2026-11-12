//! Interfaces the OS-specific crates implement (`hc-os-win` today; a Linux
//! crate could implement the same traits later).

use crate::error::Result;

/// Process memory figures in KiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryUsage {
    /// Private committed bytes (`PrivateUsage` on Windows). This is NOT the
    /// "private working set" the < 60 MB target is defined on; measure that with
    /// `scripts/measure-memory.ps1`.
    pub private_kb: u64,
    pub working_set_kb: u64,
}

pub trait MemoryProbe {
    fn current_process_memory(&self) -> Option<MemoryUsage>;
}

/// Opens a folder or file in the system file manager / default app.
pub trait Shell {
    fn open_path(&self, path: &std::path::Path) -> Result<()>;
}
