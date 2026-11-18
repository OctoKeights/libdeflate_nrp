//! The decompressor: libdeflate's `deflate_decompress.c` (with `decompress_template.h`),
//! `zlib_decompress.c` and `adler32.c`.
//!
//! libdeflate decodes with a fast loop away from the ends of its buffers and a careful one
//! near them; both produce the same output and fail for the same reasons. This decoder works
//! as the careful loop does, symbol by symbol, refilling its bits at the same points, so that
//! a damaged stream fails in the same way. Input libdeflate reads past the end of its buffer is
//! taken as zeros, up to eight bytes.

use crate::{Decompressed, Error};

const NUM_PRECODE_SYMS: usize = 19;
const NUM_LITLEN_SYMS: usize = 288;
const NUM_OFFSET_SYMS: usize = 32;
const MAX_PRECODE_LEN: u32 = 7;
const MAX_LITLEN_LEN: u32 = 15;
const MAX_OFFSET_LEN: u32 = 15;
const END_OF_BLOCK: usize = 256;

/// A refill fills the bit buffer to at least this many bits.
const REFILL_TARGET: u32 = 56;

/// The most zero bytes that may be read past the end of the input.
const MAX_OVERREAD: usize = 8;

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

/// A Huffman code as a lookup table: for every `bits`-bit window of the stream (read from its
/// least significant bit), the symbol whose codeword starts it and that codeword's length.
struct DecodeTable {
    bits: u32,
    entries: Vec<(u16, u8)>,
}

impl DecodeTable {
    /// The canonical code with codeword lengths `lens`, or `None` where libdeflate rejects
    /// them: an over-subscribed code, or an incomplete one other than a single one-bit
    /// codeword. (An empty code, and a code of a single one-bit codeword, both decode every
    /// one-bit window to a single symbol, as libdeflate's do.)
    fn new(lens: &[u8], max_len: u32) -> Option<DecodeTable> {
        let mut len_counts = [0u32; 16];
        for &len in lens {
            len_counts[usize::from(len)] += 1;
        }
        let longest = (1..=max_len)
            .rev()
            .find(|&len| len_counts[len as usize] != 0)
            .unwrap_or(1);
        let codespace_used = len_counts[1..=longest as usize]
            .iter()
            .fold(0, |used, &count| (used << 1) + count);
        let codespace = 1 << longest;

        if codespace_used > codespace {
            return None;
        }
        if codespace_used < codespace {
            let symbol = if codespace_used == 0 {
                0
            } else if codespace_used == codespace / 2 && len_counts[1] == 1 {
                lens.iter().position(|&len| len != 0)? as u16
            } else {
                return None;
            };
            return Some(DecodeTable {
                bits: 1,
                entries: vec![(symbol, 1); 2],
            });
        }

        // Canonical codewords in order of length, then symbol; each fills every window that
        // starts with it.
        let mut entries = vec![(0u16, 0u8); codespace as usize];
        let mut codeword = 0u32;
        for len in 1..=longest {
            for (symbol, _) in lens
                .iter()
                .enumerate()
                .filter(|&(_, &l)| u32::from(l) == len)
            {
                let first_window = codeword.reverse_bits() >> (32 - len);
                for window in (first_window..codespace).step_by(1 << len) {
                    entries[window as usize] = (symbol as u16, len as u8);
                }
                codeword += 1;
            }
            codeword <<= 1;
        }
        Some(DecodeTable {
            bits: longest,
            entries,
        })
    }

    /// The table of a static block's code for literals and lengths, or for offsets.
    fn fixed_litlen() -> DecodeTable {
        let mut lens = [0u8; NUM_LITLEN_SYMS];
        lens[..144].fill(8);
        lens[144..256].fill(9);
        lens[256..280].fill(7);
        lens[280..].fill(8);
        DecodeTable::new(&lens, MAX_LITLEN_LEN).expect("the fixed code is complete")
    }

    fn fixed_offset() -> DecodeTable {
        DecodeTable::new(&[5; NUM_OFFSET_SYMS], MAX_OFFSET_LEN).expect("the fixed code is complete")
    }
}

/// The input as a stream of bits, least significant first, refilled a byte at a time as
/// libdeflate refills it.
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
    fn decode(&mut self, table: &DecodeTable) -> usize {
        let (symbol, len) = table.entries[self.peek(table.bits) as usize];
        self.consume(u32::from(len));
        usize::from(symbol)
    }

    /// The input position after the last bit consumed, discounting whole bytes still
    /// buffered; an error if the stream needed more bytes than the input holds.
    fn consumed_bytes(&self) -> Result<usize, Error> {
        let unused = (self.available / 8) as usize;
        let unused_real = unused.checked_sub(self.overread).ok_or(Error::BadData)?;
        Ok(self.pos - unused_real)
    }
}

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
    let mut bits = BitReader::new(input);
    let mut written = 0;
    loop {
        bits.refill()?;
        let is_final = bits.peek(1) == 1;
        match (bits.buffer >> 1) & 3 {
            0 => {
                bits.consume(3);
                written = copy_stored_block(&mut bits, output, written)?;
            }
            1 => {
                bits.consume(3);
                let (litlen, offset) = (DecodeTable::fixed_litlen(), DecodeTable::fixed_offset());
                written = decode_huffman_block(&mut bits, &litlen, &offset, output, written)?;
            }
            2 => {
                let (litlen, offset) = read_dynamic_tables(&mut bits)?;
                written = decode_huffman_block(&mut bits, &litlen, &offset, output, written)?;
            }
            _ => return Err(Error::BadData),
        }
        if is_final {
            break;
        }
    }
    Ok(Decompressed {
        bytes_read: bits.consumed_bytes()?,
        bytes_written: written,
    })
}

/// Reads a dynamic block's header (whose first 3 bits are still buffered) and builds its
/// two codes: for literals and lengths, and for offsets.
fn read_dynamic_tables(bits: &mut BitReader) -> Result<(DecodeTable, DecodeTable), Error> {
    // The header fits in the refilled buffer: 3 + 5 + 5 + 4 bits, then the first precode
    // length.
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
    let precode = DecodeTable::new(&precode_lens, MAX_PRECODE_LEN).ok_or(Error::BadData)?;

    // Both codes' lengths, run-length coded with the precode. Room past the end takes the
    // longest run that can overshoot, which is then rejected.
    let total = num_litlen + num_offset;
    let mut lens = vec![0u8; total + 137];
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
    let offset =
        DecodeTable::new(&lens[num_litlen..total], MAX_OFFSET_LEN).ok_or(Error::BadData)?;
    let litlen = DecodeTable::new(&lens[..num_litlen], MAX_LITLEN_LEN).ok_or(Error::BadData)?;
    Ok((litlen, offset))
}

/// Copies a stored block to `output` from `written` on; returns the new end of the output.
fn copy_stored_block(
    bits: &mut BitReader,
    output: &mut [u8],
    written: usize,
) -> Result<usize, Error> {
    // Back to the byte after the block header, discarding the buffer.
    let start = bits.consumed_bytes()?;
    let input = bits.input;
    let header = input.get(start..start + 4).ok_or(Error::BadData)?;
    let len = u16::from_le_bytes([header[0], header[1]]);
    let nlen = u16::from_le_bytes([header[2], header[3]]);
    if len != !nlen {
        return Err(Error::BadData);
    }
    let len = usize::from(len);
    if len > output.len() - written {
        return Err(Error::InsufficientSpace);
    }
    let data = input
        .get(start + 4..start + 4 + len)
        .ok_or(Error::BadData)?;
    output[written..written + len].copy_from_slice(data);
    *bits = BitReader {
        pos: start + 4 + len,
        ..BitReader::new(input)
    };
    Ok(written + len)
}

/// Decodes a Huffman-coded block into `output` from `written` on; returns the new end of the
/// output.
fn decode_huffman_block(
    bits: &mut BitReader,
    litlen: &DecodeTable,
    offset: &DecodeTable,
    output: &mut [u8],
    mut written: usize,
) -> Result<usize, Error> {
    loop {
        bits.refill()?;
        let symbol = bits.decode(litlen);
        if symbol < 256 {
            *output.get_mut(written).ok_or(Error::InsufficientSpace)? = symbol as u8;
            written += 1;
            continue;
        }
        if symbol == END_OF_BLOCK {
            return Ok(written);
        }

        let (base, extra) = LENGTHS[symbol - 257];
        let length = (base + bits.take(extra)) as usize;
        if length > output.len() - written {
            return Err(Error::InsufficientSpace);
        }
        // A refill always leaves room for an offset after a length.
        let (base, extra) = DISTANCES[bits.decode(offset)];
        let distance = (base + bits.take(extra)) as usize;
        if distance > written {
            return Err(Error::BadData);
        }
        let from = written - distance;
        if distance >= length {
            output.copy_within(from..from + length, written);
        } else {
            // An overlapping copy repeats the last `distance` bytes, so it goes a byte at a
            // time.
            for k in 0..length {
                output[written + k] = output[from + k];
            }
        }
        written += length;
    }
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

    let deflated = deflate_decompress(&input[HEADER..input.len() - TRAILER], output)?;
    let trailer_at = HEADER + deflated.bytes_read;
    let stored = &input[trailer_at..trailer_at + TRAILER];
    let expected = u32::from_be_bytes([stored[0], stored[1], stored[2], stored[3]]);
    if adler32(&output[..deflated.bytes_written]) != expected {
        return Err(Error::BadData);
    }
    Ok(Decompressed {
        bytes_read: trailer_at + TRAILER,
        bytes_written: deflated.bytes_written,
    })
}
