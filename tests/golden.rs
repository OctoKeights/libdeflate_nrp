//! The port of libdeflate against results recorded from the original, so that a change shows
//! without a C compiler at hand.
//!
//! `tests/data/libdeflate_expected.txt` is what `compare/dump_libdeflate.c`, built against
//! libdeflate (see `compare/compare.sh`), prints for `libdeflate 3 200` and `libdeflate 8 200`:
//! a hash of what made-up data compresses to at each level, and of what each stream, whole and
//! damaged, decompresses to or is refused with. `compare/compare.sh` is the full comparison.

mod made_up;

#[test]
fn compresses_and_decompresses_as_libdeflate_does() {
    let printed = [made_up::libdeflate(3, 200), made_up::libdeflate(8, 200)].concat();
    let expected = include_str!("data/libdeflate_expected.txt");
    for (line, (ours, theirs)) in printed.lines().zip(expected.lines()).enumerate() {
        assert_eq!(ours, theirs, "line {}", line + 1);
    }
    assert_eq!(printed.lines().count(), expected.lines().count());
}
