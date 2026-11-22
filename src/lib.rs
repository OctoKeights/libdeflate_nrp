//! DEFLATE, zlib, and gzip compression in pure, safe `#![no_std]` Rust: a native port of
//! Eric Biggers's [libdeflate](https://github.com/ebiggers/libdeflate) v1.26.
//!
//! The compressor produces the same streams as libdeflate, byte for byte, at levels 0 and 2
//! to 9, and includes a fast hash-table matchfinder at level 1. The decompressor accepts and
//! rejects the same streams libdeflate does, using compact inline lookup tables with zero heap
//! allocations.
//!
//! - [`zlib_compress`], [`deflate_compress`], and [`gzip_compress`] compress to a `Vec<u8>` at
//!   a [`Level`]; [`zlib_compress_into`], [`deflate_compress_into`], and [`gzip_compress_into`]
//!   write into a caller-supplied slice; [`zlib_compress_bound`], [`deflate_compress_bound`],
//!   and [`gzip_compress_bound`] (`const fn`) give the largest output possible.
//!
//! - [`Compressor`] reuses matchfinder and block buffers across calls; [`Decompressor`]
//!   decompresses with zero heap allocations.
//!
//! - [`zlib_decompress`], [`deflate_decompress`], and [`gzip_decompress`] decompress into a
//!   slice and report how many bytes they read and wrote ([`Decompressed`]), or why they
//!   failed ([`Error`]); [`zlib_decompress_vec`], [`deflate_decompress_vec`], and
//!   [`gzip_decompress_vec`] decompress into a bounded `Vec<u8>`.
//!
//! - [`adler32`] / [`adler32_update`] and [`crc32`] / [`crc32_update`] compute the zlib and
//!   gzip checksums.
//!
//! ```
//! use libdeflate_nrp::{Level, zlib_compress, zlib_decompress};
//!
//! let data = b"to be or not to be, that is the question; ".repeat(100);
//! let stream = zlib_compress(&data, Level::DEFAULT);
//!
//! let mut decompressed = vec![0; data.len()];
//! let result = zlib_decompress(&stream, &mut decompressed).unwrap();
//! assert_eq!(result.bytes_written, data.len());
//! assert_eq!(decompressed, data);
//! ```
//!
//! Other DEFLATE compressors, such as zlib's or miniz's, produce different (equally valid)
//! streams from the same data; libdeflate's are reproduced here because some formats, such as
//! OpenEXR's DWA compression, store them.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod compress;
mod crc32;
mod decompress;

pub use compress::{
    Compressor, deflate_compress, deflate_compress_bound, deflate_compress_into, gzip_compress,
    gzip_compress_bound, gzip_compress_into, zlib_compress, zlib_compress_bound,
    zlib_compress_into,
};
pub use crc32::{crc32, crc32_update};
pub use decompress::{
    Decompressor, adler32, adler32_update, deflate_decompress, deflate_decompress_vec,
    gzip_decompress, gzip_decompress_vec, zlib_decompress, zlib_decompress_vec,
};

/// A compression level: 0 for none, 1 for fast hash-table compression, or 2 to 9 for
/// increasingly thorough hash-chain compression (bit-identical to libdeflate at 0 and 2 to 9).
///
/// libdeflate's near-optimal levels 10 to 12 are not ported, so they cannot be constructed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Level(u8);

impl Level {
    /// Level 0: data stored without compression.
    pub const L0: Level = Level(0);
    /// Level 1: fast two-way hash-table compression.
    pub const L1: Level = Level(1);
    /// Level 2: greedy hash-chain compression.
    pub const L2: Level = Level(2);
    /// Level 3: greedy hash-chain compression.
    pub const L3: Level = Level(3);
    /// Level 4: greedy hash-chain compression.
    pub const L4: Level = Level(4);
    /// Level 5: lazy hash-chain compression.
    pub const L5: Level = Level(5);
    /// Level 6: default lazy hash-chain compression.
    pub const L6: Level = Level(6);
    /// Level 7: deeper lazy hash-chain compression.
    pub const L7: Level = Level(7);
    /// Level 8: two-step lazy hash-chain compression.
    pub const L8: Level = Level(8);
    /// Level 9: deepest two-step lazy hash-chain compression ported.
    pub const L9: Level = Level(9);

    /// Level 0: data stored without compression.
    pub const NONE: Level = Self::L0;

    /// Level 1: the fastest compression level.
    pub const FASTEST: Level = Self::L1;

    /// Level 6: libdeflate's default balance of speed and size.
    pub const DEFAULT: Level = Self::L6;

    /// Level 9: the smallest output ported.
    pub const BEST: Level = Self::L9;

    /// Returns `Some(Level)` if `level` is in `0..=9`, or `None` otherwise.
    #[must_use]
    pub const fn new(level: u8) -> Option<Level> {
        match level {
            0..=9 => Some(Level(level)),
            _ => None,
        }
    }

    /// The level as a number in `0..=9`.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl Default for Level {
    fn default() -> Self {
        Level::DEFAULT
    }
}

impl From<Level> for u8 {
    fn from(level: Level) -> Self {
        level.0
    }
}

impl core::fmt::Display for Level {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

impl TryFrom<u8> for Level {
    type Error = UnsupportedLevel;

    fn try_from(level: u8) -> Result<Self, Self::Error> {
        Level::new(level).ok_or(UnsupportedLevel(level))
    }
}

/// The error for a compression level outside `0..=9`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UnsupportedLevel(pub u8);

impl core::fmt::Display for UnsupportedLevel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "compression level {} is not supported (use 0 to 9)",
            self.0
        )
    }
}

impl core::error::Error for UnsupportedLevel {}

/// How much a successful decompression read and wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Decompressed {
    /// The bytes of input the stream occupied. Any input after it was not read.
    pub bytes_read: usize,
    /// The bytes written to the start of the output buffer.
    pub bytes_written: usize,
}

/// Why a compression or decompression operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Error {
    /// The input is not a valid stream (`LIBDEFLATE_BAD_DATA`).
    BadData,
    /// The output does not fit in the destination buffer or limit
    /// (`LIBDEFLATE_INSUFFICIENT_SPACE`).
    InsufficientSpace,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Error::BadData => "invalid compressed data",
            Error::InsufficientSpace => "output buffer too small",
        })
    }
}

impl core::error::Error for Error {}

/// The README's examples, compiled and run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

/// The Guide's examples, compiled and run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../docs/guide.md")]
struct GuideDoctests;

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::vec::Vec;
    use core::hash::{Hash, Hasher};

    use super::*;

    fn hash_of<T: Hash>(val: &T) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        val.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn supported_levels_constants_and_traits() {
        let valid: Vec<u8> = (0..=255).filter(|&l| Level::new(l).is_some()).collect();
        assert_eq!(valid, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(
            [
                Level::L0,
                Level::L1,
                Level::L2,
                Level::L3,
                Level::L4,
                Level::L5,
                Level::L6,
                Level::L7,
                Level::L8,
                Level::L9,
            ]
            .map(u8::from),
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
        );
        assert_eq!(Level::NONE, Level::L0);
        assert_eq!(Level::FASTEST, Level::L1);
        assert_eq!(Level::DEFAULT, Level::L6);
        assert_eq!(Level::BEST, Level::L9);
        assert_eq!(Level::default(), Level::DEFAULT);
        assert_eq!(Level::L6.to_string(), "6");

        assert_eq!(Level::try_from(1), Ok(Level::L1));
        assert_eq!(Level::try_from(10), Err(UnsupportedLevel(10)));
        assert!(UnsupportedLevel(10).to_string().contains("10"));

        assert_eq!(
            hash_of(&UnsupportedLevel(10)),
            hash_of(&UnsupportedLevel(10))
        );
        assert_eq!(
            hash_of(&Decompressed {
                bytes_read: 5,
                bytes_written: 10
            }),
            hash_of(&Decompressed {
                bytes_read: 5,
                bytes_written: 10
            })
        );
        assert_eq!(hash_of(&Error::BadData), hash_of(&Error::BadData));
        assert_ne!(hash_of(&Error::BadData), hash_of(&Error::InsufficientSpace));
    }
}
