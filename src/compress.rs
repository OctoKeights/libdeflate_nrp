//! The compressor: libdeflate's `deflate_compress.c` (with `hc_matchfinder.h` and
//! `matchfinder_common.h`) and `zlib_compress.c`, for levels 0 and 2 to 9.
//!
//! Level 0 stores the data. Levels 2 to 9 find matches in hash chains and choose between
//! them greedily (2 to 4), lazily (5 to 7) or looking two positions ahead (8 and 9), ending
//! blocks where the data's statistics change and writing each as whichever kind of block is
//! smallest.

use crate::{Level, adler32};

const MIN_MATCH_LEN: u32 = 3;
const MAX_MATCH_LEN: u32 = 258;
const NUM_LITLEN_SYMS: usize = 288;
const NUM_OFFSET_SYMS: usize = 32;
const NUM_PRECODE_SYMS: usize = 19;
const NUM_LITERALS: usize = 256;
const END_OF_BLOCK: usize = 256;
const FIRST_LENGTH_SYM: usize = 257;

const MIN_BLOCK_LENGTH: usize = 5000;
const SOFT_MAX_BLOCK_LENGTH: usize = 300_000;
const MAX_SEQUENCES: usize = 50_000;

const MAX_LITLEN_LEN: u32 = 14;
const MAX_OFFSET_LEN: u32 = 15;
const MAX_PRECODE_LEN: u32 = 7;

/// The longest stored block.
const MAX_STORED_LEN: usize = u16::MAX as usize;

const LENGTH_BASE: [u32; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA_BITS: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const OFFSET_BASE: [u32; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const OFFSET_EXTRA_BITS: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const PRECODE_ORDER: [usize; NUM_PRECODE_SYMS] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];
const PRECODE_EXTRA_BITS: [u32; NUM_PRECODE_SYMS] =
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 7];

/// The length slot of every match length.
const LENGTH_SLOTS: [u8; MAX_MATCH_LEN as usize + 1] = {
    let mut slots = [0u8; MAX_MATCH_LEN as usize + 1];
    let mut slot = 0;
    let mut len = MIN_MATCH_LEN as usize;
    while len <= MAX_MATCH_LEN as usize {
        while slot + 1 < LENGTH_BASE.len() && LENGTH_BASE[slot + 1] as usize <= len {
            slot += 1;
        }
        slots[len] = slot as u8;
        len += 1;
    }
    slots
};

/// The offset slot of a match offset, 1 to 32768.
#[inline]
fn offset_slot(offset: u32) -> usize {
    let n = offset - 1;
    if n < 4 {
        n as usize
    } else {
        // Two slots per power of two: the top bit's position, and the bit below it.
        let top = n.ilog2();
        (2 * top + ((n >> (top - 1)) & 1)) as usize
    }
}

// ----------------------------------------------------------------------------------------
// Huffman codes

/// Sorting and tree building pack a symbol into the low bits of each entry and a frequency,
/// parent index or depth into the high bits.
const SYMBOL_BITS: u32 = 10;
const SYMBOL_MASK: u32 = (1 << SYMBOL_BITS) - 1;
const VALUE_MASK: u32 = !SYMBOL_MASK;

/// Sorts the symbols of nonzero frequency by frequency, then by symbol, into `sorted` (each
/// packed with its frequency) and gives every unused symbol length 0. Returns how many
/// symbols are used.
fn sort_symbols(freqs: &[u32], lens: &mut [u8], sorted: &mut [u32]) -> usize {
    let mut used = 0;
    for (symbol, &freq) in freqs.iter().enumerate() {
        if freq == 0 {
            lens[symbol] = 0;
        } else {
            sorted[used] = symbol as u32 | (freq << SYMBOL_BITS);
            used += 1;
        }
    }
    // Every entry is distinct (each holds its own symbol), so any sort gives libdeflate's
    // order.
    sorted[..used].sort_unstable();
    used
}

/// Builds the internal nodes of the Huffman tree over the `count` sorted leaves in `nodes`,
/// in place: entry `i` becomes the frequency of internal node `i`, and each node consumed is
/// marked with its parent's index.
fn build_tree(nodes: &mut [u32], count: usize) {
    let last = count - 1;
    let (mut leaf, mut branch, mut end) = (0, 0, 0);
    loop {
        let freq = |entry: u32| entry & VALUE_MASK;
        let with_parent =
            |entry: u32, parent: usize| ((parent as u32) << SYMBOL_BITS) | (entry & SYMBOL_MASK);
        let merged;
        if leaf < last && (branch == end || freq(nodes[leaf + 1]) <= freq(nodes[branch])) {
            // Two leaves.
            merged = freq(nodes[leaf]).wrapping_add(freq(nodes[leaf + 1]));
            leaf += 2;
        } else if branch + 2 <= end && (leaf > last || freq(nodes[branch + 1]) < freq(nodes[leaf]))
        {
            // Two internal nodes.
            merged = freq(nodes[branch]).wrapping_add(freq(nodes[branch + 1]));
            nodes[branch] = with_parent(nodes[branch], end);
            nodes[branch + 1] = with_parent(nodes[branch + 1], end);
            branch += 2;
        } else {
            // A leaf and an internal node.
            merged = freq(nodes[leaf]).wrapping_add(freq(nodes[branch]));
            nodes[branch] = with_parent(nodes[branch], end);
            leaf += 1;
            branch += 1;
        }
        nodes[end] = merged | (nodes[end] & SYMBOL_MASK);
        end += 1;
        if end >= last {
            break;
        }
    }
}

/// Counts the codewords of each length in the tree whose root is `nodes[root]`, limiting
/// lengths to `max_len` as libdeflate does.
fn length_counts(nodes: &mut [u32], root: usize, max_len: u32) -> [u32; 16] {
    let mut counts = [0u32; 16];
    counts[1] = 2;
    nodes[root] &= SYMBOL_MASK;
    for node in (0..root).rev() {
        let parent = (nodes[node] >> SYMBOL_BITS) as usize;
        let mut depth = (nodes[parent] >> SYMBOL_BITS) + 1;
        nodes[node] = (nodes[node] & SYMBOL_MASK) | (depth << SYMBOL_BITS);
        if depth >= max_len {
            // Too deep: hang it below the deepest leaf that can take it instead.
            depth = (1..max_len)
                .rev()
                .find(|&d| counts[d as usize] != 0)
                .unwrap_or(0);
        }
        counts[depth as usize] -= 1;
        counts[depth as usize + 1] += 2;
    }
    counts
}

/// Assigns lengths to the sorted symbols (the rarest the longest) and fills `codewords`
/// with their canonical codewords, bit-reversed for writing.
fn assign_codewords(
    sorted: &[u32],
    lens: &mut [u8],
    counts: &[u32; 16],
    max_len: u32,
    codewords: &mut [u32],
) {
    let mut symbols = sorted.iter().map(|&entry| (entry & SYMBOL_MASK) as usize);
    for len in (1..=max_len).rev() {
        for symbol in symbols.by_ref().take(counts[len as usize] as usize) {
            lens[symbol] = len as u8;
        }
    }
    let mut next = [0u32; 16];
    for len in 2..=max_len as usize {
        next[len] = (next[len - 1] + counts[len - 1]) << 1;
    }
    for (codeword, &len) in codewords.iter_mut().zip(lens.iter()) {
        let len = u32::from(len);
        *codeword = if len == 0 {
            0
        } else {
            next[len as usize].reverse_bits() >> (32 - len)
        };
        next[len as usize] += 1;
    }
}

/// Builds a canonical Huffman code for `freqs` of codewords no longer than `max_len`, into
/// `lens` and `codewords`.
fn make_huffman_code(max_len: u32, freqs: &[u32], lens: &mut [u8], codewords: &mut [u32]) {
    // `codewords` doubles as the work area for sorting and building the tree.
    let used = sort_symbols(freqs, lens, codewords);
    if used < 2 {
        // DEFLATE needs at least two codewords: give symbol 0 and one other a bit each.
        let symbol = if used == 1 {
            (codewords[0] & SYMBOL_MASK) as usize
        } else {
            0
        };
        let other = if symbol == 0 { 1 } else { symbol };
        codewords[0] = 0;
        lens[0] = 1;
        codewords[other] = 1;
        lens[other] = 1;
        return;
    }
    build_tree(codewords, used);
    let counts = length_counts(codewords, used - 2, max_len);
    // The tree's entries still hold the sorted symbols in their low bits.
    let mut sorted = [0u32; NUM_LITLEN_SYMS];
    sorted[..used].copy_from_slice(&codewords[..used]);
    assign_codewords(&sorted[..used], lens, &counts, max_len, codewords);
}

/// The two codes of a block. Their lengths share one array, literal/length code first, as
/// the block header writes them.
#[derive(Clone)]
struct Codes {
    litlen_codewords: [u32; NUM_LITLEN_SYMS],
    offset_codewords: [u32; NUM_OFFSET_SYMS],
    lens: [u8; NUM_LITLEN_SYMS + NUM_OFFSET_SYMS],
}

impl Codes {
    const EMPTY: Codes = Codes {
        litlen_codewords: [0; NUM_LITLEN_SYMS],
        offset_codewords: [0; NUM_OFFSET_SYMS],
        lens: [0; NUM_LITLEN_SYMS + NUM_OFFSET_SYMS],
    };

    fn build(freqs: &Frequencies) -> Codes {
        let mut codes = Codes::EMPTY;
        codes.rebuild(freqs);
        codes
    }

    fn rebuild(&mut self, freqs: &Frequencies) {
        let (litlen_lens, offset_lens) = self.lens.split_at_mut(NUM_LITLEN_SYMS);
        make_huffman_code(
            MAX_LITLEN_LEN,
            &freqs.litlen,
            litlen_lens,
            &mut self.litlen_codewords,
        );
        make_huffman_code(
            MAX_OFFSET_LEN,
            &freqs.offset,
            offset_lens,
            &mut self.offset_codewords,
        );
    }

    fn litlen_len(&self, symbol: usize) -> u32 {
        u32::from(self.lens[symbol])
    }

    fn offset_len(&self, slot: usize) -> u32 {
        u32::from(self.lens[NUM_LITLEN_SYMS + slot])
    }

    /// The fixed codes of a static block, as libdeflate builds them: from frequencies that
    /// give the format's codeword lengths.
    fn fixed() -> Codes {
        let mut freqs = Frequencies::default();
        for (symbol, freq) in freqs.litlen.iter_mut().enumerate() {
            let len = match symbol {
                144..=255 => 9,
                256..=279 => 7,
                _ => 8, // 0 to 143, and 280 to 287
            };
            *freq = 1 << (9 - len);
        }
        freqs.offset = [1; NUM_OFFSET_SYMS];
        Codes::build(&freqs)
    }
}

/// Symbol frequencies of the block being chosen.
#[derive(Clone)]
struct Frequencies {
    litlen: [u32; NUM_LITLEN_SYMS],
    offset: [u32; NUM_OFFSET_SYMS],
}

impl Default for Frequencies {
    fn default() -> Self {
        Frequencies {
            litlen: [0; NUM_LITLEN_SYMS],
            offset: [0; NUM_OFFSET_SYMS],
        }
    }
}

// ----------------------------------------------------------------------------------------
// Output

/// Writes bits least significant first, a byte at a time.
struct BitWriter {
    bytes: Vec<u8>,
    buffer: u64,
    count: u32,
}

impl BitWriter {
    fn new(bytes: Vec<u8>) -> Self {
        BitWriter {
            bytes,
            buffer: 0,
            count: 0,
        }
    }

    #[inline]
    fn write(&mut self, bits: u32, n: u32) {
        self.buffer |= u64::from(bits) << self.count;
        self.count += n;
        while self.count >= 8 {
            self.bytes.push(self.buffer as u8);
            self.buffer >>= 8;
            self.count -= 8;
        }
    }

    /// Writes the remaining bits, padded with zeros to a byte.
    fn finish(mut self) -> Vec<u8> {
        if self.count != 0 {
            self.bytes.push(self.buffer as u8);
        }
        self.bytes
    }
}

// ----------------------------------------------------------------------------------------
// Hash chains

/// The match finder's window: matches reach back at most this far.
const WINDOW_SIZE: usize = 1 << 15;
const HASH3_BITS: u32 = 15;
const HASH4_BITS: u32 = 16;
/// A chain link that points nowhere: before the start of the window.
const NO_POSITION: i16 = i16::MIN;

/// Hash chains over the input (`hc_matchfinder`): for the latest position of every 3-byte
/// and 4-byte hash, and from each position to the previous one with the same 4-byte hash.
///
/// Positions are stored relative to `base` as `i16`; when they reach the window size, every
/// position is moved back by it ("sliding the window"), and those that fall out become
/// [`NO_POSITION`].
struct HashChains {
    hash3_heads: Vec<i16>,
    hash4_heads: Vec<i16>,
    previous: Vec<i16>,
    base: usize,
    /// The hashes of the next position, computed one step ahead.
    next_hash3: u32,
    next_hash4: u32,
}

/// The lengths matches may have, which shrink near the end of the input and stay shrunk, so
/// that no match reaches past it.
#[derive(Clone, Copy)]
struct MatchLimits {
    /// The longest match allowed.
    max_len: u32,
    /// A match this long ends a search.
    nice_len: u32,
}

impl MatchLimits {
    fn new(nice_len: u32) -> Self {
        Self {
            max_len: MAX_MATCH_LEN,
            nice_len: nice_len.min(MAX_MATCH_LEN),
        }
    }

    /// Shrinks the limits so that no match from a position `remaining` bytes from the end
    /// reaches past it.
    fn shrink_near_end(&mut self, remaining: usize) {
        if remaining < MAX_MATCH_LEN as usize {
            self.max_len = remaining as u32;
            self.nice_len = self.nice_len.min(self.max_len);
        }
    }

    /// A search within these limits for a match longer than `min_beat`, following at most
    /// `max_depth` chain links.
    fn search(self, min_beat: u32, max_depth: u32) -> Search {
        Search {
            min_beat,
            max_len: self.max_len,
            nice_len: self.nice_len,
            max_depth,
        }
    }
}

/// The bounds of one search.
#[derive(Clone, Copy)]
struct Search {
    /// Only matches longer than this are of interest.
    min_beat: u32,
    /// The longest match allowed.
    max_len: u32,
    /// A match this long ends the search.
    nice_len: u32,
    /// The most chain links to follow.
    max_depth: u32,
}

#[inline]
fn hash(seq: u32, bits: u32) -> u32 {
    seq.wrapping_mul(0x1E35_A7BD) >> (32 - bits)
}

#[inline]
fn load_u32(input: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([input[at], input[at + 1], input[at + 2], input[at + 3]])
}

/// How far the bytes at `a` and `b` agree, counting from `start_len` up to `max_len`.
#[inline]
fn match_len(input: &[u8], a: usize, b: usize, start_len: u32, max_len: u32) -> u32 {
    let (start, max) = (start_len as usize, max_len as usize);
    let same = input[a + start..a + max]
        .iter()
        .zip(&input[b + start..b + max])
        .take_while(|(x, y)| x == y)
        .count();
    start_len + same as u32
}

/// Moves every position in `table` back by the window size; those that fall before the
/// window become [`NO_POSITION`].
fn slide(table: &mut [i16]) {
    for position in table {
        // Negative positions saturate to the minimum; others lose their top bit.
        *position = NO_POSITION | (*position & !(*position >> 15));
    }
}

impl HashChains {
    fn new() -> Self {
        HashChains {
            hash3_heads: vec![NO_POSITION; 1 << HASH3_BITS],
            hash4_heads: vec![NO_POSITION; 1 << HASH4_BITS],
            previous: vec![NO_POSITION; WINDOW_SIZE],
            base: 0,
            next_hash3: 0,
            next_hash4: 0,
        }
    }

    fn slide_window(&mut self) {
        slide(&mut self.hash3_heads);
        slide(&mut self.hash4_heads);
        slide(&mut self.previous);
        self.base += WINDOW_SIZE;
    }

    /// The position of `pos` relative to the window, sliding it first if needed.
    #[inline]
    fn relative(&mut self, pos: usize) -> usize {
        if pos - self.base == WINDOW_SIZE {
            self.slide_window();
        }
        pos - self.base
    }

    #[inline]
    fn advance_hashes(&mut self, input: &[u8], pos: usize) {
        let seq = load_u32(input, pos);
        self.next_hash3 = hash(seq & 0xFF_FFFF, HASH3_BITS);
        self.next_hash4 = hash(seq, HASH4_BITS);
    }

    /// Inserts `pos` and finds the longest match there longer than `search.min_beat`, as
    /// `(length, offset)`; `(min_beat, 0)` if there is none.
    fn longest_match(&mut self, input: &[u8], pos: usize, search: Search) -> (u32, u32) {
        let cur = self.relative(pos);
        let base = self.base as isize;
        let absolute = |node: i16| (base + isize::from(node)) as usize;
        let cutoff = (cur as u32).wrapping_sub(WINDOW_SIZE as u32) as i16;
        let mut best_len = search.min_beat;
        let mut best_at = pos;

        'search: {
            if search.max_len < 5 {
                break 'search;
            }
            let (hash3, hash4) = (self.next_hash3 as usize, self.next_hash4 as usize);
            let node3 = self.hash3_heads[hash3];
            let mut node4 = self.hash4_heads[hash4];
            self.hash3_heads[hash3] = cur as i16;
            self.hash4_heads[hash4] = cur as i16;
            self.previous[cur] = node4;
            self.advance_hashes(input, pos + 1);

            let mut depth_left = search.max_depth;
            let previous = &self.previous;
            // Follows a chain link; `false` when the chain or the search depth runs out.
            let mut step = |node: &mut i16| {
                *node = previous[usize::from(*node as u16) & (WINDOW_SIZE - 1)];
                depth_left = depth_left.wrapping_sub(1);
                *node > cutoff && depth_left != 0
            };

            if best_len < 4 {
                if node3 <= cutoff {
                    break 'search;
                }
                let seq4 = load_u32(input, pos);
                if best_len < 3 {
                    let at = absolute(node3);
                    if load_u32(input, at) & 0xFF_FFFF == seq4 & 0xFF_FFFF {
                        best_len = 3;
                        best_at = at;
                    }
                }
                if node4 <= cutoff {
                    break 'search;
                }
                // The first match of at least 4 bytes.
                while load_u32(input, absolute(node4)) != seq4 {
                    if !step(&mut node4) {
                        break 'search;
                    }
                }
                best_at = absolute(node4);
                best_len = match_len(input, pos, best_at, 4, search.max_len);
                if best_len >= search.nice_len || !step(&mut node4) {
                    break 'search;
                }
            } else if node4 <= cutoff || best_len >= search.nice_len {
                break 'search;
            }

            // Longer matches: a candidate must agree with the best so far at its last four
            // bytes and its first four before it is measured.
            loop {
                let at = loop {
                    let at = absolute(node4);
                    let tail = best_len as usize - 3;
                    if load_u32(input, at + tail) == load_u32(input, pos + tail)
                        && load_u32(input, at) == load_u32(input, pos)
                    {
                        break at;
                    }
                    if !step(&mut node4) {
                        break 'search;
                    }
                };
                let len = match_len(input, pos, at, 4, search.max_len);
                if len > best_len {
                    best_len = len;
                    best_at = at;
                    if best_len >= search.nice_len {
                        break 'search;
                    }
                }
                if !step(&mut node4) {
                    break 'search;
                }
            }
        }
        (best_len, (pos - best_at) as u32)
    }

    /// Inserts the `count` positions from `pos` without searching them.
    fn skip(&mut self, input: &[u8], mut pos: usize, count: u32) {
        if count as usize + 5 > input.len() - pos {
            return;
        }
        for _ in 0..count {
            let cur = self.relative(pos);
            self.hash3_heads[self.next_hash3 as usize] = cur as i16;
            self.previous[cur] = self.hash4_heads[self.next_hash4 as usize];
            self.hash4_heads[self.next_hash4 as usize] = cur as i16;
            pos += 1;
            self.advance_hashes(input, pos);
        }
    }
}

// ----------------------------------------------------------------------------------------
// Block splitting

const NUM_LITERAL_OBSERVATION_TYPES: usize = 8;
const NUM_OBSERVATION_TYPES: usize = NUM_LITERAL_OBSERVATION_TYPES + 2;
const OBSERVATIONS_PER_CHECK: u32 = 512;

/// Coarse statistics of the block so far, to end it where the data changes.
#[derive(Default)]
struct BlockSplitter {
    new_observations: [u32; NUM_OBSERVATION_TYPES],
    observations: [u32; NUM_OBSERVATION_TYPES],
    num_new: u32,
    num_old: u32,
}

impl BlockSplitter {
    fn observe_literal(&mut self, literal: u8) {
        self.new_observations[usize::from(((literal >> 5) & 0x6) | (literal & 1))] += 1;
        self.num_new += 1;
    }

    fn observe_match(&mut self, length: u32) {
        self.new_observations[NUM_LITERAL_OBSERVATION_TYPES + usize::from(length >= 9)] += 1;
        self.num_new += 1;
    }

    fn merge_new_observations(&mut self) {
        for (old, new) in self.observations.iter_mut().zip(&mut self.new_observations) {
            *old += *new;
            *new = 0;
        }
        self.num_old += self.num_new;
        self.num_new = 0;
    }

    /// Whether the recent observations differ enough from the earlier ones to end the block.
    fn differs_enough(&mut self, block_length: u32) -> bool {
        if self.num_old > 0 {
            let total_delta = self
                .observations
                .iter()
                .zip(&self.new_observations)
                .map(|(&old, &new)| {
                    let expected = old.wrapping_mul(self.num_new);
                    let actual = new.wrapping_mul(self.num_old);
                    actual.abs_diff(expected)
                })
                .fold(0u32, u32::wrapping_add);
            let num_items = self.num_old + self.num_new;
            let mut cutoff = (self.num_new.wrapping_mul(200) / 512).wrapping_mul(self.num_old);
            // Short blocks need a larger difference.
            if block_length < 10_000 && num_items < 8192 {
                let extra = u64::from(cutoff) * u64::from(8192 - num_items) / 8192;
                cutoff = cutoff.wrapping_add(extra as u32);
            }
            let length_bias = (block_length / 4096).wrapping_mul(self.num_old);
            if total_delta.wrapping_add(length_bias) >= cutoff {
                return true;
            }
        }
        self.merge_new_observations();
        false
    }

    /// Whether to end the block at `pos`.
    fn should_end_block(&mut self, block_start: usize, pos: usize, end: usize) -> bool {
        self.num_new >= OBSERVATIONS_PER_CHECK
            && pos - block_start >= MIN_BLOCK_LENGTH
            && end - pos >= MIN_BLOCK_LENGTH
            && self.differs_enough((pos - block_start) as u32)
    }
}

/// The shortest match worth taking, by how many distinct literals the data uses and how
/// deeply the level searches.
fn choose_min_match_len(num_used_literals: u32, max_search_depth: u32) -> u32 {
    const MIN_LENS: [u8; 80] = [
        9, 9, 9, 9, 9, 9, 8, 8, 7, 7, 6, 6, 6, 6, 6, 6, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5,
        5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
        4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    ];
    let Some(&min_len) = MIN_LENS.get(num_used_literals as usize) else {
        return MIN_MATCH_LEN;
    };
    let cap = match max_search_depth {
        0..5 => 4,
        5..10 => 5,
        10..16 => 7,
        _ => u32::MAX,
    };
    u32::from(min_len).min(cap)
}

/// The shortest match worth taking for a block starting with `data`.
fn initial_min_match_len(data: &[u8], max_search_depth: u32) -> u32 {
    if data.len() < 512 {
        return MIN_MATCH_LEN;
    }
    let mut used = [false; 256];
    for &byte in &data[..data.len().min(4096)] {
        used[usize::from(byte)] = true;
    }
    let num_used = used.iter().filter(|&&u| u).count() as u32;
    choose_min_match_len(num_used, max_search_depth)
}

/// The shortest match worth taking, from the literals of the block so far.
fn updated_min_match_len(freqs: &Frequencies, max_search_depth: u32) -> u32 {
    let literals = &freqs.litlen[..NUM_LITERALS];
    let total = literals.iter().fold(0u32, |sum, &f| sum.wrapping_add(f));
    let cutoff = total >> 10;
    let num_used = literals.iter().filter(|&&f| f > cutoff).count() as u32;
    choose_min_match_len(num_used, max_search_depth)
}

// ----------------------------------------------------------------------------------------
// The compressor

/// A run of literals and the match that follows it (none, for the last of a block).
#[derive(Clone, Copy, Default)]
struct Sequence {
    literals: u32,
    length: u32,
    offset: u32,
    offset_slot: usize,
}

/// How a level parses: which strategy, how deeply it searches, and what match ends a search.
#[derive(Clone, Copy)]
enum Parser {
    Greedy,
    Lazy,
    Lazy2,
}

/// The run-length coded codeword lengths of a dynamic block's header.
struct Precode {
    freqs: [u32; NUM_PRECODE_SYMS],
    codewords: [u32; NUM_PRECODE_SYMS],
    lens: [u8; NUM_PRECODE_SYMS],
    /// Each item: a precode symbol, with its extra bits above bit 5.
    items: Vec<u32>,
    num_litlen: usize,
    num_offset: usize,
    num_explicit_lens: usize,
}

struct Compressor {
    parser: Parser,
    max_search_depth: u32,
    nice_match_len: u32,
    freqs: Frequencies,
    splitter: BlockSplitter,
    codes: Codes,
    fixed_codes: Codes,
    precode: Precode,
    /// The codeword of every match length with its extra bits, and its total length.
    length_codewords: [u32; MAX_MATCH_LEN as usize + 1],
    length_codeword_lens: [u32; MAX_MATCH_LEN as usize + 1],
    chains: HashChains,
    sequences: Vec<Sequence>,
    /// The sequence being built.
    current: usize,
}

impl Compressor {
    fn new(level: Level) -> Self {
        let (parser, max_search_depth, nice_match_len) = match level.get() {
            2 => (Parser::Greedy, 6, 10),
            3 => (Parser::Greedy, 12, 14),
            4 => (Parser::Greedy, 16, 30),
            5 => (Parser::Lazy, 16, 30),
            6 => (Parser::Lazy, 35, 65),
            7 => (Parser::Lazy, 100, 130),
            8 => (Parser::Lazy2, 300, MAX_MATCH_LEN),
            _ => (Parser::Lazy2, 600, MAX_MATCH_LEN),
        };
        Compressor {
            parser,
            max_search_depth,
            nice_match_len,
            freqs: Frequencies::default(),
            splitter: BlockSplitter::default(),
            codes: Codes::EMPTY,
            fixed_codes: Codes::fixed(),
            precode: Precode {
                freqs: [0; NUM_PRECODE_SYMS],
                codewords: [0; NUM_PRECODE_SYMS],
                lens: [0; NUM_PRECODE_SYMS],
                items: Vec::new(),
                num_litlen: 0,
                num_offset: 0,
                num_explicit_lens: 0,
            },
            length_codewords: [0; MAX_MATCH_LEN as usize + 1],
            length_codeword_lens: [0; MAX_MATCH_LEN as usize + 1],
            chains: HashChains::new(),
            sequences: vec![Sequence::default(); MAX_SEQUENCES + 1],
            current: 0,
        }
    }

    fn compress(&mut self, input: &[u8], output: &mut BitWriter) {
        let end = input.len();
        let mut pos = 0;
        let mut limits = MatchLimits::new(self.nice_match_len);
        loop {
            let block_start = pos;
            let block_limit = if end - pos < SOFT_MAX_BLOCK_LENGTH + MIN_BLOCK_LENGTH {
                end
            } else {
                pos + SOFT_MAX_BLOCK_LENGTH
            };
            self.begin_block();
            let mut min_len =
                initial_min_match_len(&input[pos..block_limit], self.max_search_depth);
            let mut next_min_len_update = pos + (end - pos).min(10_000);
            loop {
                if !matches!(self.parser, Parser::Greedy) && pos >= next_min_len_update {
                    min_len = updated_min_match_len(&self.freqs, self.max_search_depth);
                    next_min_len_update += (end - next_min_len_update).min(pos - block_start);
                }
                pos = match self.parser {
                    Parser::Greedy => self.parse_greedy(input, pos, min_len, &mut limits),
                    Parser::Lazy | Parser::Lazy2 => {
                        self.parse_lazy(input, pos, min_len, &mut limits)
                    }
                };
                let block_full = pos >= block_limit || self.current >= MAX_SEQUENCES;
                if block_full || self.splitter.should_end_block(block_start, pos, end) {
                    break;
                }
            }
            self.finish_block(output, &input[block_start..pos], pos == end);
            if pos == end {
                break;
            }
        }
    }

    fn begin_block(&mut self) {
        self.current = 0;
        self.sequences[0] = Sequence::default();
        self.splitter = BlockSplitter::default();
        self.freqs = Frequencies::default();
    }

    /// One step of greedy parsing at `pos`: the longest match, or a literal. Returns the next
    /// position.
    fn parse_greedy(
        &mut self,
        input: &[u8],
        pos: usize,
        min_len: u32,
        limits: &mut MatchLimits,
    ) -> usize {
        limits.shrink_near_end(input.len() - pos);
        let search = limits.search(min_len - 1, self.max_search_depth);
        let (length, offset) = self.chains.longest_match(input, pos, search);
        if length >= min_len && (length > MIN_MATCH_LEN || offset <= 4096) {
            self.choose_match(length, offset);
            self.chains.skip(input, pos + 1, length - 1);
            pos + length as usize
        } else {
            self.choose_literal(input[pos]);
            pos + 1
        }
    }

    /// One step of lazy parsing at `pos`: a match is taken only if a match one position
    /// later (or two, for [`Parser::Lazy2`]) is not better. Returns the next position.
    fn parse_lazy(
        &mut self,
        input: &[u8],
        mut pos: usize,
        min_len: u32,
        limits: &mut MatchLimits,
    ) -> usize {
        let end = input.len();
        limits.shrink_near_end(end - pos);
        let search = limits.search(min_len - 1, self.max_search_depth);
        let (mut cur_len, mut cur_offset) = self.chains.longest_match(input, pos, search);
        if cur_len < min_len || (cur_len == MIN_MATCH_LEN && cur_offset > 8192) {
            self.choose_literal(input[pos]);
            return pos + 1;
        }
        pos += 1;
        // Whether a match `(len, offset)` found later beats the current one by `margin`,
        // weighing four per byte of length against the bits of the offsets.
        let log2 = |offset: u32| 31 - offset.leading_zeros() as i32;
        let beats = |len: u32, offset: u32, cur_len: u32, cur_offset: u32, margin: i32| {
            len >= cur_len
                && 4 * (len - cur_len) as i32 + (log2(cur_offset) - log2(offset)) > margin
        };
        loop {
            if cur_len >= limits.nice_len {
                self.choose_match(cur_len, cur_offset);
                self.chains.skip(input, pos, cur_len - 1);
                return pos + cur_len as usize - 1;
            }

            limits.shrink_near_end(end - pos);
            let search = limits.search(cur_len - 1, self.max_search_depth >> 1);
            let (next_len, next_offset) = self.chains.longest_match(input, pos, search);
            pos += 1;
            if beats(next_len, next_offset, cur_len, cur_offset, 2) {
                self.choose_literal(input[pos - 2]);
                (cur_len, cur_offset) = (next_len, next_offset);
                continue;
            }

            if matches!(self.parser, Parser::Lazy) {
                self.choose_match(cur_len, cur_offset);
                self.chains.skip(input, pos, cur_len - 2);
                return pos + cur_len as usize - 2;
            }

            limits.shrink_near_end(end - pos);
            let search = limits.search(cur_len - 1, self.max_search_depth >> 2);
            let (next_len, next_offset) = self.chains.longest_match(input, pos, search);
            pos += 1;
            if beats(next_len, next_offset, cur_len, cur_offset, 6) {
                self.choose_literal(input[pos - 3]);
                self.choose_literal(input[pos - 2]);
                (cur_len, cur_offset) = (next_len, next_offset);
                continue;
            }
            self.choose_match(cur_len, cur_offset);
            if cur_len > 3 {
                self.chains.skip(input, pos, cur_len - 3);
                pos += cur_len as usize - 3;
            }
            return pos;
        }
    }

    fn choose_literal(&mut self, literal: u8) {
        self.freqs.litlen[usize::from(literal)] += 1;
        self.splitter.observe_literal(literal);
        self.sequences[self.current].literals += 1;
    }

    fn choose_match(&mut self, length: u32, offset: u32) {
        let slot = offset_slot(offset);
        self.freqs.litlen[FIRST_LENGTH_SYM + usize::from(LENGTH_SLOTS[length as usize])] += 1;
        self.freqs.offset[slot] += 1;
        self.splitter.observe_match(length);
        let sequence = &mut self.sequences[self.current];
        sequence.length = length;
        sequence.offset = offset;
        sequence.offset_slot = slot;
        self.current += 1;
        self.sequences[self.current] = Sequence::default();
    }

    /// Builds the block's codes and writes it, `data` being its input.
    fn finish_block(&mut self, output: &mut BitWriter, data: &[u8], is_final: bool) {
        self.freqs.litlen[END_OF_BLOCK] += 1;
        self.codes.rebuild(&self.freqs);
        self.write_block(output, data, is_final);
    }

    /// Run-length codes the first `num_lens` codeword lengths, in `lens`, for the header.
    fn compute_precode_items(&mut self, lens: &[u8]) {
        let precode = &mut self.precode;
        precode.freqs = [0; NUM_PRECODE_SYMS];
        precode.items.clear();
        let mut emit = |symbol: usize, extra: usize| {
            precode.freqs[symbol] += 1;
            precode.items.push(symbol as u32 | ((extra as u32) << 5));
        };
        let mut start = 0;
        while start < lens.len() {
            let len = lens[start];
            let run_end = start + lens[start..].iter().take_while(|&&l| l == len).count();
            if len == 0 {
                // Runs of zeros: 11 to 138 with symbol 18, 3 to 10 with symbol 17.
                while run_end - start >= 11 {
                    let extra = (run_end - start - 11).min(0x7F);
                    emit(18, extra);
                    start += 11 + extra;
                }
                if run_end - start >= 3 {
                    let extra = (run_end - start - 3).min(0x7);
                    emit(17, extra);
                    start += 3 + extra;
                }
            } else if run_end - start >= 4 {
                // A length, then repeats of it, 3 to 6 at a time, with symbol 16.
                emit(usize::from(len), 0);
                start += 1;
                loop {
                    let extra = (run_end - start - 3).min(0x3);
                    emit(16, extra);
                    start += 3 + extra;
                    if run_end - start < 3 {
                        break;
                    }
                }
            }
            while start != run_end {
                emit(usize::from(len), 0);
                start += 1;
            }
        }
    }

    /// Works out a dynamic block's header: how many codeword lengths it lists, and the
    /// precode that codes them.
    fn prepare_dynamic_header(&mut self) {
        let lens = &self.codes.lens;
        let num_litlen = 257
            + lens[257..NUM_LITLEN_SYMS]
                .iter()
                .rposition(|&l| l != 0)
                .map_or(0, |i| i + 1);
        let offset_lens = &lens[NUM_LITLEN_SYMS..];
        let num_offset = 1 + offset_lens[1..]
            .iter()
            .rposition(|&l| l != 0)
            .map_or(0, |i| i + 1);
        // The header lists both sets of lengths back to back.
        let mut listed = Vec::with_capacity(num_litlen + num_offset);
        listed.extend_from_slice(&lens[..num_litlen]);
        listed.extend_from_slice(&offset_lens[..num_offset]);
        self.compute_precode_items(&listed);

        let precode = &mut self.precode;
        let freqs = precode.freqs;
        make_huffman_code(
            MAX_PRECODE_LEN,
            &freqs,
            &mut precode.lens,
            &mut precode.codewords,
        );
        precode.num_litlen = num_litlen;
        precode.num_offset = num_offset;
        precode.num_explicit_lens = 4 + PRECODE_ORDER[4..]
            .iter()
            .rposition(|&symbol| precode.lens[symbol] != 0)
            .map_or(0, |i| i + 1);
    }

    /// The cost in bits of the block as dynamic, static and stored blocks.
    fn block_costs(&self, bits_pending: u32, block_length: usize) -> [u32; 3] {
        let precode = &self.precode;
        let mut dynamic = 3 + 5 + 5 + 4 + 3 * precode.num_explicit_lens as u32;
        for (symbol, &freq) in precode.freqs.iter().enumerate() {
            let bits = PRECODE_EXTRA_BITS[symbol] + u32::from(precode.lens[symbol]);
            dynamic = dynamic.wrapping_add(freq.wrapping_mul(bits));
        }
        let mut fixed = 3u32;
        for (symbol, &freq) in self.freqs.litlen[..NUM_LITERALS].iter().enumerate() {
            dynamic = dynamic.wrapping_add(freq.wrapping_mul(self.codes.litlen_len(symbol)));
            fixed = fixed.wrapping_add(freq.wrapping_mul(if symbol < 144 { 8 } else { 9 }));
        }
        dynamic = dynamic.wrapping_add(self.codes.litlen_len(END_OF_BLOCK));
        fixed += 7;
        for (slot, &extra) in LENGTH_EXTRA_BITS.iter().enumerate() {
            let symbol = FIRST_LENGTH_SYM + slot;
            let freq = self.freqs.litlen[symbol];
            dynamic =
                dynamic.wrapping_add(freq.wrapping_mul(extra + self.codes.litlen_len(symbol)));
            fixed =
                fixed.wrapping_add(freq.wrapping_mul(extra + self.fixed_codes.litlen_len(symbol)));
        }
        for (slot, &extra) in OFFSET_EXTRA_BITS.iter().enumerate() {
            let freq = self.freqs.offset[slot];
            dynamic = dynamic.wrapping_add(freq.wrapping_mul(extra + self.codes.offset_len(slot)));
            fixed = fixed.wrapping_add(freq.wrapping_mul(extra + 5));
        }
        // A stored block: its 3 header bits, padding to a byte, then 4 bytes of length per
        // 65535 bytes of data.
        let stored_blocks = block_length.div_ceil(MAX_STORED_LEN) as u32;
        let stored = 3
            + ((bits_pending + 3).wrapping_neg() & 7)
            + 32
            + 40 * (stored_blocks - 1)
            + 8 * block_length as u32;
        [dynamic, fixed, stored]
    }

    /// Writes the block of `data` as whichever kind of block is smallest.
    fn write_block(&mut self, output: &mut BitWriter, data: &[u8], is_final: bool) {
        self.prepare_dynamic_header();
        let [dynamic, fixed, stored] = self.block_costs(output.count, data.len());
        let best = dynamic.min(fixed.min(stored));

        if best == stored {
            write_stored_blocks(output, data, is_final);
            return;
        }
        let use_fixed = best == fixed;
        output.write(u32::from(is_final), 1);
        if use_fixed {
            output.write(1, 2);
        } else {
            output.write(2, 2);
            self.write_dynamic_header(output);
        }
        self.compute_length_codewords(use_fixed);
        let codes = if use_fixed {
            &self.fixed_codes
        } else {
            &self.codes
        };

        let mut pos = 0;
        for sequence in &self.sequences[..=self.current] {
            for &literal in &data[pos..pos + sequence.literals as usize] {
                let literal = usize::from(literal);
                output.write(codes.litlen_codewords[literal], codes.litlen_len(literal));
            }
            pos += sequence.literals as usize;
            if sequence.length == 0 {
                break;
            }
            let length = sequence.length as usize;
            let slot = sequence.offset_slot;
            output.write(
                self.length_codewords[length],
                self.length_codeword_lens[length],
            );
            output.write(codes.offset_codewords[slot], codes.offset_len(slot));
            output.write(sequence.offset - OFFSET_BASE[slot], OFFSET_EXTRA_BITS[slot]);
            pos += length;
        }
        output.write(
            codes.litlen_codewords[END_OF_BLOCK],
            codes.litlen_len(END_OF_BLOCK),
        );
    }

    fn write_dynamic_header(&self, output: &mut BitWriter) {
        let precode = &self.precode;
        output.write(precode.num_litlen as u32 - 257, 5);
        output.write(precode.num_offset as u32 - 1, 5);
        output.write(precode.num_explicit_lens as u32 - 4, 4);
        for &symbol in &PRECODE_ORDER[..precode.num_explicit_lens] {
            output.write(u32::from(precode.lens[symbol]), 3);
        }
        for &item in &precode.items {
            let symbol = (item & 0x1F) as usize;
            output.write(precode.codewords[symbol], u32::from(precode.lens[symbol]));
            output.write(item >> 5, PRECODE_EXTRA_BITS[symbol]);
        }
    }

    /// Precomputes, for every match length, its length symbol's codeword followed by its
    /// extra bits.
    fn compute_length_codewords(&mut self, use_fixed: bool) {
        let codes = if use_fixed {
            &self.fixed_codes
        } else {
            &self.codes
        };
        for len in MIN_MATCH_LEN..=MAX_MATCH_LEN {
            let slot = usize::from(LENGTH_SLOTS[len as usize]);
            let symbol = FIRST_LENGTH_SYM + slot;
            let symbol_len = codes.litlen_len(symbol);
            let extra = len - LENGTH_BASE[slot];
            self.length_codewords[len as usize] =
                codes.litlen_codewords[symbol] | extra.wrapping_shl(symbol_len);
            self.length_codeword_lens[len as usize] = symbol_len + LENGTH_EXTRA_BITS[slot];
        }
    }
}

/// Writes `data` as stored blocks of at most 65535 bytes, after the bits already pending.
fn write_stored_blocks(output: &mut BitWriter, data: &[u8], is_final: bool) {
    let count = data.len().div_ceil(MAX_STORED_LEN).max(1);
    for (k, chunk) in data.chunks(MAX_STORED_LEN).enumerate() {
        let last = is_final && k + 1 == count;
        // The block header joins the pending bits, then everything is padded to a byte.
        let header = (u32::from(last) << output.count) | output.buffer as u32;
        output.bytes.push(header as u8);
        if output.count > 5 {
            output.bytes.push(0);
        }
        output.buffer = 0;
        output.count = 0;
        let len = chunk.len() as u16;
        output.bytes.extend_from_slice(&len.to_le_bytes());
        output.bytes.extend_from_slice(&(!len).to_le_bytes());
        output.bytes.extend_from_slice(chunk);
    }
}

/// Level 0: `input` as stored blocks, as `deflate_compress_none` writes them.
fn store(input: &[u8], output: &mut Vec<u8>) {
    if input.is_empty() {
        // A single, final, empty stored block.
        output.push(1);
        output.extend_from_slice(&0xFFFF_0000u32.to_le_bytes());
        return;
    }
    let count = input.len().div_ceil(MAX_STORED_LEN);
    for (k, chunk) in input.chunks(MAX_STORED_LEN).enumerate() {
        let len = chunk.len() as u16;
        output.push(u8::from(k + 1 == count));
        output.extend_from_slice(&len.to_le_bytes());
        output.extend_from_slice(&(!len).to_le_bytes());
        output.extend_from_slice(chunk);
    }
}

/// Appends `input` compressed as a raw DEFLATE stream to `output`.
fn deflate_into(input: &[u8], level: Level, output: Vec<u8>) -> Vec<u8> {
    // Short inputs are stored: libdeflate's cutoff falls as the level rises.
    let store_up_to = match level.get() {
        0 => usize::MAX,
        l => 55 - 4 * usize::from(l),
    };
    if input.len() <= store_up_to {
        let mut output = output;
        store(input, &mut output);
        return output;
    }
    let mut writer = BitWriter::new(output);
    Compressor::new(level).compress(input, &mut writer);
    writer.finish()
}

/// The largest raw DEFLATE stream [`deflate_compress`] can produce from `len` bytes, at any
/// level (`libdeflate_deflate_compress_bound`).
#[must_use]
pub fn deflate_compress_bound(len: usize) -> usize {
    // Five bytes for each stored block (at least one), and the data.
    let max_blocks = len.div_ceil(MIN_BLOCK_LENGTH).max(1);
    (5 * max_blocks).saturating_add(len)
}

/// The largest zlib stream [`zlib_compress`] can produce from `len` bytes, at any level
/// (`libdeflate_zlib_compress_bound`).
#[must_use]
pub fn zlib_compress_bound(len: usize) -> usize {
    // A 2-byte header and a 4-byte checksum.
    deflate_compress_bound(len).saturating_add(2 + 4)
}

/// Compresses `input` to a raw DEFLATE stream at `level` (`libdeflate_deflate_compress`).
#[must_use]
pub fn deflate_compress(input: &[u8], level: Level) -> Vec<u8> {
    deflate_into(input, level, Vec::new())
}

/// Compresses `input` to a zlib stream at `level` (`libdeflate_zlib_compress`): a 2-byte
/// header, the raw DEFLATE stream, and the Adler-32 checksum of `input`.
#[must_use]
pub fn zlib_compress(input: &[u8], level: Level) -> Vec<u8> {
    let level_hint: u16 = match level.get() {
        0..2 => 0,
        2..6 => 1,
        6..8 => 2,
        _ => 3,
    };
    // Deflate with a 32 KiB window, the level hint, and the check bits that make the header
    // a multiple of 31.
    let mut header: u16 = (8 << 8) | (7 << 12) | (level_hint << 6);
    header |= 31 - header % 31;
    let mut output = Vec::with_capacity(zlib_compress_bound(input.len()));
    output.extend_from_slice(&header.to_be_bytes());
    let mut output = deflate_into(input, level, output);
    output.extend_from_slice(&adler32(input).to_be_bytes());
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_match_their_bases() {
        for len in MIN_MATCH_LEN..=MAX_MATCH_LEN {
            let slot = LENGTH_BASE.iter().rposition(|&base| base <= len).unwrap();
            assert_eq!(
                usize::from(LENGTH_SLOTS[len as usize]),
                slot,
                "length {len}"
            );
        }
        for offset in 1..=32768 {
            let slot = OFFSET_BASE
                .iter()
                .rposition(|&base| base <= offset)
                .unwrap();
            assert_eq!(offset_slot(offset), slot, "offset {offset}");
        }
    }

    #[test]
    fn streams_round_trip() {
        let mut state = 7u64;
        let data: Vec<u8> = (0..200_000u32)
            .map(|i| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                if i % 1000 < 600 {
                    (i / 7 % 13) as u8
                } else {
                    (state >> 56) as u8
                }
            })
            .collect();
        for level in [0, 2, 3, 4, 5, 6, 7, 8, 9].map(|l| Level::new(l).unwrap()) {
            for len in [0, 1, 30, 100, 5000, 70_000, 200_000] {
                let stream = zlib_compress(&data[..len], level);
                assert!(stream.len() <= zlib_compress_bound(len));
                let mut back = vec![0; len];
                let result = crate::zlib_decompress(&stream, &mut back).unwrap();
                assert_eq!(
                    (result.bytes_read, result.bytes_written),
                    (stream.len(), len)
                );
                assert_eq!(back, &data[..len], "level {}, {len} bytes", level.get());
            }
        }
    }
}
