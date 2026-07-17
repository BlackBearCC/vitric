//! BC7 (BPTC) offline compression — pure Rust, deterministic, zero external dependencies.
//!
//! Rationale (in the handoff): BC7 spec has 8 modes; full implementation is a big project;
//! existing crates (intel_tex_2 / ctt-bc7enc) are all ISPC/C bindings — neither pure Rust,
//! nor safe for x86_64-pc-windows-gnu cross-compilation (need ISPC toolchain / prebuilt
//! static libs). This project's hard constraints are "pure Rust first + windows-gnu
//! cross-compile must pass + same input → byte-identical output", so we hand-roll a
//! **single-mode (mode 6)** encoder here:
//!
//! - **mode 6**: 1 subset, RGBA all channels 7-bit endpoints + 1-bit P-bit, 64 indices
//!   (4 bits/pixel). A 16-pixel block always compresses to 16 bytes = 8bpp, exactly
//!   **1/4** (4×) of RGBA8 (32bpp).
//! - mode 6 is chosen because it's the **only** mode that can losslessly represent full
//!   alpha (other modes either have no alpha, or alpha is partitioned separately) — frame
//!   animation assets have transparent edges, alpha must not be lost.
//! - Endpoint fitting uses "channel-independent min/max + nearest endpoint index":
//!   deterministic (no float RNG, no parallel timing), quality is sufficient for
//!   palette-quantized assets (the assets pipeline already unifies palettes, so color
//!   steps are few to begin with).
//!
//! Real GPU upload + visual verification stays on the Windows GPU machine; the container
//! side only does **offline encoding + byte-count comparison** (RGBA8 raw vs BC7, assert
//! 4× + dedup saves more). Decoding (for round-trip self-check) is also here, pure CPU.

/// A BC7 block is 4×4 pixels, fixed 16 bytes.
pub const BLOCK_BYTES: usize = 16;

/// Compress an RGBA8 image (width×height, row-major, 4 bytes/pixel) into BC7.
///
/// When dimensions aren't multiples of 4, follow the BC7/GPU convention of **rounding up
/// to multiples of 4**, with edge blocks padded by the nearest edge pixel (clamp) — same
/// semantics as atlas packing's duplicate edges, so sampling won't bleed garbage. Returns
/// `(blocks_x, blocks_y, bytes)`, with byte length always `blocks_x*blocks_y*16`.
pub fn encode_rgba8(width: u32, height: u32, rgba: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let expect = width as usize * height as usize * 4;
    if rgba.len() != expect {
        return Err(format!(
            "BC7 编码：像素长度 {} 与 {width}x{height}×RGBA 的 {expect} 不符",
            rgba.len()
        ));
    }
    let bx = width.div_ceil(4);
    let by = height.div_ceil(4);
    let row_bytes = bx as usize * BLOCK_BYTES;
    let mut out = vec![0u8; by as usize * row_bytes];

    // Encode an entire block row byi into dst (dst is exactly that row's bx×16 bytes).
    // Each block reads only its own 4×4 pixels, writes its own fixed 16 bytes — no shared
    // mutable state → serial/parallel produce **byte-identical results**.
    let fill_row = |byi: u32, dst: &mut [u8]| {
        for bxi in 0..bx {
            // Take 4×4 pixels (out-of-bounds clamps to nearest edge pixel, same as packing duplicate edges)
            let mut block = [[0u8; 4]; 16];
            for py in 0..4u32 {
                for px in 0..4u32 {
                    let sx = (bxi * 4 + px).min(width - 1);
                    let sy = (byi * 4 + py).min(height - 1);
                    let o = (sy as usize * width as usize + sx as usize) * 4;
                    block[(py * 4 + px) as usize].copy_from_slice(&rgba[o..o + 4]);
                }
            }
            let d = bxi as usize * BLOCK_BYTES;
            dst[d..d + BLOCK_BYTES].copy_from_slice(&encode_block_mode6(&block));
        }
    };

    // Character atlases of hundreds/thousands of frames can reach tens of MB; serial
    // encoding stalls startup for several seconds. Block rows don't depend on each other
    // and each writes a disjoint output slice, so split block rows across CPU cores —
    // `thread::scope` borrows rgba/out, outputs are disjoint, result is byte-identical to
    // serial (no float RNG, no order dependency, determinism preserved). Small images
    // (< PAR_MIN_ROWS block rows) go serial to skip thread overhead. Pure std, no
    // third-party deps.
    const PAR_MIN_ROWS: u32 = 64;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    if threads <= 1 || by < PAR_MIN_ROWS {
        for (byi, dst) in out.chunks_mut(row_bytes).enumerate() {
            fill_row(byi as u32, dst);
        }
    } else {
        let rows_per = (by as usize).div_ceil(threads);
        let fill_row = &fill_row; // each thread shares the read-only closure (captures &rgba, Sync)
        std::thread::scope(|s| {
            for (chunk_i, dst_chunk) in out.chunks_mut(rows_per * row_bytes).enumerate() {
                let base = (chunk_i * rows_per) as u32;
                s.spawn(move || {
                    for (i, dst) in dst_chunk.chunks_mut(row_bytes).enumerate() {
                        fill_row(base + i as u32, dst);
                    }
                });
            }
        });
    }
    Ok((bx, by, out))
}

/// Encode one block (16 RGBA pixels) into mode 6's 16 bytes.
fn encode_block_mode6(px: &[[u8; 4]; 16]) -> [u8; BLOCK_BYTES] {
    // Endpoints: per channel, take block min / max (deterministic). Each endpoint's P-bit
    // takes the lowest bit of the side it represents (endpoint is 8-bit; mode6 stores 7-bit
    // main value + 1-bit P-bit = restored 8-bit).
    let mut lo = [255u8; 4];
    let mut hi = [0u8; 4];
    for p in px {
        for c in 0..4 {
            lo[c] = lo[c].min(p[c]);
            hi[c] = hi[c].max(p[c]);
        }
    }
    // 7-bit main value + P-bit split: original 8-bit value v → main value v>>1, P-bit v&1.
    // Decoder restores v' = (main<<1)|pbit; lossless when the P-bit matches the original v.
    let p0 = endpoint_pbit(&lo);
    let p1 = endpoint_pbit(&hi);
    let e0: [u8; 4] = std::array::from_fn(|c| lo[c] >> 1); // 7-bit main value
    let e1: [u8; 4] = std::array::from_fn(|c| hi[c] >> 1);
    // The two endpoints the decoder will actually use (restored to 8-bit) — index fitting
    // must align with them for the round-trip to be accurate.
    let d0: [u8; 4] = std::array::from_fn(|c| (e0[c] << 1) | p0);
    let d1: [u8; 4] = std::array::from_fn(|c| (e1[c] << 1) | p1);

    // Each pixel picks the nearest of the 16 interpolation indices (mode6 index is 4-bit).
    let mut indices = [0u8; 16];
    for (i, p) in px.iter().enumerate() {
        indices[i] = nearest_index(&d0, &d1, p);
    }
    // mode6 convention: the anchor (pixel 0) index's highest bit must be 0; otherwise
    // swap endpoints.
    if indices[0] & 0b1000 != 0 {
        // Swap endpoints + invert indices (15-idx), keeping the visual unchanged and the
        // anchor high bit at 0
        return encode_block_mode6_swapped(&e1, &e0, p1, p0, px);
    }
    pack_mode6(&e0, &e1, p0, p1, &indices)
}

/// Endpoint-swap branch: swap endpoints then refit indices (15-idx is equivalent to
/// swapping endpoints; skipping the refit would work, but refitting is clearer and
/// still deterministic).
fn encode_block_mode6_swapped(
    e0: &[u8; 4],
    e1: &[u8; 4],
    p0: u8,
    p1: u8,
    px: &[[u8; 4]; 16],
) -> [u8; BLOCK_BYTES] {
    let d0: [u8; 4] = std::array::from_fn(|c| (e0[c] << 1) | p0);
    let d1: [u8; 4] = std::array::from_fn(|c| (e1[c] << 1) | p1);
    let mut indices = [0u8; 16];
    for (i, p) in px.iter().enumerate() {
        indices[i] = nearest_index(&d0, &d1, p);
    }
    // After swap, anchor high bit is guaranteed 0 (endpoints swapped; indices that were
    // >7 are now <8)
    pack_mode6(e0, e1, p0, p1, &indices)
}

/// Endpoint P-bit: take the lowest bit of the endpoint's alpha channel (any channel
/// would do; convention is alpha — frame assets care most about alpha restoration).
fn endpoint_pbit(v: &[u8; 4]) -> u8 {
    v[3] & 1
}

/// mode6's 16-step weight table (spec BC7 4-bit index interpolation weights, 0..64).
const W6: [u32; 16] =
    [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

/// Find the nearest of the 16 interpolation steps along d0→d1 (by the weight table,
/// deterministic).
fn nearest_index(d0: &[u8; 4], d1: &[u8; 4], p: &[u8; 4]) -> u8 {
    let mut best = 0u8;
    let mut best_err = u32::MAX;
    for (idx, &w) in W6.iter().enumerate() {
        let mut err = 0u32;
        for c in 0..4 {
            let v = interp(d0[c], d1[c], w);
            let diff = v as i32 - p[c] as i32;
            err += (diff * diff) as u32;
        }
        if err < best_err {
            best_err = err;
            best = idx as u8;
        }
    }
    best
}

/// BC7 endpoint interpolation (spec formula: (64-w)*a + w*b rounded to 8-bit).
fn interp(a: u8, b: u8, w: u32) -> u8 {
    (((64 - w) * a as u32 + w * b as u32 + 32) >> 6) as u8
}

/// Pack mode6 fields into a 128-bit block (little-endian byte order, LSB at byte0 bit0).
/// Bit layout (BC7 mode 6, starting from bit0, 128 bits total):
///   7-bit mode prefix (6 zeros + 1 one; "mode n = the nth bit is the first 1")
///   | R0,R1,G0,G1,B0,B1,A0,A1 each 7 bits (56 total) | P0,P1 each 1 bit (2 total)
///   | 16 indices × 4 bits (first index is the anchor, highest bit omitted, only 3 bits — 63 total)
///   Total 7+56+2+63 = 128.
fn pack_mode6(e0: &[u8; 4], e1: &[u8; 4], p0: u8, p1: u8, indices: &[u8; 16]) -> [u8; BLOCK_BYTES] {
    let mut bits = BitWriter::new();
    bits.put(0b100_0000, 7); // mode 6: bit0..5 are 0, bit6 is 1
    // Endpoint order: R0 R1 G0 G1 B0 B1 A0 A1 (7 bits each)
    for c in 0..4 {
        bits.put(e0[c] as u128, 7);
        bits.put(e1[c] as u128, 7);
    }
    bits.put(p0 as u128, 1);
    bits.put(p1 as u128, 1);
    // Indices: anchor (the 0th) takes only 3 bits (highest bit is conventionally 0, omitted);
    // the rest take 4 bits each
    bits.put(indices[0] as u128, 3);
    for &idx in &indices[1..] {
        bits.put(idx as u128, 4);
    }
    bits.finish()
}

/// Decode one mode6 block back into 16 RGBA pixels (for round-trip self-check and
/// byte comparison; pure CPU).
pub fn decode_block_mode6(block: &[u8; BLOCK_BYTES]) -> [[u8; 4]; 16] {
    let mut bits = BitReader::new(block);
    let mode = bits.get(7);
    assert_eq!(mode, 0b100_0000, "只解码 mode 6 块（7 位前缀 = 0b1000000）");
    let mut e0 = [0u8; 4];
    let mut e1 = [0u8; 4];
    for c in 0..4 {
        e0[c] = bits.get(7) as u8;
        e1[c] = bits.get(7) as u8;
    }
    let p0 = bits.get(1) as u8;
    let p1 = bits.get(1) as u8;
    let d0: [u8; 4] = std::array::from_fn(|c| (e0[c] << 1) | p0);
    let d1: [u8; 4] = std::array::from_fn(|c| (e1[c] << 1) | p1);
    let mut out = [[0u8; 4]; 16];
    for (i, slot) in out.iter_mut().enumerate() {
        let idx = if i == 0 { bits.get(3) } else { bits.get(4) } as usize;
        let w = W6[idx];
        for c in 0..4 {
            slot[c] = interp(d0[c], d1[c], w);
        }
    }
    out
}

/// Decode an entire BC7 image (for round-trip self-check). `blocks_x/blocks_y` is the
/// block grid; `width/height` is the real size to crop back to (the block grid may have
/// been rounded up).
pub fn decode_to_rgba8(
    blocks_x: u32,
    blocks_y: u32,
    width: u32,
    height: u32,
    data: &[u8],
) -> Vec<u8> {
    let mut out = vec![0u8; width as usize * height as usize * 4];
    for byi in 0..blocks_y {
        for bxi in 0..blocks_x {
            let off = ((byi * blocks_x + bxi) as usize) * BLOCK_BYTES;
            let block: [u8; BLOCK_BYTES] = data[off..off + BLOCK_BYTES].try_into().expect("块长 16");
            let texels = decode_block_mode6(&block);
            for py in 0..4u32 {
                for px in 0..4u32 {
                    let x = bxi * 4 + px;
                    let y = byi * 4 + py;
                    if x < width && y < height {
                        let d = (y as usize * width as usize + x as usize) * 4;
                        out[d..d + 4].copy_from_slice(&texels[(py * 4 + px) as usize]);
                    }
                }
            }
        }
    }
    out
}

/// Little-endian bit writer (LSB first, matching the GPU's byte interpretation of BC7 blocks).
struct BitWriter {
    acc: u128,
    pos: u32,
}
impl BitWriter {
    fn new() -> BitWriter {
        BitWriter { acc: 0, pos: 0 }
    }
    fn put(&mut self, value: u128, bits: u32) {
        debug_assert!(bits == 128 || value < (1u128 << bits), "值越位");
        self.acc |= value << self.pos;
        self.pos += bits;
    }
    fn finish(self) -> [u8; BLOCK_BYTES] {
        debug_assert_eq!(self.pos, 128, "mode6 块必须正好 128 位");
        self.acc.to_le_bytes()
    }
}

/// Little-endian bit reader (symmetric with [`BitWriter`]).
struct BitReader {
    acc: u128,
    pos: u32,
}
impl BitReader {
    fn new(block: &[u8; BLOCK_BYTES]) -> BitReader {
        BitReader { acc: u128::from_le_bytes(*block), pos: 0 }
    }
    fn get(&mut self, bits: u32) -> u128 {
        let mask = if bits == 128 { u128::MAX } else { (1u128 << bits) - 1 };
        let v = (self.acc >> self.pos) & mask;
        self.pos += bits;
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parallel encode is byte-identical to serial: encode_rgba8 splits large images across
    /// block rows on multiple threads; the result must be exactly the same as encoding block
    /// by block in order (determinism is a hard contract for `--frames`/`vitric check`;
    /// parallelism must not break it).
    #[test]
    fn parallel_encode_byte_identical_to_serial() {
        // 320px height = 80 block rows > PAR_MIN_ROWS(64), ensures the parallel path;
        // width not a multiple of 4 tests the round-up
        let (w, h) = (70u32, 320u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        // Build content with a gradient (not solid color, forcing parallel/serial to agree
        // on "lossy blocks" too)
        for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
            px.copy_from_slice(&[(i % 251) as u8, (i / 7 % 253) as u8, (i / 13 % 247) as u8, 255]);
        }
        let (bx, by, par) = encode_rgba8(w, h, &rgba).unwrap();
        // Serial reference: encode block by block in order
        let mut serial = Vec::with_capacity(bx as usize * by as usize * BLOCK_BYTES);
        for byi in 0..by {
            for bxi in 0..bx {
                let mut block = [[0u8; 4]; 16];
                for py in 0..4u32 {
                    for px in 0..4u32 {
                        let sx = (bxi * 4 + px).min(w - 1);
                        let sy = (byi * 4 + py).min(h - 1);
                        let o = (sy as usize * w as usize + sx as usize) * 4;
                        block[(py * 4 + px) as usize].copy_from_slice(&rgba[o..o + 4]);
                    }
                }
                serial.extend_from_slice(&encode_block_mode6(&block));
            }
        }
        assert_eq!(par, serial, "并行编码必须与串行逐字节一致");
    }

    /// A mode6 block is always 16 bytes, 4×4 pixels = 8bpp.
    #[test]
    fn block_is_8bpp() {
        let block = [[10u8, 20, 30, 40]; 16];
        let bytes = encode_block_mode6(&block);
        assert_eq!(bytes.len(), 16);
        // 16 pixels × 4 bytes RGBA = 64 bytes raw → 16 bytes BC7 = 4×
        assert_eq!(64 / bytes.len(), 4);
    }

    /// Solid-color block round-trip: extremes (all 0 / all 255) are byte-lossless;
    /// arbitrary colors have error ≤1. (mode 6 endpoint P-bit is shared across all
    /// channels; channels with mismatched parity have ±1 quantization — this is a format
    /// property, not a bug; the assets pipeline already unifies palettes, so color steps
    /// are few and the error is visually imperceptible.)
    #[test]
    fn solid_block_roundtrip() {
        // Extremes are lossless (0 → main0 pbit0; 255 → main127 pbit1)
        for color in [[0u8, 0, 0, 0], [255, 255, 255, 255]] {
            let back = decode_block_mode6(&encode_block_mode6(&[color; 16]));
            for t in back {
                assert_eq!(t, color, "极值 {color:?} 往返应无损");
            }
        }
        // Arbitrary colors have error ≤1 (inherent quantization from shared P-bit)
        for color in [[123u8, 45, 200, 99], [7, 250, 13, 128]] {
            let back = decode_block_mode6(&encode_block_mode6(&[color; 16]));
            for t in back {
                for c in 0..4 {
                    assert!(t[c].abs_diff(color[c]) <= 1, "纯色 {color:?} 误差应 ≤1，得 {t:?}");
                }
            }
        }
    }

    /// Axis-aligned two-endpoint color block round-trip with small error (endpoints are
    /// exactly the block's min/max, indices hit steps 0 and 15). This is the norm for real
    /// assets: palette-quantized frames usually have a few steps on the same gradient
    /// (collinear), not pathological R/G-anticorrelated colors. With axis-aligned endpoints,
    /// mode 6 single-subset representation is good.
    #[test]
    fn two_color_block_roundtrip() {
        // a→b along the same direction (all channels increase together), lying on the lo→hi line
        let a = [20u8, 40, 60, 255];
        let b = [200u8, 220, 240, 255];
        let mut block = [[0u8; 4]; 16];
        for (i, t) in block.iter_mut().enumerate() {
            *t = if i % 2 == 0 { a } else { b };
        }
        let back = decode_block_mode6(&encode_block_mode6(&block));
        for (i, t) in back.iter().enumerate() {
            let want = if i % 2 == 0 { a } else { b };
            for c in 0..4 {
                let d = (t[c] as i32 - want[c] as i32).abs();
                assert!(d <= 4, "通道 {c} 误差 {d} 过大（轴对齐两端点应近无损）");
            }
        }
    }

    /// Pathological colors (R/G anticorrelated) don't crash or panic — mode 6 single-subset
    /// expressiveness is limited, a known tradeoff (essentially never happens after the
    /// assets pipeline unifies palettes), but the encoder must deterministically produce a
    /// valid block.
    #[test]
    fn anticorrelated_block_is_stable_not_panicking() {
        let a = [10u8, 250, 5, 255];
        let b = [240u8, 8, 200, 30];
        let mut block = [[0u8; 4]; 16];
        for (i, t) in block.iter_mut().enumerate() {
            *t = if i % 2 == 0 { a } else { b };
        }
        // Deterministic + round-trip doesn't panic (quality not guaranteed, stability is)
        let enc = encode_block_mode6(&block);
        assert_eq!(enc, encode_block_mode6(&block), "同输入同产物");
        let _ = decode_block_mode6(&enc);
    }

    /// Same input → byte-identical output (the iron law of determinism).
    #[test]
    fn deterministic_bytes() {
        let mut rgba = Vec::new();
        for i in 0..(8 * 8) {
            rgba.extend_from_slice(&[(i * 3) as u8, (i * 5) as u8, (i * 7) as u8, 255]);
        }
        let a = encode_rgba8(8, 8, &rgba).unwrap();
        let b = encode_rgba8(8, 8, &rgba).unwrap();
        assert_eq!(a, b, "同输入必须逐字节同产物");
    }

    /// 4× compression ratio: W×H RGBA8 raw bytes == BC7 bytes × 4 (exact when dimensions are multiples of 4).
    #[test]
    fn compression_ratio_is_4x() {
        let (w, h) = (16u32, 16u32);
        let rgba = vec![128u8; (w * h * 4) as usize];
        let (bx, by, data) = encode_rgba8(w, h, &rgba).unwrap();
        assert_eq!((bx, by), (4, 4));
        let raw = (w * h * 4) as usize;
        assert_eq!(raw, data.len() * 4, "BC7 必须是 RGBA8 的 1/4");
    }

    /// Non-multiple-of-4 dimensions round up to the block grid; decoding crops back to original size.
    #[test]
    fn non_multiple_of_four_rounds_up() {
        let (w, h) = (5u32, 3u32);
        let rgba = vec![77u8; (w * h * 4) as usize];
        let (bx, by, data) = encode_rgba8(w, h, &rgba).unwrap();
        assert_eq!((bx, by), (2, 1), "5x3 → 2x1 块");
        let back = decode_to_rgba8(bx, by, w, h, &data);
        assert_eq!(back.len(), (w * h * 4) as usize);
        assert_eq!(back, rgba, "纯色应无损还原到原尺寸");
    }

    /// Length mismatch is an explicit error (not silent).
    #[test]
    fn wrong_length_errors() {
        let err = encode_rgba8(4, 4, &[0u8; 10]).unwrap_err();
        assert!(err.contains("不符"), "应点名长度不符: {err}");
    }
}
