//! Real-time safety helpers (FTZ/DAZ denormal guard and RT assertions).

/// RAII guard that enables Flush-to-Zero (FTZ) and Denormals-Are-Zero (DAZ)
/// in the `x86_64` MXCSR register and restores the previous value on drop.
///
/// On non-`x86_64` targets this is a zero-sized no-op.
pub struct DenormalGuard {
    #[cfg(target_arch = "x86_64")]
    saved_mxcsr: u32,
}

impl DenormalGuard {
    /// Enables FTZ and DAZ on the current thread and returns a guard that
    /// restores the original MXCSR state when dropped.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            const FTZ_BIT: u32 = 1 << 15;
            const DAZ_BIT: u32 = 1 << 6;
            let mut saved: u32 = 0;
            // SAFETY: `stmxcsr` stores the 32-bit MXCSR register into `saved` (SSE2 baseline on x86_64).
            unsafe {
                core::arch::asm!(
                    "stmxcsr [{}]",
                    in(reg) &raw mut saved,
                    options(nostack, preserves_flags)
                );
            }
            let updated = saved | FTZ_BIT | DAZ_BIT;
            // SAFETY: `ldmxcsr` loads the 32-bit MXCSR register from `updated` (SSE2 baseline on x86_64).
            unsafe {
                core::arch::asm!(
                    "ldmxcsr [{}]",
                    in(reg) &raw const updated,
                    options(nostack, preserves_flags)
                );
            }
            Self { saved_mxcsr: saved }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            Self {}
        }
    }
}

impl Default for DenormalGuard {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for DenormalGuard {
    #[inline]
    fn drop(&mut self) {
        #[cfg(target_arch = "x86_64")]
        {
            let saved = self.saved_mxcsr;
            // SAFETY: Restores the thread-local MXCSR value saved in `DenormalGuard::new()`.
            unsafe {
                core::arch::asm!(
                    "ldmxcsr [{}]",
                    in(reg) &raw const saved,
                    options(nostack, preserves_flags)
                );
            }
        }
    }
}

/// Marks a real-time audio path in debug builds.
#[macro_export]
macro_rules! debug_assert_rt {
    () => {
        #[cfg(debug_assertions)]
        {
            // Hook point for debug-mode RT thread verification.
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_arch = "x86_64")]
    fn read_mxcsr() -> u32 {
        let mut val: u32 = 0;
        // SAFETY: `stmxcsr` stores the 32-bit MXCSR register into `val` (SSE2 baseline on x86_64).
        unsafe {
            core::arch::asm!(
                "stmxcsr [{}]",
                in(reg) &raw mut val,
                options(nostack, preserves_flags)
            );
        }
        val
    }

    #[test]
    fn denormal_guard_sets_and_restores_mxcsr() {
        #[cfg(target_arch = "x86_64")]
        {
            let before = read_mxcsr();
            {
                let _guard = DenormalGuard::new();
                let inside = read_mxcsr();
                assert_eq!(inside & ((1 << 15) | (1 << 6)), (1 << 15) | (1 << 6));
            }
            let after = read_mxcsr();
            assert_eq!(before, after);
        }
    }
}
