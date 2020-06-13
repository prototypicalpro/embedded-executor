//! # Embedded Futures Executor
//!
//! This crate provides a futures executor whose only dependency is `alloc`.
//!
//! ## Dependency Setup
//!
//! To make https://docs.rs/ happy, this crate uses `std` by default. For use
//! on embedded systems, disable default features and enable the `alloc` feature:
//!
//! ```toml
//! [dependencies.embedded-executor]
//! version = "0.2.2"
//! default-features = false
//! features = ["alloc"]
//! ```
//!
//! ## The Executor
//!
//! The base executor type is [`AllocExecutor`]. To use it, you need both a
//! [`RawMutex`] implementation and a [`Sleep`] implementation.
//!
//! For convenience, [`SpinSleep`] provides a simple spinlock-based [`Sleep`]
//! implementation, and an example spinlock [`RawMutex`] can be found in the
//! [`lock_api`] docs. It is recommended, however, to use implementations more
//! suited to your particular platform, such as a `Sleep` that calls
//! [`cortex_m::asm::wfi`] and a `RawMutex` that disables/enables interrupts.
//!
//! [`RawMutex`]: https://docs.rs/lock_api/0.1.5/lock_api/trait.RawMutex.html
//! [`lock_api`]: https://docs.rs/lock_api/0.1.5/lock_api/
//! [`cortex_m::asm::wfi`]: https://docs.rs/cortex-m/0.5.8/cortex_m/asm/fn.wfi.html
//!
//! Once you have all of these pieces, it's usually easiest to create an alias for
//! your platform-specific executor:
//!
//! ```rust,ignore
//! type Executor<'a> = AllocExecutor<'a, IFreeMutex, WFISleep>;
//! ```
//!
//! which can then be instantiated via its `new` or `with_capacity` methods.
//!
//! ## Spawning
//!
//! Futures can be spawned either by calling [`AllocExecutor::spawn`] or by
//! getting a [`alloc_executor::Spawner`] and calling
//! [`alloc_executor::Spawner::spawn`]. The `Spawner` can also be passed to
//! other `Futures` for even more spawning goodness.
//!
//! ## Running
//!
//! Running the executor is done via the [`AllocExecutor::run`] method. This
//! will drive all spawned futures and any new futures that get spawned to
//! completion before returning.

#![recursion_limit = "128"]
#![feature(generators, proc_macro_hygiene, alloc_prelude, cfg_target_has_atomic)]
#![cfg_attr(test, feature(async_await))]
#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
#![warn(missing_docs)]

extern crate alloc;

mod sleep;
pub use self::sleep::*;

#[cfg(any(feature = "alloc", feature = "std"))]
pub mod alloc_executor;
#[cfg(any(feature = "alloc", feature = "std"))]
pub use self::alloc_executor::inner::AllocExecutor;

#[cfg(any(feature = "alloc", feature = "std"))]
mod future_box;

mod wake;
pub use self::wake::*;
