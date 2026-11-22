//! Integration tests for new APIs, RFC 1952 gzip support, CRC-32, Level 1, `*_into`, and
//! `*_decompress_vec`.

use libdeflate_nrp::{
    Compressor, Decompressed, Decompressor, Error, Level, adler32, adler32_update, crc32,
    crc32_update, deflate_compress, deflate_compress_bound, deflate_compress_into,
    deflate_decompress, deflate_decompress_vec, gzip_compress, gzip_compress_bound,
    gzip_compress_into, gzip_decompress, gzip_decompress_vec, zlib_compress, zlib_compress_bound,
    zlib_compress_into, zlib_decompress, zlib_decompress_vec,
};

const CONST_DEFLATE_BOUND: usize = deflate_compress_bound(1024);
const CONST_ZLIB_BOUND: usize = zlib_compress_bound(1024);
const CONST_GZIP_BOUND: usize = gzip_compress_bound(1024);
const CONST_DECOMPRESSOR: Decompressor = Decompressor::new();

#[test]
fn const_bounds_and_const_decompressor() {
    const {
        assert!(CONST_DEFLATE_BOUND > 1024);
        assert!(CONST_ZLIB_BOUND == CONST_DEFLATE_BOUND + 6);
        assert!(CONST_GZIP_BOUND == CONST_DEFLATE_BOUND + 18);
    }
    assert_eq!(deflate_compress_bound(usize::MAX), usize::MAX);
    assert_eq!(zlib_compress_bound(usize::MAX), usize::MAX);
    assert_eq!(gzip_compress_bound(usize::MAX), usize::MAX);

    let mut dec = CONST_DECOMPRESSOR;
    let stream = deflate_compress(b"const decompressor check", Level::DEFAULT);
    let mut out = [0u8; 32];
    let res = dec.deflate_decompress(&stream, &mut out).unwrap();
    assert_eq!(&out[..res.bytes_written], b"const decompressor check");
}

#[test]
fn compress_into_and_decompress_vec_across_formats_and_levels() {
    let payload: Vec<u8> = (0..12_000u32)
        .map(|i| {
            if i % 97 < 60 {
                b'a' + (i % 7) as u8
            } else {
                (i.wrapping_mul(2_654_435_761) >> 24) as u8
            }
        })
        .collect();

    let mut comp = Compressor::default();
    let mut dec = Decompressor::default();
    assert_ne!(format!("{comp:?}"), "");
    assert_ne!(format!("{dec:?}"), "");

    for raw_level in 0..=9 {
        let level = Level::new(raw_level).unwrap();
        comp.set_level(level);

        // Raw DEFLATE
        let expected_d = deflate_compress(&payload, level);
        let mut exact_d = vec![0u8; expected_d.len()];
        assert_eq!(
            deflate_compress_into(&payload, &mut exact_d, level),
            Ok(expected_d.len())
        );
        assert_eq!(exact_d, expected_d);
        assert_eq!(
            comp.deflate_compress_into(&payload, &mut exact_d),
            Ok(expected_d.len())
        );
        assert_eq!(exact_d, expected_d);
        let mut short_d = vec![0u8; expected_d.len() - 1];
        assert_eq!(
            deflate_compress_into(&payload, &mut short_d, level),
            Err(Error::InsufficientSpace)
        );
        // Huge max_len (usize::MAX) must not over-allocate or OOM.
        assert_eq!(
            deflate_decompress_vec(&expected_d, usize::MAX).unwrap(),
            payload
        );
        assert_eq!(
            dec.deflate_decompress_vec(&expected_d, payload.len())
                .unwrap(),
            payload
        );
        assert_eq!(
            deflate_decompress_vec(&expected_d, payload.len() - 1),
            Err(Error::InsufficientSpace)
        );

        // Zlib
        let expected_z = zlib_compress(&payload, level);
        let mut exact_z = vec![0u8; expected_z.len()];
        assert_eq!(
            zlib_compress_into(&payload, &mut exact_z, level),
            Ok(expected_z.len())
        );
        assert_eq!(exact_z, expected_z);
        assert_eq!(
            comp.zlib_compress_into(&payload, &mut exact_z),
            Ok(expected_z.len())
        );
        assert_eq!(
            zlib_compress_into(&payload, &mut exact_z[..expected_z.len() - 1], level),
            Err(Error::InsufficientSpace)
        );
        assert_eq!(
            zlib_decompress_vec(&expected_z, usize::MAX).unwrap(),
            payload
        );
        assert_eq!(
            zlib_decompress_vec(&expected_z, payload.len() - 1),
            Err(Error::InsufficientSpace)
        );

        // Gzip
        let expected_g = gzip_compress(&payload, level);
        assert!(expected_g.len() <= gzip_compress_bound(payload.len()));
        let mut exact_g = vec![0u8; expected_g.len()];
        assert_eq!(
            gzip_compress_into(&payload, &mut exact_g, level),
            Ok(expected_g.len())
        );
        assert_eq!(exact_g, expected_g);
        assert_eq!(comp.gzip_compress(&payload), expected_g);
        assert_eq!(
            comp.gzip_compress_into(&payload, &mut exact_g),
            Ok(expected_g.len())
        );
        assert_eq!(
            gzip_compress_into(&payload, &mut exact_g[..expected_g.len() - 1], level),
            Err(Error::InsufficientSpace)
        );
        assert_eq!(
            gzip_decompress_vec(&expected_g, usize::MAX).unwrap(),
            payload
        );
        assert_eq!(
            gzip_decompress_vec(&expected_g, payload.len() - 1),
            Err(Error::InsufficientSpace)
        );
    }
}

#[test]
fn level_one_compresses_and_crosses_windows_and_blocks() {
    let text = b"fast hash table compression at level 1 repeats phrases across 32 KiB windows! "
        .repeat(4800);
    assert!(text.len() > 350_000);

    let compressed = zlib_compress(&text, Level::FASTEST);
    assert!(
        compressed.len() < text.len() / 5,
        "level 1 should compress repetitive text well: {} vs {}",
        compressed.len(),
        text.len()
    );
    let round_tripped = zlib_decompress_vec(&compressed, text.len()).unwrap();
    assert_eq!(round_tripped, text);
}

#[test]
fn gzip_rfc1952_optional_headers_and_validation() {
    let data = b"RFC 1952 gzip payload with FEXTRA, FNAME, FCOMMENT, and FHCRC!";
    let raw_deflate = deflate_compress(data, Level::DEFAULT);

    // Build a gzip member containing FTEXT | FHCRC | FEXTRA | FNAME | FCOMMENT.
    let flg: u8 = 0x01 | 0x02 | 0x04 | 0x08 | 0x10;
    let mut member = vec![
        0x1F, 0x8B, 0x08, flg, 0x78, 0x56, 0x34, 0x12, // MTIME
        0x00, 0x03, // XFL, OS (Unix)
    ];
    // FEXTRA: 5 bytes
    let extra = b"extra";
    member.extend_from_slice(&(extra.len() as u16).to_le_bytes());
    member.extend_from_slice(extra);
    // FNAME: zero-terminated
    member.extend_from_slice(b"example.txt\0");
    // FCOMMENT: zero-terminated
    member.extend_from_slice(b"file comment here\0");
    // FHCRC: low 16 bits of CRC-32 of header bytes so far
    let hcrc = (crc32(&member) as u16).to_le_bytes();
    let hcrc_pos = member.len();
    member.extend_from_slice(&hcrc);
    // Body + trailer
    member.extend_from_slice(&raw_deflate);
    member.extend_from_slice(&crc32(data).to_le_bytes());
    member.extend_from_slice(&(data.len() as u32).to_le_bytes());

    let mut out = vec![0u8; data.len()];
    assert_eq!(
        gzip_decompress(&member, &mut out),
        Ok(Decompressed {
            bytes_read: member.len(),
            bytes_written: data.len(),
        })
    );
    assert_eq!(&out, data);
    assert_eq!(gzip_decompress_vec(&member, 1024).unwrap(), data);

    // Concatenated two-member gzip stream read sequentially via bytes_read.
    let second_data = b"second gzip member";
    let mut two_members = member.clone();
    two_members.extend_from_slice(&gzip_compress(second_data, Level::L1));
    let mut buf = [0u8; 128];
    let first = gzip_decompress(&two_members, &mut buf).unwrap();
    assert_eq!(&buf[..first.bytes_written], data);
    let second = gzip_decompress(&two_members[first.bytes_read..], &mut buf).unwrap();
    assert_eq!(&buf[..second.bytes_written], second_data);
    assert_eq!(first.bytes_read + second.bytes_read, two_members.len());

    // Corrupt FHCRC -> BadData
    let mut wrong_header_crc = member.clone();
    wrong_header_crc[hcrc_pos] ^= 1;
    assert_eq!(
        gzip_decompress(&wrong_header_crc, &mut out),
        Err(Error::BadData)
    );

    // Corrupt CRC32 trailer -> BadData
    let mut wrong_trailer_crc = member.clone();
    let len = wrong_trailer_crc.len();
    wrong_trailer_crc[len - 8] ^= 1;
    assert_eq!(
        gzip_decompress(&wrong_trailer_crc, &mut out),
        Err(Error::BadData)
    );

    // Corrupt ISIZE trailer -> BadData
    let mut bad_isize = member.clone();
    bad_isize[len - 4] ^= 1;
    assert_eq!(gzip_decompress(&bad_isize, &mut out), Err(Error::BadData));

    // Reserved FLG bits -> BadData
    let mut bad_flg = gzip_compress(data, Level::DEFAULT);
    bad_flg[3] |= 0x20;
    assert_eq!(gzip_decompress(&bad_flg, &mut out), Err(Error::BadData));

    // Wrong magic / compression method / too short -> BadData
    let mut bad_magic = gzip_compress(data, Level::DEFAULT);
    bad_magic[1] = 0x00;
    assert_eq!(gzip_decompress(&bad_magic, &mut out), Err(Error::BadData));
    let mut bad_cm = gzip_compress(data, Level::DEFAULT);
    bad_cm[2] = 9;
    assert_eq!(gzip_decompress(&bad_cm, &mut out), Err(Error::BadData));
    assert_eq!(
        gzip_decompress(&member[..17], &mut out),
        Err(Error::BadData)
    );

    // Truncated optional headers -> BadData
    for cut in 10..hcrc_pos + 2 {
        assert_eq!(
            gzip_decompress(&member[..cut + 8], &mut out),
            Err(Error::BadData)
        );
    }
}

#[test]
fn empty_inputs_round_trip_all_formats() {
    for raw_level in 0..=9 {
        let level = Level::new(raw_level).unwrap();
        let stream_d = deflate_compress(b"", level);
        let stream_z = zlib_compress(b"", level);
        let stream_g = gzip_compress(b"", level);

        assert_eq!(deflate_decompress_vec(&stream_d, 0).unwrap(), b"");
        assert_eq!(zlib_decompress_vec(&stream_z, 0).unwrap(), b"");
        assert_eq!(gzip_decompress_vec(&stream_g, 0).unwrap(), b"");

        let mut empty_buf = [];
        assert_eq!(
            deflate_decompress(&stream_d, &mut empty_buf)
                .unwrap()
                .bytes_written,
            0
        );
        assert_eq!(
            zlib_decompress(&stream_z, &mut empty_buf)
                .unwrap()
                .bytes_written,
            0
        );
        assert_eq!(
            gzip_decompress(&stream_g, &mut empty_buf)
                .unwrap()
                .bytes_written,
            0
        );
    }

    assert_eq!(adler32(b""), adler32_update(1, b""));
    assert_eq!(crc32(b""), crc32_update(0, b""));
}
