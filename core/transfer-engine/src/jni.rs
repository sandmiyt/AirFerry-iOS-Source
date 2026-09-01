//! Android JNI bindings for the receiver side.
//!
//! Uses the official `jni` crate (correct JNIEnv ABI across all Android
//! versions / vendors / ART implementations) with `extern "system"` — the
//! correct calling convention for JNI native methods on 64-bit Android.
//!
//! ## Handle model
//! A receiver session is heap-allocated (`Box<ReceiverSession>`), and its raw
//! pointer is returned to Kotlin as an opaque `jlong` handle. Every function
//! takes that handle back as an argument; pass it to
//! [`Java_com_airferry_app_nativelib_NativeBridge_receiverDestroy`] to release
//! the session. The handle is *not* thread-safe — the host must serialize all
//! calls that touch the same handle (the Android client does this with
//! `QrDecodePool`'s `ingestLock`, exactly like the Windows client's single
//! ingest lock around the C ABI; see [`crate::cffi`] for the mirrored
//! contract).

#![cfg(feature = "jni")]

use crate::ingest_status;
use crate::receiver::ReceiverSession;
use crate::Progress;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jboolean, jint, jlong, jsize};
use jni::JNIEnv;
use qr_protocol::frame::SessionIdRaw;
use raptorq_core::MAX_ORIGINAL_BYTES;

/// ABI / protocol capability version of this JNI library.
///
/// The outer QR frame format stays at protocol version 1 (`SessionId::derive_segment`
/// demultiplexes large-file segments by session id); this counter is a
/// *separate* Android-side capability marker that advances whenever the native
/// library gains behaviour the Kotlin host depends on. It is bumped once (to
/// `1`) for the descriptor-v5 segmented (large-file) receive path.
///
/// The host (`NativeBridge.nativeAbiVersion`) handshakes on startup: if the
/// loaded `.so` predates this symbol (`UnsatisfiedLinkError`) or reports a
/// lower version, the app refuses to run as a receiver instead of silently
/// "staying synchronising" on >32 MiB transfers with a stale library.
pub const AIRFERRY_NATIVE_ABI_VERSION: jint = 1;

/// Report the native ABI / protocol capability version. Returns
/// [`AIRFERRY_NATIVE_ABI_VERSION`].
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_nativeAbiVersion(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    AIRFERRY_NATIVE_ABI_VERSION
}

#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverCreate(
    _env: JNIEnv,
    _class: JClass,
    session_id_lo: jlong,
    session_id_hi: jlong,
    _total_blocks: jint,
    _total_symbols: jint,
    _symbol_size: jint,
) -> jlong {
    let sid: SessionIdRaw = ((session_id_hi as u64 as u128) << 64) | (session_id_lo as u64 as u128);
    // Cache-only bootstrap: do NOT build a decoder from these caller-supplied
    // totals (a guessed early layout, and `derive_meta_from_totals`'s OTI build
    // can itself assert on large values). Data frames are buffered until the
    // first *validated* descriptor frame supplies the authoritative, sanity-
    // checked OTI (see ReceiverSession::ingest), which builds the real decoder.
    let session = ReceiverSession::new_pending(sid);
    Box::into_raw(Box::new(session)) as jlong
}

#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverDestroy(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle != 0 {
        unsafe { drop(Box::from_raw(handle as *mut ReceiverSession)) };
    }
}

/// Ingest a frame.
///
/// Returns a packed `jlong` status word instead of a per-frame JSON byte[].
/// Building + crossing the JNI boundary with a JSON string on *every* decoded
/// frame (the UI only refreshes ~7 Hz) is pure waste: it allocates a Rust
/// `String`, a Java `byte[]`, a Kotlin `String`, and a `JSONObject` parse per
/// frame at 60 fps. The packed word carries just what the ingest path needs to
/// decide completion + re-init, and the full progress is fetched on demand via
/// [`receiverProgressJson`] at the UI's throttle cadence.
///
/// Bit layout of the returned `jlong` (all fields unsigned):
///   - bit  0      : `complete` (1 once the object is fully decoded)
///   - bit  1      : `accepted` (1 if this frame contributed a new symbol)
///   - bits 8..23  : `session_mismatch_streak` (0..=0xFFFF)
///   - bits 32..63 : `received_symbols` (low 32 bits; capped well below 2^32)
///
/// Returns 0 only on a null handle. A byte-array conversion failure or a
/// frame that fails wire validation (bad magic / CRC / version) returns the
/// [`ingest_status::INGEST_ERROR`] sentinel (`received_symbols == u32::MAX`,
/// all flags clear) — the host treats it as "frame rejected, nothing to do".
/// A session-level `ingest` error (e.g. `SessionMismatch`) is logged but the
/// function still returns the *current* packed status word, since the session
/// stays alive and its progress remains readable.
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverIngest(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
    frame_bytes: JByteArray,
) -> jlong {
    if handle == 0 {
        return 0;
    }
    let frame_vec: Vec<u8> = match env.convert_byte_array(&frame_bytes) {
        Ok(v) => v,
        Err(_) => return ingest_status::INGEST_ERROR as jlong,
    };
    let session = unsafe { &mut *(handle as *mut ReceiverSession) };
    let frame = match qr_protocol::Frame::from_bytes(&frame_vec) {
        Ok(f) => f,
        Err(e) => {
            android_log(&format!(
                "frame rejected (len={}): {:?}",
                frame_vec.len(),
                e
            ));
            return ingest_status::INGEST_ERROR as jlong;
        }
    };
    let is_descriptor = frame.header.flags & qr_protocol::frame::FLAG_DESCRIPTOR != 0;
    let prev_received = session.progress().received_symbols;
    match session.ingest(frame) {
        Ok(_) => {}
        Err(e) => {
            // A SessionMismatch on a data frame would silently drop every
            // symbol — surface it so the cause is visible.
            android_log(&format!("ingest error: {e}"));
        }
    }
    let p = session.progress();
    // Log the first few frames + any descriptor + when received is suspiciously
    // stuck at 0 while frames are flowing. Throttled by frame_index to avoid
    // flooding logcat.
    if p.frames_seen <= 3 || is_descriptor || (p.frames_seen % 50 == 0 && !session.is_complete()) {
        android_log(&format!(
            "f={} desc={} meta={} recv={} dec={} {}/{} mismatch={}",
            p.frames_seen,
            is_descriptor,
            p.meta_confirmed,
            p.received_symbols,
            p.decoded_blocks,
            p.decoded_symbols,
            p.total_symbols,
            p.session_mismatch_streak
        ));
    }
    let complete = if session.is_complete() { 1 } else { 0 };
    // A frame "contributed" if received_symbols advanced, OR it was a descriptor
    // that confirmed meta (so re-init state on the Kotlin side updates even when
    // no new symbol arrived on that descriptor tick).
    let accepted = if p.received_symbols > prev_received {
        1
    } else {
        0
    };
    ingest_status::pack(
        complete == 1,
        accepted == 1,
        p.session_mismatch_streak,
        p.received_symbols,
    ) as jlong
}

/// On-demand progress query (JSON). The UI calls this on its ~7 Hz refresh
/// cadence instead of parsing a JSON on every ingested frame. Returns a freshly
/// allocated `byte[]` of the NUL-terminated JSON, or an empty array on error.
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverProgressJson(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jni::sys::jbyteArray {
    if handle == 0 {
        return null_byte_array(&mut env);
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    let json = progress_json(&session.progress());
    let mut buf = json.into_bytes();
    buf.push(0); // NUL terminator for C-string reads on the Kotlin side.
    fill_array(&mut env, &buf)
}

/// Allocate a fresh byte[] of `len` bytes and fill it from `buf`. Returns null
/// on allocation failure. Inlined (not a closure) so it does not capture a
/// borrow of `env` and conflict with later uses of `env`.
fn fill_array(env: &mut JNIEnv, buf: &[u8]) -> jni::sys::jbyteArray {
    let len = buf.len() as jsize;
    let arr = match env.new_byte_array(len) {
        Ok(a) => a,
        Err(_) => return std::ptr::null_mut(),
    };
    // SAFETY: u8 and i8 have the same layout; the slice is a valid
    // reinterpretation for the JNI SetByteArrayRegion call.
    let i8_buf: &[i8] = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const i8, buf.len()) };
    if env.set_byte_array_region(&arr, 0, i8_buf).is_ok() {
        arr.into_raw()
    } else {
        std::ptr::null_mut()
    }
}

#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverIsComplete(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jint {
    if handle == 0 {
        return 0;
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    session.is_complete() as jint
}

/// Recover the assembled file as a freshly-allocated `byte[]`.
///
/// Returns the bytes directly (null if not complete / on error), instead of the
/// old two-call `receiverAssembledLength` (jint) + `receiverAssemble(into buf)`
/// pattern. That pattern had two problems this fixes:
///  1. `receiverAssembledLength` returned `jint`, so files > 2 GB truncated the
///     length and `ByteArray(len)` then threw on a negative size.
///  2. The length and the fill were two separate JNI calls with no locking, so a
///     concurrent mutation could make the second call's length differ from the
///     first's. Returning a new array is a single atomic call.
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverAssembleBytes(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jni::sys::jbyteArray {
    if handle == 0 {
        return null_byte_array(&mut env);
    }
    let session = unsafe { &mut *(handle as *mut ReceiverSession) };
    let data = match session.assemble_result() {
        Ok(Some(d)) => d,
        Ok(None) => return null_byte_array(&mut env),
        Err(e) => {
            android_log(&format!("assemble failed: {e}"));
            return null_byte_array(&mut env);
        }
    };
    // Allocate a fresh byte[] of exactly data.len() and copy. jsize is i32, so a
    // Vec longer than i32::MAX (2 GiB) cannot be represented as a Java array in
    // one piece anyway — log and return null rather than truncating silently.
    let len = match jsize::try_from(data.len()) {
        Ok(n) => n,
        Err(_) => {
            android_log(&format!(
                "assemble result {} bytes exceeds Java array max (2 GiB)",
                data.len()
            ));
            return null_byte_array(&mut env);
        }
    };
    let arr = match env.new_byte_array(len) {
        Ok(a) => a,
        Err(_) => return null_byte_array(&mut env),
    };
    // SAFETY: u8 and i8 have the same layout; the slice is a valid
    // reinterpretation for SetByteArrayRegion.
    let i8_buf: &[i8] =
        unsafe { std::slice::from_raw_parts(data.as_ptr() as *const i8, data.len()) };
    if env.set_byte_array_region(&arr, 0, i8_buf).is_ok() {
        arr.into_raw()
    } else {
        null_byte_array(&mut env)
    }
}

/// Reassemble the RaptorQ object bytes **exactly as transmitted** (trimmed to
/// `compressed_size` when known), **without** applying decompression.
///
/// For descriptor-v5 segmented transfers the compressed-stream model stores each
/// segment's **compressed** bytes and concatenates + decompresses once at the
/// end, so Kotlin calls this instead of `receiverAssembleBytes` (which
/// decompresses per segment). Returns an empty byte[] if decoding is incomplete.
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverAssembleRawBytes(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jni::sys::jbyteArray {
    if handle == 0 {
        return null_byte_array(&mut env);
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    let Some(mut raw) = session.assemble_raw() else {
        return null_byte_array(&mut env);
    };
    let fm = session.file_meta();
    if fm.compressed_size_known {
        if let Ok(len) = usize::try_from(fm.compressed_size) {
            if len <= raw.len() {
                raw.truncate(len);
            }
        }
    }
    let len = match jsize::try_from(raw.len()) {
        Ok(n) => n,
        Err(_) => return null_byte_array(&mut env),
    };
    let arr = match env.new_byte_array(len) {
        Ok(a) => a,
        Err(_) => return null_byte_array(&mut env),
    };
    let i8_buf: &[i8] = unsafe { std::slice::from_raw_parts(raw.as_ptr() as *const i8, raw.len()) };
    if env.set_byte_array_region(&arr, 0, i8_buf).is_ok() {
        arr.into_raw()
    } else {
        null_byte_array(&mut env)
    }
}

/// Compression-algorithm tag of the confirmed descriptor (0=None, 1=Zstd,
/// 2=Xz). For segmented transfers this is the algorithm of the whole stream,
/// shared by every segment.
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverCompression(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jint {
    if handle == 0 {
        return 0;
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    i32::from(session.file_meta().compression)
}

/// Transmitted (possibly compressed) payload length of this object. For a
/// segmented transfer this is the segment's compressed size.
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverCompressedSize(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    if handle == 0 {
        return 0;
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    session.file_meta().compressed_size as jlong
}

/// Whole **decompressed** original size of the transfer (same across every
/// segment of a segmented root).
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverOriginalSize(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    if handle == 0 {
        return 0;
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    session.file_meta().original_size as jlong
}

/// Decompress a byte array according to a compression tag (0=None, 1=Zstd,
/// 2=Xz), bounded by `max_output`. Used by Kotlin to decompress the
/// concatenated compressed stream of a segmented transfer exactly once.
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_decompressBytes(
    mut env: JNIEnv,
    _class: JClass,
    data: jni::sys::jbyteArray,
    compression: jint,
    max_output: jlong,
) -> jni::sys::jbyteArray {
    if data.is_null() {
        return null_byte_array(&mut env);
    }
    // SAFETY: `data` is a non-null `jbyteArray` owned by the JVM for the call
    // duration (JNI local reference). Wrap it so the typed `jni` crate methods
    // accept it.
    let arr = unsafe { JByteArray::from_raw(data) };
    let len = match env.get_array_length(&arr) {
        Ok(n) => n,
        Err(_) => return null_byte_array(&mut env),
    };
    if len < 0 {
        return null_byte_array(&mut env);
    }
    // `jni` expects `&mut [i8]`; `u8`/`i8` share the same in-memory layout.
    let mut buf = vec![0u8; len as usize];
    let buf_i8: &mut [i8] =
        unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut i8, buf.len()) };
    if env.get_byte_array_region(&arr, 0, buf_i8).is_err() {
        return null_byte_array(&mut env);
    }
    let max_out = if max_output > 0 {
        // Clamp the host-supplied cap to MAX_ORIGINAL_BYTES so a careless or
        // hostile caller (e.g. forwarding a descriptor's whole-file size, which
        // is unbounded for segmented transfers) cannot disable
        // decompress_with_limit's bomb bound by passing usize::MAX-equivalent.
        // `max_output` is `jlong` (i64); we've checked `> 0` so the cast to u64
        // is safe, and only then can we `min` against the u64 ceiling.
        let cap = (max_output as u64).min(MAX_ORIGINAL_BYTES);
        cap as usize
    } else {
        0
    };
    let out = match qr_protocol::compress::decompress_with_limit(&buf, compression as u8, max_out) {
        Ok(bytes) => bytes,
        Err(e) => {
            android_log(&format!("decompressBytes failed: {e}"));
            return null_byte_array(&mut env);
        }
    };
    let out_len = match jsize::try_from(out.len()) {
        Ok(n) => n,
        Err(_) => return null_byte_array(&mut env),
    };
    let arr = match env.new_byte_array(out_len) {
        Ok(a) => a,
        Err(_) => return null_byte_array(&mut env),
    };
    let i8_buf: &[i8] = unsafe { std::slice::from_raw_parts(out.as_ptr() as *const i8, out.len()) };
    if env.set_byte_array_region(&arr, 0, i8_buf).is_ok() {
        arr.into_raw()
    } else {
        null_byte_array(&mut env)
    }
}

/// Allocate an empty (0-length) byte[] — the "nothing to return" sentinel.
fn null_byte_array(env: &mut JNIEnv) -> jni::sys::jbyteArray {
    match env.new_byte_array(0) {
        Ok(a) => a.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

// ===== File metadata accessors =====
// Kotlin reads these after a descriptor frame arrives to display the filename,
// original size, and verify CRC32.

/// Returns the original filename as a Java String (or empty if unknown).
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverFileName(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jni::sys::jstring {
    if handle == 0 {
        return std::ptr::null_mut();
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    let name = session.file_meta().filename.clone();
    match env.new_string(&name) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Returns the original file size (0 if unknown).
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverFileSize(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    if handle == 0 {
        return 0;
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    session.file_meta().original_size as jlong
}

/// Returns the CRC32 of the original file (0 if unknown).
///
/// Returned as `jlong` (not `jint`) so the full unsigned 32-bit range
/// (0..=0xFFFF_FFFF) survives intact — Kotlin's `Int` is signed, so a value
/// like `0xDEADBEEF` would otherwise arrive as a negative number and break
/// equality comparisons with a receiver-computed CRC.
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverCrc32(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    if handle == 0 {
        return 0;
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    session.file_meta().crc32 as u64 as jlong
}

/// Returns 1 if the descriptor supplied a real CRC32 (so the receiver should
/// verify it), or 0 if the CRC is unknown (v1 descriptor / not yet received)
/// and must NOT be compared against the recovered bytes.
///
/// This exists because CRC32 can legitimately be 0 (~1 in 2^32 files): the old
/// `expectedCrc == 0L` sentinel on the Kotlin side mislabelled such files as
/// "unverified". `crc32_known` is the authoritative "is there a value" flag.
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverCrc32Known(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jint {
    if handle == 0 {
        return 0;
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    session.file_meta().crc32_known as jint
}

// ===== descriptor-v5 segment metadata accessors =====
// Kotlin reads these after a descriptor frame arrives to detect a large-transfer
// child object and to drive the per-segment `.partial` writer. All mirror the
// WASM `ReceiverSessionWasm` getters (wasm.rs) and read `session.segment_meta()`.

/// 1 if the confirmed descriptor was a v5 large-transfer child object, else 0.
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverIsSegmented(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jint {
    if handle == 0 {
        return 0;
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    session.segment_meta().is_some() as jint
}

/// Zero-based index of this segment within the root transfer (0 if not segmented).
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverSegmentIndex(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jint {
    if handle == 0 {
        return 0;
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    session
        .segment_meta()
        .map(|s| s.segment_index as jint)
        .unwrap_or(0)
}

/// Total segment count of the root transfer (1 if not segmented).
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverSegmentCount(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jint {
    if handle == 0 {
        return 0;
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    session
        .segment_meta()
        .map(|s| s.segment_count as jint)
        .unwrap_or(1)
}

/// Root (whole-file) original size in bytes (0 if not segmented).
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverRootOriginalSize(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    if handle == 0 {
        return 0;
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    session
        .segment_meta()
        .map(|s| s.root_original_size as jlong)
        .unwrap_or(0)
}

/// Original (uncompressed) offset of this segment in the root file (0 if not segmented).
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverOriginalOffset(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    if handle == 0 {
        return 0;
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    session
        .segment_meta()
        .map(|s| s.original_offset as jlong)
        .unwrap_or(0)
}

/// Root session id low 64 bits (whole transfer id), or 0 if not segmented.
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverRootSessionIdLo(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    if handle == 0 {
        return 0;
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    session
        .segment_meta()
        .map(|s| (s.root_session_id as u64) as jlong)
        .unwrap_or(0)
}

/// Root session id high 64 bits, or 0 if not segmented.
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverRootSessionIdHi(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    if handle == 0 {
        return 0;
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    session
        .segment_meta()
        .map(|s| (s.root_session_id >> 64) as jlong)
        .unwrap_or(0)
}

/// SHA-256 (raw 32 bytes) of this segment's uncompressed bytes, or empty if not
/// segmented.
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverRawSha256(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jni::sys::jbyteArray {
    if handle == 0 {
        return std::ptr::null_mut();
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    match session.segment_meta() {
        Some(s) => match env.byte_array_from_slice(&s.raw_sha256) {
            Ok(arr) => arr.into_raw(),
            Err(_) => std::ptr::null_mut(),
        },
        None => std::ptr::null_mut(),
    }
}

/// SHA-256 of the complete uncompressed root file, or null for a legacy
/// non-segmented descriptor.
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverRootSha256(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jni::sys::jbyteArray {
    if handle == 0 {
        return std::ptr::null_mut();
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    match session.segment_meta() {
        Some(s) => match env.byte_array_from_slice(&s.root_sha256) {
            Ok(arr) => arr.into_raw(),
            Err(_) => std::ptr::null_mut(),
        },
        None => std::ptr::null_mut(),
    }
}

/// Last [`ReceiverSession::assemble_result`] error message, or empty if none.
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_receiverLastAssembleError(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jni::sys::jstring {
    if handle == 0 {
        return std::ptr::null_mut();
    }
    let session = unsafe { &*(handle as *const ReceiverSession) };
    let msg = session.last_assemble_error().unwrap_or("");
    match env.new_string(msg) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Stream a concatenated compressed stream from `input_path` to `output_path`,
/// decompressing as it goes (zstd/xz streaming decoder) while computing CRC32 +
/// SHA-256 incrementally — neither input nor output is held wholly in memory, so
/// very large files can be recovered in bounded RAM. Verifies the decompressed
/// size, CRC32 (when known) and SHA-256 against the descriptor before returning
/// success; any mismatch or I/O error removes the partial output and returns
/// false.
///
/// `max_output` caps the decompressed size (decompression-bomb guard).
#[no_mangle]
pub extern "system" fn Java_com_airferry_app_nativelib_NativeBridge_decompressStreamToFile(
    env: JNIEnv,
    _class: JClass,
    input_path: JString,
    output_path: JString,
    compression: jint,
    max_output: jlong,
    expected_size: jlong,
    expected_crc: jlong,
    crc_known: jboolean,
    expected_sha_hex: JString,
) -> jboolean {
    fn jstr(env: &mut JNIEnv, s: JString) -> Option<String> {
        env.get_string(&s).ok().map(|j| j.into())
    }
    let mut env = env;
    let input = match jstr(&mut env, input_path) {
        Some(v) => v,
        None => {
            android_log("decompressStreamToFile: missing input path");
            return 0;
        }
    };
    let output = match jstr(&mut env, output_path) {
        Some(v) => v,
        None => {
            android_log("decompressStreamToFile: missing output path");
            return 0;
        }
    };
    let expected_sha = match jstr(&mut env, expected_sha_hex) {
        Some(v) => v,
        None => {
            android_log("decompressStreamToFile: missing expected sha");
            return 0;
        }
    };

    // Streamed decompression writes to disk in bounded RAM, so unlike the
    // in-memory `decompressBytes` path it is NOT capped at MAX_ORIGINAL_BYTES
    // (256 MiB). For a descriptor-v5 segmented transfer `decompressedSize` is
    // the whole-file original size, which legitimately exceeds 256 MiB. The
    // host (SegmentAssembler) has already validated it as a positive Long and
    // checked the disk has room for the compressed stream; using that exact
    // value as the streaming cap still defends against a decompression bomb
    // (the stream is rejected as soon as it would exceed the declared output).
    // The downstream size + CRC + SHA checks enforce correctness regardless.
    let max_out = if max_output > 0 {
        // `max_output` is a positive jlong (i64), so the cast to u64 is safe.
        max_output as u64
    } else {
        0
    };
    let outcome = match qr_protocol::compress::decompress_stream_to_file(
        &input,
        &output,
        compression as u8,
        max_out,
    ) {
        Ok(o) => o,
        Err(e) => {
            android_log(&format!("decompressStreamToFile failed: {e}"));
            return 0;
        }
    };

    if outcome.output_size != expected_size as u64 {
        android_log(&format!(
            "decompressStreamToFile size mismatch: {} != {}",
            outcome.output_size, expected_size
        ));
        let _ = std::fs::remove_file(&output);
        return 0;
    }
    if crc_known != 0 && outcome.crc32 != expected_crc as u32 {
        android_log(&format!(
            "decompressStreamToFile crc mismatch: {:08x} != {:08x}",
            outcome.crc32, expected_crc as u32
        ));
        let _ = std::fs::remove_file(&output);
        return 0;
    }
    let actual_sha: String = outcome.sha256.iter().map(|b| format!("{b:02x}")).collect();
    if !actual_sha.eq_ignore_ascii_case(&expected_sha) {
        android_log("decompressStreamToFile sha mismatch");
        let _ = std::fs::remove_file(&output);
        return 0;
    }
    1
}

fn progress_json(p: &Progress) -> String {
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
        p.is_complete(),
        p.meta_confirmed,
        p.session_mismatch_streak
    )
}

#[cfg(target_os = "android")]
fn android_log(msg: &str) {
    extern "C" {
        fn __android_log_write(prio: i32, tag: *const u8, text: *const u8) -> i32;
    }
    const ANDROID_LOG_ERROR: i32 = 6;
    static TAG: &[u8] = b"airferry\0";
    let mut buf: Vec<u8> = Vec::with_capacity(msg.len() + 1);
    buf.extend_from_slice(msg.as_bytes());
    buf.push(0);
    unsafe {
        __android_log_write(ANDROID_LOG_ERROR, TAG.as_ptr(), buf.as_ptr());
    }
}

#[cfg(not(target_os = "android"))]
fn android_log(msg: &str) {
    eprintln!("[airferry] {msg}");
}
