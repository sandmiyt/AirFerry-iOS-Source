//! WebAssembly bindings for the browser extension (sender side).
//!
//! Exposes a thin `SenderSessionWasm` that the TypeScript/React layer drives
//! from its render loop. Returns frames as `Uint8Array` (via `Vec<u8>` → JS).

#![cfg(feature = "wasm")]

use crate::sender::{SenderConfig, SenderSession};
use qr_protocol::SessionId;
use raptorq_core::Config;
use sha2::{Digest, Sha256};
use wasm_bindgen::prelude::*;

/// Install a panic hook that routes Rust panic messages to the JS console.
/// Without this, a panic surfaces only as a bare `RuntimeError: unreachable`.
/// Call once at module init.
#[wasm_bindgen(start)]
pub fn _start() {
    static SET: std::sync::Once = std::sync::Once::new();
    SET.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            web_sys::console::error_1(&format!("AirFerry WASM panic: {info}").into());
        }));
    });
}

/// Incremental SHA-256 for browser workers. WebCrypto only exposes a one-shot
/// digest API, which would require materialising a multi-gigabyte file in one
/// `ArrayBuffer`; this wrapper lets the sender hash `File.slice()` chunks while
/// keeping memory bounded.
#[wasm_bindgen]
pub struct Sha256Wasm {
    inner: Sha256,
}

#[wasm_bindgen]
impl Sha256Wasm {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: Sha256::new(),
        }
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.inner.update(bytes);
    }

    pub fn digest(&self) -> Vec<u8> {
        self.inner.clone().finalize().to_vec()
    }
}

/// WASM-facing sender session.
#[wasm_bindgen]
pub struct SenderSessionWasm {
    inner: SenderSession,
    /// Stable, pre-sized WASM-owned output used by the browser hot path.
    qr_scratch: Vec<u8>,
}

const MAX_QR_SIDE: usize = 177;
const MAX_QR_MODULES: usize = MAX_QR_SIDE * MAX_QR_SIDE;
const MAX_UI_QR_COUNT: usize = 4;
const QR_SCRATCH_BYTES: usize = 4 + MAX_UI_QR_COUNT * (4 + MAX_QR_MODULES);

#[wasm_bindgen]
impl SenderSessionWasm {
    /// Create from payload bytes + session id + file metadata.
    ///
    /// `compression` is a [`qr_protocol::compress`] tag (0=None, 1=Zstd, 2=Xz)
    /// identifying how `compressed_payload` was produced; the receiver runs the
    /// matching decompressor after RaptorQ recovery.
    #[wasm_bindgen(constructor)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        compressed_payload: &[u8],
        session_id_lo: u64,
        session_id_hi: u64,
        redundancy_pct: u8,
        symbol_size: u32,
        filename: &str,
        original_file_size: u64,
        crc32: u32,
        compression: u8,
    ) -> Result<SenderSessionWasm, JsValue> {
        _start();
        let sid = SessionId(((session_id_hi as u64 as u128) << 64) | session_id_lo as u64 as u128);
        // Enforce the public range/alignment contract before the value reaches
        // RaptorQ's MTU arithmetic.
        let codec = Config::new(symbol_size).map_err(|e| JsValue::from_str(&format!("{e}")))?;
        let cfg = SenderConfig {
            codec,
            redundancy_pct,
        };
        let file_meta = crate::descriptor::FileMeta {
            filename: filename.to_string(),
            original_size: original_file_size,
            crc32,
            compression,
            compressed_size: compressed_payload.len() as u64,
            compressed_size_known: true,
            crc32_known: true,
        };
        let inner =
            SenderSession::new(compressed_payload, sid, cfg, file_meta).map_err(err_to_js)?;
        Ok(SenderSessionWasm {
            inner,
            qr_scratch: vec![0; QR_SCRATCH_BYTES],
        })
    }

    /// Create one descriptor-v5 compressed-stream segment session.
    ///
    /// A logical transfer is compressed once into a single compressed stream,
    /// then split into N fixed `SEGMENT_RAW_BYTES` (≈ 32 MiB) slices of that
    /// stream; each slice is RaptorQ-encoded independently. This constructor
    /// wraps one such slice as its own session whose child `session_id` is
    /// deterministically derived from `(root_session_id, segment_index)` — so
    /// the outer 60-byte frame format is unchanged and a receiver
    /// demultiplexes segments purely by that distinct session id.
    ///
    /// `raw_sha256` is the SHA-256 of this segment's *compressed* bytes
    /// (32 bytes); `original_size` is the whole decompressed original size
    /// (shared across every segment; may exceed 32 MiB). `root_session_id_lo/
    /// hi`, `segment_index`, `segment_count`, `original_offset`,
    /// `root_original_size` describe the segment's canonical range within the
    /// root file.
    /// Static factory: `SenderSessionWasm.new_segment(...)`. Exposed as an
    /// associated function (not a constructor) because `SenderSessionWasm`
    /// already has a primary `new` constructor.
    #[wasm_bindgen]
    #[allow(clippy::too_many_arguments)]
    pub fn new_segment(
        compressed_payload: &[u8],
        root_session_id_lo: u64,
        root_session_id_hi: u64,
        segment_index: u32,
        segment_count: u32,
        original_offset: u64,
        root_original_size: u64,
        root_sha256: &[u8],
        raw_sha256: &[u8],
        redundancy_pct: u8,
        symbol_size: u32,
        filename: &str,
        original_size: u64,
        crc32: u32,
        compression: u8,
    ) -> Result<SenderSessionWasm, JsValue> {
        _start();
        let root = ((root_session_id_hi as u128) << 64) | root_session_id_lo as u128;
        let child = SessionId::derive_segment(root, segment_index);
        let codec = Config::new(symbol_size).map_err(|e| JsValue::from_str(&format!("{e}")))?;
        let cfg = SenderConfig {
            codec,
            redundancy_pct,
        };
        let file_meta = crate::descriptor::FileMeta {
            filename: filename.to_string(),
            original_size,
            crc32,
            compression,
            compressed_size: compressed_payload.len() as u64,
            compressed_size_known: true,
            crc32_known: true,
        };
        if root_sha256.len() != 32 || raw_sha256.len() != 32 {
            return Err(JsValue::from_str(
                "root_sha256 and raw_sha256 must be exactly 32 bytes",
            ));
        }
        let mut root_sha = [0u8; 32];
        root_sha.copy_from_slice(&root_sha256[..32]);
        let mut sha = [0u8; 32];
        sha.copy_from_slice(&raw_sha256[..32]);
        let segment_meta = crate::segment::SegmentMeta {
            root_session_id: root,
            segment_index,
            segment_count,
            original_offset,
            root_original_size,
            root_sha256: root_sha,
            raw_sha256: sha,
        };
        let inner =
            SenderSession::new_segment(compressed_payload, child, cfg, file_meta, segment_meta)
                .map_err(err_to_js)?;
        Ok(SenderSessionWasm {
            inner,
            qr_scratch: vec![0; QR_SCRATCH_BYTES],
        })
    }

    /// Produce the next frame's raw bytes (header + payload + footer).
    pub fn next_frame(&mut self) -> Result<Vec<u8>, JsValue> {
        let frame = self.inner.next_frame().map_err(err_to_js)?;
        Ok(frame.to_bytes())
    }

    /// Produce the next frame AND encode it to a QR matrix in one call.
    ///
    /// This fuses `next_frame()` + `encode_qr()` so the per-frame path crosses
    /// the WASM/JS boundary once instead of twice, avoiding the intermediate
    /// `Uint8Array` copy of the raw frame bytes (the JS layer never needs the
    /// raw frame — only the rendered matrix). Returns the flat module grid as
    /// `Vec<u8>` (1 = dark, 0 = light, row-major); `out_side[0]` is set to the
    /// side length. An empty `Vec` (side 0) signals the session produced no
    /// frame this tick.
    pub fn next_qr(&mut self, out_side: &mut [u32]) -> Result<Vec<u8>, JsValue> {
        if out_side.is_empty() {
            return Err(JsValue::from_str("out_side buffer empty"));
        }
        let frame = self.inner.next_frame().map_err(err_to_js)?;
        let bytes = frame.to_bytes();
        let matrix = qr_protocol::qr_render::encode(&bytes)
            .map_err(|e| JsValue::from_str(&format!("qr encode failed: {e:?}")))?;
        out_side[0] = matrix.size as u32;
        Ok(matrix.modules.iter().map(|&dark| dark as u8).collect())
    }

    /// Produce `count` next frames, each encoded to a QR matrix, in one WASM
    /// call — for the multi-QR-per-screen experimental mode. Each frame is a
    /// distinct symbol of the same session (different sbn/esi), so a receiver
    /// that decodes all on-screen codes at once gets `count` new symbols per
    /// tick instead of one, multiplying throughput by ~`count` (bounded by the
    /// camera resolving N smaller codes).
    ///
    /// Returns a flat little-endian buffer the JS layer parses:
    ///   `[u32 count_actual][for each matrix: u32 side + side*side bytes]`
    /// where each module byte is 1=dark / 0=light, row-major. `count_actual`
    /// may be less than `count` if the session could not produce that many
    /// (normally equal to `count`; it may be lower at the RFC 24-bit ESI
    /// boundary). An empty buffer / count_actual == 0 signals failure.
    pub fn next_qr_multi(&mut self, count: u32) -> Result<Vec<u8>, JsValue> {
        // Cap at a sane maximum to bound allocation; the UI only offers 2/4.
        let n = count.min(8) as usize;
        let mut out: Vec<u8> = Vec::new();
        // Reserve the count slot; we'll fill it after we know how many succeeded.
        out.extend_from_slice(&0u32.to_le_bytes());
        let mut produced = 0u32;
        for _ in 0..n {
            let frame = match self.inner.next_frame() {
                Ok(f) => f,
                Err(e) => {
                    // Stop producing on error; return what we have so far.
                    android_log_wasm(&format!("next_qr_multi frame err: {e}"));
                    break;
                }
            };
            let bytes = frame.to_bytes();
            let matrix = match qr_protocol::qr_render::encode(&bytes) {
                Ok(m) => m,
                Err(e) => {
                    android_log_wasm(&format!("next_qr_multi qr err: {e:?}"));
                    break;
                }
            };
            out.extend_from_slice(&(matrix.size as u32).to_le_bytes());
            out.extend(matrix.modules.iter().map(|&dark| dark as u8));
            produced += 1;
        }
        out[..4].copy_from_slice(&produced.to_le_bytes());
        Ok(out)
    }

    /// Compatibility buffer variant of [`Self::next_qr`]. wasm-bindgen may copy
    /// JS-owned slices at the ABI boundary; new renderers should use
    /// [`Self::next_qr_scratch`] + [`Self::qr_scratch_view`] for true zero-copy.
    ///
    /// The caller must size `out_modules` to at least `side*side` bytes — the
    /// largest possible QR is Version 40 (177×177 = 31329 B), so a 32 KiB
    /// buffer is always safe. `out_side[0]` is set to the matrix side length.
    /// Returns the number of module bytes written (= `side*side`).
    ///
    pub fn next_qr_into(
        &mut self,
        out_modules: &mut [u8],
        out_side: &mut [u32],
    ) -> Result<u32, JsValue> {
        if out_side.is_empty() {
            return Err(JsValue::from_str("out_side buffer empty"));
        }
        let frame = self.inner.next_frame().map_err(err_to_js)?;
        let bytes = frame.to_bytes();
        let matrix = qr_protocol::qr_render::encode(&bytes)
            .map_err(|e| JsValue::from_str(&format!("qr encode failed: {e:?}")))?;
        let n = matrix.modules.len();
        if n > out_modules.len() {
            return Err(JsValue::from_str(&format!(
                "out_modules too small: need {n}, have {}",
                out_modules.len()
            )));
        }
        for (dst, &dark) in out_modules[..n].iter_mut().zip(matrix.modules.iter()) {
            *dst = dark as u8;
        }
        out_side[0] = matrix.size as u32;
        Ok(n as u32)
    }

    /// Compatibility multi-buffer variant. New renderers should prefer the
    /// WASM-owned scratch API to avoid wasm-bindgen slice copies.
    ///
    /// Buffer layout (same as `next_qr_multi`):
    ///   `[u32 count_actual][for each matrix: u32 side + side*side bytes]`
    /// Sizing: `4 + count * (4 + 177*177)` bytes is always safe (one u32 count
    /// slot + per-matrix header + largest-version matrix). For the UI's 4-code
    /// mode that is `4 + 4*(4+31329) ≈ 125 KiB`. Returns total bytes written.
    pub fn next_qr_multi_into(&mut self, count: u32, out_buf: &mut [u8]) -> Result<u32, JsValue> {
        let n = count.min(8) as usize;
        let mut pos: usize = 0;
        // Reserve and later backfill the count slot.
        if out_buf.len() < 4 {
            return Err(JsValue::from_str("out_buf too small for count slot"));
        }
        let count_slot = pos;
        pos += 4;
        let mut produced: u32 = 0;
        for _ in 0..n {
            let frame = match self.inner.next_frame() {
                Ok(f) => f,
                Err(e) => {
                    android_log_wasm(&format!("next_qr_multi_into frame err: {e}"));
                    break;
                }
            };
            let bytes = frame.to_bytes();
            let matrix = match qr_protocol::qr_render::encode(&bytes) {
                Ok(m) => m,
                Err(e) => {
                    android_log_wasm(&format!("next_qr_multi_into qr err: {e:?}"));
                    break;
                }
            };
            let side_bytes = (matrix.size as u32).to_le_bytes();
            let need = 4 + matrix.modules.len();
            if pos + need > out_buf.len() {
                return Err(JsValue::from_str(&format!(
                    "out_buf overflow at matrix {}: need {need} at pos {pos}, have {}",
                    produced,
                    out_buf.len()
                )));
            }
            out_buf[pos..pos + 4].copy_from_slice(&side_bytes);
            pos += 4;
            for (dst, &dark) in out_buf[pos..pos + matrix.modules.len()]
                .iter_mut()
                .zip(matrix.modules.iter())
            {
                *dst = dark as u8;
            }
            pos += matrix.modules.len();
            produced += 1;
        }
        out_buf[count_slot..count_slot + 4].copy_from_slice(&produced.to_le_bytes());
        Ok(pos as u32)
    }

    /// Encode 1..=4 fresh matrices into stable WASM-owned scratch memory.
    /// Call [`Self::qr_scratch_view`] immediately afterwards to obtain a
    /// zero-copy JavaScript view of the first returned byte count.
    pub fn next_qr_scratch(&mut self, count: u32) -> Result<u32, JsValue> {
        let n = count.clamp(1, MAX_UI_QR_COUNT as u32) as usize;
        let mut pos = 4usize;
        let mut produced = 0u32;
        for _ in 0..n {
            let frame = self.inner.next_frame().map_err(err_to_js)?;
            let matrix = qr_protocol::qr_render::encode(&frame.to_bytes())
                .map_err(|e| JsValue::from_str(&format!("qr encode failed: {e:?}")))?;
            let need = 4 + matrix.modules.len();
            if pos + need > self.qr_scratch.len() {
                return Err(JsValue::from_str("internal QR scratch buffer overflow"));
            }
            self.qr_scratch[pos..pos + 4].copy_from_slice(&(matrix.size as u32).to_le_bytes());
            pos += 4;
            for (dst, &dark) in self.qr_scratch[pos..pos + matrix.modules.len()]
                .iter_mut()
                .zip(matrix.modules.iter())
            {
                *dst = dark as u8;
            }
            pos += matrix.modules.len();
            produced += 1;
        }
        self.qr_scratch[..4].copy_from_slice(&produced.to_le_bytes());
        Ok(pos as u32)
    }

    /// Return a JavaScript `Uint8Array` view over the stable WASM scratch.
    /// The view is valid until the next call into WASM and must not be retained.
    pub fn qr_scratch_view(&self) -> js_sys::Uint8Array {
        // SAFETY: `qr_scratch` is allocated at its final size in the constructor
        // and is never resized. JS consumes the view synchronously before its
        // next WASM call, as required by `Uint8Array::view`.
        unsafe { js_sys::Uint8Array::view(&self.qr_scratch) }
    }

    /// Session id (low 64 bits).
    pub fn session_id_lo(&self) -> u64 {
        self.inner.session_id() as u64
    }
    pub fn session_id_hi(&self) -> u64 {
        (self.inner.session_id() >> 64) as u64
    }

    pub fn total_symbols(&self) -> u32 {
        self.inner.total_k()
    }
    pub fn num_blocks(&self) -> u32 {
        self.inner.num_blocks() as u32
    }

    /// Zero-based index of the current large-transfer segment, or 0 for a
    /// non-segmented session.
    pub fn segment_index(&self) -> u32 {
        self.inner
            .segment_meta()
            .map(|s| s.segment_index)
            .unwrap_or(0)
    }
    /// Total number of segments in the root large-transfer, or 1 for a
    /// non-segmented session.
    pub fn segment_count(&self) -> u32 {
        self.inner
            .segment_meta()
            .map(|s| s.segment_count)
            .unwrap_or(1)
    }
    /// Whether this session is a descriptor-v5 large-transfer child object.
    pub fn is_segmented(&self) -> bool {
        self.inner.segment_meta().is_some()
    }

    /// Live stats as JSON: { bytes, frames, elapsed_ms, fps, throughput_bps }.
    pub fn stats_json(&self) -> String {
        let s = self.inner.stats();
        serde_json::json!({
            "bytes": s.bytes,
            "frames": s.frames,
            "elapsed_ms": s.elapsed_ms,
            "fps": s.fps(),
            "throughput_bps": s.throughput_bps(),
        })
        .to_string()
    }
}

fn err_to_js(e: crate::Error) -> JsValue {
    JsValue::from_str(&format!("{e}"))
}

/// Log a warning to the JS console (used by the multi-QR path on rare frame/QR
/// errors so they're visible without aborting the whole multi-frame tick).
fn android_log_wasm(msg: &str) {
    web_sys::console::warn_1(&format!("AirFerry: {msg}").into());
}

/// Encode `frame_bytes` (a serialized Frame) into a byte-mode EC-L QR matrix.
///
/// Returns the flat module grid as a `Uint8Array` of `side*side` bytes
/// (1 = dark, 0 = light), row-major. `out_side` is set to the side length.
#[wasm_bindgen]
pub fn encode_qr(frame_bytes: &[u8], out_side: &mut [u32]) -> Result<Vec<u8>, JsValue> {
    if out_side.is_empty() {
        return Err(JsValue::from_str("out_side buffer empty"));
    }
    let matrix = qr_protocol::qr_render::encode(frame_bytes)
        .map_err(|e| JsValue::from_str(&format!("qr encode failed: {e:?}")))?;
    out_side[0] = matrix.size as u32;
    Ok(matrix.modules.iter().map(|&dark| dark as u8).collect())
}

// serde_json is required by stats_json (serde_json::json!) and by the
// serde-derived ObjectMeta re-exported here. The `wasm` feature implies
// `serde` (see Cargo.toml), so no compile_error! guard is needed.

// ─── receiver bindings ──────────────────────────────────────────────────────

use crate::ingest_status;
use crate::receiver::ReceiverSession;

/// WASM-facing receiver session.
///
/// Mirrors the JNI (`receiverCreate`/`receiverIngest`/...) and C ABI
/// (`airferry_receiver_*`) bindings so all three hosts share the same wire
/// contract. Construct via [`ReceiverSessionWasm::from_descriptor`] (preferred:
/// validates the first descriptor frame end-to-end) or
/// [`ReceiverSessionWasm::new`] (cache-only bootstrap when the caller already
/// split the session id out of band).
///
/// ## Compression / decompression split
/// Unlike the native bindings, this WASM binding does **not** run the matching
/// decompressor after RaptorQ recovery — the wasm32 build of `qr-protocol`
/// cannot link the native zstd/xz C libraries. Instead the JS layer calls
/// [`assemble_raw`](Self::assemble_raw) to get the transmitted (possibly
/// compressed) bytes, then decompresses with its own zstd/xz WASM and verifies
/// the CRC32. [`assemble_result`](crate::receiver::ReceiverSession::assemble_result)
/// is intentionally not exposed because the wasm32 `decompress_with_limit` is a
/// fail-closed stub for compressed payloads.
#[wasm_bindgen]
pub struct ReceiverSessionWasm {
    inner: ReceiverSession,
}

#[wasm_bindgen]
impl ReceiverSessionWasm {
    /// Create a "cache-only" receiver — no metadata yet, data frames are
    /// buffered until the first validated descriptor arrives. `session_id_lo`
    /// /`session_id_hi` split the 128-bit session id into its low/high 64-bit
    /// halves (host order), matching the JNI/C ABI `receiverCreate` /
    /// `airferry_receiver_create` contract.
    #[wasm_bindgen(constructor)]
    pub fn new(session_id_lo: u64, session_id_hi: u64) -> ReceiverSessionWasm {
        _start();
        let sid = ((session_id_hi as u64 as u128) << 64) | session_id_lo as u64 as u128;
        ReceiverSessionWasm {
            inner: ReceiverSession::new_pending(sid),
        }
    }

    /// Build a receiver from its first descriptor frame.
    ///
    /// Validates the full frame CRC + descriptor flag, locks the session id to
    /// that frame's header, and ingests the descriptor so `meta` is confirmed
    /// immediately. The JS layer should call this on the first descriptor it
    /// observes and discard any earlier data frames.
    ///
    /// Returns `Err` if the bytes are not a valid frame, or a valid frame that
    /// is not a descriptor, or the descriptor payload is hostile/unparseable
    /// (the inner `ingest` rejects it without confirming meta — surfaced as an
    /// error here so the JS caller retries with the next descriptor).
    pub fn from_descriptor(frame_bytes: &[u8]) -> Result<ReceiverSessionWasm, JsValue> {
        _start();
        let frame = qr_protocol::Frame::from_bytes(frame_bytes)
            .map_err(|e| err_to_js(crate::Error::Protocol(e)))?;
        if frame.header.flags & qr_protocol::frame::FLAG_DESCRIPTOR == 0 {
            return Err(JsValue::from_str(
                "from_descriptor: frame is not a descriptor (FLAG_DESCRIPTOR clear)",
            ));
        }
        let mut session = ReceiverSessionWasm::new(
            frame.header.session_id as u64,
            (frame.header.session_id >> 64) as u64,
        );
        // Ingest the descriptor; on success meta is confirmed. A hostile or
        // unparseable descriptor payload is rejected by `ingest` (it bumps
        // frames_corrupt and returns Ok without confirming meta), so we detect
        // "meta still not confirmed" and surface it as an error to the caller.
        let _ = session.ingest(frame_bytes);
        if !session.inner.is_meta_confirmed() {
            return Err(JsValue::from_str(
                "from_descriptor: descriptor rejected (corrupt/hostile payload); meta not confirmed",
            ));
        }
        Ok(session)
    }

    /// Ingest one decoded QR frame's raw bytes (header + payload + footer).
    ///
    /// Returns a packed `u64` status word with the SAME bit layout as the JNI
    /// `receiverIngest` / C ABI `airferry_receiver_ingest` (see
    /// [`ingest_status`]):
    /// - bit  0      : `complete`
    /// - bit  1      : `accepted` (this frame contributed a new symbol)
    /// - bits 8..23  : `session_mismatch_streak`
    /// - bits 32..63 : `received_symbols`
    ///
    /// Returns [`ingest_status::INGEST_ERROR`] (`received_symbols == u32::MAX`)
    /// on a frame that fails CRC / length validation.
    pub fn ingest(&mut self, frame_bytes: &[u8]) -> u64 {
        let frame = match qr_protocol::Frame::from_bytes(frame_bytes) {
            Ok(f) => f,
            Err(e) => {
                android_log_wasm(&format!(
                    "frame rejected (len={}): {:?}",
                    frame_bytes.len(),
                    e
                ));
                return ingest_status::INGEST_ERROR;
            }
        };
        let prev_received = self.inner.progress().received_symbols;
        if let Err(e) = self.inner.ingest(frame) {
            android_log_wasm(&format!("ingest error: {e}"));
        }
        let p = self.inner.progress();
        let complete = self.inner.is_complete();
        let accepted = p.received_symbols > prev_received;
        ingest_status::pack(
            complete,
            accepted,
            p.session_mismatch_streak,
            p.received_symbols,
        )
    }

    /// Live recovery progress as JSON (same fields as the JNI/C ABI
    /// `progressJson`). The JS UI calls this on its refresh cadence instead of
    /// parsing a value per ingested frame.
    pub fn progress_json(&self) -> String {
        let p = self.inner.progress();
        format!(
            r#"{{"decoded_symbols":{},"total_symbols":{},"symbol_size":{},"received_symbols":{},"frames_seen":{},"frames_duplicate":{},"frames_corrupt":{},"decoded_blocks":{},"total_blocks":{},"decoded_fraction":{:.4},"loss_ratio":{:.4},"complete":{},"meta_confirmed":{},"session_mismatch_streak":{}}}"#,
            p.decoded_symbols,
            p.total_symbols,
            p.symbol_size,
            p.received_symbols,
            p.frames_seen,
            p.frames_duplicate,
            p.frames_corrupt,
            p.decoded_blocks,
            p.total_blocks,
            p.decoded_fraction(),
            p.loss_ratio(),
            self.inner.is_complete(),
            p.meta_confirmed,
            p.session_mismatch_streak
        )
    }

    /// 1 once the object is fully decoded, else 0.
    pub fn is_complete(&self) -> bool {
        self.inner.is_complete()
    }

    // ─── metadata getters (mirror JNI `receiverFileName`/... + C ABI) ───────

    /// Session id (low 64 bits).
    pub fn session_id_lo(&self) -> u64 {
        self.inner.session_id() as u64
    }
    /// Session id (high 64 bits).
    pub fn session_id_hi(&self) -> u64 {
        (self.inner.session_id() >> 64) as u64
    }

    /// Recovered file name (UTF-8). Empty until a descriptor has been accepted.
    pub fn file_name(&self) -> String {
        self.inner.file_meta().filename.clone()
    }
    /// Original (decompressed) file size in bytes. 0 until a descriptor arrives.
    pub fn original_size(&self) -> u64 {
        self.inner.file_meta().original_size
    }
    /// Transmitted (possibly compressed) payload length. 0 until known.
    pub fn compressed_size(&self) -> u64 {
        self.inner.file_meta().compressed_size
    }
    /// 1 if the descriptor supplied a real `compressed_size` (else 0). When 0,
    /// the receiver operates on the raw padded bytes.
    pub fn compressed_size_known(&self) -> bool {
        self.inner.file_meta().compressed_size_known
    }
    /// Compression-algorithm tag (0=None, 1=Zstd, 2=Xz). The JS layer uses this
    /// to pick the decompressor after [`assemble_raw`](Self::assemble_raw).
    pub fn compression(&self) -> u8 {
        self.inner.file_meta().compression
    }
    /// CRC32 of the original file (0 if unknown). Returned as `u32` — JS must
    /// read it unsigned (`>>> 0`) since values like `0xDEADBEEF` exceed the
    /// signed 32-bit range.
    pub fn crc32(&self) -> u32 {
        self.inner.file_meta().crc32
    }
    /// 1 if the descriptor supplied a real CRC32 (so the host should verify it
    /// against the recovered bytes), else 0. CRC32 can legitimately be 0, so
    /// `crc32() == 0` is not a safe "unknown" test.
    pub fn crc32_known(&self) -> bool {
        self.inner.file_meta().crc32_known
    }
    /// 1 once the authoritative OTI has been received via a descriptor frame.
    /// Before this, data frames are only buffered (not decoded).
    pub fn meta_confirmed(&self) -> bool {
        self.inner.is_meta_confirmed()
    }

    // ─── descriptor-v5 segment metadata (large-transfer child objects) ─────

    /// 1 if the confirmed descriptor was a v5 large-transfer child object.
    pub fn is_segmented(&self) -> bool {
        self.inner.segment_meta().is_some()
    }
    /// Zero-based index of this segment within the root transfer, or 0 if not
    /// segmented.
    pub fn segment_index(&self) -> u32 {
        self.inner
            .segment_meta()
            .map(|s| s.segment_index)
            .unwrap_or(0)
    }
    /// Total segment count of the root transfer, or 1 if not segmented.
    pub fn segment_count(&self) -> u32 {
        self.inner
            .segment_meta()
            .map(|s| s.segment_count)
            .unwrap_or(1)
    }
    /// Root (whole-file) original size in bytes, or 0 if not segmented.
    pub fn root_original_size(&self) -> u64 {
        self.inner
            .segment_meta()
            .map(|s| s.root_original_size)
            .unwrap_or(0)
    }
    /// Original (uncompressed) offset of this segment in the root file, or 0.
    pub fn original_offset(&self) -> u64 {
        self.inner
            .segment_meta()
            .map(|s| s.original_offset)
            .unwrap_or(0)
    }
    /// Root session id low 64 bits (whole transfer id), or 0 if not segmented.
    pub fn root_session_id_lo(&self) -> u64 {
        self.inner
            .segment_meta()
            .map(|s| s.root_session_id as u64)
            .unwrap_or(0)
    }
    /// Root session id high 64 bits, or 0 if not segmented.
    pub fn root_session_id_hi(&self) -> u64 {
        self.inner
            .segment_meta()
            .map(|s| (s.root_session_id >> 64) as u64)
            .unwrap_or(0)
    }
    /// SHA-256 of this segment's **compressed** bytes, or an empty vector for a
    /// legacy non-segmented descriptor.
    pub fn raw_sha256(&self) -> Vec<u8> {
        self.inner
            .segment_meta()
            .map(|s| s.raw_sha256.to_vec())
            .unwrap_or_default()
    }
    /// SHA-256 of the complete uncompressed root file, or an empty vector for
    /// a legacy non-segmented descriptor.
    pub fn root_sha256(&self) -> Vec<u8> {
        self.inner
            .segment_meta()
            .map(|s| s.root_sha256.to_vec())
            .unwrap_or_default()
    }

    /// Reassemble the RaptorQ object bytes exactly as transmitted (trimmed to
    /// `compressed_size` when known), **without** applying decompression.
    ///
    /// Returns an empty `Vec` if decoding is incomplete. For
    /// `compression == COMPRESSION_NONE` the bytes are the original file; for
    /// Zstd/Xz the JS layer decompresses with its own WASM and verifies the
    /// CRC32 against [`crc32`](Self::crc32) (when [`crc32_known`](Self::crc32_known)).
    pub fn assemble_raw(&self) -> Vec<u8> {
        // `assemble_raw` on the inner session returns the padded transmitted
        // bytes; trim to compressed_size to match the contract the JS layer
        // expects (the decompressor must not see symbol-padding zeros).
        let mut raw = match self.inner.assemble_raw() {
            Some(b) => b,
            None => return Vec::new(),
        };
        let fm = self.inner.file_meta();
        if fm.compressed_size_known {
            if let Ok(len) = usize::try_from(fm.compressed_size) {
                if len <= raw.len() {
                    raw.truncate(len);
                }
            }
        }
        raw
    }
}
