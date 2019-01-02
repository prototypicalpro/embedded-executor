#![feature(futures_api, const_fn, arbitrary_self_types)]
#![feature(async_await, await_macro)]
#![cfg_attr(not(test), no_std)]
#![cfg_attr(feature = "alloc", feature(alloc))]

#[cfg(feature = "alloc")]
extern crate alloc;

mod prelude {
    #[cfg(feature = "alloc")]
    pub use alloc::prelude::*;
    pub use futures::prelude::*;
}

pub mod core_traits;

#[cfg(feature = "alloc")]
pub mod alloc_exec;
