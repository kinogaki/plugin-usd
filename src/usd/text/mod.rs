// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The usda text format: reading ([`read_usda`]) and writing
//! ([`write_usda`]) layers exactly as OpenUSD does.

pub(crate) mod number;
pub(crate) mod quote;
mod reader;
mod writer;

pub use reader::read_usda;
pub use writer::write_usda;
