/**
 * WASM receiver benchmark — measures the pure Rust-core throughput of the
 * M1 web-receiver bindings (SenderSessionWasm → ReceiverSessionWasm) in Node,
 * isolated from camera / ZXing / DOM overhead.
 *
 * Reports:
 *   - sender next_frame throughput (frames/s, MiB/s wire)
 *   - receiver ingest throughput (frames/s, symbols/s, MiB/s payload)
 *   - assemble_raw throughput (MiB/s) + correctness check
 *   - end-to-end (generate + ingest + assemble) wall time for a few sizes
 *
 * Run: node core/transfer-engine/scripts/bench_receiver.mjs
 */
import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { dirname, join } from "node:path"
import init, {
  SenderSessionWasm,
  ReceiverSessionWasm,
} from "../../../apps/sender/wasm-pkg-simd/transfer_engine.js"

const __dirname = dirname(fileURLToPath(import.meta.url))
const wasmPath = join(
  __dirname,
  "..",
  "..",
  "..",
  "apps",
  "sender",
  "wasm-pkg-simd",
  "transfer_engine_bg.wasm"
)

const SYMBOL_SIZE = 1024 // core default
const REDUNDANCY = 30
const SID_LO = 0x1122334455667788n
const SID_HI = 0x99aabbccddeeff00n

function makePayload(n) {
  const a = new Uint8Array(n)
  for (let i = 0; i < n; i++) a[i] = (i * 2654435761) & 0xff
  return a
}

function msSince(t0) {
  return Number(process.hrtime.bigint() - t0) / 1e6
}

/** Drive one end-to-end cycle for `size` bytes; return timings + check. */
async function cycle(size) {
  const payload = makePayload(size)
  const paddedLenProbe = new SenderSessionWasm(
    payload,
    SID_LO,
    SID_HI,
    REDUNDANCY,
    SYMBOL_SIZE,
    "bench.bin",
    BigInt(size),
    0xdeadbeef
  )
  // probe transfer_length (padded) so compressed_size is exact
  const totalK = paddedLenProbe.total_symbols()
  // The sender doesn't expose transfer_length directly; rebuild with a
  // compressed_size guess then rely on receiver trimming via assemble_raw.
  paddedLenProbe.free?.()

  // Generate frames with a fresh sender.
  const sender = new SenderSessionWasm(
    payload,
    SID_LO,
    SID_HI,
    REDUNDANCY,
    SYMBOL_SIZE,
    "bench.bin",
    BigInt(size),
    0xdeadbeef
  )

  const totalK2 = sender.total_symbols()
  const frameCount = totalK2 * 2 + 40

  // ── sender throughput ──
  const tGen0 = process.hrtime.bigint()
  const frames = new Array(frameCount)
  for (let i = 0; i < frameCount; i++) frames[i] = sender.next_frame()
  const genMs = msSince(tGen0)

  let wireBytes = 0
  for (const f of frames) wireBytes += f.length

  // ── receiver ingest ──
  // Bootstrap from the first descriptor observed.
  let rx = null
  let descriptorIdx = -1
  for (let i = 0; i < frames.length; i++) {
    // descriptor flag is bit0 of byte 3 (header byte 3), big-endian header.
    if (frames[i][3] & 0x01) {
      descriptorIdx = i
      break
    }
  }
  if (descriptorIdx < 0) throw new Error("no descriptor frame produced")

  const tIngest0 = process.hrtime.bigint()
  rx = ReceiverSessionWasm.from_descriptor(frames[descriptorIdx])
  let ingested = 1
  let complete = false
  for (let i = 0; i < frames.length; i++) {
    if (i === descriptorIdx) continue
    const status = rx.ingest(frames[i])
    ingested++
    // bit0 = complete
    if (status & 0x1n) {
      complete = true
      break
    }
  }
  const ingestMs = msSince(tIngest0)

  if (!complete) throw new Error(`receiver did not complete for size ${size}`)

  // ── assemble_raw ──
  const tAsm0 = process.hrtime.bigint()
  const recovered = rx.assemble_raw()
  const asmMs = msSince(tAsm0)

  // correctness: first `size` bytes must equal payload (padding trimmed by
  // assemble_raw to compressed_size when known; but the descriptor was built
  // with compressed_size = padded guess, so trim to original for compare).
  const n = Math.min(recovered.length, size)
  let ok = true
  for (let i = 0; i < n; i++) {
    if (recovered[i] !== payload[i]) {
      ok = false
      break
    }
  }

  const payloadMiB = size / (1024 * 1024)
  const wireMiB = wireBytes / (1024 * 1024)
  // unique source symbols that were accepted = totalK (at completion received_symbols ≈ totalK)
  return {
    size,
    totalK: totalK2,
    frameCount,
    genMs,
    ingestMs,
    asmMs,
    wireMiB,
    payloadMiB,
    framesPerSecGen: frameCount / (genMs / 1000),
    framesPerSecIngest: ingested / (ingestMs / 1000),
    symbolsPerSec: totalK2 / (ingestMs / 1000),
    payloadMiBPerSecIngest: payloadMiB / (ingestMs / 1000),
    assembleMiBPerSec: payloadMiB / (asmMs / 1000),
    recoveredLen: recovered.length,
    correct: ok,
  }
}

function fmt(n, digits = 1) {
  if (!isFinite(n)) return "n/a"
  return n.toFixed(digits)
}

async function main() {
  const wasm = readFileSync(wasmPath)
  await init(await WebAssembly.compile(wasm))

  console.log("== AirFerry WASM receiver benchmark (Node, pure Rust core) ==")
  console.log(`symbol_size=${SYMBOL_SIZE}B  redundancy=${REDUNDANCY}%`)
  console.log(
    "(isolates SenderSessionWasm + ReceiverSessionWasm; no camera/ZXing/DOM)\n"
  )

  const sizes = [50_000, 500_000, 2_000_000, 8_000_000]
  const rows = []
  for (const s of sizes) {
    // warm-up run (first run pays WASM tier-up)
    await cycle(s)
    const r = await cycle(s)
    rows.push(r)
    console.log(
      `size=${String(s).padStart(8)}B  K=${String(r.totalK).padStart(5)}  ` +
        `gen=${fmt(r.genMs)}ms  ingest=${fmt(r.ingestMs)}ms  asm=${fmt(
          r.asmMs
        )}ms  ` +
        `ingest=${fmt(r.framesPerSecIngest, 0)} fps / ${fmt(
          r.symbolsPerSec,
          0
        )} sym/s / ${fmt(r.payloadMiBPerSecIngest, 1)} MiB/s  ` +
        `asm=${fmt(r.assembleMiBPerSec, 1)} MiB/s  ` +
        `recover=${r.recoveredLen}B  correct=${r.correct}`
    )
  }

  console.log("\n== summary (median-ish of 2nd run per size) ==")
  console.log(
    "receiver ingest throughput (payload MiB/s, excludes camera + ZXing):"
  )
  for (const r of rows) {
    console.log(
      `  ${String(r.size).padStart(8)}B: ${fmt(
        r.payloadMiBPerSecIngest,
        1
      )} MiB/s  (${fmt(r.symbolsPerSec, 0)} symbols/s)`
    )
  }
  console.log(
    "\nNote: this is the Rust-core ceiling. Real web-receiver throughput is\n" +
      "bounded by camera FPS, VideoFrame→Y-plane copy, and ZXing decode —\n" +
      "measured separately once M2/M3 land."
  )
}

main().catch((e) => {
  console.error("FAILED:", e)
  process.exit(1)
})
