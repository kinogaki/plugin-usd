// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The usdc binary format ("crate files"): reading ([`read_usdc`]) and
//! writing ([`write_usdc`]) layers, after OpenUSD's `crateFile.cpp`.

mod compression;
mod element;
mod lz4;
mod reader;
mod rep;
mod writer;

pub use reader::read_usdc;
pub use writer::write_usdc;
