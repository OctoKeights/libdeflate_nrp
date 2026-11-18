//! `libdeflate_nrp` against hashes recorded from libdeflate, so that any change in output shows
//! without a C compiler.
//!
//! `tests/data/libdeflate_expected.txt` is what `compare/dump_libdeflate.c`, built against
//! libdeflate 1.26, prints for `libdeflate 3 200` and `libdeflate 8 200`: hashes of synthetic
//! data compressed at each level, and of what each stream decompresses to, intact and damaged,
//! or the error it gives. `compare/compare.sh` runs the full comparison.

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
