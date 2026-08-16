//! Byte-exact integrity pin of the in-crate §D.10 transcriptions
//! against the **staged clean-room CSVs** they were machine-generated
//! from (`docs/audio/dts/tables/dts-d10-1-adpcm-coeff-vq.csv` /
//! `dts-d10-2-hfreq-vq.csv`).
//!
//! The unit pins in `src/d10_vq.rs` check anchors, ranges and
//! structural invariants; the sweeps in `tests/d10_vq_decode.rs` and
//! the black-box comparisons check behaviour. This test closes the
//! remaining gap — a silent edit to a *middle* table row that keeps
//! every anchor and invariant intact — by re-serializing each
//! in-crate table to the staged CSV's exact byte format (`index,…`
//! header, one ascending-index row per vector, CRLF line endings,
//! trailing newline) and requiring its SHA-256 to equal the digest
//! printed in the table's `.meta.md` sidecar. The staged files
//! themselves reproduce these digests, so equality here means the
//! transcription is the staged data, all 16 384 + 32 768 values of
//! it, byte for byte.
//!
//! The SHA-256 below is a self-contained transcription of the public
//! FIPS 180-4 algorithm (test-local so the crate takes no extra
//! dependency).

use oxideav_dts::d10_tables::{ADPCM_VQ_TABLE, HFREQ_VQ_TABLE};

/// `dts-d10-1-adpcm-coeff-vq.csv` — pinned in its `.meta.md`.
const ADPCM_CSV_SHA256: &str = "65ee69cd518229b5e0936844db794e49dec2e993af39440fef36658e0dfa30d9";
/// `dts-d10-2-hfreq-vq.csv` — pinned in its `.meta.md`.
const HFREQ_CSV_SHA256: &str = "3d5d409a975f57720fa67760c2ec96cc1d00d12b9aa1c7de3e5b48a982acede6";

// ---- FIPS 180-4 SHA-256 (test-local) --------------------------------

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256_hex(data: &[u8]) -> String {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(word.try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(v);
        }
    }
    h.iter().map(|v| format!("{v:08x}")).collect()
}

/// The one published test vector of FIPS 180-4 everyone pins first:
/// the empty message. Guards the test-local implementation itself.
#[test]
fn sha256_known_answer() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

/// Re-serialize [`ADPCM_VQ_TABLE`] to the staged CSV's byte format and
/// require the `.meta.md`-pinned digest: the in-crate §D.10.1
/// transcription IS the staged data, all 4096 × 4 values, byte for
/// byte.
#[test]
fn adpcm_table_reserializes_to_the_staged_csv_digest() {
    let mut csv = String::from("index,a0,a1,a2,a3\r\n");
    for (index, row) in ADPCM_VQ_TABLE.iter().enumerate() {
        csv.push_str(&format!(
            "{index},{},{},{},{}\r\n",
            row[0], row[1], row[2], row[3]
        ));
    }
    assert_eq!(sha256_hex(csv.as_bytes()), ADPCM_CSV_SHA256);
}

/// Re-serialize [`HFREQ_VQ_TABLE`] to the staged CSV's byte format and
/// require the `.meta.md`-pinned digest: the in-crate §D.10.2
/// transcription IS the staged data, all 1024 × 32 elements, byte for
/// byte (in the settled vector-element order).
#[test]
fn hfreq_table_reserializes_to_the_staged_csv_digest() {
    let mut csv = String::from("index");
    for e in 0..32 {
        csv.push_str(&format!(",e{e}"));
    }
    csv.push_str("\r\n");
    for (index, row) in HFREQ_VQ_TABLE.iter().enumerate() {
        csv.push_str(&index.to_string());
        for element in row {
            csv.push_str(&format!(",{element}"));
        }
        csv.push_str("\r\n");
    }
    assert_eq!(sha256_hex(csv.as_bytes()), HFREQ_CSV_SHA256);
}
