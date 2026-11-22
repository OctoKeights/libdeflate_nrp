//! The decompressor: libdeflate's `deflate_decompress.c` (with `decompress_template.h`),
//! `zlib_decompress.c`, `gzip_decompress.c`, and `adler32.c`.
//!
//! Away from the ends of the input and output buffers, a fast interior loop refills the bit
//! buffer a 64-bit word at a time and decodes without per-symbol bounds checks; near the ends,
//! a careful loop refills byte by byte so that valid and damaged streams behave identically to
//! libdeflate. Input read past the end of the buffer is taken as zeros, up to eight bytes.

use alloc::vec;
use alloc::vec::Vec;

use crate::{Decompressed, Error, crc32};

const NUM_PRECODE_SYMS: usize = 19;
const NUM_LITLEN_SYMS: usize = 288;
const NUM_OFFSET_SYMS: usize = 32;
const MAX_PRECODE_LEN: u32 = 7;
const MAX_LITLEN_LEN: u32 = 15;
const MAX_OFFSET_LEN: u32 = 15;
const END_OF_BLOCK: usize = 256;

const LITLEN_TABLE_CAP: usize = 2560;
const OFFSET_TABLE_CAP: usize = 1280;
const PRECODE_TABLE_CAP: usize = 128;

/// A refill fills the bit buffer to at least this many bits.
const REFILL_TARGET: u32 = 56;

/// The most zero bytes that may be read past the end of the input.
const MAX_OVERREAD: usize = 8;

/// Fast-loop safety margin in output bytes (enough for two literals or a 258-byte match).
const FAST_OUT_MARGIN: usize = 260;

/// The base length and extra bits of each length symbol from 257. Symbols 286 and 287 are
/// invalid in the format, and libdeflate decodes them as 258.
const LENGTHS: [(u32, u32); 31] = [
    (3, 0),
    (4, 0),
    (5, 0),
    (6, 0),
    (7, 0),
    (8, 0),
    (9, 0),
    (10, 0),
    (11, 1),
    (13, 1),
    (15, 1),
    (17, 1),
    (19, 2),
    (23, 2),
    (27, 2),
    (31, 2),
    (35, 3),
    (43, 3),
    (51, 3),
    (59, 3),
    (67, 4),
    (83, 4),
    (99, 4),
    (115, 4),
    (131, 5),
    (163, 5),
    (195, 5),
    (227, 5),
    (258, 0),
    (258, 0),
    (258, 0),
];

/// The base distance and extra bits of each offset symbol. Symbols 30 and 31 are invalid in
/// the format, and libdeflate decodes them as symbol 29.
const DISTANCES: [(u32, u32); 32] = [
    (1, 0),
    (2, 0),
    (3, 0),
    (4, 0),
    (5, 1),
    (7, 1),
    (9, 2),
    (13, 2),
    (17, 3),
    (25, 3),
    (33, 4),
    (49, 4),
    (65, 5),
    (97, 5),
    (129, 6),
    (193, 6),
    (257, 7),
    (385, 7),
    (513, 8),
    (769, 8),
    (1025, 9),
    (1537, 9),
    (2049, 10),
    (3073, 10),
    (4097, 11),
    (6145, 11),
    (8193, 12),
    (12289, 12),
    (16385, 13),
    (24577, 13),
    (24577, 13),
    (24577, 13),
];

/// The order in which a dynamic block lists its precode lengths.
const PRECODE_ORDER: [usize; NUM_PRECODE_SYMS] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Compact two-level Huffman lookup table stored inline with zero heap allocation.
///
/// Primary entries occupy `0..(1 << PRIMARY_BITS)`. Direct entries have bit 15 clear and pack
/// `(symbol << 4) | codeword_len`. Subtable links (used only when `codeword_len > PRIMARY_BITS`)
/// have bit 15 set and pack `0x8000 | (sub_offset << 3) | sub_bits`.
#[derive(Clone, Copy)]
struct DecodeTable<const CAP: usize, const PRIMARY_BITS: u32> {
    entries: [u16; CAP],
}

type LitLenTable = DecodeTable<LITLEN_TABLE_CAP, 10>;
type OffsetTable = DecodeTable<OFFSET_TABLE_CAP, 10>;
type PrecodeTable = DecodeTable<PRECODE_TABLE_CAP, 7>;

/// Precomputed static literal/length Huffman decode table.
static FIXED_LITLEN: LitLenTable = {
    let mut lens = [8u8; NUM_LITLEN_SYMS];
    let mut i = 144;
    while i < 256 {
        lens[i] = 9;
        i += 1;
    }
    while i < 280 {
        lens[i] = 7;
        i += 1;
    }
    let mut table = DecodeTable::empty();
    assert!(table.build(&lens, MAX_LITLEN_LEN));
    table
};

/// Precomputed static offset Huffman decode table.
static FIXED_OFFSET: OffsetTable = {
    let lens = [5u8; NUM_OFFSET_SYMS];
    let mut table = DecodeTable::empty();
    assert!(table.build(&lens, MAX_OFFSET_LEN));
    table
};

impl<const CAP: usize, const PRIMARY_BITS: u32> DecodeTable<CAP, PRIMARY_BITS> {
    const fn empty() -> Self {
        Self { entries: [0; CAP] }
    }

    /// Builds the canonical code with codeword lengths `lens`, returning `false` where
    /// libdeflate rejects them: an over-subscribed code, or an incomplete one other than an
    /// empty code or a single one-bit codeword.
    const fn build(&mut self, lens: &[u8], max_len: u32) -> bool {
        let primary_size = 1usize << PRIMARY_BITS;
        let mut len_counts = [0u32; 16];
        let mut idx = 0;
        while idx < lens.len() {
            len_counts[lens[idx] as usize] += 1;
            idx += 1;
        }
        let mut longest = max_len;
        while longest > 1 && len_counts[longest as usize] == 0 {
            longest -= 1;
        }
        let mut codespace_used = 0u32;
        let mut l = 1usize;
        while l <= longest as usize {
            codespace_used = (codespace_used << 1) + len_counts[l];
            l += 1;
        }
        let codespace = 1u32 << longest;
        if codespace_used > codespace {
            return false;
        }
        if codespace_used < codespace {
            let symbol = if codespace_used == 0 {
                0u16
            } else if codespace_used == codespace / 2 && len_counts[1] == 1 {
                let mut s = 0usize;
                while s < lens.len() && lens[s] == 0 {
                    s += 1;
                }
                if s == lens.len() {
                    return false;
                }
                s as u16
            } else {
                return false;
            };
            let entry = (symbol << 4) | 1;
            let mut w = 0;
            while w < primary_size {
                self.entries[w] = entry;
                w += 1;
            }
            return true;
        }

        self.populate_complete(lens, &len_counts, longest);
        true
    }

    const fn populate_complete(&mut self, lens: &[u8], len_counts: &[u32; 16], longest: u32) {
        let primary_size = 1usize << PRIMARY_BITS;
        if longest > PRIMARY_BITS {
            let mut w = 0;
            while w < primary_size {
                self.entries[w] = 0;
                w += 1;
            }
        }

        let mut first_code = [0u32; 16];
        let mut sym_offset = [0u16; 16];
        let mut code = 0u32;
        let mut total_syms = 0u16;
        let mut l = 1usize;
        while l <= longest as usize {
            first_code[l] = code;
            sym_offset[l] = total_syms;
            let count = len_counts[l];
            code = (code + count) << 1;
            total_syms += count as u16;
            l += 1;
        }

        let mut sorted_syms = [0u16; NUM_LITLEN_SYMS];
        let mut write_pos = sym_offset;
        let mut sym = 0usize;
        while sym < lens.len() {
            let len = lens[sym] as usize;
            if len != 0 {
                sorted_syms[write_pos[len] as usize] = sym as u16;
                write_pos[len] += 1;
            }
            sym += 1;
        }

        let mut alloc = primary_size;
        let mut cur_len = longest;
        while cur_len > PRIMARY_BITS {
            let count = len_counts[cur_len as usize] as usize;
            let base_sym = sym_offset[cur_len as usize] as usize;
            let mut codeword = first_code[cur_len as usize];
            let mut k = 0usize;
            while k < count {
                let symbol = sorted_syms[base_sym + k];
                let rev = codeword.reverse_bits() >> (32 - cur_len);
                let low = (rev as usize) & (primary_size - 1);
                let mut header = self.entries[low];
                if header == 0 {
                    let sub_bits = cur_len - PRIMARY_BITS;
                    header = 0x8000 | ((alloc as u16) << 3) | (sub_bits as u16);
                    self.entries[low] = header;
                    alloc += 1usize << sub_bits;
                }
                let sub_offset = ((header >> 3) & 0xFFF) as usize;
                let sub_size = 1usize << (header & 7);
                let step = 1usize << (cur_len - PRIMARY_BITS);
                let entry = (symbol << 4) | (cur_len as u16);
                let mut w = (rev >> PRIMARY_BITS) as usize;
                while w < sub_size {
                    self.entries[sub_offset + w] = entry;
                    w += step;
                }
                codeword += 1;
                k += 1;
            }
            cur_len -= 1;
        }

        let direct_max = if longest < PRIMARY_BITS {
            longest
        } else {
            PRIMARY_BITS
        };
        let mut direct_len = 1u32;
        while direct_len <= direct_max {
            let count = len_counts[direct_len as usize] as usize;
            let base_sym = sym_offset[direct_len as usize] as usize;
            let mut codeword = first_code[direct_len as usize];
            let step = 1usize << direct_len;
            let mut k = 0usize;
            while k < count {
                let symbol = sorted_syms[base_sym + k];
                let entry = (symbol << 4) | (direct_len as u16);
                let mut w = (codeword.reverse_bits() >> (32 - direct_len)) as usize;
                while w < primary_size {
                    self.entries[w] = entry;
                    w += step;
                }
                codeword += 1;
                k += 1;
            }
            direct_len += 1;
        }
    }

    #[inline]
    fn lookup(&self, buffer: u64) -> u16 {
        let primary_mask = (1usize << PRIMARY_BITS) - 1;
        let entry = self.entries[(buffer as usize) & primary_mask];
        if entry & 0x8000 == 0 {
            entry
        } else {
            let sub_offset = ((entry >> 3) & 0xFFF) as usize;
            let sub_mask = (1usize << (entry & 7)) - 1;
            self.entries[sub_offset + (((buffer as usize) >> PRIMARY_BITS) & sub_mask)]
        }
    }
}

// ----------------------------------------------------------------------------------------
// Bit reader & output buffers

/// The input as a stream of bits, least significant first.
struct BitReader<'a> {
    input: &'a [u8],
    /// The next byte to load.
    pos: usize,
    buffer: u64,
    /// Bits in `buffer`, including any zeros loaded past the end of the input.
    available: u32,
    /// Zero bytes loaded past the end of the input.
    overread: usize,
}

impl<'a> BitReader<'a> {
    fn new(input: &'a [u8]) -> Self {
        BitReader {
            input,
            pos: 0,
            buffer: 0,
            available: 0,
            overread: 0,
        }
    }

    /// Fills the buffer to at least [`REFILL_TARGET`] bits, with zero bytes past the end of
    /// the input; more than [`MAX_OVERREAD`] of those means the stream is truncated.
    #[inline]
    fn refill(&mut self) -> Result<(), Error> {
        while self.available < REFILL_TARGET {
            if let Some(&byte) = self.input.get(self.pos) {
                self.buffer |= u64::from(byte) << self.available;
                self.pos += 1;
            } else {
                self.overread += 1;
                if self.overread > MAX_OVERREAD {
                    return Err(Error::BadData);
                }
            }
            self.available += 8;
        }
        Ok(())
    }

    /// Branchless 64-bit refill when `self.pos + 8 <= self.input.len()`.
    #[inline]
    fn refill_fast(&mut self) {
        let b = &self.input[self.pos..self.pos + 8];
        let word = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        self.buffer |= word << self.available;
        self.pos += ((63 - self.available) >> 3) as usize;
        self.available |= 0b11_1000;
    }

    /// Clears any unconsumed lookahead bits above `self.available` when leaving the fast loop.
    #[inline]
    fn trim_lookahead(&mut self) {
        self.buffer &= (1u64 << self.available) - 1;
    }

    /// The next `n` bits, without consuming them.
    #[inline]
    fn peek(&self, n: u32) -> u64 {
        self.buffer & ((1 << n) - 1)
    }

    #[inline]
    fn consume(&mut self, n: u32) {
        self.buffer >>= n;
        self.available -= n;
    }

    #[inline]
    fn take(&mut self, n: u32) -> u32 {
        let value = self.peek(n) as u32;
        self.consume(n);
        value
    }

    /// Decodes one symbol; the buffer must hold enough bits for it.
    #[inline]
    fn decode<const CAP: usize, const PRIMARY_BITS: u32>(
        &mut self,
        table: &DecodeTable<CAP, PRIMARY_BITS>,
    ) -> usize {
        let entry = table.lookup(self.buffer);
        self.consume(u32::from(entry & 0xF));
        usize::from(entry >> 4)
    }

    /// The input position after the last bit consumed, discounting whole bytes still
    /// buffered; an error if the stream needed more bytes than the input holds.
    fn consumed_bytes(&self) -> Result<usize, Error> {
        let unused = (self.available / 8) as usize;
        let unused_real = unused.checked_sub(self.overread).ok_or(Error::BadData)?;
        Ok(self.pos - unused_real)
    }
}

#[inline]
fn copy_match_slice(buf: &mut [u8], mut written: usize, distance: usize, length: usize) {
    let from = written - distance;
    if distance >= length {
        buf.copy_within(from..from + length, written);
    } else if distance == 1 {
        let byte = buf[from];
        buf[written..written + length].fill(byte);
    } else {
        let end = written + length;
        buf.copy_within(from..written, written);
        written += distance;
        while written < end {
            let step = (written - from).min(end - written);
            buf.copy_within(from..from + step, written);
            written += step;
        }
    }
}

trait OutputBuf {
    fn written(&self) -> usize;
    fn as_written_slice(&self) -> &[u8];
    fn ensure_fast_room(&mut self) -> bool;
    fn push_fast(&mut self, byte: u8);
    fn push_checked(&mut self, byte: u8) -> Result<(), Error>;
    fn reserve_checked(&mut self, additional: usize) -> Result<(), Error>;
    fn copy_stored(&mut self, data: &[u8]);
    fn copy_match_fast(&mut self, distance: usize, length: usize);
}

struct SliceOut<'a> {
    buf: &'a mut [u8],
    written: usize,
}

impl OutputBuf for SliceOut<'_> {
    #[inline]
    fn written(&self) -> usize {
        self.written
    }

    #[inline]
    fn as_written_slice(&self) -> &[u8] {
        &self.buf[..self.written]
    }

    #[inline]
    fn ensure_fast_room(&mut self) -> bool {
        self.written + FAST_OUT_MARGIN <= self.buf.len()
    }

    #[inline]
    fn push_fast(&mut self, byte: u8) {
        self.buf[self.written] = byte;
        self.written += 1;
    }

    #[inline]
    fn push_checked(&mut self, byte: u8) -> Result<(), Error> {
        *self
            .buf
            .get_mut(self.written)
            .ok_or(Error::InsufficientSpace)? = byte;
        self.written += 1;
        Ok(())
    }

    #[inline]
    fn reserve_checked(&mut self, additional: usize) -> Result<(), Error> {
        if additional > self.buf.len() - self.written {
            return Err(Error::InsufficientSpace);
        }
        Ok(())
    }

    #[inline]
    fn copy_stored(&mut self, data: &[u8]) {
        self.buf[self.written..self.written + data.len()].copy_from_slice(data);
        self.written += data.len();
    }

    #[inline]
    fn copy_match_fast(&mut self, distance: usize, length: usize) {
        copy_match_slice(self.buf, self.written, distance, length);
        self.written += length;
    }
}

struct VecOut {
    buf: Vec<u8>,
    written: usize,
    max_len: usize,
}

impl VecOut {
    fn new(input_len: usize, max_len: usize) -> Self {
        let initial = input_len.saturating_mul(3).clamp(64, 262_144).min(max_len);
        Self {
            buf: vec![0u8; initial],
            written: 0,
            max_len,
        }
    }

    fn grow_to_at_least(&mut self, target: usize) {
        let doubled = self.buf.len().saturating_mul(2);
        let new_len = doubled.max(target).max(FAST_OUT_MARGIN).min(self.max_len);
        if new_len > self.buf.len() {
            self.buf.resize(new_len, 0);
        }
    }

    fn into_vec(mut self) -> Vec<u8> {
        self.buf.truncate(self.written);
        self.buf
    }
}

impl OutputBuf for VecOut {
    #[inline]
    fn written(&self) -> usize {
        self.written
    }

    #[inline]
    fn as_written_slice(&self) -> &[u8] {
        &self.buf[..self.written]
    }

    #[inline]
    fn ensure_fast_room(&mut self) -> bool {
        if self.written + FAST_OUT_MARGIN <= self.buf.len() {
            return true;
        }
        if self.max_len - self.written < FAST_OUT_MARGIN {
            return false;
        }
        self.grow_to_at_least(self.written + FAST_OUT_MARGIN);
        self.written + FAST_OUT_MARGIN <= self.buf.len()
    }

    #[inline]
    fn push_fast(&mut self, byte: u8) {
        self.buf[self.written] = byte;
        self.written += 1;
    }

    #[inline]
    fn push_checked(&mut self, byte: u8) -> Result<(), Error> {
        self.reserve_checked(1)?;
        self.buf[self.written] = byte;
        self.written += 1;
        Ok(())
    }

    #[inline]
    fn reserve_checked(&mut self, additional: usize) -> Result<(), Error> {
        if additional > self.max_len - self.written {
            return Err(Error::InsufficientSpace);
        }
        let needed = self.written + additional;
        if needed > self.buf.len() {
            self.grow_to_at_least(needed);
        }
        Ok(())
    }

    #[inline]
    fn copy_stored(&mut self, data: &[u8]) {
        self.buf[self.written..self.written + data.len()].copy_from_slice(data);
        self.written += data.len();
    }

    #[inline]
    fn copy_match_fast(&mut self, distance: usize, length: usize) {
        copy_match_slice(&mut self.buf, self.written, distance, length);
        self.written += length;
    }
}

// ----------------------------------------------------------------------------------------
// Decompressor

/// A reusable, zero-heap-allocation DEFLATE, zlib, and gzip decompressor
/// (`libdeflate_decompressor`).
///
/// `Decompressor` holds its two-level Huffman decode tables inline, so [`deflate_decompress`],
/// [`zlib_decompress`], and [`gzip_decompress`] perform zero heap allocations at runtime.
///
/// [`deflate_decompress`]: Decompressor::deflate_decompress
/// [`zlib_decompress`]: Decompressor::zlib_decompress
/// [`gzip_decompress`]: Decompressor::gzip_decompress
pub struct Decompressor {
    litlen: LitLenTable,
    offset: OffsetTable,
}

impl core::fmt::Debug for Decompressor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Decompressor").finish_non_exhaustive()
    }
}

impl Default for Decompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Decompressor {
    /// Creates a new decompressor with inline lookup tables and zero heap allocation
    /// (`libdeflate_alloc_decompressor`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            litlen: DecodeTable::empty(),
            offset: DecodeTable::empty(),
        }
    }

    /// Decompresses the raw DEFLATE stream at the start of `input` into `output` with zero
    /// heap allocations (`libdeflate_deflate_decompress_ex`).
    ///
    /// Input after the end of the stream is ignored; [`Decompressed::bytes_read`] says where
    /// it ended.
    ///
    /// # Errors
    ///
    /// [`Error::BadData`] if the input is not a valid stream, and [`Error::InsufficientSpace`]
    /// if it decompresses to more than `output` holds.
    pub fn deflate_decompress(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<Decompressed, Error> {
        let mut out = SliceOut {
            buf: output,
            written: 0,
        };
        let bytes_read = self.inflate_into(input, &mut out)?;
        Ok(Decompressed {
            bytes_read,
            bytes_written: out.written,
        })
    }

    /// Decompresses the raw DEFLATE stream at the start of `input` into a newly allocated
    /// `Vec<u8>` of at most `max_len` bytes.
    ///
    /// # Errors
    ///
    /// [`Error::BadData`] if the input is not a valid stream, and [`Error::InsufficientSpace`]
    /// if the uncompressed output exceeds `max_len` bytes.
    pub fn deflate_decompress_vec(
        &mut self,
        input: &[u8],
        max_len: usize,
    ) -> Result<Vec<u8>, Error> {
        let mut out = VecOut::new(input.len(), max_len);
        self.inflate_into(input, &mut out)?;
        Ok(out.into_vec())
    }

    /// Decompresses the zlib stream at the start of `input` into `output` with zero heap
    /// allocations (`libdeflate_zlib_decompress_ex`), checking its header and Adler-32
    /// checksum.
    ///
    /// Input after the end of the stream is ignored; [`Decompressed::bytes_read`] says where
    /// it ended.
    ///
    /// # Errors
    ///
    /// [`Error::BadData`] if the input is not a valid zlib stream, and
    /// [`Error::InsufficientSpace`] if it decompresses to more than `output` holds.
    pub fn zlib_decompress(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<Decompressed, Error> {
        let mut out = SliceOut {
            buf: output,
            written: 0,
        };
        let bytes_read = self.zlib_into(input, &mut out)?;
        Ok(Decompressed {
            bytes_read,
            bytes_written: out.written,
        })
    }

    /// Decompresses the zlib stream at the start of `input` into a newly allocated `Vec<u8>`
    /// of at most `max_len` bytes, checking its header and Adler-32 checksum.
    ///
    /// # Errors
    ///
    /// [`Error::BadData`] if the input is not a valid zlib stream, and
    /// [`Error::InsufficientSpace`] if the uncompressed output exceeds `max_len` bytes.
    pub fn zlib_decompress_vec(&mut self, input: &[u8], max_len: usize) -> Result<Vec<u8>, Error> {
        let mut out = VecOut::new(input.len(), max_len);
        self.zlib_into(input, &mut out)?;
        Ok(out.into_vec())
    }

    /// Decompresses the RFC 1952 gzip member at the start of `input` into `output` with zero
    /// heap allocations (`libdeflate_gzip_decompress_ex`), handling optional header fields
    /// (`FEXTRA`, `FNAME`, `FCOMMENT`, `FHCRC`) and validating the `CRC32` and `ISIZE`
    /// trailer.
    ///
    /// Input after the end of the member is ignored; [`Decompressed::bytes_read`] says where
    /// it ended.
    ///
    /// # Errors
    ///
    /// [`Error::BadData`] if the input is not a valid gzip stream, and
    /// [`Error::InsufficientSpace`] if it decompresses to more than `output` holds.
    pub fn gzip_decompress(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<Decompressed, Error> {
        let mut out = SliceOut {
            buf: output,
            written: 0,
        };
        let bytes_read = self.gzip_into(input, &mut out)?;
        Ok(Decompressed {
            bytes_read,
            bytes_written: out.written,
        })
    }

    /// Decompresses the RFC 1952 gzip member at the start of `input` into a newly allocated
    /// `Vec<u8>` of at most `max_len` bytes, validating all header fields and the `CRC32` and
    /// `ISIZE` trailer.
    ///
    /// # Errors
    ///
    /// [`Error::BadData`] if the input is not a valid gzip stream, and
    /// [`Error::InsufficientSpace`] if the uncompressed output exceeds `max_len` bytes.
    pub fn gzip_decompress_vec(&mut self, input: &[u8], max_len: usize) -> Result<Vec<u8>, Error> {
        let mut out = VecOut::new(input.len(), max_len);
        self.gzip_into(input, &mut out)?;
        Ok(out.into_vec())
    }

    fn inflate_into<O: OutputBuf>(&mut self, input: &[u8], output: &mut O) -> Result<usize, Error> {
        let mut bits = BitReader::new(input);
        loop {
            bits.refill()?;
            let is_final = bits.peek(1) == 1;
            match (bits.buffer >> 1) & 3 {
                0 => {
                    bits.consume(3);
                    copy_stored_block(&mut bits, output)?;
                }
                1 => {
                    bits.consume(3);
                    decode_huffman_block(&mut bits, &FIXED_LITLEN, &FIXED_OFFSET, output)?;
                }
                2 => {
                    self.read_dynamic_tables(&mut bits)?;
                    decode_huffman_block(&mut bits, &self.litlen, &self.offset, output)?;
                }
                _ => return Err(Error::BadData),
            }
            if is_final {
                break;
            }
        }
        bits.consumed_bytes()
    }

    fn zlib_into<O: OutputBuf>(&mut self, input: &[u8], output: &mut O) -> Result<usize, Error> {
        const HEADER: usize = 2;
        const TRAILER: usize = 4;
        if input.len() < HEADER + TRAILER {
            return Err(Error::BadData);
        }
        let header = u16::from_be_bytes([input[0], input[1]]);
        let check_ok = header.is_multiple_of(31);
        let method_ok = (header >> 8) & 0xf == 8;
        let window_ok = header >> 12 <= 7;
        let no_dictionary = (header >> 5) & 1 == 0;
        if !(check_ok && method_ok && window_ok && no_dictionary) {
            return Err(Error::BadData);
        }

        let deflated_read = self.inflate_into(&input[HEADER..input.len() - TRAILER], output)?;
        let trailer_at = HEADER + deflated_read;
        let stored = &input[trailer_at..trailer_at + TRAILER];
        let expected = u32::from_be_bytes([stored[0], stored[1], stored[2], stored[3]]);
        if adler32(output.as_written_slice()) != expected {
            return Err(Error::BadData);
        }
        Ok(trailer_at + TRAILER)
    }

    fn gzip_into<O: OutputBuf>(&mut self, input: &[u8], output: &mut O) -> Result<usize, Error> {
        const MIN_HEADER: usize = 10;
        const TRAILER: usize = 8;
        const FHCRC: u8 = 0x02;
        const FEXTRA: u8 = 0x04;
        const FNAME: u8 = 0x08;
        const FCOMMENT: u8 = 0x10;
        const RESERVED: u8 = 0xE0;

        if input.len() < MIN_HEADER + TRAILER {
            return Err(Error::BadData);
        }
        if input[0] != 0x1F || input[1] != 0x8B || input[2] != 8 {
            return Err(Error::BadData);
        }
        let flg = input[3];
        if flg & RESERVED != 0 {
            return Err(Error::BadData);
        }

        let body_end = input.len() - TRAILER;
        let mut pos = MIN_HEADER;

        if flg & FEXTRA != 0 {
            let len_bytes = input.get(pos..pos + 2).ok_or(Error::BadData)?;
            let xlen = usize::from(u16::from_le_bytes([len_bytes[0], len_bytes[1]]));
            pos += 2;
            if xlen > body_end.saturating_sub(pos) {
                return Err(Error::BadData);
            }
            pos += xlen;
        }
        if flg & FNAME != 0 {
            let nul = input
                .get(pos..body_end)
                .ok_or(Error::BadData)?
                .iter()
                .position(|&b| b == 0)
                .ok_or(Error::BadData)?;
            pos += nul + 1;
        }
        if flg & FCOMMENT != 0 {
            let nul = input
                .get(pos..body_end)
                .ok_or(Error::BadData)?
                .iter()
                .position(|&b| b == 0)
                .ok_or(Error::BadData)?;
            pos += nul + 1;
        }
        if flg & FHCRC != 0 {
            if pos + 2 > body_end {
                return Err(Error::BadData);
            }
            let expected_hcrc = u16::from_le_bytes([input[pos], input[pos + 1]]);
            let actual_hcrc = crc32(&input[..pos]) as u16;
            if actual_hcrc != expected_hcrc {
                return Err(Error::BadData);
            }
            pos += 2;
        }

        let deflated_read = self.inflate_into(&input[pos..body_end], output)?;
        let trailer_at = pos + deflated_read;
        let stored = &input[trailer_at..trailer_at + TRAILER];
        let expected_crc = u32::from_le_bytes([stored[0], stored[1], stored[2], stored[3]]);
        let expected_isize = u32::from_le_bytes([stored[4], stored[5], stored[6], stored[7]]);
        if crc32(output.as_written_slice()) != expected_crc
            || output.written() as u32 != expected_isize
        {
            return Err(Error::BadData);
        }
        Ok(trailer_at + TRAILER)
    }

    /// Reads a dynamic block's header (whose first 3 bits are still buffered) and builds its
    /// two codes in place.
    fn read_dynamic_tables(&mut self, bits: &mut BitReader) -> Result<(), Error> {
        let num_litlen = 257 + ((bits.buffer >> 3) & 31) as usize;
        let num_offset = 1 + ((bits.buffer >> 8) & 31) as usize;
        let num_precode = 4 + ((bits.buffer >> 13) & 15) as usize;
        let mut precode_lens = [0u8; NUM_PRECODE_SYMS];
        precode_lens[PRECODE_ORDER[0]] = ((bits.buffer >> 17) & 7) as u8;
        bits.consume(20);
        bits.refill()?;
        for &symbol in &PRECODE_ORDER[1..num_precode] {
            precode_lens[symbol] = bits.take(3) as u8;
        }
        let mut precode = PrecodeTable::empty();
        if !precode.build(&precode_lens, MAX_PRECODE_LEN) {
            return Err(Error::BadData);
        }

        // Both codes' lengths, run-length coded with the precode. Room past the end (320 +
        // 137 = 457) takes the longest run that can overshoot, which is then rejected.
        let total = num_litlen + num_offset;
        let mut lens = [0u8; NUM_LITLEN_SYMS + NUM_OFFSET_SYMS + 137];
        let mut i = 0;
        while i < total {
            if bits.available < MAX_PRECODE_LEN + 7 {
                bits.refill()?;
            }
            let (value, repeat) = match bits.decode(&precode) {
                len @ 0..=15 => (len as u8, 1),
                16 => {
                    let previous = *lens[..i].last().ok_or(Error::BadData)?;
                    (previous, 3 + bits.take(2) as usize)
                }
                17 => (0, 3 + bits.take(3) as usize),
                _ => (0, 11 + bits.take(7) as usize),
            };
            lens[i..i + repeat].fill(value);
            i += repeat;
        }
        if i != total {
            return Err(Error::BadData);
        }
        // libdeflate builds the offset code first, so an invalid one is reported first.
        if !self.offset.build(&lens[num_litlen..total], MAX_OFFSET_LEN) {
            return Err(Error::BadData);
        }
        if !self.litlen.build(&lens[..num_litlen], MAX_LITLEN_LEN) {
            return Err(Error::BadData);
        }
        Ok(())
    }
}

/// Copies a stored block into `output`.
fn copy_stored_block<O: OutputBuf>(bits: &mut BitReader, output: &mut O) -> Result<(), Error> {
    let start = bits.consumed_bytes()?;
    let input = bits.input;
    let header = input.get(start..start + 4).ok_or(Error::BadData)?;
    let len = u16::from_le_bytes([header[0], header[1]]);
    let nlen = u16::from_le_bytes([header[2], header[3]]);
    if len != !nlen {
        return Err(Error::BadData);
    }
    let len = usize::from(len);
    output.reserve_checked(len)?;
    let data = input
        .get(start + 4..start + 4 + len)
        .ok_or(Error::BadData)?;
    output.copy_stored(data);
    *bits = BitReader {
        pos: start + 4 + len,
        ..BitReader::new(input)
    };
    Ok(())
}

/// Decodes a Huffman-coded block into `output`.
fn decode_huffman_block<O: OutputBuf>(
    bits: &mut BitReader,
    litlen: &LitLenTable,
    offset: &OffsetTable,
    output: &mut O,
) -> Result<(), Error> {
    // Safe interior fast loop: >= 8 unread input bytes and >= 260 output bytes remain.
    while bits.pos + 8 <= bits.input.len() && output.ensure_fast_room() {
        bits.refill_fast();
        let symbol = bits.decode(litlen);
        if symbol < 256 {
            output.push_fast(symbol as u8);
            let next_entry = litlen.lookup(bits.buffer);
            let next_sym = usize::from(next_entry >> 4);
            if next_sym < 256 {
                bits.consume(u32::from(next_entry & 0xF));
                output.push_fast(next_sym as u8);
            }
            continue;
        }
        if symbol == END_OF_BLOCK {
            bits.trim_lookahead();
            return Ok(());
        }
        let (base, extra) = LENGTHS[symbol - 257];
        let length = (base + bits.take(extra)) as usize;
        let (dist_base, dist_extra) = DISTANCES[bits.decode(offset)];
        let distance = (dist_base + bits.take(dist_extra)) as usize;
        if distance > output.written() {
            return Err(Error::BadData);
        }
        output.copy_match_fast(distance, length);
    }
    bits.trim_lookahead();

    // Careful exterior loop near the end of the input or output buffer.
    loop {
        bits.refill()?;
        let symbol = bits.decode(litlen);
        if symbol < 256 {
            output.push_checked(symbol as u8)?;
            continue;
        }
        if symbol == END_OF_BLOCK {
            return Ok(());
        }

        let (base, extra) = LENGTHS[symbol - 257];
        let length = (base + bits.take(extra)) as usize;
        output.reserve_checked(length)?;
        let (dist_base, dist_extra) = DISTANCES[bits.decode(offset)];
        let distance = (dist_base + bits.take(dist_extra)) as usize;
        if distance > output.written() {
            return Err(Error::BadData);
        }
        output.copy_match_fast(distance, length);
    }
}

// ----------------------------------------------------------------------------------------
// Top-level functions & Adler-32

/// Decompresses the raw DEFLATE stream at the start of `input` into `output`
/// (`libdeflate_deflate_decompress_ex`).
///
/// Input after the end of the stream is ignored; [`Decompressed::bytes_read`] says where it
/// ended.
///
/// # Errors
///
/// [`Error::BadData`] if the input is not a valid stream, and [`Error::InsufficientSpace`] if
/// it decompresses to more than `output` holds.
pub fn deflate_decompress(input: &[u8], output: &mut [u8]) -> Result<Decompressed, Error> {
    Decompressor::new().deflate_decompress(input, output)
}

/// Decompresses the raw DEFLATE stream at the start of `input` into a newly allocated
/// `Vec<u8>` of at most `max_len` bytes.
///
/// # Errors
///
/// [`Error::BadData`] if the input is not a valid stream, and [`Error::InsufficientSpace`] if
/// the uncompressed output exceeds `max_len` bytes.
pub fn deflate_decompress_vec(input: &[u8], max_len: usize) -> Result<Vec<u8>, Error> {
    Decompressor::new().deflate_decompress_vec(input, max_len)
}

/// Decompresses the zlib stream at the start of `input` into `output`
/// (`libdeflate_zlib_decompress_ex`), checking its header and Adler-32 checksum.
///
/// Input after the end of the stream is ignored; [`Decompressed::bytes_read`] says where it
/// ended.
///
/// # Errors
///
/// [`Error::BadData`] if the input is not a valid zlib stream (a bad header, or a checksum
/// that does not match), and [`Error::InsufficientSpace`] if it decompresses to more than
/// `output` holds.
pub fn zlib_decompress(input: &[u8], output: &mut [u8]) -> Result<Decompressed, Error> {
    Decompressor::new().zlib_decompress(input, output)
}

/// Decompresses the zlib stream at the start of `input` into a newly allocated `Vec<u8>` of
/// at most `max_len` bytes, checking its header and Adler-32 checksum.
///
/// # Errors
///
/// [`Error::BadData`] if the input is not a valid zlib stream, and
/// [`Error::InsufficientSpace`] if the uncompressed output exceeds `max_len` bytes.
pub fn zlib_decompress_vec(input: &[u8], max_len: usize) -> Result<Vec<u8>, Error> {
    Decompressor::new().zlib_decompress_vec(input, max_len)
}

/// Decompresses the RFC 1952 gzip member at the start of `input` into `output`
/// (`libdeflate_gzip_decompress_ex`), handling optional header fields (`FEXTRA`, `FNAME`,
/// `FCOMMENT`, `FHCRC`) and validating the `CRC32` and `ISIZE` trailer.
///
/// Input after the end of the member is ignored; [`Decompressed::bytes_read`] says where it
/// ended.
///
/// # Errors
///
/// [`Error::BadData`] if the input is not a valid gzip stream, and
/// [`Error::InsufficientSpace`] if it decompresses to more than `output` holds.
pub fn gzip_decompress(input: &[u8], output: &mut [u8]) -> Result<Decompressed, Error> {
    Decompressor::new().gzip_decompress(input, output)
}

/// Decompresses the RFC 1952 gzip member at the start of `input` into a newly allocated
/// `Vec<u8>` of at most `max_len` bytes, validating all header fields and the `CRC32` and
/// `ISIZE` trailer.
///
/// # Errors
///
/// [`Error::BadData`] if the input is not a valid gzip stream, and
/// [`Error::InsufficientSpace`] if the uncompressed output exceeds `max_len` bytes.
pub fn gzip_decompress_vec(input: &[u8], max_len: usize) -> Result<Vec<u8>, Error> {
    Decompressor::new().gzip_decompress_vec(input, max_len)
}

/// The Adler-32 checksum of `data`, as zlib streams end with (`libdeflate_adler32` from 1).
#[must_use]
pub fn adler32(data: &[u8]) -> u32 {
    adler32_update(1, data)
}

/// Continues the Adler-32 checksum `adler` over `data` (`libdeflate_adler32`).
#[must_use]
pub fn adler32_update(adler: u32, data: &[u8]) -> u32 {
    const MODULUS: u32 = 65521;
    // The most bytes that can be summed before the sums might overflow.
    const CHUNK: usize = 5552;
    let (mut a, mut b) = (adler & 0xffff, adler >> 16);
    for chunk in data.chunks(CHUNK) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MODULUS;
        b %= MODULUS;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Level, deflate_compress, gzip_compress, zlib_compress};

    #[test]
    fn adler32_empty_and_multichunk_updates() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32_update(1, b""), 1);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);

        // Span several 5552-byte chunks to verify modulus boundaries and incremental updates.
        let large: Vec<u8> = (0..20_000u32).map(|i| (i.wrapping_mul(37)) as u8).collect();
        let whole = adler32(&large);
        assert_eq!(adler32_update(whole, b""), whole);

        let mut running = 1;
        for chunk in large.chunks(1337) {
            running = adler32_update(running, chunk);
        }
        assert_eq!(running, whole);

        let mut crossed = adler32(&large[..5552]);
        crossed = adler32_update(crossed, &large[5552..11_104]);
        crossed = adler32_update(crossed, &large[11_104..]);
        assert_eq!(crossed, whole);
    }

    #[test]
    fn direct_deflate_and_trailing_bytes() {
        let payload = b"repeating deflate payload! ".repeat(200);
        for raw_level in [0, 1, 2, 6, 9] {
            let level = Level::new(raw_level).unwrap();
            let raw_stream = deflate_compress(&payload, level);
            let stream_len = raw_stream.len();

            let mut with_trailing = raw_stream.clone();
            with_trailing.extend_from_slice(b"trailing garbage bytes after stream");

            let mut out = vec![0u8; payload.len()];
            let res = deflate_decompress(&with_trailing, &mut out).unwrap();
            assert_eq!(res.bytes_read, stream_len);
            assert!(res.bytes_read < with_trailing.len());
            assert_eq!(res.bytes_written, payload.len());
            assert_eq!(out, payload);

            // Also verify zlib_decompress and gzip_decompress with trailing bytes.
            let zstream = zlib_compress(&payload, level);
            let zlen = zstream.len();
            let mut z_with_trailing = zstream;
            z_with_trailing.extend_from_slice(b"extra-zlib-tail");
            out.fill(0);
            let zres = zlib_decompress(&z_with_trailing, &mut out).unwrap();
            assert_eq!(zres.bytes_read, zlen);
            assert!(zres.bytes_read < z_with_trailing.len());
            assert_eq!(zres.bytes_written, payload.len());
            assert_eq!(out, payload);

            let gstream = gzip_compress(&payload, level);
            let glen = gstream.len();
            let mut g_with_trailing = gstream;
            g_with_trailing.extend_from_slice(b"extra-gzip-tail");
            out.fill(0);
            let gres = gzip_decompress(&g_with_trailing, &mut out).unwrap();
            assert_eq!(gres.bytes_read, glen);
            assert!(gres.bytes_read < g_with_trailing.len());
            assert_eq!(gres.bytes_written, payload.len());
            assert_eq!(out, payload);
        }
    }

    #[test]
    fn overlapping_match_copies_all_distances() {
        // Exercise distance == 1 (RLE fill) and distance > 1 (doubling copy_within) across
        // both the fast interior loop and the careful tail loop.
        for dist in [1usize, 2, 3, 5, 7, 16, 31] {
            let pattern: Vec<u8> = (0..dist).map(|i| (i * 19 + 3) as u8).collect();
            let mut expected = Vec::new();
            while expected.len() < 4096 {
                expected.push(pattern[expected.len() % dist]);
            }
            let stream = deflate_compress(&expected, Level::BEST);
            let decoded = deflate_decompress_vec(&stream, expected.len()).unwrap();
            assert_eq!(decoded, expected, "distance {dist}");
        }
    }

    #[test]
    fn two_level_subtable_deep_huffman_codes() {
        // Fibonacci-like frequencies force codeword lengths up to 14-15 bits, exercising
        // subtable allocation and lookup in DecodeTable.
        let mut data = Vec::new();
        for sym in 0..30u8 {
            let count = (1usize << (sym / 2)).min(8192);
            data.resize(data.len() + count, sym);
        }
        // Mix symbols so matches don't collapse all literals.
        let mut mixed = Vec::with_capacity(data.len());
        let mut idx = 0usize;
        for _ in 0..data.len() {
            idx = (idx + 7919) % data.len();
            mixed.push(data[idx]);
        }
        let stream = zlib_compress(&mixed, Level::BEST);
        let decoded = zlib_decompress_vec(&stream, mixed.len() + 100).unwrap();
        assert_eq!(decoded, mixed);
    }
}
