pub use cooked_waker::{ WakeRef, Wake };
use archery::{ SharedPointer, SharedPointerKind };

pub struct SP<T, P: SharedPointerKind> (pub SharedPointer<T, P>);

impl<T: WakeRef + Sized, P: SharedPointerKind> WakeRef for SP<T, P> {
    #[inline]
    fn wake_by_ref(&self) {
        T::wake_by_ref(self.0.as_ref())
    }
}

impl<T: Wake + Sized, P: SharedPointerKind> Wake for SP<T, P> {}

impl<T: WakeRef + Sized, P: SharedPointerKind> Clone for SP<T, P> {
    fn clone(&self) -> Self {
        SP(self.0.clone())
    }
}

unsafe impl<T: WakeRef + Sized, P: SharedPointerKind> Send for SP<T, P> {}
unsafe impl<T: WakeRef + Sized, P: SharedPointerKind> Sync for SP<T, P> {}
unsafe impl<T: WakeRef + Sized, P: SharedPointerKind> stowaway::Stowable for SP<T, P> {}

/*
use core::task::{
    RawWaker,
    Waker,
};

// WARNING: This should be Arc!
#[cfg(any(feature = "alloc", feature = "std"))]
use archery::SharedPointer;

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
    fn into_raw_waker(self: Rc<Self>) -> RawWaker {
        RawWaker::new(Rc::into_raw(self) as _, arc::arc_waker_vtable::<Self>())
    }

    fn into_waker(self: Rc<Self>) -> Waker {
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
        let arc = Rc::<T>::from_raw(ptr as _);
        let cloned = arc.clone();
        mem::forget(arc);
        cloned.into_raw_waker()
    }

    pub unsafe fn drop_arc_waker_raw<T>(ptr: *const ()) {
        drop(Rc::<T>::from_raw(ptr as _));
    }

    pub unsafe fn wake_arc_waker_raw<T: Wake>(ptr: *const ()) {
        let arc = Rc::<T>::from_raw(ptr as _);
        arc.wake();
    }

    pub unsafe fn wake_by_ref_arc_waker_raw<T: Wake>(ptr: *const ()) {
        let arc = Rc::<T>::from_raw(ptr as _);
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
*/