const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a 64-bit hash.
///
/// Self-implemented rather than using std `DefaultHasher`: std does not promise cross-version stability,
/// and the state hash is written into recording files for replay validation, so it must be cross-platform
/// and stable across engine versions forever.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// **Incremental writer** for FNV-1a: folds a byte stream into the hash state, avoiding the large
/// allocation of "first materialize the entire serialized string, then hash it". Implements `io::Write`,
/// so it can be fed directly to `serde_json::to_writer`.
///
/// FNV-1a is inherently a per-byte fold, **the number of chunks and the size of each chunk do not
/// affect the result** — `finish()` is bit-identical to `fnv1a_64(concatenate all the bytes)`
/// (locked down by `streaming_equals_oneshot`).
#[derive(Debug, Clone)]
pub struct Fnv1aWriter {
    state: u64,
}

impl Fnv1aWriter {
    pub fn new() -> Fnv1aWriter {
        Fnv1aWriter { state: FNV_OFFSET }
    }

    /// The currently accumulated hash value.
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
        // FNV-1a standard test vectors
        assert_eq!(fnv1a_64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x85944171f73967e8);
    }

    /// The streaming writer is bit-identical to the one-shot hash, and the result is invariant under any
    /// chunking scheme (the foundation for streaming state_hash).
    #[test]
    fn streaming_equals_oneshot() {
        use std::io::Write;
        let data = b"the quick brown fox jumps over the lazy dog 1234567890";
        // Whole chunk
        let mut w = Fnv1aWriter::new();
        w.write_all(data).unwrap();
        assert_eq!(w.finish(), fnv1a_64(data), "整块流式 == 一次性");
        // Byte by byte
        let mut w = Fnv1aWriter::new();
        for &b in data {
            w.write_all(&[b]).unwrap();
        }
        assert_eq!(w.finish(), fnv1a_64(data), "逐字节流式 == 一次性");
        // Irregular chunks
        let mut w = Fnv1aWriter::new();
        for chunk in data.chunks(7) {
            w.write_all(chunk).unwrap();
        }
        assert_eq!(w.finish(), fnv1a_64(data), "不规则分块 == 一次性");
        // Empty input = offset basis
        assert_eq!(Fnv1aWriter::new().finish(), fnv1a_64(b""));
    }
}
