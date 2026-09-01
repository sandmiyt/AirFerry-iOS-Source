//! End-to-end large-transfer segmentation test.
//!
//! A logical transfer is compressed **once** into a single compressed stream,
//! then split into fixed `SEGMENT_RAW_BYTES` (32 MiB) segments. Each segment is
//! independently RaptorQ-encoded with its own child session id (see
//! `SessionId::derive_segment`) and a descriptor-v5 frame. The receiver
//! recovers each segment with an ordinary `ReceiverSession`, then hands the
//! recovered *compressed* bytes to a `TransferAssembler` which validates each
//! segment's length + SHA-256 and concatenates the full compressed stream once
//! all segments have arrived. Decompression happens at the host layer after the
//! concatenation (here COMPRESSION_NONE so the compressed stream == original).
//!
//! This test drives the whole chain through the real QR frame wire format,
//! including simulated frame loss, so it exercises descriptor-v5 parsing, the
//! sender's `new_segment`, the receiver's segment metadata exposure, and the
//! assembler's validation + concatenation.

use qr_protocol::{Frame, SessionId};
use raptorq_core::Config;
use sha2::{Digest, Sha256};
use transfer_engine::assembler::TransferAssembler;
use transfer_engine::receiver::ReceiverSession;
use transfer_engine::sender::{SenderConfig, SenderSession};
use transfer_engine::{FileMeta, SegmentMeta, SEGMENT_RAW_BYTES};

fn pseudo_random(n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| ((i * 1103515245 + 12345) & 0xff) as u8)
        .collect()
}

/// High-entropy deterministic bytes via xorshift64*: unlike [`pseudo_random`]
/// (a low-entropy linear pattern that zstd crushes), this is ~incompressible, so
/// a zstd-compressed version of a > 32 MiB input genuinely spans multiple
/// segments.
fn high_entropy(n: usize) -> Vec<u8> {
    let mut state = 0x2545F4914F6CDD1Du64;
    let mut data = Vec::with_capacity(n);
    while data.len() < n {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.push((state >> 24) as u8);
    }
    data
}

/// Segment the `root` compressed stream into its canonical 32 MiB (minus final)
/// slices and wrap each in a `SegmentMeta` whose `raw_sha256` is the SHA-256 of
/// that slice (the segment's compressed bytes). Here compression is a no-op
/// (COMPRESSION_NONE), so the compressed stream is the original bytes.
fn split_root(root: &[u8], root_session_id: u128) -> Vec<SegmentMeta> {
    let root_sha256: [u8; 32] = Sha256::digest(root).into();
    let count = if root.is_empty() {
        1
    } else {
        (root.len() as u64).div_ceil(SEGMENT_RAW_BYTES) as u32
    };
    (0..count)
        .map(|i| {
            let start = (i as usize) * SEGMENT_RAW_BYTES as usize;
            let end = (start + SEGMENT_RAW_BYTES as usize).min(root.len());
            let raw = &root[start..end];
            let digest = Sha256::digest(raw);
            let mut raw_sha256 = [0u8; 32];
            raw_sha256.copy_from_slice(&digest);
            SegmentMeta {
                root_session_id,
                segment_index: i,
                segment_count: count,
                original_offset: (i as u64) * SEGMENT_RAW_BYTES,
                root_original_size: root.len() as u64,
                root_sha256,
                raw_sha256,
            }
        })
        .collect()
}

/// Recover one segment over the wire with a `ReceiverSession`, returning the
/// recovered **compressed** bytes. Applies simulated loss (drop 1 in
/// `drop_every`).
fn recover_segment(
    root_session_id: u128,
    seg: &SegmentMeta,
    segment_bytes: &[u8],
    root_original_size: u64,
    redundancy: u8,
    drop_every: u32,
) -> Vec<u8> {
    recover_segment_with(
        root_session_id,
        seg,
        segment_bytes,
        root_original_size,
        redundancy,
        drop_every,
        qr_protocol::compress::COMPRESSION_NONE,
    )
}

/// Like [`recover_segment`] but recovers this segment's **compressed** bytes
/// (via `assemble_raw`, no decompression) under an arbitrary compression tag.
/// In the compressed-stream model a segment is a slice of the whole compressed
/// stream, so it is not independently decompressible — recovering it raw is
/// what lets the host concatenate and decompress once.
fn recover_segment_with(
    root_session_id: u128,
    seg: &SegmentMeta,
    segment_bytes: &[u8],
    root_original_size: u64,
    redundancy: u8,
    drop_every: u32,
    compression: u8,
) -> Vec<u8> {
    let child = SessionId::derive_segment(root_session_id, seg.segment_index);
    // `compressed_size` is this segment's real pre-padding payload length. The
    // receiver trims RaptorQ symbol padding back to this value before returning
    // bytes. `original_size` carries the whole-file decompressed size (constant
    // across segments in the compressed-stream model).
    let fm = FileMeta {
        filename: "big.bin".into(),
        original_size: root_original_size,
        crc32: 0,
        compression,
        compressed_size: segment_bytes.len() as u64,
        compressed_size_known: true,
        crc32_known: false,
    };
    let mut sender = SenderSession::new_segment(
        segment_bytes,
        child,
        SenderConfig {
            // This test exercises the full descriptor/frame/RaptorQ pipeline,
            // not QR matrix capacity. A large legal symbol keeps the real
            // 32 MiB segment test to ~514 source symbols instead of ~32768, so a
            // debug `cargo test` finishes in seconds rather than hours.
            codec: Config::new(65_528).expect("large test symbol"),
            redundancy_pct: redundancy,
        },
        fm,
        seg.clone(),
    )
    .expect("segment sender");

    let total_k = sender.total_k();
    let batch = (total_k as usize) * 3 + 64;
    let mut rx: Option<ReceiverSession> = None;
    let mut emitted = 0u32;
    for _ in 0..batch {
        if rx.as_ref().is_some_and(|r| r.is_complete()) {
            break;
        }
        let f = sender.next_frame().unwrap();
        emitted += 1;
        if drop_every > 0 && emitted % drop_every == 0 {
            continue;
        }
        let bytes = f.to_bytes();
        let parsed = Frame::from_bytes(&bytes).unwrap();
        if rx.is_none() {
            rx = Some(ReceiverSession::from_first_frame(&parsed));
        }
        let _ = rx.as_mut().unwrap().ingest(parsed);
    }
    let rx = rx.expect("receiver never created");
    assert!(
        rx.is_complete(),
        "segment {} failed to recover",
        seg.segment_index
    );
    // A v5 descriptor must expose the segment metadata.
    assert_eq!(
        rx.segment_meta().map(|s| s.segment_index),
        Some(seg.segment_index),
        "receiver must expose descriptor-v5 segment meta"
    );
    // Trim RaptorQ symbol padding back to this segment's real compressed size,
    // mirroring how the native hosts (jni/cffi) post-process `assemble_raw`.
    let raw = rx.assemble_raw().expect("assemble raw segment bytes");
    let size = rx.file_meta().compressed_size as usize;
    let size = size.min(raw.len());
    raw[..size].to_vec()
}

/// Full segmented transfer: send every segment, recover each, concatenate.
fn segmented_cycle(root: &[u8], redundancy: u8, drop_every: u32) -> Vec<u8> {
    let root_session_id = SessionId::derive("big", root.len() as u64, 0, &[]).0;
    let segments = split_root(root, root_session_id);

    // Start the assembler from the first segment's metadata.
    let mut assembler = {
        let first = &segments[0];
        let fm = FileMeta {
            filename: "big.bin".into(),
            original_size: root.len() as u64,
            crc32: 0,
            compression: qr_protocol::compress::COMPRESSION_NONE,
            compressed_size: first_compressed_len(root, first),
            compressed_size_known: true,
            crc32_known: false,
        };
        TransferAssembler::new(first, &fm).expect("assembler from first segment")
    };

    for (i, seg) in segments.iter().enumerate() {
        let start = i * SEGMENT_RAW_BYTES as usize;
        let end = (start + SEGMENT_RAW_BYTES as usize).min(root.len());
        let recovered = recover_segment(
            root_session_id,
            seg,
            &root[start..end],
            root.len() as u64,
            redundancy,
            drop_every,
        );
        let fm = FileMeta {
            filename: "big.bin".into(),
            original_size: root.len() as u64,
            crc32: 0,
            compression: qr_protocol::compress::COMPRESSION_NONE,
            compressed_size: recovered.len() as u64,
            compressed_size_known: true,
            crc32_known: false,
        };
        let stored = assembler
            .add_segment(seg, &fm, recovered)
            .expect("assembler add segment");
        assert!(stored, "segment {i} should be newly stored");
    }

    assert!(assembler.is_complete());
    assert_eq!(assembler.received_segments(), segments.len() as u32);
    assembler
        .reassemble()
        .expect("reassemble compressed stream")
}

/// Canonical compressed length of the first segment of `root`.
fn first_compressed_len(root: &[u8], first: &SegmentMeta) -> u64 {
    let remaining = (root.len() as u64).saturating_sub(first.original_offset);
    remaining.min(SEGMENT_RAW_BYTES)
}

#[test]
fn segmented_transfer_reassembles_multi_segment_file() {
    // 2 segments worth of data (1 full 32 MiB + a tail) — keep the allocator
    // footprint to ~32 MiB so the test still finishes quickly in debug.
    let root = pseudo_random(SEGMENT_RAW_BYTES as usize + 4096);
    let out = segmented_cycle(&root, 15, 0);
    assert_eq!(
        out, root,
        "reassembled compressed stream must match original"
    );
}

#[test]
fn segmented_transfer_survives_frame_loss() {
    let root = pseudo_random(SEGMENT_RAW_BYTES as usize + 8192);
    let out = segmented_cycle(&root, 30, 7); // drop ~1 in 7 frames per segment
    assert_eq!(out, root);
}

#[test]
fn segmented_single_segment_file() {
    // Exercise a payload that is not aligned to the default 1024-byte symbol.
    let root = pseudo_random(1234);
    let out = segmented_cycle(&root, 10, 0);
    assert_eq!(out, root);
}

/// Compression-mode segmented transfer: compress a (compressible) original once
/// with zstd, split the resulting compressed stream into segments, recover each
/// segment's **compressed** bytes over the wire, concatenate them, then
/// decompress exactly once and verify it equals the original. This exercises
/// the compressed-stream model's one core invariant that the COMPRESSION_NONE
/// tests cannot: a single zstd stream is NOT independently sliceable, so only
/// concatenation + single decompression recovers the original.
#[test]
fn segmented_transfer_with_compression_reassembles() {
    // High-entropy (≈ incompressible) payload sized so the zstd "compressed
    // stream" is ~two 32 MiB segments. zstd still round-trips it, and — the
    // point of this test — it forces the compressed stream to genuinely span
    // multiple segments, which a single monolithic stream cannot be sliced
    // into independently-decodable pieces for.
    let original = high_entropy(2 * SEGMENT_RAW_BYTES as usize + 4096);
    let compressed_stream = qr_protocol::compress::compress(&original, 1).expect("zstd compress");
    assert!(
        compressed_stream.len() > SEGMENT_RAW_BYTES as usize,
        "compressed stream must span multiple segments for this test"
    );

    let root_session_id = SessionId::derive("big", original.len() as u64, 0, &[]).0;
    let segments = split_root(&compressed_stream, root_session_id);
    assert!(
        segments.len() > 1,
        "compressed stream should split into >1 segment"
    );

    let mut assembler = {
        let first = &segments[0];
        let fm = FileMeta {
            filename: "big.bin".into(),
            original_size: original.len() as u64, // whole decompressed size
            crc32: 0,
            compression: qr_protocol::compress::COMPRESSION_ZSTD,
            compressed_size: first_compressed_len(&compressed_stream, first),
            compressed_size_known: true,
            crc32_known: false,
        };
        TransferAssembler::new(first, &fm).expect("assembler from first segment")
    };

    for (i, seg) in segments.iter().enumerate() {
        let start = i * SEGMENT_RAW_BYTES as usize;
        let end = (start + SEGMENT_RAW_BYTES as usize).min(compressed_stream.len());
        let recovered = recover_segment_with(
            root_session_id,
            seg,
            &compressed_stream[start..end],
            original.len() as u64, // whole decompressed size
            15,
            0,
            qr_protocol::compress::COMPRESSION_ZSTD,
        );
        let fm = FileMeta {
            filename: "big.bin".into(),
            original_size: original.len() as u64,
            crc32: 0,
            compression: qr_protocol::compress::COMPRESSION_ZSTD,
            compressed_size: recovered.len() as u64,
            compressed_size_known: true,
            crc32_known: false,
        };
        let stored = assembler
            .add_segment(seg, &fm, recovered)
            .expect("assembler add segment");
        assert!(stored, "segment {i} should be newly stored");
    }

    assert!(assembler.is_complete());
    let stream = assembler
        .reassemble()
        .expect("reassemble compressed stream");
    assert_eq!(
        stream, compressed_stream,
        "reassembled compressed stream must match"
    );

    // Host-side single decompression over the concatenated compressed stream.
    let out = qr_protocol::compress::decompress_with_limit(
        &stream,
        qr_protocol::compress::COMPRESSION_ZSTD,
        original.len() + 1,
    )
    .expect("decompress concatenated stream once");
    assert_eq!(out, original, "decompressed original must match");
}
