const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a 64 位哈希。
///
/// 自实现而不用 std `DefaultHasher`：std 不承诺跨版本稳定，而状态哈希要写进
/// 录像文件做重放校验，必须跨平台、跨引擎版本永远一致。
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// FNV-1a 的**增量写入器**：把字节流式折进哈希状态，省掉「先 materialize 整个序列化
/// 字符串再哈希」那一大块分配。实现 `io::Write`，可直接喂给 `serde_json::to_writer`。
///
/// FNV-1a 本就是逐字节折叠，**分多少块、每块多大都不影响结果**——`finish()` 与
/// `fnv1a_64(把所有字节拼起来)` 逐位相同（由 `streaming_equals_oneshot` 锁死）。
#[derive(Debug, Clone)]
pub struct Fnv1aWriter {
    state: u64,
}

impl Fnv1aWriter {
    pub fn new() -> Fnv1aWriter {
        Fnv1aWriter { state: FNV_OFFSET }
    }

    /// 当前累计的哈希值。
    pub fn finish(&self) -> u64 {
        self.state
    }
}

impl Default for Fnv1aWriter {
    fn default() -> Fnv1aWriter {
        Fnv1aWriter::new()
    }
}

impl std::io::Write for Fnv1aWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for &b in buf {
            self.state ^= b as u64;
            self.state = self.state.wrapping_mul(FNV_PRIME);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // FNV-1a 标准测试向量
        assert_eq!(fnv1a_64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x85944171f73967e8);
    }

    /// 流式写入器与一次性哈希逐位等价，且任意分块方式结果不变（state_hash 流式化的地基）。
    #[test]
    fn streaming_equals_oneshot() {
        use std::io::Write;
        let data = b"the quick brown fox jumps over the lazy dog 1234567890";
        // 整块
        let mut w = Fnv1aWriter::new();
        w.write_all(data).unwrap();
        assert_eq!(w.finish(), fnv1a_64(data), "整块流式 == 一次性");
        // 逐字节
        let mut w = Fnv1aWriter::new();
        for &b in data {
            w.write_all(&[b]).unwrap();
        }
        assert_eq!(w.finish(), fnv1a_64(data), "逐字节流式 == 一次性");
        // 不规则分块
        let mut w = Fnv1aWriter::new();
        for chunk in data.chunks(7) {
            w.write_all(chunk).unwrap();
        }
        assert_eq!(w.finish(), fnv1a_64(data), "不规则分块 == 一次性");
        // 空输入 = offset basis
        assert_eq!(Fnv1aWriter::new().finish(), fnv1a_64(b""));
    }
}
