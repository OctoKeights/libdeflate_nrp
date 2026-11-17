#!/bin/sh
# Compares libdeflate_nrp with libdeflate (https://github.com/ebiggers/libdeflate) v1.26.
#
#   compare/compare.sh <checkout of libdeflate> [count] [seeds] [dumps]
#
# (OpenEXR 3.5's external/deflate is the same libdeflate, and does as well.)
# compare/dump_libdeflate.c is built against libdeflate's library, and examples/dump.rs from
# the port; both run the same made-up data for each seed and what they print is compared byte
# for byte. Dump: libdeflate, with a hundredth of [count] (200,000 by default) as the number of
# made-up sources, each compressed at a level and decompressed whole and damaged.
set -u
if [ $# -lt 1 ]; then
    echo "usage: $0 <checkout of libdeflate> [count] [seeds] [dumps]" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")/.." && pwd)
source=$1
blocks=${2:-200000}
seeds=${3:-3}
dumps=${4:-libdeflate}
tmp=${TMPDIR:-/tmp}/libdeflate_nrp-compare
mkdir -p "$tmp" || exit 2
if ! [ -f "$source/libdeflate.h" ] && [ -f "$source/lib/deflate_compress.c" ]; then
    echo "$source does not look like checkout of libdeflate" >&2
    exit 2
fi

(cd "$here" && cargo build --release -q --ignore-rust-version --example dump) || exit 2
rust=$(cd "$here" && cargo metadata --format-version 1 --no-deps 2>/dev/null | sed 's/.*"target_directory":"\([^"]*\)".*/\1/')/release/examples/dump

# (Both cpu_features.c are built, each for its own processors, into a folder of its own.)
mkdir -p "$tmp/obj/x86" "$tmp/obj/arm" || exit 2
(cd "$tmp/obj" && ${CC:-cc} -O2 -c -I "$source" "$source"/lib/*.c) || exit 2
(cd "$tmp/obj/x86" && ${CC:-cc} -O2 -c -I "$source" "$source"/lib/x86/cpu_features.c) || exit 2
(cd "$tmp/obj/arm" && ${CC:-cc} -O2 -c -I "$source" "$source"/lib/arm/cpu_features.c) || exit 2
${CC:-cc} -O2 -I "$source" "$here/compare/dump_libdeflate.c" "$tmp"/obj/*.o "$tmp"/obj/x86/*.o "$tmp"/obj/arm/*.o -o "$tmp/dump-libdeflate" || exit 2

bad=0
for what in $dumps; do
    extra=
    case $what in
    libdeflate)
        reference=$tmp/dump-libdeflate
        count=$((blocks / 100))
        ;;
    *)
        echo "unknown dump $what" >&2
        exit 2
        ;;
    esac
    seed=1
    while [ "$seed" -le "$seeds" ]; do
        # shellcheck disable=SC2086
        "$reference" "$what" "$seed" "$count" $extra > "$tmp/reference.txt" 2> /dev/null
        # shellcheck disable=SC2086
        "$rust" "$what" "$seed" "$count" $extra > "$tmp/rust.txt"
        if [ ! -s "$tmp/reference.txt" ]; then
            echo "$what, seed $seed: NOT COMPARED, the original's side printed nothing"
            bad=$((bad + 1))
        elif cmp -s "$tmp/reference.txt" "$tmp/rust.txt"; then
            echo "$what, seed $seed: the same ($(tail -1 "$tmp/rust.txt"))"
        else
            echo "$what, seed $seed: DIFFERENT"
            bad=$((bad + 1))
        fi
        seed=$((seed + 1))
    done
done
[ "$bad" -eq 0 ]
