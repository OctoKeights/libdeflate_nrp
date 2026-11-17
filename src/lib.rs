//! DEFLATE and zlib compression in pure, safe Rust: a native port of Eric Biggers's
//! [libdeflate](https://github.com/ebiggers/libdeflate) v1.26.
//!
//! The compressor produces the same streams as libdeflate, byte for byte, at its levels 0 and
//! 2 to 9. The decompressor accepts and rejects exactly the streams libdeflate does.
//!
//! - [`zlib_compress`] and [`deflate_compress`] compress to a zlib or raw DEFLATE stream at a
//!   [`Level`]; [`zlib_compress_bound`] and [`deflate_compress_bound`] give the largest output
//!   possible.
//!
//! - [`zlib_decompress`] and [`deflate_decompress`] decompress into a buffer and report how
//!   many bytes they read and wrote ([`Decompressed`]), or why they failed ([`Error`]).
//!
//! - [`adler32`] and [`adler32_update`] compute the zlib checksum.
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

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod compress;
mod decompress;

pub use compress::{deflate_compress, deflate_compress_bound, zlib_compress, zlib_compress_bound};
pub use decompress::{adler32, adler32_update, deflate_decompress, zlib_decompress};

/// A compression level: 0 for none, or 2 to 9 for increasingly thorough compression.
///
/// libdeflate's level 1, which uses a different match finder, and its near-optimal levels 10
/// to 12 are not ported, so they cannot be constructed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Level(u8);

impl Level {
    /// Level 0: data stored without compression.
    pub const NONE: Level = Level(0);

    /// Level 2: the fastest compression ported.
    pub const FASTEST: Level = Level(2);

    /// Level 6: libdeflate's default balance of speed and size.
    pub const DEFAULT: Level = Level(6);

    /// Level 9: the smallest output ported.
    pub const BEST: Level = Level(9);

    /// The level `level`, if it is 0 or 2 to 9.
    pub const fn new(level: u8) -> Option<Level> {
        match level {
            0 | 2..=9 => Some(Level(level)),
            _ => None,
        }
    }

    /// The level as a number.
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl Default for Level {
    fn default() -> Self {
        Level::DEFAULT
    }
}

impl TryFrom<u8> for Level {
    type Error = UnsupportedLevel;

    fn try_from(level: u8) -> Result<Self, Self::Error> {
        Level::new(level).ok_or(UnsupportedLevel(level))
    }
}

/// The error for a compression level that is not ported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnsupportedLevel(pub u8);

impl core::fmt::Display for UnsupportedLevel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "compression level {} is not supported (use 0 or 2 to 9)",
            self.0
        )
    }
}

impl std::error::Error for UnsupportedLevel {}

/// How much a successful decompression read and wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decompressed {
    /// The bytes of input the stream occupied. Any input after it was not read.
    pub bytes_read: usize,
    /// The bytes written to the start of the output buffer.
    pub bytes_written: usize,
}

/// Why a stream could not be decompressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The input is not a valid stream (`LIBDEFLATE_BAD_DATA`).
    BadData,
    /// The stream decompresses to more bytes than the output buffer holds
    /// (`LIBDEFLATE_INSUFFICIENT_SPACE`).
    InsufficientSpace,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Error::BadData => "invalid compressed data",
            Error::InsufficientSpace => "output buffer too small for the decompressed data",
        })
    }
}

impl std::error::Error for Error {}

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
    use super::*;

    #[test]
    fn only_ported_levels_exist() {
        let valid: Vec<u8> = (0..=255).filter(|&l| Level::new(l).is_some()).collect();
        assert_eq!(valid, [0, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(Level::try_from(1), Err(UnsupportedLevel(1)));
        assert_eq!(Level::default(), Level::DEFAULT);
    }
}
