//! DEFLATE and zlib, both ways: a native Rust port of Eric Biggers's
//! [libdeflate](https://github.com/ebiggers/libdeflate) v1.26.
//!
//! - [`zlib_compress`] and [`deflate_compress`]: bytes to a zlib stream or a raw DEFLATE stream
//!   at one of libdeflate's levels (`deflate_compress.c`, `zlib_compress.c`), with
//!   [`zlib_compress_bound`] and [`deflate_compress_bound`], the most they can make.
//! - [`zlib_decompress`] and [`deflate_decompress`]: a stream back to bytes
//!   (`deflate_decompress.c`, `zlib_decompress.c`), as the `_ex` functions answer.
//! - [`adler32`] (`adler32.c`).
//!
//! The compressor gives the bytes libdeflate gives at levels 0 and 2 to 9: no compression,
//! and its greedy, lazy and lazy2 parsers over hash chains, with its splitting of blocks and
//! its choice of the cheapest kind of block. Level 1, with its own hash table, and the
//! near-optimal levels 10 to 12 are not ported. The decompressor takes and refuses the
//! streams libdeflate takes and refuses, and stops where it stops.
//!
//! libdeflate's streams are its own: another DEFLATE compressor (zlib's or miniz's, say) makes
//! other streams of the same bytes, which any decompressor reads back.
#![forbid(unsafe_code)]
// The code follows the original's line for line, and these are its shapes.
#![allow(
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::manual_range_contains
)]

mod compress;
mod decompress;

pub use compress::{
    MAX_LEVEL, deflate_compress, deflate_compress_bound, zlib_compress, zlib_compress_bound,
};
pub use decompress::{adler32, deflate_decompress, zlib_decompress};

/// Why a stream did not decompress: `enum libdeflate_result`, but for success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The stream is not a valid one (`LIBDEFLATE_BAD_DATA`).
    BadData,
    /// It makes more bytes than there is room for (`LIBDEFLATE_INSUFFICIENT_SPACE`).
    InsufficientSpace,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Error::BadData => "the data is not a valid stream",
            Error::InsufficientSpace => "the stream makes more bytes than there is room for",
        })
    }
}

impl std::error::Error for Error {}

/// (The README's examples, compiled and run with the tests.)
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
