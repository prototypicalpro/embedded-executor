//! Abstraction over platform-specific thread sleeping mechanisms.

/// Main Sleep trait
///
/// Provides a mechanism to create an `Alarm` to wait for.
///
/// Intended usage is to call `make_alarm` to create the `Alarm`, arrange for it
/// to be "rung" when some event requiring wakeup occurs, and then `sleep` on it.
///
/// Here, the `Alarm` will be used in the `Wake` implementation to bring the
/// event loop out of sleep.
pub trait Sleep {
    type Alarm: Alarm;

    /// Create an `Alarm` to wake up sleepers.
    fn make_alarm() -> Self::Alarm;
    /// Put the current thread to sleep until `Alarm::ring` is called.
    fn sleep(handle: &Self::Alarm);
}

/// Alarm trait for waking up sleepers
pub trait Alarm: Clone + Send + Sync + 'static {
    /// Wake up sleeping threads
    ///
    /// Currently unspecified as to whether this will wake up all, one, or some
    /// of potentially multiple sleeping threads.
    fn ring(&self);
}
