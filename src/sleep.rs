//! Abstraction over platform-specific thread sleeping mechanisms.

/// Platform-agnostic Sleep trait
///
/// Provides a mechanism that can be used to sleep the
/// current thread.
pub trait Sleep {
    /// Put the current thread to sleep.
    fn sleep(&self);
}

#[cfg(any(feature = "alloc", feature = "std"))]
mod provided {
    use super::*;

    use alloc::sync::Arc;

    use core::sync::atomic::{
        self,
        AtomicBool,
        Ordering::*,
    };

    use crate::Wake;

    /// Simple atomic spinlock sleep implementation.
    #[derive(Default, Clone, Debug)]
    pub struct SpinSleep(Arc<AtomicBool>);

    impl Sleep for SpinSleep {
        fn sleep(&self) {
            loop {
                if self.0.swap(false, Acquire) {
                    break;
                } else {
                    atomic::spin_loop_hint();
                }
            }
        }
    }

    impl Wake for SpinSleep {
        fn wake(&self) {
            self.0.store(true, Release)
        }
    }
}

#[cfg(any(feature = "std", feature = "alloc"))]
pub use self::provided::*;
