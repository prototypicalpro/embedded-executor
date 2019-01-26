#![feature(futures_api)]
#![feature(generators, proc_macro_hygiene)]
#![cfg_attr(not(test), no_std)]
#![cfg_attr(feature = "alloc", feature(alloc))]

#[cfg(feature = "alloc")]
extern crate alloc;

mod prelude {
    #[cfg(feature = "alloc")]
    pub use alloc::prelude::*;
    pub use futures::prelude::*;
}

mod sleep;
pub use sleep::*;

#[cfg(feature = "alloc")]
pub mod alloc_executor;
#[cfg(feature = "alloc")]
pub use alloc_executor::AllocExecutor;

#[cfg(feature = "alloc")]
mod future_box;
