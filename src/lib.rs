#![feature(
    futures_api,
    const_fn,
    arbitrary_self_types,
    generators,
    proc_macro_hygiene
)]
#![cfg_attr(not(test), no_std)]
#![cfg_attr(feature = "alloc", feature(alloc))]

#[cfg(feature = "alloc")]
extern crate alloc;

mod prelude {
    #[cfg(feature = "alloc")]
    pub use crate::future_box::make_obj;
    #[cfg(feature = "alloc")]
    pub use alloc::prelude::*;
    pub use futures::prelude::*;
}

pub mod core_traits;

#[cfg(feature = "alloc")]
pub mod alloc_exec;

#[cfg(feature = "alloc")]
mod future_box;
