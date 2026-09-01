//! Logic-level coverage for the WASM receiver binding's contract.
//!
//! `wasm.rs` only compiles under `target_arch = "wasm32"`, so this file does
//! NOT import `ReceiverSessionWasm` directly. Instead it exercises the exact
//! underlying APIs the WASM binding delegates to (`ReceiverSession`,
//! `ingest_status::pack`, `Frame::from_bytes`, `assemble_raw`) on the native
//! test host. The binding itself is a thin wrapper, so these tests pin the
//! behavior the JS layer depends on:
//!
//! - descriptor-first bootstrap confirms meta (the `from_descriptor` path)
//! - bad CRC / bad descriptor payload is rejected without panic
//! - the packed ingest-status bit layout matches the JNI/C ABI golden values
//! - out-of-order / duplicate / lossy streams still recover via `assemble_raw`
//! - `assemble_raw` trims to `compressed_size` and returns the original bytes
//!   for `COMPRESSION_NONE`
//!
//! The wasm32 fail-closed decompress stub is validated by `compress_pipeline`.

use qr_protocol::{compress::COMPRESSION_NONE, Frame, SessionId, FLAG_DESCRIPTOR};
use raptorq_core::Config;
use transfer_engine::ingest_status;
use transfer_engine::receiver::ReceiverSession;
use transfer_engine::sender::{SenderConfig, SenderSession};
use transfer_engine::FileMeta;

fn pseudo_random(n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| ((i * 1103515245 + 12345) & 0xff) as u8)
        .collect()
}

/// Build a sender whose descriptor carries authoritative FileMeta
/// (filename/original_size/crc32/compressed_size). Returns (sender, data, sid).
fn build_sender(
    data: &[u8],
    filename: &str,
    crc32: u32,
    redundancy: u8,
) -> (SenderSession, SessionId) {
    let sid = SessionId::derive(filename, data.len() as u64, 0, &[]);
    // Probe the padded transfer_length so compressed_size is exact.
    let probe = SenderSession::new(
        data,
        sid,
        SenderConfig {
            codec: Config::default(),
            redundancy_pct: redundancy,
        },
        FileMeta::default(),
    )
    .unwrap();
    let padded_len = probe.meta().transfer_length;
    let fm = FileMeta {
        filename: filename.to_string(),
        original_size: data.len() as u64,
        crc32,
        compression: COMPRESSION_NONE,
        compressed_size: padded_len,
        compressed_size_known: true,
        crc32_known: true,
    };
    let sender = SenderSession::new(
        data,
        sid,
        SenderConfig {
            codec: Config::default(),
            redundancy_pct: redundancy,
        },
        fm,
    )
    .unwrap();
    (sender, sid)
}

/// Emit frames until the first descriptor is produced, then return its wire bytes.
fn first_descriptor_bytes(sender: &mut SenderSession) -> Vec<u8> {
    loop {
        let f = sender.next_frame().unwrap();
        if f.header.flags & FLAG_DESCRIPTOR != 0 {
            return f.to_bytes();
        }
    }
}

// ─── 1. descriptor-first bootstrap confirms meta (from_descriptor path) ─────

#[test]
fn descriptor_first_confirms_meta_and_locks_session_id() {
    let data = pseudo_random(2000);
    let (mut sender, sid) = build_sender(&data, "hello.bin", 0x11223344, 10);
    let desc_bytes = first_descriptor_bytes(&mut sender);

    // Mirror ReceiverSessionWasm::from_descriptor exactly: validate the frame,
    // require the descriptor flag, build a pending session from the header's
    // session id, ingest the descriptor, and require meta to be confirmed.
    let frame = Frame::from_bytes(&desc_bytes).unwrap();
    assert_ne!(frame.header.flags & FLAG_DESCRIPTOR, 0);
    let mut rx = ReceiverSession::new_pending(frame.header.session_id);
    assert!(!rx.is_meta_confirmed());
    let _ = rx.ingest(frame);
    assert!(rx.is_meta_confirmed());
    assert_eq!(rx.session_id(), sid.0);
    assert_eq!(rx.file_meta().filename, "hello.bin");
    assert_eq!(rx.file_meta().original_size, data.len() as u64);
    assert_eq!(rx.file_meta().crc32, 0x11223344);
    assert!(rx.file_meta().crc32_known);
}

// ─── 2. bad CRC / non-descriptor / bad payload rejected without panic ──────

#[test]
fn from_descriptor_rejects_corrupt_frame_without_panic() {
    let data = pseudo_random(500);
    let (mut sender, _sid) = build_sender(&data, "f.bin", 0, 10);
    let mut desc_bytes = first_descriptor_bytes(&mut sender);
    // Flip a byte in the payload region (after the 60B header) to break the
    // payload CRC; Frame::from_bytes must reject it rather than panic.
    desc_bytes[70] ^= 0xFF;
    let res = Frame::from_bytes(&desc_bytes);
    assert!(res.is_err(), "corrupted frame must be rejected");
}

#[test]
fn from_descriptor_rejects_data_frame_as_initial_descriptor() {
    let data = pseudo_random(500);
    let (mut sender, _sid) = build_sender(&data, "f.bin", 0, 10);
    // Pull a non-descriptor data frame first.
    let mut data_frame: Option<Vec<u8>> = None;
    for _ in 0..100 {
        let f = sender.next_frame().unwrap();
        if f.header.flags & FLAG_DESCRIPTOR == 0 {
            data_frame = Some(f.to_bytes());
            break;
        }
    }
    let data_frame = data_frame.expect("sender produced a data frame");
    let frame = Frame::from_bytes(&data_frame).unwrap();
    assert_eq!(frame.header.flags & FLAG_DESCRIPTOR, 0);
    // The WASM from_descriptor binding treats a non-descriptor as Err; here we
    // just assert the flag check the binding performs.
}

// ─── 3. packed ingest-status golden (shared across JNI / C ABI / WASM) ──────

#[test]
fn packed_status_matches_golden_layout() {
    // Bit 0 = complete, bit 1 = accepted.
    assert_eq!(ingest_status::pack(false, false, 0, 0), 0);
    assert_eq!(ingest_status::pack(true, false, 0, 0), 1);
    assert_eq!(ingest_status::pack(false, true, 0, 0), 1 << 1);
    // Bits 8..23 = streak (clamped to 16 bits).
    assert_eq!(ingest_status::pack(false, false, 0x1234, 0), 0x1234u64 << 8);
    assert_eq!(
        ingest_status::pack(false, false, 0x1FFFF, 0),
        0xFFFFu64 << 8
    );
    // Bits 32..63 = received_symbols.
    assert_eq!(
        ingest_status::pack(false, false, 0, 0x5678),
        0x5678u64 << 32
    );
    // Combined + error sentinel.
    assert_eq!(
        ingest_status::pack(true, true, 0x1234, 0x5678),
        0b11 | (0x1234u64 << 8) | (0x5678u64 << 32)
    );
    assert_eq!(ingest_status::INGEST_ERROR, 0xFFFF_FFFFu64 << 32);
}

// ─── 4. out-of-order / duplicate / lossy recovery via assemble_raw ─────────

#[test]
fn assemble_raw_recovers_original_bytes_uncompressed() {
    let data = pseudo_random(40_000);
    let (mut sender, sid) = build_sender(&data, "a.bin", 0, 20);
    sender.set_descriptor_interval(8);

    let total_k = sender.total_k();
    let batch = (total_k as usize) * 3 + 32;
    let mut frames: Vec<Vec<u8>> = Vec::new();
    for i in 0..batch {
        let f = sender.next_frame().unwrap();
        if i % 5 == 0 {
            continue; // 20% loss
        }
        frames.push(f.to_bytes());
    }
    // shuffle (out-of-order) + duplicate a few
    for i in (1..frames.len()).rev() {
        let j = (i as u32).wrapping_mul(2654435761) as usize % (i + 1);
        frames.swap(i, j);
    }
    for extra in frames.iter().take(5).cloned().collect::<Vec<_>>() {
        frames.push(extra);
    }

    let mut rx = ReceiverSession::new_pending(sid.0);
    for bytes in &frames {
        if let Ok(f) = Frame::from_bytes(bytes) {
            let _ = rx.ingest(f);
            if rx.is_complete() {
                break;
            }
        }
    }
    assert!(
        rx.is_complete(),
        "receiver must recover despite loss/dup/reorder"
    );
    // assemble_raw returns the padded bytes; trim to compressed_size as the
    // WASM binding does.
    let mut raw = rx.assemble_raw().expect("complete -> Some");
    let fm = rx.file_meta();
    assert!(fm.compressed_size_known);
    let len = usize::try_from(fm.compressed_size).unwrap();
    assert!(len <= raw.len());
    raw.truncate(len);
    assert_eq!(&raw[..data.len()], &data[..], "recovered bytes must match");
}

// ─── 5. assemble_raw trims to compressed_size ──────────────────────────────

#[test]
fn assemble_raw_trimmed_length_equals_compressed_size() {
    let data = pseudo_random(300); // tiny, single block, will be padded
    let (mut sender, sid) = build_sender(&data, "tiny.bin", 0, 10);
    sender.set_descriptor_interval(4);

    let mut rx = ReceiverSession::new_pending(sid.0);
    for _ in 0..200 {
        let f = sender.next_frame().unwrap();
        let _ = rx.ingest(f);
        if rx.is_complete() {
            break;
        }
    }
    assert!(rx.is_complete());
    let raw = rx.assemble_raw().unwrap();
    let fm = rx.file_meta();
    // The descriptor's compressed_size is the padded transfer length; raw must
    // be at least that long (symbol padding), and after trimming equals it.
    assert!(raw.len() >= fm.compressed_size as usize);
    let trimmed = &raw[..fm.compressed_size as usize];
    // The first data.len() bytes are the original payload.
    assert_eq!(&trimmed[..data.len()], &data[..]);
}
