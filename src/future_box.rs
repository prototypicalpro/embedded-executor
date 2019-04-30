//! Workaround until `futures-preview` provides `no_std` `alloc` support.

use futures::{future::UnsafeFutureObj, prelude::*};

#[cfg(feature = "alloc")]
mod workaround {
    use alloc::prelude::v1::*;

    use super::*;

    struct FutureBox<F>(Box<F>);

    unsafe impl<'a, T, F> UnsafeFutureObj<'a, T> for FutureBox<F>
    where
        F: Future<Output = T> + 'a,
    {
        fn into_raw(self) -> *mut (dyn Future<Output = T> + 'a) {
            Box::into_raw(self.0 as Box<dyn Future<Output = T> + 'a>) as *mut _
        }

        unsafe fn drop(ptr: *mut (dyn Future<Output = T> + 'a)) {
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
