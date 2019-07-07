use core::task::{
    RawWaker,
    Waker,
};

#[cfg(any(feature = "alloc", feature = "std"))]
use alloc::sync::Arc;

/// Wake trait
///
/// Should be combined with [`crate::Sleep`] in order to be used with the
/// [`crate::AllocExecutor`].
pub trait Wake {
    /// Wake up the sleepers.
    fn wake(&self);
}

#[cfg(any(feature = "alloc", feature = "std"))]
pub(crate) trait WakeExt: Wake + Sized {
    fn into_raw_waker(self: Arc<Self>) -> RawWaker {
        RawWaker::new(Arc::into_raw(self) as _, arc::arc_waker_vtable::<Self>())
    }

    fn into_waker(self: Arc<Self>) -> Waker {
        unsafe { Waker::from_raw(self.into_raw_waker()) }
    }
}

#[cfg(any(feature = "alloc", feature = "std"))]
impl<T> WakeExt for T where T: Sized + Wake {}

#[cfg(any(feature = "alloc", feature = "std"))]
mod arc {
    use core::{
        mem,
        task::RawWakerVTable,
    };

    use super::*;

    pub unsafe fn clone_arc_waker_raw<T: Wake>(ptr: *const ()) -> RawWaker {
        let arc = Arc::<T>::from_raw(ptr as _);
        let cloned = arc.clone();
        mem::forget(arc);
        cloned.into_raw_waker()
    }

    pub unsafe fn drop_arc_waker_raw<T>(ptr: *const ()) {
        drop(Arc::<T>::from_raw(ptr as _));
    }

    pub unsafe fn wake_arc_waker_raw<T: Wake>(ptr: *const ()) {
        let arc = Arc::<T>::from_raw(ptr as _);
        arc.wake();
    }

    pub unsafe fn wake_by_ref_arc_waker_raw<T: Wake>(ptr: *const ()) {
        let arc = Arc::<T>::from_raw(ptr as _);
        arc.wake();
        mem::forget(arc);
    }

    pub fn arc_waker_vtable<T: Wake>() -> &'static RawWakerVTable {
        &RawWakerVTable::new(
            clone_arc_waker_raw::<T>,
            wake_arc_waker_raw::<T>,
            wake_by_ref_arc_waker_raw::<T>,
            drop_arc_waker_raw::<T>,
        )
    }

}
