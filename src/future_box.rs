//! Workaround until `futures-preview` provides `no_std` `alloc` support.

use futures::{
    future::UnsafeFutureObj,
    prelude::*,
};

#[cfg(feature = "alloc")]
mod workaround {
    use core::{
        pin::Pin,
        task::{
            Poll,
            Waker,
        },
    };

    use alloc::prelude::*;

    use super::*;

    struct FutureBox<F>(Box<F>);

    unsafe impl<'a, T, F> UnsafeFutureObj<'a, T> for FutureBox<F>
    where
        F: Future<Output = T> + 'a,
    {
        fn into_raw(self) -> *mut () {
            Box::into_raw(self.0) as *mut ()
        }

        unsafe fn poll(ptr: *mut (), lw: &Waker) -> Poll<T> {
            let ptr = ptr as *mut F;
            let pin: Pin<&mut F> = Pin::new_unchecked(&mut *ptr);
            F::poll(pin, lw)
        }

        unsafe fn drop(ptr: *mut ()) {
            drop(Box::from_raw(ptr as *mut F))
        }
    }

    pub fn make_obj<'a, F, T>(future: F) -> impl UnsafeFutureObj<'a, T> + Send
    where
        F: Future<Output = T> + Send + 'a,
    {
        FutureBox(Box::new(future))
    }

    pub fn make_local<'a, F, T>(future: F) -> impl UnsafeFutureObj<'a, T>
    where
        F: Future<Output = T> + 'a,
    {
        FutureBox(Box::new(future))
    }
}

#[cfg(feature = "std")]
mod workaround {
    use super::*;

    pub fn make_obj<'a, F, T>(future: F) -> impl UnsafeFutureObj<'a, T> + Send
    where
        F: Future<Output = T> + Send + 'a,
    {
        Box::new(future)
    }

    pub fn make_local<'a, F, T>(future: F) -> impl UnsafeFutureObj<'a, T>
    where
        F: Future<Output = T> + 'a,
    {
        Box::new(future)
    }
}

pub use self::workaround::*;
