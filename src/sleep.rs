//! Abstraction over platform-specific thread sleeping mechanisms.

/// Platform-agnostic Sleep trait
///
/// Provides a mechanism to initialize a handle that can be used to either sleep
/// or wake the current thread.
///
/// Intended usage is to create the sleep handle via the `Default` constructor,
/// arrange for `wake` to be called on it when an event occurs, and then call
/// `sleep`. The current thread should then be put into a sleep/low
/// power/otherwise yielded state until `wake` gets called elsewhere.
pub trait Sleep: Default + Clone + Send + Sync + 'static {
    /// Put the current thread to sleep until `Sleep::wake` is called.
    fn sleep(&self);

    /// Wake up sleeping threads
    ///
    /// Currently unspecified as to whether this will wake up all, one, or some
    /// of potentially multiple sleeping threads.
    fn wake(&self);
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

        fn wake(&self) {
            self.0.store(true, Release);
        }
    }
}

#[cfg(any(feature = "std", feature = "alloc"))]
pub use self::provided::*;
