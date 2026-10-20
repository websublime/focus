//! JavaScript-semantics primitives.
//!
//! The upstream engine (`awslabs/aidlc-workflows`, TypeScript on Bun) leans on
//! behaviour that Rust's standard library does not reproduce: what `\s` and
//! `String.prototype.trim` count as whitespace, how `Array.prototype.sort`
//! orders strings, how `localeCompare` collates, how `Object.keys` orders
//! integer-like keys, how `JSON.stringify(value, null, 2)` formats, how numbers
//! print and round, ECMAScript regular-expression matching, and what
//! `Date.parse` makes of a timestamp. Compiled artifacts are byte-compared
//! (`compile --check`), so each of those is part of the on-disk contract.
//!
//! This crate is the one owner of those rules. Port code calls in here instead
//! of reaching for the nearest `std` equivalent, which keeps every deliberate
//! divergence from upstream in one reviewable place.

pub mod date;
pub mod error;
pub mod glob;
pub mod json;
pub mod num;
pub mod order;
pub mod re;
pub mod text;

pub use error::{EngineError, Result};
