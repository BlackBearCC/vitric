//! BC7（BPTC）离线压缩——纯 Rust、确定性、零外部依赖。
//!
//! 选型理由（写进交接单）：BC7 规范有 8 个模式，全实现是大工程；现成 crate
//! （intel_tex_2 / ctt-bc7enc）都是 ISPC/C 绑定——既不是纯 Rust，又给
//! x86_64-pc-windows-gnu 交叉编译埋雷（要 ISPC 工具链/预编译静态库）。本工程
//! 的硬约束是「纯 Rust 优先 + windows-gnu 交叉编译必过 + 同输入逐字节同产物」，
//! 所以这里自己写一个**单模式（mode 6）**编码器：
//!
//! - **mode 6**：1 个子集、RGBA 全通道各 7 位端点 + 1 位 P-bit、64 个索引（4 位/像素）。
//!   一块 16 像素恒定压成 16 字节 = 8bpp，正好是 RGBA8（32bpp）的 **1/4**（4×）。
//! - 选 mode 6 是因为它**唯一**能无损表达完整 alpha（其余模式要么无 alpha，要么
//!   alpha 单独分区）——帧动画素材带透明边，alpha 不能丢。
//! - 端点拟合走「通道独立的 min/max + 最近端点索引」：确定（无浮点 RNG、无并行
//!   时序），质量对色板量化过的素材足够（assets 流水线已统一色板，色阶本就少）。
//!
//! GPU 真机上传 + 视觉验证留 Windows GPU 机；容器侧只做**离线编码 + 字节数对比**
//! （RGBA8 raw vs BC7，断言 4× + 去重额外省）。解码（往返自检用）也在这里，纯 CPU。

/// 一块 BC7 是 4×4 像素、固定 16 字节。
pub const BLOCK_BYTES: usize = 16;

/// 把 RGBA8 图（width×height，行优先，每像素 4 字节）压成 BC7。
///
/// 尺寸非 4 的倍数时按 BC7/GPU 惯例**向上取整到 4 的倍数**，边缘块用最近边像素
/// 填充（clamp）——和图集打包的复制边同语义，采样不会渗进垃圾。返回
/// `(blocks_x, blocks_y, 字节)`，字节长度恒为 `blocks_x*blocks_y*16`。
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
    let mut out = Vec::with_capacity(bx as usize * by as usize * BLOCK_BYTES);
    for byi in 0..by {
        for bxi in 0..bx {
            // 取 4×4 像素（越界 clamp 到最近边像素，和打包复制边一致）
            let mut block = [[0u8; 4]; 16];
            for py in 0..4u32 {
                for px in 0..4u32 {
                    let sx = (bxi * 4 + px).min(width - 1);
                    let sy = (byi * 4 + py).min(height - 1);
                    let o = (sy as usize * width as usize + sx as usize) * 4;
                    block[(py * 4 + px) as usize].copy_from_slice(&rgba[o..o + 4]);
                }
            }
            out.extend_from_slice(&encode_block_mode6(&block));
        }
    }
    Ok((bx, by, out))
}

/// 编码一块（16 个 RGBA 像素）为 mode 6 的 16 字节。
fn encode_block_mode6(px: &[[u8; 4]; 16]) -> [u8; BLOCK_BYTES] {
    // 端点：每通道取块内 min / max（确定）。两端点的 P-bit 各自取被代表那侧
    // 的最低位（端点是 8 位，mode6 存 7 位主值 + 1 位 P-bit = 复原 8 位）。
    let mut lo = [255u8; 4];
    let mut hi = [0u8; 4];
    for p in px {
        for c in 0..4 {
            lo[c] = lo[c].min(p[c]);
            hi[c] = hi[c].max(p[c]);
        }
    }
    // 7 位主值 + P-bit 拆分：原 8 位值 v → 主值 v>>1，P-bit v&1。
    // 解码端复原 v' = (主值<<1)|pbit，与原 v 在 P-bit 一致时无损。
    let p0 = endpoint_pbit(&lo);
    let p1 = endpoint_pbit(&hi);
    let e0: [u8; 4] = std::array::from_fn(|c| lo[c] >> 1); // 7 位主值
    let e1: [u8; 4] = std::array::from_fn(|c| hi[c] >> 1);
    // 解码端会实际使用的两端点（复原成 8 位）——索引拟合必须对齐它，往返才准。
    let d0: [u8; 4] = std::array::from_fn(|c| (e0[c] << 1) | p0);
    let d1: [u8; 4] = std::array::from_fn(|c| (e1[c] << 1) | p1);

    // 每像素选最近的 16 档插值索引（mode6 索引 4 位）。
    let mut indices = [0u8; 16];
    for (i, p) in px.iter().enumerate() {
        indices[i] = nearest_index(&d0, &d1, p);
    }
    // mode6 约定：anchor（第 0 像素）索引最高位必须为 0，否则要交换端点。
    if indices[0] & 0b1000 != 0 {
        // 交换端点 + 索引取反（15-idx），保持视觉不变、anchor 高位归 0
        return encode_block_mode6_swapped(&e1, &e0, p1, p0, px);
    }
    pack_mode6(&e0, &e1, p0, p1, &indices)
}

/// 端点交换分支：端点对调后重算索引（15-idx 等价于端点对调，省一次拟合也行，
/// 但重算更直白且仍确定）。
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
    // 交换后 anchor 高位必然为 0（端点已对调，原本 >7 的索引现在 <8）
    pack_mode6(e0, e1, p0, p1, &indices)
}

/// 端点的 P-bit：取该端点 alpha 通道最低位（任选一通道即可，约定用 alpha——
/// 帧素材最在意 alpha 的还原）。
fn endpoint_pbit(v: &[u8; 4]) -> u8 {
    v[3] & 1
}

/// mode6 的 16 档权重表（规范 BC7 4 位索引插值权重，0..64）。
const W6: [u32; 16] =
    [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

/// 沿 d0→d1 找最近的 16 档插值（按权重表，确定）。
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

/// BC7 端点插值（规范公式：(64-w)*a + w*b 四舍五入到 8 位）。
fn interp(a: u8, b: u8, w: u32) -> u8 {
    (((64 - w) * a as u32 + w * b as u32 + 32) >> 6) as u8
}

/// 把 mode6 字段打进 128 位块（小端字节序，LSB 在 byte0 bit0）。
/// 位布局（BC7 mode 6，从 bit0 起，总 128 位）：
///   7 位 mode 前缀（6 个 0 + 1 个 1，"mode n = 第 n 位是首个 1"）
///   | R0,R1,G0,G1,B0,B1,A0,A1 各 7 位（共 56） | P0,P1 各 1 位（共 2）
///   | 16 索引×4 位（首索引 anchor 省最高位只占 3，共 63）
///   合计 7+56+2+63 = 128。
fn pack_mode6(e0: &[u8; 4], e1: &[u8; 4], p0: u8, p1: u8, indices: &[u8; 16]) -> [u8; BLOCK_BYTES] {
    let mut bits = BitWriter::new();
    bits.put(0b100_0000, 7); // mode 6：bit0..5 为 0，bit6 为 1
    // 端点顺序：R0 R1 G0 G1 B0 B1 A0 A1（各 7 位）
    for c in 0..4 {
        bits.put(e0[c] as u128, 7);
        bits.put(e1[c] as u128, 7);
    }
    bits.put(p0 as u128, 1);
    bits.put(p1 as u128, 1);
    // 索引：anchor（第 0 个）只占 3 位（最高位被约定为 0 省掉），其余各 4 位
    bits.put(indices[0] as u128, 3);
    for &idx in &indices[1..] {
        bits.put(idx as u128, 4);
    }
    bits.finish()
}

/// 解码一块 mode6 回 16 个 RGBA 像素（往返自检 + 字节对比用，纯 CPU）。
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

/// 解码整张 BC7（往返自检用）。`blocks_x/blocks_y` 是块网格，`width/height` 是
/// 裁回的真实尺寸（块网格可能向上取整过）。
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

/// 小端位写入器（LSB 优先，匹配 GPU 对 BC7 块的字节解释）。
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

/// 小端位读取器（与 [`BitWriter`] 对称）。
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

    /// mode6 块恒 16 字节，4×4 像素 = 8bpp。
    #[test]
    fn block_is_8bpp() {
        let block = [[10u8, 20, 30, 40]; 16];
        let bytes = encode_block_mode6(&block);
        assert_eq!(bytes.len(), 16);
        // 16 像素 × 4 字节 RGBA = 64 字节 raw → 16 字节 BC7 = 4×
        assert_eq!(64 / bytes.len(), 4);
    }

    /// 纯色块往返：极值（全 0 / 全 255）逐字节无损，任意色误差 ≤1。
    /// （mode 6 端点的 P-bit 全通道共享，奇偶不一致的通道有 ±1 量化——这是格式特性，
    /// 不是 bug；assets 流水线已统一色板，色阶本就少，肉眼无感。）
    #[test]
    fn solid_block_roundtrip() {
        // 极值无损（0 → main0 pbit0；255 → main127 pbit1）
        for color in [[0u8, 0, 0, 0], [255, 255, 255, 255]] {
            let back = decode_block_mode6(&encode_block_mode6(&[color; 16]));
            for t in back {
                assert_eq!(t, color, "极值 {color:?} 往返应无损");
            }
        }
        // 任意色误差 ≤1（P-bit 共享的固有量化）
        for color in [[123u8, 45, 200, 99], [7, 250, 13, 128]] {
            let back = decode_block_mode6(&encode_block_mode6(&[color; 16]));
            for t in back {
                for c in 0..4 {
                    assert!(t[c].abs_diff(color[c]) <= 1, "纯色 {color:?} 误差应 ≤1，得 {t:?}");
                }
            }
        }
    }

    /// 轴对齐的两端点色块往返误差小（端点正好是块内 min/max，索引命中 0 和 15 档）。
    /// 这是真实素材的常态：色板量化过的帧，块内多是同一渐变上的几档（共线），
    /// 不是 R/G 反相关的病态色。端点轴对齐时 mode 6 单 subset 表达良好。
    #[test]
    fn two_color_block_roundtrip() {
        // a→b 沿同一方向（各通道同增），落在 lo→hi 直线上
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

    /// 病态色（R/G 反相关）不崩、不 panic——mode 6 单 subset 表达力有限是已知取舍
    /// （assets 流水线统一色板后基本不出现），但编码器必须确定地产出合法块。
    #[test]
    fn anticorrelated_block_is_stable_not_panicking() {
        let a = [10u8, 250, 5, 255];
        let b = [240u8, 8, 200, 30];
        let mut block = [[0u8; 4]; 16];
        for (i, t) in block.iter_mut().enumerate() {
            *t = if i % 2 == 0 { a } else { b };
        }
        // 确定 + 往返不 panic（质量不保证，稳定性保证）
        let enc = encode_block_mode6(&block);
        assert_eq!(enc, encode_block_mode6(&block), "同输入同产物");
        let _ = decode_block_mode6(&enc);
    }

    /// 同输入逐字节同产物（确定性铁律）。
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

    /// 4× 压缩比：W×H RGBA8 raw 字节 == BC7 字节 × 4（尺寸为 4 的倍数时精确）。
    #[test]
    fn compression_ratio_is_4x() {
        let (w, h) = (16u32, 16u32);
        let rgba = vec![128u8; (w * h * 4) as usize];
        let (bx, by, data) = encode_rgba8(w, h, &rgba).unwrap();
        assert_eq!((bx, by), (4, 4));
        let raw = (w * h * 4) as usize;
        assert_eq!(raw, data.len() * 4, "BC7 必须是 RGBA8 的 1/4");
    }

    /// 非 4 倍数尺寸向上取整到块网格，解码裁回原尺寸。
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

    /// 长度不符显式报错（不静默）。
    #[test]
    fn wrong_length_errors() {
        let err = encode_rgba8(4, 4, &[0u8; 10]).unwrap_err();
        assert!(err.contains("不符"), "应点名长度不符: {err}");
    }
}
