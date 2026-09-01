/**
 * Node end-to-end test of the web receiver pipeline (minus camera + ZXing):
 *
 *   SenderSessionWasm (generate frames)
 *     → ReceiverSessionWasm.from_descriptor / ingest
 *       → assemble_raw
 *         → JS-side decompress (COMPRESSION_NONE path here; zstd/xz covered by
 *           the dedicated compress round-trip in compress.worker)
 *           → parseRecovered (text / bundle / single file)
 *
 * Verifies the three result kinds a receiver can produce:
 *  1. single-file (COMPRESSION_NONE) → file
 *  2. ETTEXTv1 text payload → text
 *  3. ETBUNDL1 multi-file → bundle
 *
 * Run: node core/transfer-engine/scripts/e2e_receiver.mjs
 */
import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { dirname, join } from "node:path"

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

// Magic constants (mirror apps/sender/src/wasm/{text,bundle}.ts).
const TEXT_MAGIC = "ETTEXTv1"
const BUNDLE_MAGIC = "ETBUNDL1"

function textBytes(s) {
  const enc = new TextEncoder()
  return enc.encode(s)
}
function u16be(v) {
  return [(v >>> 8) & 0xff, v & 0xff]
}
function u64be(v) {
  const big = BigInt(v)
  const out = []
  for (let i = 7; i >= 0; i--) out.push(Number((big >> BigInt(i * 8)) & 0xffn))
  return out
}
function buildTextPayload(text) {
  const body = textBytes(text)
  const out = new Uint8Array(8 + body.length)
  for (let i = 0; i < 8; i++) out[i] = TEXT_MAGIC.charCodeAt(i)
  out.set(body, 8)
  return out
}
function buildBundle(files) {
  // files: [{name, data:Uint8Array}]
  const nameBytes = files.map((f) => textBytes(f.name))
  let total = 8 + 2 + 2
  for (let i = 0; i < files.length; i++) total += 2 + nameBytes[i].length + 8 + files[i].data.length
  const out = new Uint8Array(total)
  const dv = new DataView(out.buffer)
  let o = 0
  for (let i = 0; i < 8; i++) out[o++] = BUNDLE_MAGIC.charCodeAt(i)
  dv.setUint16(o, 1); o += 2
  dv.setUint16(o, files.length); o += 2
  for (let i = 0; i < files.length; i++) {
    dv.setUint16(o, nameBytes[i].length); o += 2
    out.set(nameBytes[i], o); o += nameBytes[i].length
    for (const b of u64be(files[i].data.length)) out[o++] = b
    out.set(files[i].data, o); o += files[i].data.length
  }
  return out
}

// parseRecovered mirror (Node has no @/ alias).
function isTextPayload(b) {
  if (b.length < 8) return false
  for (let i = 0; i < 8; i++) if (b[i] !== TEXT_MAGIC.charCodeAt(i)) return false
  return true
}
function isBundle(b) {
  if (b.length < 12) return false
  for (let i = 0; i < 8; i++) if (b[i] !== BUNDLE_MAGIC.charCodeAt(i)) return false
  return true
}
function parseRecovered(bytes, descriptorName) {
  if (isTextPayload(bytes)) {
    const body = bytes.subarray(8)
    const text = new TextDecoder("utf-8", { fatal: false }).decode(body)
    return { kind: "text", text }
  }
  if (isBundle(bytes)) {
    let o = 8
    const version = ((bytes[o] << 8) | bytes[o + 1]) >>> 0; o += 2
    const count = ((bytes[o] << 8) | bytes[o + 1]) >>> 0; o += 2
    const entries = []
    for (let i = 0; i < count; i++) {
      const nameLen = ((bytes[o] << 8) | bytes[o + 1]) >>> 0; o += 2
      const name = new TextDecoder().decode(bytes.subarray(o, o + nameLen)); o += nameLen
      let size = 0
      for (let i2 = 0; i2 < 8; i2++) size = size * 256 + bytes[o + i2]
      o += 8
      entries.push({ name, data: bytes.slice(o, o + size) }); o += size
    }
    return { kind: "bundle", entries }
  }
  return { kind: "file", name: descriptorName, data: bytes }
}

let pass = 0
let fail = 0
function assert(cond, msg) {
  if (cond) {
    pass++
  } else {
    fail++
    console.error(`  ✗ ${msg}`)
  }
}

async function runCycle(init, { SenderSessionWasm, ReceiverSessionWasm }, descriptorName) {
  // Probe padded length with a throwaway sender.
  const probe = new SenderSessionWasm(
    init.payload,
    0x111n,
    0x222n,
    20,
    1024,
    descriptorName,
    BigInt(init.payload.length),
    0x12345678
  )
  const totalK = probe.total_symbols()
  // Build the real sender; the receiver trims to compressed_size via the
  // descriptor, and assemble_raw returns the trimmed bytes.
  const sender = new SenderSessionWasm(
    init.payload,
    0x111n,
    0x222n,
    20,
    1024,
    descriptorName,
    BigInt(init.payload.length),
    0x12345678
  )
  const frameCount = totalK * 2 + 40
  const frames = []
  for (let i = 0; i < frameCount; i++) frames.push(sender.next_frame())

  // find descriptor
  let descIdx = -1
  for (let i = 0; i < frames.length; i++) {
    if (frames[i][3] & 0x01) {
      descIdx = i
      break
    }
  }
  if (descIdx < 0) throw new Error("no descriptor")
  const rx = ReceiverSessionWasm.from_descriptor(frames[descIdx])
  assert(rx.meta_confirmed(), "meta confirmed after from_descriptor")
  assert(rx.file_name() === descriptorName, `file_name === ${descriptorName}`)
  assert(Number(rx.original_size()) === init.payload.length, "original_size matches")
  assert(rx.compression() === 0, "compression == NONE")

  let complete = false
  for (let i = 0; i < frames.length; i++) {
    if (i === descIdx) continue
    const status = rx.ingest(frames[i])
    if (status & 0x1n) {
      complete = true
      break
    }
  }
  assert(complete, "receiver completed")
  const raw = rx.assemble_raw()
  assert(raw.length === init.payload.length, `assemble_raw len ${raw.length} == ${init.payload.length}`)
  // byte equality
  let eq = true
  for (let i = 0; i < init.payload.length; i++) {
    if (raw[i] !== init.payload[i]) {
      eq = false
      break
    }
  }
  assert(eq, "recovered bytes equal original payload")
  return { raw, recovered: parseRecovered(raw, descriptorName) }
}

async function main() {
  const wasmBytes = readFileSync(wasmPath)
  const mod = await import("../../../apps/sender/wasm-pkg-simd/transfer_engine.js")
  await mod.default(await WebAssembly.compile(wasmBytes))
  const { SenderSessionWasm, ReceiverSessionWasm } = mod

  // ── Test 1: single file (COMPRESSION_NONE) ──
  console.log("Test 1: single file payload")
  {
    const payload = new Uint8Array(3000)
    for (let i = 0; i < payload.length; i++) payload[i] = i & 0xff
    const { recovered } = await runCycle(
      { payload },
      { SenderSessionWasm, ReceiverSessionWasm },
      "doc.bin"
    )
    assert(recovered.kind === "file", `kind=file (got ${recovered.kind})`)
    assert(recovered.name === "doc.bin", "name=doc.bin")
    assert(recovered.data.length === 3000, "data length 3000")
  }

  // ── Test 2: text (ETTEXTv1) ──
  console.log("Test 2: text payload (ETTEXTv1)")
  {
    const text = "你好，AirFerry！Hello 世界 🌍"
    const payload = buildTextPayload(text)
    const { recovered } = await runCycle(
      { payload },
      { SenderSessionWasm, ReceiverSessionWasm },
      "文字消息.txt"
    )
    assert(recovered.kind === "text", `kind=text (got ${recovered.kind})`)
    assert(recovered.text === text, `text round-trip`)
  }

  // ── Test 3: bundle (ETBUNDL1, 3 files) ──
  console.log("Test 3: bundle payload (ETBUNDL1, 3 files)")
  {
    const files = [
      { name: "a.txt", data: textBytes("file A content") },
      { name: "b.bin", data: new Uint8Array([0, 1, 2, 3, 255, 254]) },
      { name: "c.json", data: textBytes('{"k":42}') },
    ]
    const payload = buildBundle(files)
    const { recovered } = await runCycle(
      { payload },
      { SenderSessionWasm, ReceiverSessionWasm },
      "3个文件打包"
    )
    assert(recovered.kind === "bundle", `kind=bundle (got ${recovered.kind})`)
    assert(recovered.entries.length === 3, "3 entries")
    assert(recovered.entries[0].name === "a.txt", "entry0 name")
    assert(
      new TextDecoder().decode(recovered.entries[0].data) === "file A content",
      "entry0 content"
    )
    assert(recovered.entries[1].name === "b.bin", "entry1 name")
    assert(recovered.entries[2].name === "c.json", "entry2 name")
  }

  // ── Test 4: empty-ish / tiny file ──
  console.log("Test 4: tiny file (100B)")
  {
    const payload = new Uint8Array(100)
    for (let i = 0; i < 100; i++) payload[i] = (i * 7) & 0xff
    const { recovered } = await runCycle(
      { payload },
      { SenderSessionWasm, ReceiverSessionWasm },
      "tiny.dat"
    )
    assert(recovered.kind === "file", "tiny kind=file")
    assert(recovered.data.length === 100, "tiny data length 100")
  }

  console.log(`\n${fail === 0 ? "✅ ALL PASS" : "❌ FAILURES"}: ${pass} passed, ${fail} failed`)
  if (fail > 0) process.exit(1)
}

main().catch((e) => {
  console.error("FAILED:", e)
  process.exit(1)
})
