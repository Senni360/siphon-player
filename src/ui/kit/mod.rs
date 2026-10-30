//! Siphon's design kit: the pieces every redesigned view shares, as
//! `DESIGN.md` describes them. Views draw with these and report what the
//! user did; applying it stays with `crate::app`.

pub mod motion;
pub mod source;
pub mod tools;

pub use source::Source;
