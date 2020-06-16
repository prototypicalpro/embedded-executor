//! Dynamic `alloc`-backed executor
//!

// Note: using an "inner" crate to avoid obvious "pub use ..." stuff in the docs.
pub(crate) mod inner;
pub use self::inner::*;
