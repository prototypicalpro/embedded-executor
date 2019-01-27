#![feature(futures_api)]
#![feature(generators, proc_macro_hygiene)]
#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
#![cfg_attr(feature = "alloc", feature(alloc))]

mod sleep;
pub use sleep::*;

#[cfg(any(feature = "alloc", feature = "std"))]
pub mod alloc_executor;
#[cfg(any(feature = "alloc", feature = "std"))]
pub use alloc_executor::AllocExecutor;

#[cfg(any(feature = "alloc", feature = "std"))]
mod future_box;
