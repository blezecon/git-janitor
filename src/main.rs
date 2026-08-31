pub mod testdata {
    pub const ZLIB_HELLO: &str = "7801cb48cdc9c95728cf2fca49e102001e720467";
    pub const ZLIB_MULTILINE: &str = "7801011500eaff6c696e6520310a6c696e6520320a6c696e6520330a4831060d";
    pub const INDEX_V2_HEX: &str = "444952430000000200000001000000000000000000000000000000000000000000000000000081a400000000000000000000000c2bb4830536366da53082b9870768eec3292fe997000866696c652e747874000000000000000000000000000000000000000000000000000000000000000000000000";
}

pub mod testutil {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Converts a hex string into raw bytes.
    pub fn hex(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        let mut out = Vec::with_capacity(s.len() / 2);
        let bytes = s.as_bytes();
        let mut i = 0;
        while i + 1 < bytes.len() {
            let high = (bytes[i] as char).to_digit(16).unwrap_or(0) as u8;
            let low = (bytes[i + 1] as char).to_digit(16).unwrap_or(0) as u8;
            out.push((high << 4) | low);
            i += 2;
        }
        out
    }

    /// Creates a unique temporary directory for tests.
    pub fn unique_tempdir(tag: &str) -> PathBuf {
        let count = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("git_jan_{tag}_{nanos}_{count}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Compresses data into uncompressed RFC 1950 zlib format.
    pub fn zlib_store(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        let mut offset = 0;
        while offset < data.len() || offset == 0 {
            let chunk_len = (data.len() - offset).min(65535);
            let is_last = offset + chunk_len == data.len();
            out.push(if is_last { 0x01 } else { 0x00 });
            let len = chunk_len as u16;
            let nlen = !len;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&nlen.to_le_bytes());
            out.extend_from_slice(&data[offset..offset + chunk_len]);
            offset += chunk_len;
            if is_last {
                break;
            }
        }
        let adler = crate::inflate::adler32(data);
        out.extend_from_slice(&adler.to_be_bytes());
        out
    }

    /// Writes a mock loose object into a git repository.
    pub fn write_loose_object(repo_path: &Path, obj_type: &str, content: &[u8]) -> crate::objects::Oid {
        let mut raw = Vec::new();
        raw.extend_from_slice(obj_type.as_bytes());
        raw.push(b' ');
        raw.extend_from_slice(content.len().to_string().as_bytes());
        raw.push(0);
        raw.extend_from_slice(content);
        let oid = crate::objects::sha1(&raw);
        let oid_hex = crate::objects::Oid(oid).to_hex();
        let obj_dir = repo_path.join(".git/objects").join(&oid_hex[..2]);
        fs::create_dir_all(&obj_dir).unwrap();
        let obj_path = obj_dir.join(&oid_hex[2..]);
        fs::write(obj_path, zlib_store(&raw)).unwrap();
        crate::objects::Oid(oid)
    }
}

pub mod inflate {
    use std::fmt;

    /// Error returned during zlib/DEFLATE decompression.
    #[derive(Debug, PartialEq, Eq)]
    pub struct InflateError(pub String);

    impl fmt::Display for InflateError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "inflate error: {}", self.0)
        }
    }

    impl std::error::Error for InflateError {}

    /// Calculates the Adler-32 checksum of a byte slice.
    pub fn adler32(data: &[u8]) -> u32 {
        let mut s1 = 1u32;
        let mut s2 = 0u32;
        for chunk in data.chunks(5552) {
            for &byte in chunk {
                s1 += byte as u32;
                s2 += s1;
            }
            s1 %= 65521;
            s2 %= 65521;
        }
        (s2 << 16) | s1
    }

    /// Decompresses a full RFC 1950 zlib stream.
    pub fn inflate_zlib(data: &[u8]) -> Result<Vec<u8>, InflateError> {
        if data.len() < 6 {
            return Err(InflateError("zlib stream too short".into()));
        }
        let cmf = data[0];
        let flg = data[1];
        if ((cmf as u16 * 256 + flg as u16) % 31) != 0 {
            return Err(InflateError("invalid zlib header check".into()));
        }
        if (cmf & 0x0F) != 8 {
            return Err(InflateError("unsupported compression method".into()));
        }
        if (flg & 0x20) != 0 {
            return Err(InflateError("preset dictionary not supported".into()));
        }
        let uncompressed = inflate_raw(&data[2..data.len() - 4])?;
        let expected_adler = u32::from_be_bytes(
            data[data.len() - 4..]
                .try_into()
                .map_err(|_| InflateError("invalid adler bytes".into()))?,
        );
        let actual_adler = adler32(&uncompressed);
        if actual_adler != expected_adler {
            return Err(InflateError("adler32 mismatch".into()));
        }
        Ok(uncompressed)
    }

    struct BitReader<'a> {
        data: &'a [u8],
        byte_pos: usize,
        bit_buf: u64,
        bits_in_buf: u8,
    }

    impl<'a> BitReader<'a> {
        fn new(data: &'a [u8]) -> Self {
            Self {
                data,
                byte_pos: 0,
                bit_buf: 0,
                bits_in_buf: 0,
            }
        }

        fn fill(&mut self, n: u8) -> Result<(), InflateError> {
            while self.bits_in_buf < n {
                if self.byte_pos >= self.data.len() {
                    return Err(InflateError("unexpected end of input".into()));
                }
                self.bit_buf |= (self.data[self.byte_pos] as u64) << self.bits_in_buf;
                self.byte_pos += 1;
                self.bits_in_buf += 8;
            }
            Ok(())
        }

        fn read_bits(&mut self, n: u8) -> Result<u32, InflateError> {
            if n == 0 {
                return Ok(0);
            }
            self.fill(n)?;
            let mask = (1u64 << n) - 1;
            let val = (self.bit_buf & mask) as u32;
            self.bit_buf >>= n;
            self.bits_in_buf -= n;
            Ok(val)
        }

        fn align_byte(&mut self) {
            let drop = self.bits_in_buf % 8;
            self.bit_buf >>= drop;
            self.bits_in_buf -= drop;
        }

        fn read_exact_bytes(&mut self, len: usize) -> Result<&'a [u8], InflateError> {
            self.align_byte();
            while self.bits_in_buf > 0 && self.byte_pos > 0 {
                self.byte_pos -= 1;
                self.bits_in_buf -= 8;
            }
            self.bit_buf = 0;
            self.bits_in_buf = 0;
            if self.byte_pos + len > self.data.len() {
                return Err(InflateError("unexpected end in stored block".into()));
            }
            let res = &self.data[self.byte_pos..self.byte_pos + len];
            self.byte_pos += len;
            Ok(res)
        }
    }

    struct HuffmanTree {
        nodes: Vec<[u16; 2]>,
        values: Vec<Option<u16>>,
    }

    impl HuffmanTree {
        fn from_lengths(lengths: &[u8]) -> Result<Self, InflateError> {
            let mut bl_count = [0usize; 16];
            for &l in lengths {
                if l > 0 && (l as usize) < 16 {
                    bl_count[l as usize] += 1;
                }
            }
            let mut next_code = [0u32; 16];
            let mut code = 0u32;
            for bits in 1..=15 {
                code = (code + bl_count[bits - 1] as u32) << 1;
                next_code[bits] = code;
            }
            let mut tree = HuffmanTree {
                nodes: vec![[0, 0]],
                values: vec![None],
            };
            for (sym, &len) in lengths.iter().enumerate() {
                if len > 0 && len <= 15 {
                    let c = next_code[len as usize];
                    next_code[len as usize] += 1;
                    let mut node = 0usize;
                    for i in (0..len).rev() {
                        let bit = ((c >> i) & 1) as usize;
                        let mut next = tree.nodes[node][bit] as usize;
                        if next == 0 {
                            next = tree.nodes.len();
                            tree.nodes.push([0, 0]);
                            tree.values.push(None);
                            tree.nodes[node][bit] = next as u16;
                        }
                        node = next;
                    }
                    tree.values[node] = Some(sym as u16);
                }
            }
            Ok(tree)
        }

        fn decode(&self, reader: &mut BitReader) -> Result<u16, InflateError> {
            let mut node = 0usize;
            loop {
                if let Some(val) = self.values[node] {
                    return Ok(val);
                }
                let bit = reader.read_bits(1)? as usize;
                let next = self.nodes[node][bit] as usize;
                if next == 0 {
                    return Err(InflateError("invalid huffman sequence".into()));
                }
                node = next;
            }
        }
    }

    const BASE_LEN: [u16; 29] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99,
        115, 131, 163, 195, 227, 258,
    ];
    const EXTRA_LEN_BITS: [u8; 29] = [
        0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
    ];
    const BASE_DIST: [u16; 30] = [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025,
        1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
    ];
    const EXTRA_DIST_BITS: [u8; 30] = [
        0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12,
        12, 13, 13,
    ];
    const CL_ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];

    /// Decompresses raw RFC 1951 DEFLATE data.
    pub fn inflate_raw(data: &[u8]) -> Result<Vec<u8>, InflateError> {
        let mut reader = BitReader::new(data);
        let mut out = Vec::new();
        let max_len = crate::objects::MAX_INFLATE_LEN;

        loop {
            let bfinal = reader.read_bits(1)?;
            let btype = reader.read_bits(2)?;

            match btype {
                0 => {
                    reader.align_byte();
                    let len = reader.read_bits(16)? as u16;
                    let nlen = reader.read_bits(16)? as u16;
                    if len != !nlen {
                        return Err(InflateError("bad stored block len".into()));
                    }
                    let bytes = reader.read_exact_bytes(len as usize)?;
                    if out.len() + bytes.len() > max_len {
                        return Err(InflateError("decompression size limit exceeded".into()));
                    }
                    out.extend_from_slice(bytes);
                }
                1 => {
                    let mut lit_lens = [0u8; 288];
                    for i in 0..=143 {
                        lit_lens[i] = 8;
                    }
                    for i in 144..=255 {
                        lit_lens[i] = 9;
                    }
                    for i in 256..=279 {
                        lit_lens[i] = 7;
                    }
                    for i in 280..=287 {
                        lit_lens[i] = 8;
                    }
                    let dist_lens = [5u8; 32];
                    let lit_tree = HuffmanTree::from_lengths(&lit_lens)?;
                    let dist_tree = HuffmanTree::from_lengths(&dist_lens)?;
                    decode_huffman_block(&mut reader, &lit_tree, &dist_tree, &mut out, max_len)?;
                }
                2 => {
                    let hlit = (reader.read_bits(5)? + 257) as usize;
                    let hdist = (reader.read_bits(5)? + 1) as usize;
                    let hclen = (reader.read_bits(4)? + 4) as usize;

                    let mut cl_lens = [0u8; 19];
                    for i in 0..hclen {
                        cl_lens[CL_ORDER[i]] = reader.read_bits(3)? as u8;
                    }
                    let cl_tree = HuffmanTree::from_lengths(&cl_lens)?;

                    let mut lengths = Vec::with_capacity(hlit + hdist);
                    while lengths.len() < hlit + hdist {
                        let sym = cl_tree.decode(&mut reader)?;
                        match sym {
                            0..=15 => lengths.push(sym as u8),
                            16 => {
                                let last = *lengths.last().ok_or_else(|| {
                                    InflateError("repeat with no previous length".into())
                                })?;
                                let count = (reader.read_bits(2)? + 3) as usize;
                                for _ in 0..count {
                                    lengths.push(last);
                                }
                            }
                            17 => {
                                let count = (reader.read_bits(3)? + 3) as usize;
                                for _ in 0..count {
                                    lengths.push(0);
                                }
                            }
                            18 => {
                                let count = (reader.read_bits(7)? + 11) as usize;
                                for _ in 0..count {
                                    lengths.push(0);
                                }
                            }
                            _ => return Err(InflateError("invalid code length symbol".into())),
                        }
                    }
                    if lengths.len() > hlit + hdist {
                        lengths.truncate(hlit + hdist);
                    }

                    let lit_tree = HuffmanTree::from_lengths(&lengths[..hlit])?;
                    let dist_tree = HuffmanTree::from_lengths(&lengths[hlit..])?;
                    decode_huffman_block(&mut reader, &lit_tree, &dist_tree, &mut out, max_len)?;
                }
                _ => return Err(InflateError("reserved block type".into())),
            }

            if bfinal == 1 {
                break;
            }
        }

        Ok(out)
    }

    fn decode_huffman_block(
        reader: &mut BitReader,
        lit_tree: &HuffmanTree,
        dist_tree: &HuffmanTree,
        out: &mut Vec<u8>,
        max_len: usize,
    ) -> Result<(), InflateError> {
        loop {
            let sym = lit_tree.decode(reader)?;
            if sym < 256 {
                if out.len() >= max_len {
                    return Err(InflateError("decompression size limit exceeded".into()));
                }
                out.push(sym as u8);
            } else if sym == 256 {
                break;
            } else {
                let len_idx = (sym - 257) as usize;
                if len_idx >= BASE_LEN.len() {
                    return Err(InflateError("invalid length symbol".into()));
                }
                let extra_len = reader.read_bits(EXTRA_LEN_BITS[len_idx])?;
                let length = BASE_LEN[len_idx] as usize + extra_len as usize;

                let dist_sym = dist_tree.decode(reader)? as usize;
                if dist_sym >= BASE_DIST.len() {
                    return Err(InflateError("invalid distance symbol".into()));
                }
                let extra_dist = reader.read_bits(EXTRA_DIST_BITS[dist_sym])?;
                let distance = BASE_DIST[dist_sym] as usize + extra_dist as usize;

                if distance > out.len() {
                    return Err(InflateError("distance exceeds output buffer".into()));
                }
                if out.len() + length > max_len {
                    return Err(InflateError("decompression size limit exceeded".into()));
                }
                let start = out.len() - distance;
                for i in 0..length {
                    let val = out[start + i];
                    out.push(val);
                }
            }
        }
        Ok(())
    }
}

pub mod objects {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::fmt;
    use std::fs;
    use std::path::PathBuf;
    use std::rc::Rc;

    /// 256 MiB maximum guard for decompressed objects.
    pub const MAX_INFLATE_LEN: usize = 256 * 1024 * 1024;

    /// Computes a standard SHA-1 hash over raw bytes.
    pub fn sha1(data: &[u8]) -> [u8; 20] {
        let mut h = [
            0x67452301u32,
            0xEFCDAB89u32,
            0x98BADCFEu32,
            0x10325476u32,
            0xC3D2E1F0u32,
        ];
        let bit_len = (data.len() as u64) * 8;
        let mut msg = data.to_vec();
        msg.push(0x80);
        while (msg.len() % 64) != 56 {
            msg.push(0x00);
        }
        msg.extend_from_slice(&bit_len.to_be_bytes());

        for chunk in msg.chunks_exact(64) {
            let mut w = [0u32; 80];
            for (i, w_val) in w.iter_mut().enumerate().take(16) {
                *w_val = u32::from_be_bytes(chunk[i * 4..(i + 1) * 4].try_into().unwrap());
            }
            for i in 16..80 {
                w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
            }
            let mut a = h[0];
            let mut b = h[1];
            let mut c = h[2];
            let mut d = h[3];
            let mut e = h[4];
            for (i, &w_val) in w.iter().enumerate() {
                let (f, k) = match i {
                    0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                    20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                    40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                    _ => (b ^ c ^ d, 0xCA62C1D6u32),
                };
                let temp = a
                    .rotate_left(5)
                    .wrapping_add(f)
                    .wrapping_add(e)
                    .wrapping_add(k)
                    .wrapping_add(w_val);
                e = d;
                d = c;
                c = b.rotate_left(30);
                b = a;
                a = temp;
            }
            h[0] = h[0].wrapping_add(a);
            h[1] = h[1].wrapping_add(b);
            h[2] = h[2].wrapping_add(c);
            h[3] = h[3].wrapping_add(d);
            h[4] = h[4].wrapping_add(e);
        }

        let mut out = [0u8; 20];
        for (i, &val) in h.iter().enumerate() {
            out[i * 4..(i + 1) * 4].copy_from_slice(&val.to_be_bytes());
        }
        out
    }

    /// 20-byte SHA-1 Object ID.
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct Oid(pub [u8; 20]);

    impl fmt::Debug for Oid {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Oid({})", self.to_hex())
        }
    }

    impl fmt::Display for Oid {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.to_hex())
        }
    }

    impl Oid {
        /// Parses a 40-character hexadecimal string into an OID.
        pub fn from_hex(s: &str) -> Result<Oid, ObjError> {
            if s.len() != 40 {
                return Err(ObjError::Corrupt(format!("invalid hex length: {}", s.len())));
            }
            let mut bytes = [0u8; 20];
            for (i, b) in bytes.iter_mut().enumerate() {
                let high = s
                    .chars()
                    .nth(i * 2)
                    .and_then(|c| c.to_digit(16))
                    .ok_or_else(|| ObjError::Corrupt("invalid hex character".into()))? as u8;
                let low = s
                    .chars()
                    .nth(i * 2 + 1)
                    .and_then(|c| c.to_digit(16))
                    .ok_or_else(|| ObjError::Corrupt("invalid hex character".into()))? as u8;
                *b = (high << 4) | low;
            }
            Ok(Oid(bytes))
        }

        /// Returns the 40-character hexadecimal representation.
        pub fn to_hex(&self) -> String {
            let mut out = String::with_capacity(40);
            for &b in &self.0 {
                use fmt::Write;
                let _ = write!(out, "{:02x}", b);
            }
            out
        }

        /// Checks if the OID is the null/zero hash.
        pub fn is_zero(&self) -> bool {
            self.0 == [0u8; 20]
        }
    }

    /// Error encountered during Git object lookup or parsing.
    #[derive(Debug, PartialEq, Eq)]
    pub enum ObjError {
        Corrupt(String),
        NotFound(Oid),
        Unsupported(String),
        TooLarge(usize),
    }

    impl fmt::Display for ObjError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                ObjError::Corrupt(s) => write!(f, "corrupt object: {s}"),
                ObjError::NotFound(oid) => write!(f, "object not found: {oid}"),
                ObjError::Unsupported(s) => write!(f, "unsupported: {s}"),
                ObjError::TooLarge(size) => write!(f, "object too large: {size} bytes"),
            }
        }
    }

    impl std::error::Error for ObjError {}

    /// Git Commit Object.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Commit {
        pub tree: Oid,
        pub parents: Vec<Oid>,
    }

    /// Git Tree Entry.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct TreeEntry {
        pub mode: String,
        pub name: Vec<u8>,
        pub oid: Oid,
    }

    /// Representation of a parsed Git object.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Obj {
        Commit(Commit),
        Tree(Vec<TreeEntry>),
        Blob(Vec<u8>),
        Tag(Oid),
    }

    thread_local! {
        static CACHE: RefCell<HashMap<[u8; 20], Rc<Obj>>> = RefCell::new(HashMap::new());
    }

    /// Returns the filesystem path to a loose object if present.
    pub fn object_path(repo: &crate::repo::Repo, oid: &Oid) -> Option<PathBuf> {
        let hex = oid.to_hex();
        let path = repo.git_dir.join("objects").join(&hex[..2]).join(&hex[2..]);
        if path.is_file() {
            Some(path)
        } else {
            None
        }
    }

    /// Loads and parses a Git object by OID.
    pub fn load_object(repo: &crate::repo::Repo, oid: &Oid) -> Result<Obj, ObjError> {
        let cfg = crate::repo::read_config(repo).map_err(|e| ObjError::Corrupt(format!("{e}")))?;
        if cfg.sha256 {
            return Err(ObjError::Unsupported("sha256 repos are not supported".into()));
        }
        if let Some(cached) = CACHE.with(|c| c.borrow().get(&oid.0).cloned()) {
            return Ok((*cached).clone());
        }

        let raw_data = if let Some(path) = object_path(repo, oid) {
            let compressed = fs::read(path).map_err(|e| ObjError::Corrupt(format!("{e}")))?;
            crate::inflate::inflate_zlib(&compressed)
                .map_err(|e| ObjError::Corrupt(format!("zlib error: {e}")))?
        } else {
            load_from_pack(repo, oid)?
        };

        let null_pos = raw_data
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| ObjError::Corrupt("missing header null terminator".into()))?;
        let header = std::str::from_utf8(&raw_data[..null_pos])
            .map_err(|_| ObjError::Corrupt("invalid utf-8 header".into()))?;
        let mut header_parts = header.split_whitespace();
        let obj_type = header_parts
            .next()
            .ok_or_else(|| ObjError::Corrupt("missing object type in header".into()))?;
        let content = &raw_data[null_pos + 1..];

        let obj = match obj_type {
            "commit" => Obj::Commit(parse_commit(content)?),
            "tree" => Obj::Tree(parse_tree(content)?),
            "blob" => Obj::Blob(content.to_vec()),
            "tag" => Obj::Tag(parse_tag(content)?),
            other => return Err(ObjError::Unsupported(format!("unknown object type: {other}"))),
        };

        CACHE.with(|c| c.borrow_mut().insert(oid.0, Rc::new(obj.clone())));
        Ok(obj)
    }

    /// Loads a commit object.
    pub fn load_commit(repo: &crate::repo::Repo, oid: &Oid) -> Result<Commit, ObjError> {
        match load_object(repo, oid)? {
            Obj::Commit(c) => Ok(c),
            _ => Err(ObjError::Corrupt(format!("{oid} is not a commit"))),
        }
    }

    /// Loads a tree object.
    pub fn load_tree(repo: &crate::repo::Repo, oid: &Oid) -> Result<Vec<TreeEntry>, ObjError> {
        match load_object(repo, oid)? {
            Obj::Tree(t) => Ok(t),
            _ => Err(ObjError::Corrupt(format!("{oid} is not a tree"))),
        }
    }

    /// Loads a blob object.
    pub fn load_blob(repo: &crate::repo::Repo, oid: &Oid) -> Result<Vec<u8>, ObjError> {
        match load_object(repo, oid)? {
            Obj::Blob(b) => Ok(b),
            _ => Err(ObjError::Corrupt(format!("{oid} is not a blob"))),
        }
    }

    fn parse_commit(data: &[u8]) -> Result<Commit, ObjError> {
        let text = std::str::from_utf8(data)
            .map_err(|_| ObjError::Corrupt("non-utf8 commit object".into()))?;
        let mut tree = None;
        let mut parents = Vec::new();

        for line in text.lines() {
            if line.is_empty() {
                break;
            }
            if let Some(rest) = line.strip_prefix("tree ") {
                tree = Some(Oid::from_hex(rest.trim())?);
            } else if let Some(rest) = line.strip_prefix("parent ") {
                parents.push(Oid::from_hex(rest.trim())?);
            }
        }

        let tree = tree.ok_or_else(|| ObjError::Corrupt("missing tree in commit".into()))?;
        Ok(Commit { tree, parents })
    }

    fn parse_tree(data: &[u8]) -> Result<Vec<TreeEntry>, ObjError> {
        let mut entries = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let space_pos = data[i..]
                .iter()
                .position(|&b| b == b' ')
                .ok_or_else(|| ObjError::Corrupt("invalid tree entry format".into()))?
                + i;
            let mode = std::str::from_utf8(&data[i..space_pos])
                .map_err(|_| ObjError::Corrupt("invalid tree mode".into()))?
                .to_string();
            let null_pos = data[space_pos + 1..]
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| ObjError::Corrupt("invalid tree entry name".into()))?
                + space_pos
                + 1;
            let name = data[space_pos + 1..null_pos].to_vec();
            let oid_start = null_pos + 1;
            if oid_start + 20 > data.len() {
                return Err(ObjError::Corrupt("truncated tree oid".into()));
            }
            let mut oid_bytes = [0u8; 20];
            oid_bytes.copy_from_slice(&data[oid_start..oid_start + 20]);
            entries.push(TreeEntry {
                mode,
                name,
                oid: Oid(oid_bytes),
            });
            i = oid_start + 20;
        }
        Ok(entries)
    }

    fn parse_tag(data: &[u8]) -> Result<Oid, ObjError> {
        let text = std::str::from_utf8(data)
            .map_err(|_| ObjError::Corrupt("non-utf8 tag object".into()))?;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("object ") {
                return Oid::from_hex(rest.trim());
            }
        }
        Err(ObjError::Corrupt("missing target object in tag".into()))
    }

    fn load_from_pack(repo: &crate::repo::Repo, target_oid: &Oid) -> Result<Vec<u8>, ObjError> {
        let pack_dir = repo.git_dir.join("objects/pack");
        if !pack_dir.is_dir() {
            return Err(ObjError::NotFound(*target_oid));
        }

        let entries = fs::read_dir(&pack_dir).map_err(|e| ObjError::Corrupt(format!("{e}")))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("idx") {
                let pack_path = path.with_extension("pack");
                if let Ok((obj_type_str, body)) =
                    read_pack_object(&path, &pack_path, repo, target_oid)
                {
                    let mut full = Vec::new();
                    full.extend_from_slice(obj_type_str.as_bytes());
                    full.push(b' ');
                    full.extend_from_slice(body.len().to_string().as_bytes());
                    full.push(0);
                    full.extend_from_slice(&body);
                    return Ok(full);
                }
            }
        }
        Err(ObjError::NotFound(*target_oid))
    }

    fn read_pack_object(
        idx_path: &PathBuf,
        pack_path: &PathBuf,
        repo: &crate::repo::Repo,
        target_oid: &Oid,
    ) -> Result<(String, Vec<u8>), ObjError> {
        let idx_data = fs::read(idx_path).map_err(|e| ObjError::Corrupt(format!("{e}")))?;
        if idx_data.len() < 4 + 4 + 256 * 4 + 20 {
            return Err(ObjError::Corrupt("idx file too short".into()));
        }
        if &idx_data[0..4] != b"\xfftOc" || u32::from_be_bytes(idx_data[4..8].try_into().unwrap()) != 2 {
            return Err(ObjError::Unsupported("unsupported pack index format".into()));
        }

        let fanout_end = 8 + 256 * 4;
        let total_objs = u32::from_be_bytes(idx_data[fanout_end - 4..fanout_end].try_into().unwrap()) as usize;
        let oids_start = fanout_end;
        let oids_end = oids_start + total_objs * 20;
        let _crc_end = oids_end + total_objs * 4;
        let offsets_start = _crc_end;
        let offsets_end = offsets_start + total_objs * 4;

        if idx_data.len() < offsets_end {
            return Err(ObjError::Corrupt("idx file corrupted".into()));
        }

        let mut low = 0usize;
        let mut high = total_objs;
        let mut found_idx = None;
        while low < high {
            let mid = (low + high) / 2;
            let cur_oid = &idx_data[oids_start + mid * 20..oids_start + (mid + 1) * 20];
            match cur_oid.cmp(&target_oid.0) {
                std::cmp::Ordering::Equal => {
                    found_idx = Some(mid);
                    break;
                }
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
            }
        }

        let idx = found_idx.ok_or(ObjError::NotFound(*target_oid))?;
        let offset_raw = u32::from_be_bytes(
            idx_data[offsets_start + idx * 4..offsets_start + (idx + 1) * 4]
                .try_into()
                .unwrap(),
        );

        let pack_offset = if (offset_raw & 0x80000000) != 0 {
            let large_idx = (offset_raw & 0x7FFFFFFF) as usize;
            let large_start = offsets_end + large_idx * 8;
            if idx_data.len() < large_start + 8 {
                return Err(ObjError::Corrupt("invalid large offset table".into()));
            }
            u64::from_be_bytes(idx_data[large_start..large_start + 8].try_into().unwrap())
        } else {
            offset_raw as u64
        };

        let pack_data = fs::read(pack_path).map_err(|e| ObjError::Corrupt(format!("{e}")))?;
        read_pack_entry_at(&pack_data, pack_offset as usize, repo)
    }

    fn read_pack_entry_at(
        pack_data: &[u8],
        start_offset: usize,
        repo: &crate::repo::Repo,
    ) -> Result<(String, Vec<u8>), ObjError> {
        let mut pos = start_offset;
        if pos >= pack_data.len() {
            return Err(ObjError::Corrupt("pack offset out of bounds".into()));
        }
        let b = pack_data[pos];
        pos += 1;
        let obj_type = (b >> 4) & 7;
        let mut _size = (b & 0x0F) as usize;
        let mut shift = 4;
        let mut cur = b;
        while (cur & 0x80) != 0 {
            if pos >= pack_data.len() {
                return Err(ObjError::Corrupt("truncated pack object header".into()));
            }
            cur = pack_data[pos];
            pos += 1;
            _size |= ((cur & 0x7F) as usize) << shift;
            shift += 7;
        }

        match obj_type {
            1 => Ok(("commit".into(), crate::inflate::inflate_zlib(&pack_data[pos..]).map_err(|e| ObjError::Corrupt(format!("{e}")))?)),
            2 => Ok(("tree".into(), crate::inflate::inflate_zlib(&pack_data[pos..]).map_err(|e| ObjError::Corrupt(format!("{e}")))?)),
            3 => Ok(("blob".into(), crate::inflate::inflate_zlib(&pack_data[pos..]).map_err(|e| ObjError::Corrupt(format!("{e}")))?)),
            4 => Ok(("tag".into(), crate::inflate::inflate_zlib(&pack_data[pos..]).map_err(|e| ObjError::Corrupt(format!("{e}")))?)),
            6 => {
                let mut b = pack_data[pos];
                pos += 1;
                let mut delta_offset = (b & 0x7F) as usize;
                while (b & 0x80) != 0 {
                    b = pack_data[pos];
                    pos += 1;
                    delta_offset = (delta_offset + 1) << 7 | ((b & 0x7F) as usize);
                }
                if delta_offset > start_offset {
                    return Err(ObjError::Corrupt("invalid ofs-delta offset".into()));
                }
                let base_offset = start_offset - delta_offset;
                let (base_type, base_bytes) = read_pack_entry_at(pack_data, base_offset, repo)?;
                let delta_instructions = crate::inflate::inflate_zlib(&pack_data[pos..])
                    .map_err(|e| ObjError::Corrupt(format!("delta zlib: {e}")))?;
                let applied = apply_delta(&base_bytes, &delta_instructions)?;
                Ok((base_type, applied))
            }
            7 => {
                if pos + 20 > pack_data.len() {
                    return Err(ObjError::Corrupt("truncated ref-delta base oid".into()));
                }
                let mut base_oid_bytes = [0u8; 20];
                base_oid_bytes.copy_from_slice(&pack_data[pos..pos + 20]);
                pos += 20;
                let base_oid = Oid(base_oid_bytes);
                let base_obj = load_object(repo, &base_oid)?;
                let (base_type, base_bytes) = match base_obj {
                    Obj::Commit(c) => ("commit".into(), serialize_commit(&c)),
                    Obj::Tree(t) => ("tree".into(), serialize_tree(&t)),
                    Obj::Blob(b) => ("blob".into(), b),
                    Obj::Tag(t) => ("tag".into(), format!("object {}\n", t.to_hex()).into_bytes()),
                };
                let delta_instructions = crate::inflate::inflate_zlib(&pack_data[pos..])
                    .map_err(|e| ObjError::Corrupt(format!("delta zlib: {e}")))?;
                let applied = apply_delta(&base_bytes, &delta_instructions)?;
                Ok((base_type, applied))
            }
            _ => Err(ObjError::Unsupported(format!("unknown pack entry type: {obj_type}"))),
        }
    }

    fn serialize_commit(c: &Commit) -> Vec<u8> {
        let mut s = format!("tree {}\n", c.tree.to_hex());
        for p in &c.parents {
            s.push_str(&format!("parent {}\n", p.to_hex()));
        }
        s.into_bytes()
    }

    fn serialize_tree(entries: &[TreeEntry]) -> Vec<u8> {
        let mut out = Vec::new();
        for e in entries {
            out.extend_from_slice(e.mode.as_bytes());
            out.push(b' ');
            out.extend_from_slice(&e.name);
            out.push(0);
            out.extend_from_slice(&e.oid.0);
        }
        out
    }

    fn apply_delta(base: &[u8], delta: &[u8]) -> Result<Vec<u8>, ObjError> {
        let mut pos = 0;
        loop {
            if pos >= delta.len() {
                return Err(ObjError::Corrupt("truncated delta header".into()));
            }
            let b = delta[pos];
            pos += 1;
            if (b & 0x80) == 0 {
                break;
            }
        }

        let mut shift = 0;
        let mut result_size = 0usize;
        loop {
            if pos >= delta.len() {
                return Err(ObjError::Corrupt("truncated delta header".into()));
            }
            let b = delta[pos];
            pos += 1;
            result_size |= ((b & 0x7F) as usize) << shift;
            shift += 7;
            if (b & 0x80) == 0 {
                break;
            }
        }

        let mut out = Vec::with_capacity(result_size);
        while pos < delta.len() {
            let cmd = delta[pos];
            pos += 1;
            if (cmd & 0x80) != 0 {
                let mut cp_off = 0usize;
                let mut cp_len = 0usize;
                if (cmd & 0x01) != 0 {
                    cp_off |= delta[pos] as usize;
                    pos += 1;
                }
                if (cmd & 0x02) != 0 {
                    cp_off |= (delta[pos] as usize) << 8;
                    pos += 1;
                }
                if (cmd & 0x04) != 0 {
                    cp_off |= (delta[pos] as usize) << 16;
                    pos += 1;
                }
                if (cmd & 0x08) != 0 {
                    cp_off |= (delta[pos] as usize) << 24;
                    pos += 1;
                }
                if (cmd & 0x10) != 0 {
                    cp_len |= delta[pos] as usize;
                    pos += 1;
                }
                if (cmd & 0x20) != 0 {
                    cp_len |= (delta[pos] as usize) << 8;
                    pos += 1;
                }
                if (cmd & 0x40) != 0 {
                    cp_len |= (delta[pos] as usize) << 16;
                    pos += 1;
                }
                if cp_len == 0 {
                    cp_len = 0x10000;
                }
                if cp_off + cp_len > base.len() {
                    return Err(ObjError::Corrupt("delta copy range outside base".into()));
                }
                out.extend_from_slice(&base[cp_off..cp_off + cp_len]);
            } else if cmd > 0 {
                let len = cmd as usize;
                if pos + len > delta.len() {
                    return Err(ObjError::Corrupt("truncated delta data insert".into()));
                }
                out.extend_from_slice(&delta[pos..pos + len]);
                pos += len;
            } else {
                return Err(ObjError::Corrupt("invalid zero delta opcode".into()));
            }
        }

        Ok(out)
    }
}

pub mod repo {
    use std::collections::HashMap;
    use std::fmt;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};

    /// Represents a discovered Git repository.
    #[derive(Clone, Debug)]
    pub struct Repo {
        pub work_dir: PathBuf,
        pub git_dir: PathBuf,
    }

    /// Error during repository discovery or configuration reading.
    #[derive(Debug)]
    pub enum RepoError {
        NotARepository,
        Io(io::Error),
        Parse(String),
    }

    impl fmt::Display for RepoError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                RepoError::NotARepository => write!(f, "fatal: not a git repository"),
                RepoError::Io(e) => write!(f, "repository io error: {e}"),
                RepoError::Parse(s) => write!(f, "repository parse error: {s}"),
            }
        }
    }

    impl std::error::Error for RepoError {}

    /// Current HEAD state of a repository.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum HeadRef {
        Branch(String),
        Detached(crate::objects::Oid),
        Unborn,
    }

    /// Git configuration parsed from `.git/config`.
    #[derive(Clone, Debug, Default)]
    pub struct Config {
        pub bare: bool,
        pub sha256: bool,
        pub branch_merge: HashMap<String, String>,
        pub branch_remote: HashMap<String, String>,
        pub remotes: Vec<String>,
        pub protected: Vec<String>,
        pub base_branch: Option<String>,
        pub leak_ignore_paths: Vec<String>,
    }

    /// Git Reference.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Ref {
        pub name: String,
        pub oid: crate::objects::Oid,
    }

    /// Finds a Git repository starting from a given directory and walking up.
    pub fn find_repo_from(start: &Path) -> Result<Repo, RepoError> {
        let mut cur = if start.is_file() {
            start.parent().unwrap_or(start).to_path_buf()
        } else {
            start.to_path_buf()
        };

        loop {
            let git_entry = cur.join(".git");
            if git_entry.is_dir() {
                return Ok(Repo {
                    work_dir: cur,
                    git_dir: git_entry,
                });
            } else if git_entry.is_file() {
                let content = fs::read_to_string(&git_entry).map_err(RepoError::Io)?;
                if let Some(rest) = content.trim().strip_prefix("gitdir:") {
                    let gitdir_path = rest.trim();
                    let resolved = if Path::new(gitdir_path).is_absolute() {
                        PathBuf::from(gitdir_path)
                    } else {
                        cur.join(gitdir_path)
                    };
                    return Ok(Repo {
                        work_dir: cur,
                        git_dir: resolved,
                    });
                }
            }

            if !cur.pop() {
                break;
            }
        }

        Err(RepoError::NotARepository)
    }

    /// Parses configuration from `.git/config`.
    pub fn read_config(repo: &Repo) -> Result<Config, RepoError> {
        let config_path = repo.git_dir.join("config");
        let mut cfg = Config {
            protected: vec!["main".into(), "master".into(), "develop".into()],
            ..Default::default()
        };

        if !config_path.is_file() {
            return Ok(cfg);
        }

        let content = fs::read_to_string(config_path).map_err(RepoError::Io)?;
        let mut current_section = String::new();
        let mut current_subsection = String::new();

        let mut lines = content.lines().peekable();
        while let Some(line) = lines.next() {
            let mut line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            let mut full_line = line.to_string();
            while full_line.ends_with('\\') {
                full_line.pop();
                if let Some(next_l) = lines.next() {
                    full_line.push_str(next_l.trim());
                }
            }
            line = full_line.trim();

            if line.starts_with('[') && line.ends_with(']') {
                let inner = &line[1..line.len() - 1].trim();
                if let Some(quote_start) = inner.find('"') {
                    let quote_end = inner.rfind('"').unwrap_or(inner.len());
                    current_section = inner[..quote_start].trim().to_ascii_lowercase();
                    current_subsection = inner[quote_start + 1..quote_end].to_string();
                } else {
                    current_section = inner.to_ascii_lowercase();
                    current_subsection.clear();
                }
                continue;
            }

            if let Some((k, v)) = line.split_once('=') {
                let key = k.trim().to_ascii_lowercase();
                let val = unquote(v.trim());
                match (current_section.as_str(), current_subsection.as_str()) {
                    ("core", "") => {
                        if key == "bare" {
                            cfg.bare = val == "true";
                        }
                    }
                    ("extensions", "") => {
                        if key == "objectformat" && val.eq_ignore_ascii_case("sha256") {
                            cfg.sha256 = true;
                        }
                    }
                    ("branch", b) if !b.is_empty() => {
                        if key == "merge" {
                            cfg.branch_merge.insert(b.to_string(), val.to_string());
                        } else if key == "remote" {
                            cfg.branch_remote.insert(b.to_string(), val.to_string());
                        }
                    }
                    ("remote", r) if !r.is_empty() => {
                        if !cfg.remotes.iter().any(|existing| existing == r) {
                            cfg.remotes.push(r.to_string());
                        }
                    }
                    ("git-janitor", "") => {
                        if key == "protected" {
                            cfg.protected = val
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                        } else if key == "base" {
                            cfg.base_branch = Some(val.to_string());
                        } else if key == "leakignore" {
                            cfg.leak_ignore_paths.push(val.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(cfg)
    }

    fn unquote(s: &str) -> String {
        let trimmed = s.trim();
        if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
            let inner = &trimmed[1..trimmed.len() - 1];
            let mut out = String::new();
            let mut chars = inner.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(next) = chars.next() {
                        out.push(next);
                    }
                } else {
                    out.push(c);
                }
            }
            out
        } else {
            trimmed.to_string()
        }
    }

    /// Resolves the current HEAD reference.
    pub fn head(repo: &Repo) -> Result<HeadRef, RepoError> {
        let head_path = repo.git_dir.join("HEAD");
        if !head_path.is_file() {
            return Ok(HeadRef::Unborn);
        }
        let content = fs::read_to_string(head_path).map_err(RepoError::Io)?;
        let trimmed = content.trim();

        if let Some(rest) = trimmed.strip_prefix("ref:") {
            let ref_name = rest.trim();
            if let Some(branch_name) = ref_name.strip_prefix("refs/heads/") {
                if oid_of_ref(repo, ref_name).is_ok() {
                    Ok(HeadRef::Branch(branch_name.to_string()))
                } else {
                    Ok(HeadRef::Unborn)
                }
            } else {
                Ok(HeadRef::Branch(ref_name.to_string()))
            }
        } else if let Ok(oid) = crate::objects::Oid::from_hex(trimmed) {
            Ok(HeadRef::Detached(oid))
        } else {
            Ok(HeadRef::Unborn)
        }
    }

    /// Lists all local branches sorted by name without duplicates.
    pub fn local_branches(repo: &Repo) -> Result<Vec<Ref>, RepoError> {
        let mut map = HashMap::new();

        let packed_path = repo.git_dir.join("packed-refs");
        if packed_path.is_file() {
            if let Ok(content) = fs::read_to_string(packed_path) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') || line.starts_with('^') {
                        continue;
                    }
                    if let Some((oid_str, ref_name)) = line.split_once(' ') {
                        if let Some(branch_name) = ref_name.strip_prefix("refs/heads/") {
                            if let Ok(oid) = crate::objects::Oid::from_hex(oid_str.trim()) {
                                map.insert(branch_name.to_string(), oid);
                            }
                        }
                    }
                }
            }
        }

        let heads_dir = repo.git_dir.join("refs/heads");
        if heads_dir.is_dir() {
            collect_loose_refs(&heads_dir, "", &mut map)?;
        }

        let mut list: Vec<Ref> = map
            .into_iter()
            .map(|(name, oid)| Ref { name, oid })
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    fn collect_loose_refs(
        dir: &Path,
        prefix: &str,
        map: &mut HashMap<String, crate::objects::Oid>,
    ) -> Result<(), RepoError> {
        for entry in fs::read_dir(dir).map_err(RepoError::Io)?.flatten() {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            let rel_name = if prefix.is_empty() {
                file_name
            } else {
                format!("{prefix}/{file_name}")
            };
            if path.is_dir() {
                collect_loose_refs(&path, &rel_name, map)?;
            } else if path.is_file() {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(oid) = crate::objects::Oid::from_hex(content.trim()) {
                        map.insert(rel_name, oid);
                    }
                }
            }
        }
        Ok(())
    }

    /// Resolves the OID of a remote tracking branch.
    pub fn remote_branch_oid(
        repo: &Repo,
        remote: &str,
        branch: &str,
    ) -> Result<Option<crate::objects::Oid>, RepoError> {
        let ref_name = format!("refs/remotes/{remote}/{branch}");
        oid_of_ref(repo, &ref_name).map(Some).or(Ok(None))
    }

    /// Resolves the upstream commit OID for a local branch.
    pub fn upstream_oid(
        repo: &Repo,
        cfg: &Config,
        branch: &str,
    ) -> Result<Option<crate::objects::Oid>, RepoError> {
        let remote = match cfg.branch_remote.get(branch) {
            Some(r) => r,
            None => return Ok(None),
        };
        let merge = match cfg.branch_merge.get(branch) {
            Some(m) => m,
            None => return Ok(None),
        };
        let remote_branch = merge.strip_prefix("refs/heads/").unwrap_or(merge);
        remote_branch_oid(repo, remote, remote_branch)
    }

    /// Resolves the OID of any reference path.
    pub fn oid_of_ref(repo: &Repo, name: &str) -> Result<crate::objects::Oid, RepoError> {
        let loose_path = repo.git_dir.join(name);
        if loose_path.is_file() {
            let content = fs::read_to_string(loose_path).map_err(RepoError::Io)?;
            return crate::objects::Oid::from_hex(content.trim())
                .map_err(|e| RepoError::Parse(format!("{e}")));
        }

        let packed_path = repo.git_dir.join("packed-refs");
        if packed_path.is_file() {
            let content = fs::read_to_string(packed_path).map_err(RepoError::Io)?;
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') || line.starts_with('^') {
                    continue;
                }
                if let Some((oid_str, ref_name)) = line.split_once(' ') {
                    if ref_name.trim() == name {
                        return crate::objects::Oid::from_hex(oid_str.trim())
                            .map_err(|e| RepoError::Parse(format!("{e}")));
                    }
                }
            }
        }

        Err(RepoError::Parse(format!("reference not found: {name}")))
    }

    /// Resolves a refish string (branch name, HEAD, short/full OID, HEAD~n).
    pub fn resolve_refish(repo: &Repo, s: &str) -> Result<crate::objects::Oid, RepoError> {
        if s == "HEAD" {
            return match head(repo)? {
                HeadRef::Branch(b) => oid_of_ref(repo, &format!("refs/heads/{b}")),
                HeadRef::Detached(oid) => Ok(oid),
                HeadRef::Unborn => Err(RepoError::Parse("HEAD is unborn".into())),
            };
        }

        if let Some(rest) = s.strip_prefix("HEAD~") {
            let n: usize = rest
                .parse()
                .map_err(|_| RepoError::Parse(format!("invalid HEAD~ count: {rest}")))?;
            let mut cur_oid = resolve_refish(repo, "HEAD")?;
            for _ in 0..n {
                let commit = crate::objects::load_commit(repo, &cur_oid)
                    .map_err(|e| RepoError::Parse(format!("{e}")))?;
                cur_oid = *commit
                    .parents
                    .first()
                    .ok_or_else(|| RepoError::Parse("cannot traverse past root commit".into()))?;
            }
            return Ok(cur_oid);
        }

        if s.len() == 40 {
            if let Ok(oid) = crate::objects::Oid::from_hex(s) {
                return Ok(oid);
            }
        }

        if s.len() >= 7 && s.len() < 40 && s.chars().all(|c| c.is_ascii_hexdigit()) {
            let obj_root = repo.git_dir.join("objects");
            if let Ok(rd) = std::fs::read_dir(&obj_root) {
                'outer: for bucket in rd.flatten() {
                    let bpath = bucket.path();
                    if !bpath.is_dir() { continue; }
                    if let Ok(objs) = std::fs::read_dir(&bpath) {
                        for obj in objs.flatten() {
                            let opath = obj.path();
                            if !opath.is_file() { continue; }
                            let dir_name = bucket.file_name().to_string_lossy().to_string();
                            let file_name = obj.file_name().to_string_lossy().to_string();
                            if dir_name.len() == 2 && file_name.len() == 38 {
                                let hex_full = format!("{dir_name}{file_name}");
                                if hex_full.starts_with(s) {
                                    if let Ok(oid) = crate::objects::Oid::from_hex(&hex_full) {
                                        return Ok(oid);
                                    }
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Ok(oid) = oid_of_ref(repo, &format!("refs/heads/{s}")) {
            return Ok(oid);
        }
        if let Ok(oid) = oid_of_ref(repo, &format!("refs/remotes/{s}")) {
            return Ok(oid);
        }
        if let Ok(oid) = oid_of_ref(repo, &format!("refs/tags/{s}")) {
            return Ok(oid);
        }
        if let Ok(oid) = oid_of_ref(repo, s) {
            return Ok(oid);
        }

        Err(RepoError::Parse(format!("unable to resolve refish: {s}")))
    }

    /// Deletes a local branch reference from loose files and packed-refs.
    pub fn delete_local_branch(repo: &Repo, name: &str) -> Result<(), RepoError> {
        let loose_file = repo.git_dir.join("refs/heads").join(name);
        if loose_file.is_file() {
            let _ = fs::remove_file(loose_file);
        }

        let reflog_file = repo.git_dir.join("logs/refs/heads").join(name);
        if reflog_file.is_file() {
            let _ = fs::remove_file(reflog_file);
        }

        let packed_path = repo.git_dir.join("packed-refs");
        if packed_path.is_file() {
            let content = fs::read_to_string(&packed_path).map_err(RepoError::Io)?;
            let mut new_lines = Vec::new();
            let target_ref = format!("refs/heads/{name}");
            let mut skip_next_peeled = false;

            for line in content.lines() {
                if skip_next_peeled && line.trim().starts_with('^') {
                    skip_next_peeled = false;
                    continue;
                }
                skip_next_peeled = false;

                if let Some((_oid, ref_name)) = line.trim().split_once(' ') {
                    if ref_name.trim() == target_ref {
                        skip_next_peeled = true;
                        continue;
                    }
                }
                new_lines.push(line);
            }

            let temp_packed = repo.git_dir.join("packed-refs.tmp");
            fs::write(&temp_packed, new_lines.join("\n") + "\n").map_err(RepoError::Io)?;
            fs::rename(temp_packed, packed_path).map_err(RepoError::Io)?;
        }

        Ok(())
    }

    /// Determines the repository default branch name.
    pub fn default_branch(repo: &Repo) -> Result<Option<String>, RepoError> {
        if let HeadRef::Branch(name) = head(repo)? {
            return Ok(Some(name));
        }
        let branches = local_branches(repo)?;
        for candidate in ["main", "master"] {
            if branches.iter().any(|b| b.name == candidate) {
                return Ok(Some(candidate.to_string()));
            }
        }
        Ok(branches.first().map(|b| b.name.clone()))
    }
}

pub mod index {
    use std::fmt;
    use std::fs;
    use std::io;

    /// Entry inside the Git index file.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct IndexEntry {
        pub path: String,
        pub mode: u32,
        pub oid: crate::objects::Oid,
        pub stage: u8,
    }

    /// Error encountered when reading `.git/index`.
    #[derive(Debug)]
    pub enum IndexError {
        Io(io::Error),
        Corrupt(String),
        Missing,
        Unsupported(String),
    }

    impl fmt::Display for IndexError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                IndexError::Io(e) => write!(f, "index io error: {e}"),
                IndexError::Corrupt(s) => write!(f, "corrupt index: {s}"),
                IndexError::Missing => write!(f, "index file missing"),
                IndexError::Unsupported(s) => write!(f, "unsupported index: {s}"),
            }
        }
    }

    impl std::error::Error for IndexError {}

    /// Reads staged entries from `.git/index`.
    pub fn staged_entries(repo: &crate::repo::Repo) -> Result<Vec<IndexEntry>, IndexError> {
        let index_path = repo.git_dir.join("index");
        if !index_path.is_file() {
            return Ok(Vec::new());
        }

        let data = fs::read(index_path).map_err(IndexError::Io)?;
        if data.len() < 12 + 20 {
            return Err(IndexError::Corrupt("index file too short".into()));
        }

        if &data[0..4] != b"DIRC" {
            return Err(IndexError::Corrupt("invalid index signature".into()));
        }

        let version = u32::from_be_bytes(data[4..8].try_into().unwrap());
        if version != 2 && version != 3 && version != 4 {
            return Err(IndexError::Unsupported(format!("unsupported index version: {version}")));
        }

        let num_entries = u32::from_be_bytes(data[8..12].try_into().unwrap()) as usize;
        let mut entries = Vec::with_capacity(num_entries);
        let mut pos = 12;
        let mut prev_path = String::new();

        for _ in 0..num_entries {
            if pos + 62 > data.len() {
                return Err(IndexError::Corrupt("truncated index entry header".into()));
            }
            let entry_start = pos;
            let mode = u32::from_be_bytes(data[pos + 24..pos + 28].try_into().unwrap());
            let mut oid_bytes = [0u8; 20];
            oid_bytes.copy_from_slice(&data[pos + 40..pos + 60]);
            let flags = u16::from_be_bytes(data[pos + 60..pos + 62].try_into().unwrap());
            let stage = ((flags >> 12) & 0x3) as u8;
            let extended = (flags & 0x4000) != 0;
            pos += 62;

            let mut skip_worktree = false;
            if extended && (version == 3 || version == 4) {
                if pos + 2 > data.len() {
                    return Err(IndexError::Corrupt("truncated extended flags".into()));
                }
                let ext_flags = u16::from_be_bytes(data[pos..pos + 2].try_into().unwrap());
                skip_worktree = (ext_flags & 0x4000) != 0;
                pos += 2;
            }

            let path_str = if version == 4 {
                let mut strip_len = 0usize;
                loop {
                    if pos >= data.len() {
                        return Err(IndexError::Corrupt("truncated prefix varint".into()));
                    }
                    let b = data[pos];
                    pos += 1;
                    strip_len = (strip_len << 7) | ((b & 0x7F) as usize);
                    if (b & 0x80) == 0 {
                        break;
                    }
                }
                let null_pos = data[pos..]
                    .iter()
                    .position(|&b| b == 0)
                    .ok_or_else(|| IndexError::Corrupt("missing null terminator in v4 path".into()))?
                    + pos;
                let suffix = std::str::from_utf8(&data[pos..null_pos])
                    .map_err(|_| IndexError::Corrupt("non-utf8 path in index".into()))?;
                pos = null_pos + 1;

                if strip_len > prev_path.len() {
                    return Err(IndexError::Corrupt("v4 path prefix strip out of range".into()));
                }
                let base = &prev_path[..prev_path.len() - strip_len];
                let full_path = format!("{base}{suffix}");
                prev_path = full_path.clone();
                full_path
            } else {
                let null_pos = data[pos..]
                    .iter()
                    .position(|&b| b == 0)
                    .ok_or_else(|| IndexError::Corrupt("missing null terminator in path".into()))?
                    + pos;
                let path = std::str::from_utf8(&data[pos..null_pos])
                    .map_err(|_| IndexError::Corrupt("non-utf8 path in index".into()))?
                    .to_string();
                let entry_len = (null_pos + 1) - entry_start;
                let pad = (8 - (entry_len % 8)) % 8;
                pos = null_pos + 1 + pad;
                prev_path = path.clone();
                path
            };

            if !skip_worktree {
                entries.push(IndexEntry {
                    path: path_str,
                    mode,
                    oid: crate::objects::Oid(oid_bytes),
                    stage,
                });
            }
        }

        Ok(entries)
    }
}

pub mod graph {
    use std::collections::{HashSet, VecDeque};

    /// Checks whether `target` commit is reachable from `from` commit.
    pub fn is_reachable(
        repo: &crate::repo::Repo,
        from: &crate::objects::Oid,
        target: &crate::objects::Oid,
    ) -> Result<bool, crate::objects::ObjError> {
        if from == target {
            return Ok(true);
        }
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();

        queue.push_back(*from);
        visited.insert(from.0);

        while let Some(cur) = queue.pop_front() {
            let commit = crate::objects::load_commit(repo, &cur)?;
            for parent in commit.parents {
                if parent == *target {
                    return Ok(true);
                }
                if visited.insert(parent.0) {
                    queue.push_back(parent);
                }
            }
        }

        Ok(false)
    }

    /// Counts commits reachable from `left` that are not reachable from `right`.
    pub fn commits_only_in(
        repo: &crate::repo::Repo,
        left: &crate::objects::Oid,
        right: &crate::objects::Oid,
    ) -> Result<usize, crate::objects::ObjError> {
        if left == right {
            return Ok(0);
        }

        let mut right_set = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(*right);
        right_set.insert(right.0);
        while let Some(cur) = queue.pop_front() {
            if let Ok(commit) = crate::objects::load_commit(repo, &cur) {
                for parent in commit.parents {
                    if right_set.insert(parent.0) {
                        queue.push_back(parent);
                    }
                }
            }
        }

        let mut count = 0;
        let mut left_visited = HashSet::new();
        let mut left_queue = VecDeque::new();
        left_queue.push_back(*left);
        left_visited.insert(left.0);

        while let Some(cur) = left_queue.pop_front() {
            if right_set.contains(&cur.0) {
                continue;
            }
            count += 1;
            let commit = crate::objects::load_commit(repo, &cur)?;
            for parent in commit.parents {
                if !right_set.contains(&parent.0) && left_visited.insert(parent.0) {
                    left_queue.push_back(parent);
                }
            }
        }

        Ok(count)
    }

    /// Lists all file paths contained within a commit.
    pub fn files_in_commit(
        repo: &crate::repo::Repo,
        oid: &crate::objects::Oid,
    ) -> Result<Vec<String>, crate::objects::ObjError> {
        let commit = crate::objects::load_commit(repo, oid)?;
        let mut files = Vec::new();
        walk_tree(repo, &commit.tree, "", &mut files)?;
        files.sort();
        Ok(files)
    }

    fn walk_tree(
        repo: &crate::repo::Repo,
        tree_oid: &crate::objects::Oid,
        prefix: &str,
        files: &mut Vec<String>,
    ) -> Result<(), crate::objects::ObjError> {
        let entries = crate::objects::load_tree(repo, tree_oid)?;
        for entry in entries {
            let name_str = String::from_utf8_lossy(&entry.name);
            let path = if prefix.is_empty() {
                name_str.to_string()
            } else {
                format!("{prefix}/{name_str}")
            };
            if entry.mode == "040000" || entry.mode == "40000" {
                walk_tree(repo, &entry.oid, &path, files)?;
            } else {
                files.push(path);
            }
        }
        Ok(())
    }

    /// Recursively computes changed files between two commits.
    pub fn changed_files_between(
        repo: &crate::repo::Repo,
        from: &crate::objects::Oid,
        to: &crate::objects::Oid,
    ) -> Result<Vec<String>, crate::objects::ObjError> {
        let from_commit = crate::objects::load_commit(repo, from)?;
        let to_commit = crate::objects::load_commit(repo, to)?;
        let mut out = HashSet::new();
        diff_trees(repo, &from_commit.tree, &to_commit.tree, "", &mut out)?;
        let mut res: Vec<String> = out.into_iter().collect();
        res.sort();
        Ok(res)
    }

    fn diff_trees(
        repo: &crate::repo::Repo,
        from_tree: &crate::objects::Oid,
        to_tree: &crate::objects::Oid,
        prefix: &str,
        out: &mut HashSet<String>,
    ) -> Result<(), crate::objects::ObjError> {
        if from_tree == to_tree {
            return Ok(());
        }

        let from_entries = crate::objects::load_tree(repo, from_tree)?;
        let to_entries = crate::objects::load_tree(repo, to_tree)?;

        let mut from_map = std::collections::HashMap::new();
        for e in from_entries {
            from_map.insert(e.name.clone(), e);
        }

        let mut to_map = std::collections::HashMap::new();
        for e in to_entries {
            to_map.insert(e.name.clone(), e);
        }

        for (name, from_e) in &from_map {
            let name_str = String::from_utf8_lossy(name);
            let path = if prefix.is_empty() {
                name_str.to_string()
            } else {
                format!("{prefix}/{name_str}")
            };

            match to_map.get(name) {
                Some(to_e) => {
                    if from_e.oid != to_e.oid || from_e.mode != to_e.mode {
                        let from_is_tree = from_e.mode == "040000" || from_e.mode == "40000";
                        let to_is_tree = to_e.mode == "040000" || to_e.mode == "40000";
                        if from_is_tree && to_is_tree {
                            diff_trees(repo, &from_e.oid, &to_e.oid, &path, out)?;
                        } else {
                            out.insert(path);
                        }
                    }
                }
                None => {
                    let from_is_tree = from_e.mode == "040000" || from_e.mode == "40000";
                    if from_is_tree {
                        let mut sub = Vec::new();
                        walk_tree(repo, &from_e.oid, &path, &mut sub)?;
                        out.extend(sub);
                    } else {
                        out.insert(path);
                    }
                }
            }
        }

        for (name, to_e) in &to_map {
            if !from_map.contains_key(name) {
                let name_str = String::from_utf8_lossy(name);
                let path = if prefix.is_empty() {
                    name_str.to_string()
                } else {
                    format!("{prefix}/{name_str}")
                };
                let to_is_tree = to_e.mode == "040000" || to_e.mode == "40000";
                if to_is_tree {
                    let mut sub = Vec::new();
                    walk_tree(repo, &to_e.oid, &path, &mut sub)?;
                    out.extend(sub);
                } else {
                    out.insert(path);
                }
            }
        }

        Ok(())
    }
}

pub mod output {
    use std::io::IsTerminal;

    pub const GREEN: u8 = 32;
    pub const RED: u8 = 31;
    pub const YELLOW: u8 = 33;
    pub const CYAN: u8 = 36;
    pub const BOLD: u8 = 1;

    /// Checks if stdout is attached to a terminal and NO_COLOR is not set.
    pub fn is_tty() -> bool {
        std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
    }

    /// Wraps text with ANSI escape sequence when running on a TTY.
    pub fn paint(s: &str, code: u8) -> String {
        if is_tty() {
            format!("\x1b[{}m{}\x1b[0m", code, s)
        } else {
            s.to_string()
        }
    }

    /// Escapes a string for JSON output.
    pub fn json_escape(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{0008}' => out.push_str("\\b"),
                '\u{000C}' => out.push_str("\\f"),
                c if (c as u32) < 0x20 => {
                    use std::fmt::Write;
                    let _ = write!(out, "\\u{:04x}", c as u32);
                }
                c => out.push(c),
            }
        }
        out
    }

    /// Redacts sensitive secret values.
    pub fn redact(s: &str) -> String {
        let chars: Vec<char> = s.chars().collect();
        if chars.len() <= 8 {
            "…".to_string()
        } else {
            let start: String = chars[..4].iter().collect();
            let end: String = chars[chars.len() - 4..].iter().collect();
            format!("{start}…{end}")
        }
    }

    /// Formats current UTC timestamp for reports.
    pub fn now_label() -> String {
        let d = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("timestamp: {d}")
    }
}

pub mod cli {
    use std::fmt;

    /// Output format for scanner and CLI results.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Format {
        Human,
        Json,
    }

    /// Options for secret scanner execution.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct ScanOpts {
        pub staged: bool,
        pub since: Option<String>,
        pub commit: Option<String>,
        pub format: Format,
    }

    /// CLI Subcommands.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Command {
        BranchList { base: Option<String> },
        BranchClean { base: Option<String>, apply: bool },
        SecretsScan(ScanOpts),
        InstallHook { force: bool },
        Doctor,
        Help,
        Version,
    }

    /// Error during argument parsing.
    #[derive(Debug, PartialEq, Eq)]
    pub enum CliError {
        Unknown(String),
        Missing(String),
        Parse(String),
    }

    impl fmt::Display for CliError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                CliError::Unknown(s) => write!(f, "unknown command or flag: {s}"),
                CliError::Missing(s) => write!(f, "missing required value for: {s}"),
                CliError::Parse(s) => write!(f, "cli parse error: {s}"),
            }
        }
    }

    impl std::error::Error for CliError {}

    /// Returns help / usage text.
    pub fn usage() -> &'static str {
        r#"git-janitor (git-jan) — zero-dependency git housekeeping and secrets scanner

USAGE:
    git-jan <COMMAND> [OPTIONS]

COMMANDS:
    branch list [--base <branch>]
        List local branches and their merged/upstream status.

    branch clean [--base <branch>] [--apply]
        Delete merged local branches with tracking upstream (dry-run by default).

    secrets scan [--staged] [--since <ref>] [--commit <ref>] [--format human|json]
        Scan repository for secret tokens.

    install-hook [--force]
        Install git pre-commit hook into .git/hooks/pre-commit.

    doctor
        Run repository diagnostics.

    help, --help
        Print this help message.

    version, --version
        Print version information.
"#
    }

    /// Parses command line arguments into a `Command`.
    pub fn parse<I: Iterator<Item = String>>(args: I) -> Result<Command, CliError> {
        let mut iter = args.skip(1);
        let first = match iter.next() {
            Some(arg) => arg,
            None => return Ok(Command::Help),
        };

        let subcmd = if first == "jan" || first == "janitor" {
            match iter.next() {
                Some(arg) => arg,
                None => return Ok(Command::Help),
            }
        } else {
            first
        };

        match subcmd.as_str() {
            "help" | "--help" | "-h" => Ok(Command::Help),
            "version" | "--version" | "-v" => Ok(Command::Version),
            "doctor" => Ok(Command::Doctor),
            "install-hook" => {
                let mut force = false;
                for arg in iter {
                    if arg == "--force" || arg == "-f" {
                        force = true;
                    } else {
                        return Err(CliError::Unknown(arg));
                    }
                }
                Ok(Command::InstallHook { force })
            }
            "branch" => {
                let action = iter
                    .next()
                    .ok_or_else(|| CliError::Missing("branch subcommand (list or clean)".into()))?;
                let mut base = None;
                let mut apply = false;
                while let Some(arg) = iter.next() {
                    if arg == "--base" {
                        base = Some(
                            iter.next()
                                .ok_or_else(|| CliError::Missing("--base branch name".into()))?,
                        );
                    } else if arg == "--apply" {
                        apply = true;
                    } else {
                        return Err(CliError::Unknown(arg));
                    }
                }
                match action.as_str() {
                    "list" => Ok(Command::BranchList { base }),
                    "clean" => Ok(Command::BranchClean { base, apply }),
                    _ => Err(CliError::Unknown(format!("branch {action}"))),
                }
            }
            "secrets" => {
                let action = iter
                    .next()
                    .ok_or_else(|| CliError::Missing("secrets subcommand (scan)".into()))?;
                if action != "scan" {
                    return Err(CliError::Unknown(format!("secrets {action}")));
                }
                let mut staged = false;
                let mut since = None;
                let mut commit = None;
                let mut format = Format::Human;

                while let Some(arg) = iter.next() {
                    if arg == "--staged" {
                        staged = true;
                    } else if arg == "--since" {
                        since = Some(
                            iter.next()
                                .ok_or_else(|| CliError::Missing("--since ref".into()))?,
                        );
                    } else if arg == "--commit" {
                        commit = Some(
                            iter.next()
                                .ok_or_else(|| CliError::Missing("--commit ref".into()))?,
                        );
                    } else if arg == "--format" {
                        let fmt_val = iter
                            .next()
                            .ok_or_else(|| CliError::Missing("--format value".into()))?;
                        if fmt_val == "json" {
                            format = Format::Json;
                        } else if fmt_val == "human" {
                            format = Format::Human;
                        } else {
                            return Err(CliError::Parse(format!("unknown format: {fmt_val}")));
                        }
                    } else {
                        return Err(CliError::Unknown(arg));
                    }
                }

                let active_targets = (staged as usize)
                    + (since.is_some() as usize)
                    + (commit.is_some() as usize);
                if active_targets > 1 {
                    return Err(CliError::Parse(
                        "--staged, --since, and --commit are mutually exclusive".into(),
                    ));
                }

                Ok(Command::SecretsScan(ScanOpts {
                    staged,
                    since,
                    commit,
                    format,
                }))
            }
            _ => Err(CliError::Unknown(subcmd)),
        }
    }
}

pub mod branch {
    use std::fmt;

    /// Health and status analysis of a local branch.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct BranchInfo {
        pub name: String,
        pub oid: crate::objects::Oid,
        pub merged: bool,
        pub ahead: usize,
        pub behind: usize,
        pub has_upstream: bool,
        pub protected: bool,
        pub current: bool,
    }

    /// Error during branch operations.
    #[derive(Debug)]
    pub enum BranchError {
        Repo(crate::repo::RepoError),
        Obj(crate::objects::ObjError),
        Refuse(String),
    }

    impl fmt::Display for BranchError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                BranchError::Repo(e) => write!(f, "repo error: {e}"),
                BranchError::Obj(e) => write!(f, "object error: {e}"),
                BranchError::Refuse(s) => write!(f, "refused: {s}"),
            }
        }
    }

    impl std::error::Error for BranchError {}

    /// Analyzes all local branches in the repository.
    pub fn analyze(
        repo: &crate::repo::Repo,
        cfg: &crate::repo::Config,
        base: Option<&str>,
    ) -> Result<Vec<BranchInfo>, BranchError> {
        let base_name = base
            .map(|s| s.to_string())
            .or_else(|| cfg.base_branch.clone())
            .or_else(|| crate::repo::default_branch(repo).ok().flatten())
            .unwrap_or_else(|| "main".to_string());

        let base_oid = crate::repo::resolve_refish(repo, &base_name).ok();
        let head_ref = crate::repo::head(repo).map_err(BranchError::Repo)?;
        let local_branches = crate::repo::local_branches(repo).map_err(BranchError::Repo)?;

        let mut infos = Vec::with_capacity(local_branches.len());
        for b in local_branches {
            let current = match &head_ref {
                crate::repo::HeadRef::Branch(cur) => cur == &b.name,
                _ => false,
            };
            let protected = cfg.protected.iter().any(|p| p == &b.name);

            let merged = if let Some(ref base_tip) = base_oid {
                if b.oid == *base_tip {
                    true
                } else {
                    crate::graph::is_reachable(repo, base_tip, &b.oid).unwrap_or(false)
                }
            } else {
                false
            };

            let upstream = crate::repo::upstream_oid(repo, cfg, &b.name).unwrap_or(None);
            let has_upstream = upstream.is_some();
            let (ahead, behind) = if let Some(ref up_oid) = upstream {
                let a = crate::graph::commits_only_in(repo, &b.oid, up_oid).unwrap_or(0);
                let beh = crate::graph::commits_only_in(repo, up_oid, &b.oid).unwrap_or(0);
                (a, beh)
            } else {
                (0, 0)
            };

            infos.push(BranchInfo {
                name: b.name,
                oid: b.oid,
                merged,
                ahead,
                behind,
                has_upstream,
                protected,
                current,
            });
        }

        Ok(infos)
    }

    /// Deletes a local branch with safety checks.
    pub fn delete_branch(
        repo: &crate::repo::Repo,
        cfg: &crate::repo::Config,
        name: &str,
    ) -> Result<(), BranchError> {
        if cfg.protected.iter().any(|p| p == name) {
            return Err(BranchError::Refuse(format!("branch '{name}' is protected")));
        }
        if let Ok(crate::repo::HeadRef::Branch(cur)) = crate::repo::head(repo) {
            if cur == name {
                return Err(BranchError::Refuse(format!(
                    "cannot delete checked-out branch '{name}'"
                )));
            }
        }
        crate::repo::delete_local_branch(repo, name).map_err(BranchError::Repo)?;
        Ok(())
    }

    /// Formats the branch list report.
    pub fn format_list(infos: &[BranchInfo], base: &str) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Base branch: {}\n\n",
            crate::output::paint(base, crate::output::CYAN)
        ));

        let mut cleanable = 0;
        let mut protected = 0;
        let mut active = 0;

        for info in infos {
            let is_cleanable = !info.protected
                && !info.current
                && info.merged
                && info.has_upstream
                && info.ahead == 0;
            if is_cleanable {
                cleanable += 1;
                let icon = crate::output::paint("✓", crate::output::GREEN);
                out.push_str(&format!(
                    "  {} {} [merged, up to date with upstream]\n",
                    icon, info.name
                ));
            } else if info.protected || info.current {
                protected += 1;
                let icon = crate::output::paint("○", crate::output::CYAN);
                let reason = if info.current && info.protected {
                    "current, protected"
                } else if info.current {
                    "current"
                } else {
                    "protected"
                };
                out.push_str(&format!("  {} {} [{reason}]\n", icon, info.name));
            } else {
                active += 1;
                let icon = crate::output::paint("⚠", crate::output::YELLOW);
                let mut reasons = Vec::new();
                if !info.merged {
                    reasons.push("unmerged");
                }
                if !info.has_upstream {
                    reasons.push("no upstream");
                } else if info.ahead > 0 {
                    reasons.push("unpushed commits");
                }
                let reason_str = reasons.join(", ");
                out.push_str(&format!("  {} {} [{reason_str}]\n", icon, info.name));
            }
        }

        out.push_str(&format!(
            "\nSummary: {} cleanable, {} protected/current, {} active branch(es)\n",
            cleanable, protected, active
        ));
        out
    }

    /// Formats the branch clean report.
    pub fn format_clean(
        infos: &[BranchInfo],
        deleted: &[String],
        kept: &[String],
        apply: bool,
    ) -> String {
        let mut out = String::new();
        if apply {
            if deleted.is_empty() {
                out.push_str("No cleanable branches found. No changes made.\n");
            } else {
                out.push_str(&format!("Deleted {} branch(es):\n", deleted.len()));
                for name in deleted {
                    out.push_str(&format!(
                        "  {} {}\n",
                        crate::output::paint("✓", crate::output::GREEN),
                        name
                    ));
                }
            }
        } else {
            let candidates: Vec<&BranchInfo> = infos
                .iter()
                .filter(|b| !b.protected && !b.current && b.merged && b.has_upstream && b.ahead == 0)
                .collect();
            if candidates.is_empty() {
                out.push_str("No cleanable branches found. No changes made.\n");
            } else {
                out.push_str(&format!(
                    "Would delete {} branch(es) (dry run):\n",
                    candidates.len()
                ));
                for b in candidates {
                    out.push_str(&format!("  - {}\n", b.name));
                }
                out.push_str("\nRun with --apply to delete these branches.\n");
            }
        }
        if !kept.is_empty() && !apply {
            out.push_str(&format!("Preserved {} active branch(es).\n", kept.len()));
        }
        out
    }
}

pub mod hook {
    use std::fmt;
    use std::fs;
    use std::io;
    use std::path::PathBuf;

    /// Pre-commit hook script content.
    pub const HOOK_SCRIPT: &str = r#"#!/bin/sh
# git-janitor pre-commit hook
exec git-jan secrets scan --staged
"#;

    /// Error during hook installation.
    #[derive(Debug)]
    pub enum HookError {
        Io(io::Error),
        Exists,
    }

    impl fmt::Display for HookError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                HookError::Io(e) => write!(f, "hook io error: {e}"),
                HookError::Exists => {
                    write!(f, "hook file exists. Use --force to overwrite existing hook")
                }
            }
        }
    }

    impl std::error::Error for HookError {}

    /// Installs git pre-commit hook in `.git/hooks/pre-commit`.
    pub fn install_hook(repo: &crate::repo::Repo, force: bool) -> Result<PathBuf, HookError> {
        let hooks_dir = repo.git_dir.join("hooks");
        fs::create_dir_all(&hooks_dir).map_err(HookError::Io)?;
        let hook_file = hooks_dir.join("pre-commit");

        if hook_file.exists() && !force {
            return Err(HookError::Exists);
        }

        fs::write(&hook_file, HOOK_SCRIPT).map_err(HookError::Io)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&hook_file).map_err(HookError::Io)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&hook_file, perms).map_err(HookError::Io)?;
        }

        Ok(hook_file)
    }
}
pub mod doctor {
    /// Diagnostics report output.
    pub struct DoctorReport {
        pub ok: Vec<String>,
        pub warn: Vec<String>,
        pub errs: Vec<String>,
    }

    /// Runs diagnostic checks on the repository.
    pub fn run(repo: &crate::repo::Repo) -> DoctorReport {
        let mut report = DoctorReport {
            ok: Vec::new(),
            warn: Vec::new(),
            errs: Vec::new(),
        };

        if repo.git_dir.is_dir() {
            report.ok.push(format!("Git directory found at {}", repo.git_dir.display()));
        } else {
            report.errs.push("Git directory not found or not accessible".into());
        }

        match crate::repo::read_config(repo) {
            Ok(cfg) => {
                report.ok.push(format!(
                    "Git config valid (protected branches: {})",
                    cfg.protected.join(", ")
                ));
            }
            Err(e) => report.errs.push(format!("Error parsing git config: {e}")),
        }

        match crate::repo::head(repo) {
            Ok(crate::repo::HeadRef::Branch(b)) => report.ok.push(format!("HEAD points to branch '{b}'")),
            Ok(crate::repo::HeadRef::Detached(oid)) => report.ok.push(format!("HEAD is detached at {oid}")),
            Ok(crate::repo::HeadRef::Unborn) => report.warn.push("HEAD is unborn (empty repo)".into()),
            Err(e) => report.errs.push(format!("Error reading HEAD: {e}")),
        }

        match crate::repo::local_branches(repo) {
            Ok(branches) => report.ok.push(format!("Found {} local branch(es)", branches.len())),
            Err(e) => report.errs.push(format!("Error reading local branches: {e}")),
        }

        match crate::index::staged_entries(repo) {
            Ok(entries) => report.ok.push(format!("Index parsed successfully ({} staged entries)", entries.len())),
            Err(e) => report.errs.push(format!("Error reading index: {e}")),
        }

        if let Ok(crate::repo::HeadRef::Branch(b)) = crate::repo::head(repo) {
            if let Ok(oid) = crate::repo::oid_of_ref(repo, &format!("refs/heads/{b}")) {
                match crate::objects::load_commit(repo, &oid) {
                    Ok(_) => report.ok.push("HEAD commit object verified in object database".into()),
                    Err(e) => report.errs.push(format!("Failed to load HEAD commit: {e}")),
                }
            }
        }

        if let Ok(ignore) = crate::leakignore::load(repo) {
            let _ = ignore.is_ignored("test");
            report.ok.push("Leakignore patterns validated".into());
        }

        report
    }

    /// Formats the diagnostic report into readable output.
    pub fn format_report(r: &DoctorReport) -> String {
        let mut out = String::from("git-janitor repository diagnostics:\n\n");
        for item in &r.ok {
            out.push_str(&format!("  {} {}\n", crate::output::paint("✓", crate::output::GREEN), item));
        }
        for item in &r.warn {
            out.push_str(&format!("  {} {}\n", crate::output::paint("⚠", crate::output::YELLOW), item));
        }
        for item in &r.errs {
            out.push_str(&format!("  {} {}\n", crate::output::paint("✗", crate::output::RED), item));
        }
        out
    }
}

pub mod entropy {
    pub const ENTROPY_THRESHOLD_LOW: f64 = 3.8;
    pub const TOKEN_MIN_LEN: usize = 12;

    /// Computes Shannon entropy in bits per byte.
    pub fn shannon_bits(data: &[u8]) -> f64 {
        if data.is_empty() {
            return 0.0;
        }
        let mut freq = [0usize; 256];
        for &b in data {
            freq[b as usize] += 1;
        }
        let len = data.len() as f64;
        let mut h = 0.0f64;
        for &count in &freq {
            if count > 0 {
                let p = count as f64 / len;
                h -= p * p.log2();
            }
        }
        h
    }

    /// Checks if a token has high entropy suitable for secret detection.
    pub fn is_high_entropy(token: &str) -> bool {
        let trimmed = token.trim_matches(|c: char| {
            c == '"' || c == '\'' || c == '.' || c == ',' || c == ';'
                || c == ':' || c == '=' || c == '(' || c == ')'
                || c == '[' || c == ']' || c == '{' || c == '}'
        });
        if trimmed.len() < TOKEN_MIN_LEN {
            return false;
        }
        let valid = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-'
            || c == '.' || c == '/' || c == '+' || c == '=' || c == '~';
        if !trimmed.chars().all(valid) {
            return false;
        }
        shannon_bits(trimmed.as_bytes()) >= ENTROPY_THRESHOLD_LOW
    }
}

pub mod patterns {
    pub struct Hit {
        pub kind: &'static str,
        pub value: &'static str,
    }

    fn is_upper_alnum(c: char) -> bool {
        c.is_ascii_uppercase() || c.is_ascii_digit()
    }

    fn is_base64url(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '-' || c == '_'
    }

    fn is_valid_token_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/' || c == '+' || c == '=' || c == '~'
    }

    fn check_aws_key(line: &str) -> Option<&str> {
        let bytes = line.as_bytes();
        for i in 0..bytes.len() {
            if bytes[i] == b'A' && i + 20 <= bytes.len() {
                if bytes[i + 1] == b'K'
                    && bytes[i + 2] == b'I'
                    && bytes[i + 3] == b'A'
                {
                    let rest = &line[i + 4..i + 20];
                    if rest.len() == 16 && rest.chars().all(is_upper_alnum) {
                        return Some(&line[i..i + 20]);
                    }
                }
            }
        }
        None
    }

    fn check_github_token(line: &str) -> Option<&str> {
        let prefixes = ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"];
        for prefix in &prefixes {
            if let Some(pos) = line.find(prefix) {
                let start = pos + prefix.len();
                if start + 36 <= line.len() {
                    let token = &line[start..start + 36];
                    if token.chars().all(|c| c.is_ascii_alphanumeric()) {
                        return Some(&line[pos..start + 36]);
                    }
                }
            }
        }
        None
    }

    fn check_private_key(line: &str) -> Option<&str> {
        let trimmed = line.trim();
        if trimmed.starts_with("-----BEGIN") && trimmed.contains("PRIVATE KEY-----") {
            return Some(trimmed);
        }
        None
    }

    fn check_jwt(line: &str) -> Option<&str> {
        let trimmed = line.trim();
        if trimmed.len() < 64 {
            return None;
        }
        let mut segments = trimmed.split('.');
        let s1 = segments.next()?;
        let s2 = segments.next()?;
        let s3 = segments.next()?;
        if segments.next().is_some() {
            return None;
        }
        if !s1.starts_with("eyJ") {
            return None;
        }
        if s1.is_empty() || s2.is_empty() || s3.is_empty() {
            return None;
        }
        if !(s1.chars().all(is_base64url) && s2.chars().all(is_base64url) && s3.chars().all(is_base64url)) {
            return None;
        }
        Some(trimmed)
    }

    fn check_slack_token(line: &str) -> Option<&str> {
        let bytes = line.as_bytes();
        for i in 0..bytes.len() {
            if bytes[i] == b'x' && i + 5 <= bytes.len() {
                let p = &line[i..];
                for prefix in &["xoxb-", "xoxp-", "xoxa-", "xoxr-", "xoxs-"] {
                    if p.starts_with(prefix) {
                        let rest = &p[prefix.len()..];
                        if rest.len() >= 12 && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                            let total = prefix.len() + rest.len();
                            return Some(&line[i..i + total]);
                        }
                    }
                }
            }
        }
        None
    }

    fn check_api_key(line: &str) -> Option<&str> {
        if let Some(pos) = line.find("sk_live_") {
            let start = pos + 8;
            if start + 16 <= line.len() {
                let rest = &line[start..];
                let end = rest.find(|c: char| !is_valid_token_char(c)).unwrap_or(rest.len());
                if end >= 16 {
                    return Some(&line[pos..start + end]);
                }
            }
        }
        if let Some(pos) = line.find("sk-") {
            let start = pos + 3;
            if start + 16 <= line.len() {
                let rest = &line[start..];
                let end = rest.find(|c: char| !is_valid_token_char(c)).unwrap_or(rest.len());
                if end >= 16 {
                    return Some(&line[pos..start + end]);
                }
            }
        }
        None
    }

    fn check_google_key(line: &str) -> Option<&str> {
        let bytes = line.as_bytes();
        for i in 0..bytes.len() {
            if bytes[i] == b'A' && i + 39 <= bytes.len() {
                if line[i + 1..].starts_with("Iza") {
                    let rest = &line[i + 4..i + 39];
                    if rest.len() == 35 && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                        return Some(&line[i..i + 39]);
                    }
                }
            }
        }
        None
    }

    fn check_auth_header(line: &str) -> Option<&str> {
        if let Some(pos) = line.find("Basic ") {
            let start = pos + 6;
            let rest = &line[start..];
            let end = rest.find(|c: char| c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '"').unwrap_or(rest.len());
            if end >= 4 && rest[..end].chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=') {
                return Some(&line[pos..start + end]);
            }
        }
        if let Some(pos) = line.find("Bearer ") {
            let start = pos + 7;
            let rest = &line[start..];
            let end = rest.find(|c: char| c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '"').unwrap_or(rest.len());
            if end >= 28 && rest[..end].chars().all(is_valid_token_char) {
                return Some(&line[pos..start + end]);
            }
        }
        None
    }

    fn check_key_value(line: &str) -> Option<(&str, &str)> {
        let keywords = [
            "password", "passwd", "secret", "token", "api_key", "apikey",
            "client_secret", "access_token", "private_key", "credential", "auth",
        ];
        let bytes = line.as_bytes();
        for kw in &keywords {
            let kw_bytes = kw.as_bytes();
            for i in 0..bytes.len() {
                if i + kw_bytes.len() <= bytes.len() && &bytes[i..i + kw_bytes.len()] == kw_bytes {
                    let after_kw = i + kw_bytes.len();
                    let mut j = after_kw;
                    while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                        j += 1;
                    }
                    if j < bytes.len() && (bytes[j] == b':' || bytes[j] == b'=') {
                        let mut k = j + 1;
                        while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t') {
                            k += 1;
                        }
                        if k < bytes.len() {
                            let rest = &line[k..];
                            let end = rest.find(|c: char| c == '\n' || c == '\r' || c == '#').unwrap_or(rest.len());
                            let value = rest[..end].trim_end();
                            if !value.is_empty() {
                                return Some((kw, value));
                            }
                        }
                        break;
                    }
                }
            }
        }
        None
    }

    fn is_whitelisted(value: &str) -> bool {
        let lower = value.to_ascii_lowercase();
        let stripped = value.trim();
        let is_exact_or_placeholder = {
            let clean = stripped.trim_matches(|c: char| c == '"' || c == '\'' || c == '.');
            clean.eq_ignore_ascii_case("changeme")
                || clean.eq_ignore_ascii_case("example")
                || clean.eq_ignore_ascii_case("your-password")
                || clean.eq_ignore_ascii_case("your-token")
                || clean.eq_ignore_ascii_case("your-secret")
                || clean.eq_ignore_ascii_case("placeholder")
                || clean.eq_ignore_ascii_case("xxxx")
                || clean.eq_ignore_ascii_case("demo")
                || lower.contains("your-username")
                || lower.starts_with("your-")
        };
        if is_exact_or_placeholder {
            return true;
        }
        if lower.contains("changeme") || lower.contains("placeholder") {
            return true;
        }
        if stripped.len() == 32 || stripped.len() == 40 {
            if stripped.chars().all(|c| c.is_ascii_hexdigit()) {
                return true;
            }
        }
        if stripped.len() == 10 && stripped.chars().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() }
        }) {
            return true;
        }
        false
    }

    /// Detects secret patterns in a single line.
    pub fn detect(line: &str) -> Vec<Hit> {
        let mut hits = Vec::new();

        if let Some(v) = check_aws_key(line) {
            if !is_whitelisted(v) {
                hits.push(Hit { kind: "aws-access-key", value: leaked_static(v) });
            }
        }

        if let Some(v) = check_github_token(line) {
            if !is_whitelisted(v) {
                hits.push(Hit { kind: "github-token", value: leaked_static(v) });
            }
        }

        if let Some(v) = check_private_key(line) {
            hits.push(Hit { kind: "private-key", value: leaked_static(v) });
        }

        if let Some(v) = check_jwt(line) {
            hits.push(Hit { kind: "jwt", value: leaked_static(v) });
        }

        if let Some(v) = check_slack_token(line) {
            if !is_whitelisted(v) {
                hits.push(Hit { kind: "slack-token", value: leaked_static(v) });
            }
        }

        if let Some(v) = check_api_key(line) {
            if !is_whitelisted(v) {
                hits.push(Hit { kind: "api-key", value: leaked_static(v) });
            }
        }

        if let Some(v) = check_google_key(line) {
            if !is_whitelisted(v) {
                hits.push(Hit { kind: "google-key", value: leaked_static(v) });
            }
        }

        if let Some(v) = check_auth_header(line) {
            hits.push(Hit { kind: "auth-header", value: leaked_static(v) });
        }

        if hits.is_empty() {
            if let Some((_kw, value)) = check_key_value(line) {
                if !is_whitelisted(value) && crate::entropy::is_high_entropy(value) {
                    hits.push(Hit { kind: "high-entropy-key", value: leaked_static(value) });
                }
            }
        }

        hits
    }

    fn leaked_static(s: &str) -> &'static str {
        Box::leak(s.to_string().into_boxed_str())
    }
}

pub mod leakignore {
    use std::fmt;

    struct IgnorePattern {
        negated: bool,
        anchored: bool,
        dir_only: bool,
        segments: Vec<String>,
    }

    pub struct LeakIgnore {
        patterns: Vec<IgnorePattern>,
    }

    #[derive(Debug)]
    pub enum LeakError {
        Io(std::io::Error),
    }

    impl fmt::Display for LeakError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                LeakError::Io(e) => write!(f, "leakignore io error: {e}"),
            }
        }
    }

    impl std::error::Error for LeakError {}

    /// Loads .leakignore from repo root (if present), merging config ignore paths.
    pub fn load(repo: &crate::repo::Repo) -> Result<LeakIgnore, LeakError> {
        let mut patterns = Vec::new();

        let ignore_path = repo.work_dir.join(".leakignore");
        if ignore_path.is_file() {
            if let Ok(content) = std::fs::read_to_string(&ignore_path) {
                for line in content.lines() {
                    if let Some(p) = parse_line(line) {
                        patterns.push(p);
                    }
                }
            }
        }

        if let Ok(cfg) = crate::repo::read_config(repo) {
            for extra in &cfg.leak_ignore_paths {
                let entry = IgnorePattern {
                    negated: false,
                    anchored: false,
                    dir_only: false,
                    segments: extra.split('/').filter(|s| !s.is_empty()).map(String::from).collect(),
                };
                patterns.push(entry);
            }
        }

        Ok(LeakIgnore { patterns })
    }

    fn parse_line(raw: &str) -> Option<IgnorePattern> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return None;
        }
        let mut rest = trimmed;
        let negated = if rest.starts_with('!') {
            rest = &rest[1..];
            true
        } else {
            false
        };
        let anchored = rest.starts_with('/');
        if anchored {
            rest = &rest[1..];
        }
        let dir_only = rest.ends_with('/');
        if dir_only && rest.len() > 1 {
            rest = &rest[..rest.len() - 1];
        }
        let segments: Vec<String> = rest.split('/').filter(|s| !s.is_empty()).map(String::from).collect();
        if segments.is_empty() {
            return None;
        }
        Some(IgnorePattern { negated, anchored, dir_only, segments })
    }

    fn glob_match(pattern: &str, text: &str) -> bool {
        glob_match_inner(pattern.as_bytes(), text.as_bytes())
    }

    fn glob_match_inner(p: &[u8], t: &[u8]) -> bool {
        let mut pi = 0;
        let mut ti = 0;
        let mut star_pi = usize::MAX;
        let mut star_ti = 0;

        while ti < t.len() {
            if pi < p.len() && p[pi] == b'*' && (pi + 1 >= p.len() || p[pi + 1] != b'*') {
                star_pi = pi;
                star_ti = ti;
                pi += 1;
            } else if pi < p.len() && p[pi] == b'*' && pi + 1 < p.len() && p[pi + 1] == b'*' {
                star_pi = pi;
                star_ti = ti;
                pi += 2;
                while pi < p.len() && p[pi] == b'/' {
                    pi += 1;
                }
            } else if pi < p.len() && (p[pi] == b'?' || p[pi] == t[ti]) {
                pi += 1;
                ti += 1;
            } else if star_pi != usize::MAX {
                pi = star_pi;
                star_ti += 1;
                ti = star_ti;
            } else {
                return false;
            }
        }

        while pi < p.len() && p[pi] == b'*' {
            pi += 1;
        }
        pi == p.len()
    }

    fn segments_match(pattern_segs: &[String], path_segs: &[&str], anchored: bool, dir_only: bool) -> bool {
        if pattern_segs.is_empty() || path_segs.is_empty() {
            return false;
        }
        if anchored {
            if pattern_segs.len() > path_segs.len() {
                return false;
            }
            for (pi, seg) in pattern_segs.iter().enumerate() {
                if !glob_match(seg, path_segs[pi]) {
                    return false;
                }
            }
            true
        } else {
            let double_star = pattern_segs.iter().any(|s| s == "**");
            if double_star {
                return double_star_match(pattern_segs, path_segs);
            }
            if dir_only {
                if pattern_segs.len() > path_segs.len() {
                    return false;
                }
                return pattern_segs.iter().enumerate().all(|(pi, p)| glob_match(p, path_segs[pi]));
            }
            if pattern_segs.len() > path_segs.len() {
                return false;
            }
            let start = path_segs.len() - pattern_segs.len();
            pattern_segs.iter().zip(path_segs[start..].iter()).all(|(p, t)| glob_match(p, t))
        }
    }

    fn double_star_match(pattern_segs: &[String], path_segs: &[&str]) -> bool {
        if pattern_segs.is_empty() {
            return path_segs.is_empty();
        }
        if pattern_segs[0] == "**" {
            if double_star_match(&pattern_segs[1..], path_segs) {
                return true;
            }
            if !path_segs.is_empty() {
                return double_star_match(pattern_segs, &path_segs[1..]);
            }
            return false;
        }
        if path_segs.is_empty() {
            return false;
        }
        if glob_match(&pattern_segs[0], path_segs[0]) {
            return double_star_match(&pattern_segs[1..], &path_segs[1..]);
        }
        false
    }

    impl LeakIgnore {
        /// Checks if a repo-relative path should be ignored.
        pub fn is_ignored(&self, path: &str) -> bool {
            let normalized = path.replace('\\', "/");
            let path_segs: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
            let mut ignored = false;

            for pat in &self.patterns {
                if segments_match(&pat.segments, &path_segs, pat.anchored, pat.dir_only) {
                    ignored = !pat.negated;
                }
            }

            ignored
        }
    }
}

pub mod secrets {
    use std::fmt;

    const MAX_SCAN_BYTES: usize = 1_048_576;

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum ScanTarget {
        Worktree,
        Staged,
        Since(String),
        Commit(String),
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Finding {
        pub kind: String,
        pub file: String,
        pub line: usize,
        pub redacted: String,
    }

    #[derive(Debug)]
    pub enum ScannerError {
        Repo(crate::repo::RepoError),
        Obj(crate::objects::ObjError),
        Config(crate::repo::RepoError),
        Index(crate::index::IndexError),
        Walk(std::io::Error),
        Leak(crate::leakignore::LeakError),
    }

    impl fmt::Display for ScannerError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                ScannerError::Repo(e) => write!(f, "repo error: {e}"),
                ScannerError::Obj(e) => write!(f, "object error: {e}"),
                ScannerError::Config(e) => write!(f, "config error: {e}"),
                ScannerError::Index(e) => write!(f, "index error: {e}"),
                ScannerError::Walk(e) => write!(f, "walk error: {e}"),
                ScannerError::Leak(e) => write!(f, "leakignore error: {e}"),
            }
        }
    }

    impl std::error::Error for ScannerError {}

    /// Scans text data line-by-line for secret patterns.
    pub fn scan_text(file: &str, path: &str, data: &[u8]) -> Vec<Finding> {
        let is_binary = {
            let check_len = data.len().min(8000);
            data[..check_len].contains(&0u8)
        };

        if is_binary && !is_text_extension(path) {
            let hits = crate::patterns::detect(path);
            return hits.into_iter().map(|h| Finding {
                kind: h.kind.to_string(),
                file: file.to_string(),
                line: 0,
                redacted: crate::output::redact(h.value),
            }).collect();
        }

        let text = match std::str::from_utf8(data) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let mut findings = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let hits = crate::patterns::detect(line);
            for h in hits {
                findings.push(Finding {
                    kind: h.kind.to_string(),
                    file: file.to_string(),
                    line: i + 1,
                    redacted: crate::output::redact(h.value),
                });
            }
        }

        findings.dedup_by(|a, b| a.kind == b.kind && a.file == b.file && a.line == b.line && a.redacted == b.redacted);
        findings.sort_by_key(|f| f.line);
        findings
    }

    fn is_text_extension(path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        let text_exts = [
            ".txt", ".rs", ".py", ".js", ".ts", ".tsx", ".jsx", ".json", ".yaml", ".yml",
            ".toml", ".md", ".sh", ".go", ".c", ".h", ".java", ".rb", ".php", ".xml",
            ".html", ".css", ".sql", ".csv", ".cfg", ".conf", ".ini", ".env",
            ".gitignore", ".leakignore", ".lock", ".log", ".toml", ".nix",
            ".mk", ".makefile", ".dockerfile", ".docker",
        ];
        text_exts.iter().any(|ext| lower.ends_with(ext))
    }

    fn resolve_file_in_tree(
        repo: &crate::repo::Repo,
        tree_oid: &crate::objects::Oid,
        path: &str,
    ) -> Result<Option<crate::objects::Oid>, crate::objects::ObjError> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        resolve_in_tree(repo, tree_oid, &segments)
    }

    fn resolve_in_tree(
        repo: &crate::repo::Repo,
        tree_oid: &crate::objects::Oid,
        segments: &[&str],
    ) -> Result<Option<crate::objects::Oid>, crate::objects::ObjError> {
        if segments.is_empty() {
            return Ok(None);
        }
        let entries = crate::objects::load_tree(repo, tree_oid)?;
        let target = segments[0];
        for entry in &entries {
            let name = String::from_utf8_lossy(&entry.name);
            if name.as_ref() == target {
                if segments.len() == 1 {
                    return Ok(Some(entry.oid));
                }
                if entry.mode == "040000" || entry.mode == "40000" {
                    return resolve_in_tree(repo, &entry.oid, &segments[1..]);
                }
                return Ok(None);
            }
        }
        Ok(None)
    }

    fn scan_worktree(repo: &crate::repo::Repo, ignore: &crate::leakignore::LeakIgnore) -> Result<Vec<Finding>, ScannerError> {
        let mut findings = Vec::new();
        walk_dir(repo, &repo.work_dir.clone(), &repo.work_dir, ignore, &mut findings)?;
        findings.dedup_by(|a, b| a.kind == b.kind && a.file == b.file && a.line == b.line && a.redacted == b.redacted);
        findings.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
        Ok(findings)
    }

    fn walk_dir(
        repo: &crate::repo::Repo,
        dir: &std::path::Path,
        work_dir: &std::path::Path,
        ignore: &crate::leakignore::LeakIgnore,
        findings: &mut Vec<Finding>,
    ) -> Result<(), ScannerError> {
        let entries = std::fs::read_dir(dir).map_err(ScannerError::Walk)?;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                if name == ".git" {
                    continue;
                }
                walk_dir(repo, &path, work_dir, ignore, findings)?;
            } else if path.is_file() {
                let rel = path.strip_prefix(work_dir).unwrap_or(&path);
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                if ignore.is_ignored(&rel_str) {
                    continue;
                }
                let meta = std::fs::metadata(&path).map_err(ScannerError::Walk)?;
                if meta.len() > MAX_SCAN_BYTES as u64 {
                    continue;
                }
                let data = std::fs::read(&path).map_err(ScannerError::Walk)?;
                let file_str = path.to_string_lossy().to_string();
                let mut f = scan_text(&file_str, &rel_str, &data);
                findings.append(&mut f);
            }
        }
        Ok(())
    }

    fn scan_staged(repo: &crate::repo::Repo) -> Result<Vec<Finding>, ScannerError> {
        let entries = crate::index::staged_entries(repo).map_err(ScannerError::Index)?;
        let mut findings = Vec::new();
        for entry in &entries {
            match crate::objects::load_blob(repo, &entry.oid) {
                Ok(data) => {
                    let mut f = scan_text(&entry.path, &entry.path, &data);
                    findings.append(&mut f);
                }
                Err(_) => continue,
            }
        }
        findings.dedup_by(|a, b| a.kind == b.kind && a.file == b.file && a.line == b.line && a.redacted == b.redacted);
        findings.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
        Ok(findings)
    }

    fn scan_since(repo: &crate::repo::Repo, refish: &str) -> Result<Vec<Finding>, ScannerError> {
        let from_oid = crate::repo::resolve_refish(repo, refish).map_err(ScannerError::Repo)?;
        let to_oid = crate::repo::resolve_refish(repo, "HEAD").map_err(ScannerError::Repo)?;
        let changed = crate::graph::changed_files_between(repo, &from_oid, &to_oid).map_err(ScannerError::Obj)?;
        let to_commit = crate::objects::load_commit(repo, &to_oid).map_err(ScannerError::Obj)?;
        scan_commit_files(repo, &to_commit.tree, &changed)
    }

    fn scan_commit(repo: &crate::repo::Repo, refish: &str) -> Result<Vec<Finding>, ScannerError> {
        let commit_oid = crate::repo::resolve_refish(repo, refish).map_err(ScannerError::Repo)?;
        let commit = crate::objects::load_commit(repo, &commit_oid).map_err(ScannerError::Obj)?;
        let files = crate::graph::files_in_commit(repo, &commit_oid).map_err(ScannerError::Obj)?;
        scan_commit_files(repo, &commit.tree, &files)
    }

    fn scan_commit_files(
        repo: &crate::repo::Repo,
        tree_oid: &crate::objects::Oid,
        files: &[String],
    ) -> Result<Vec<Finding>, ScannerError> {
        let mut findings = Vec::new();
        for file_path in files {
            match resolve_file_in_tree(repo, tree_oid, file_path) {
                Ok(Some(blob_oid)) => {
                    match crate::objects::load_blob(repo, &blob_oid) {
                        Ok(data) => {
                            let mut f = scan_text(file_path, file_path, &data);
                            findings.append(&mut f);
                        }
                        Err(_) => continue,
                    }
                }
                _ => continue,
            }
        }
        findings.dedup_by(|a, b| a.kind == b.kind && a.file == b.file && a.line == b.line && a.redacted == b.redacted);
        findings.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
        Ok(findings)
    }

    /// Runs the secret scanner against the given target.
    pub fn run(
        repo: &crate::repo::Repo,
        target: ScanTarget,
    ) -> Result<Vec<Finding>, ScannerError> {
        let ignore = crate::leakignore::load(repo).map_err(ScannerError::Leak)?;
        match target {
            ScanTarget::Worktree => scan_worktree(repo, &ignore),
            ScanTarget::Staged => scan_staged(repo),
            ScanTarget::Since(refish) => scan_since(repo, &refish),
            ScanTarget::Commit(refish) => scan_commit(repo, &refish),
        }
    }

    /// Formats findings into a human-readable report.
    pub fn human_report(findings: &[Finding]) -> String {
        if findings.is_empty() {
            "No secrets detected.\n".to_string()
        } else {
            let mut out = String::new();
            for f in findings {
                out.push_str(&format!(
                    "{} {} — {}:{}\n",
                    crate::output::paint("✗", crate::output::RED),
                    f.kind,
                    f.file,
                    f.line
                ));
            }
            out.push_str(&format!(
                "\n{} potential secret(s) found.\n",
                findings.len()
            ));
            out
        }
    }

    /// Formats findings into a JSON report.
    pub fn json_report(findings: &[Finding]) -> String {
        let mut out = String::from("{\"findings\":[");
        for (i, f) in findings.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"kind\":\"{}\",\"file\":\"{}\",\"line\":{},\"redacted\":\"{}\"}}",
                crate::output::json_escape(&f.kind),
                crate::output::json_escape(&f.file),
                f.line,
                crate::output::json_escape(&f.redacted)
            ));
        }
        out.push_str("]}");
        out
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = match crate::cli::parse(args.into_iter()) {
        Ok(cmd) => cmd,
        Err(err) => {
            eprintln!("Error: {err}\n\n{}", crate::cli::usage());
            std::process::exit(2);
        }
    };

    match cmd {
        crate::cli::Command::Help => {
            println!("{}", crate::cli::usage());
            std::process::exit(0);
        }
        crate::cli::Command::Version => {
            println!("git-janitor 0.1.0");
            std::process::exit(0);
        }
        crate::cli::Command::Doctor => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let repo = match crate::repo::find_repo_from(&cwd) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(2);
                }
            };
            let report = crate::doctor::run(&repo);
            print!("{}", crate::doctor::format_report(&report));
            if !report.errs.is_empty() {
                std::process::exit(2);
            }
            std::process::exit(0);
        }
        crate::cli::Command::InstallHook { force } => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let repo = match crate::repo::find_repo_from(&cwd) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(2);
                }
            };
            match crate::hook::install_hook(&repo, force) {
                Ok(p) => {
                    println!("Installed pre-commit hook to {}", p.display());
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("Error installing hook: {e}");
                    std::process::exit(2);
                }
            }
        }
        crate::cli::Command::BranchList { base } => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let repo = match crate::repo::find_repo_from(&cwd) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(2);
                }
            };
            let cfg = crate::repo::read_config(&repo).unwrap_or_default();
            let base_str = base
                .as_deref()
                .or(cfg.base_branch.as_deref())
                .unwrap_or("main");
            match crate::branch::analyze(&repo, &cfg, Some(base_str)) {
                Ok(infos) => {
                    print!("{}", crate::branch::format_list(&infos, base_str));
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(2);
                }
            }
        }
        crate::cli::Command::BranchClean { base, apply } => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let repo = match crate::repo::find_repo_from(&cwd) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(2);
                }
            };
            let cfg = crate::repo::read_config(&repo).unwrap_or_default();
            let base_str = base
                .as_deref()
                .or(cfg.base_branch.as_deref())
                .unwrap_or("main");
            let infos = match crate::branch::analyze(&repo, &cfg, Some(base_str)) {
                Ok(i) => i,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(2);
                }
            };

            let mut deleted = Vec::new();
            let mut kept = Vec::new();
            for info in &infos {
                let eligible = !info.protected
                    && !info.current
                    && info.merged
                    && info.has_upstream
                    && info.ahead == 0;
                if eligible {
                    if apply {
                        if let Err(e) = crate::branch::delete_branch(&repo, &cfg, &info.name) {
                            eprintln!("Error deleting {}: {e}", info.name);
                        } else {
                            deleted.push(info.name.clone());
                        }
                    } else {
                        deleted.push(info.name.clone());
                    }
                } else {
                    kept.push(info.name.clone());
                }
            }

            print!("{}", crate::branch::format_clean(&infos, &deleted, &kept, apply));
            std::process::exit(0);
        }
        crate::cli::Command::SecretsScan(opts) => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let repo = match crate::repo::find_repo_from(&cwd) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(2);
                }
            };

            let target = if opts.staged {
                crate::secrets::ScanTarget::Staged
            } else if let Some(s) = opts.since {
                crate::secrets::ScanTarget::Since(s)
            } else if let Some(c) = opts.commit {
                crate::secrets::ScanTarget::Commit(c)
            } else {
                crate::secrets::ScanTarget::Worktree
            };

            match crate::secrets::run(&repo, target) {
                Ok(findings) => {
                    if opts.format == crate::cli::Format::Json {
                        println!("{}", crate::secrets::json_report(&findings));
                    } else {
                        print!("{}", crate::secrets::human_report(&findings));
                    }
                    if findings.is_empty() {
                        std::process::exit(0);
                    } else {
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Scanner Error: {e}");
                    std::process::exit(2);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use crate::testdata;
    use crate::testutil;

    #[test]
    fn inflate_zlib_hello() {
        let compressed = testutil::hex(testdata::ZLIB_HELLO);
        let decompressed = crate::inflate::inflate_zlib(&compressed).unwrap();
        assert_eq!(decompressed, b"hello world\n");
    }

    #[test]
    fn inflate_zlib_multiline() {
        let compressed = testutil::hex(testdata::ZLIB_MULTILINE);
        let decompressed = crate::inflate::inflate_zlib(&compressed).unwrap();
        assert_eq!(decompressed, b"line 1\nline 2\nline 3\n");
    }

    #[test]
    fn inflate_zlib_large() {
        let pattern = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut original = Vec::new();
        for _ in 0..2000 {
            original.extend_from_slice(pattern);
        }
        let compressed = testutil::zlib_store(&original);
        let decompressed = crate::inflate::inflate_zlib(&compressed).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn inflate_bad_header() {
        let mut compressed = testutil::hex(testdata::ZLIB_HELLO);
        compressed[0] = 0xFF;
        assert!(crate::inflate::inflate_zlib(&compressed).is_err());
    }

    #[test]
    fn inflate_truncated() {
        let compressed = testutil::hex(testdata::ZLIB_HELLO);
        assert!(crate::inflate::inflate_zlib(&compressed[..5]).is_err());
    }

    #[test]
    fn inflate_bad_adler() {
        let mut compressed = testutil::hex(testdata::ZLIB_HELLO);
        let len = compressed.len();
        compressed[len - 1] ^= 0xFF;
        assert!(crate::inflate::inflate_zlib(&compressed).is_err());
    }

    #[test]
    fn adler32_known() {
        let data = b"Wikipedia";
        assert_eq!(crate::inflate::adler32(data), 0x11E60398);
    }

    #[test]
    fn loose_commit_parse() {
        let dir = testutil::unique_tempdir("loose_commit");
        let content = b"tree 0000000000000000000000000000000000000000\nauthor User <user@test.com> 1700000000 +0000\ncommitter User <user@test.com> 1700000000 +0000\n\nInitial commit\n";
        let oid = testutil::write_loose_object(&dir, "commit", content);
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        let commit = crate::objects::load_commit(&repo, &oid).unwrap();
        assert_eq!(commit.tree, crate::objects::Oid([0; 20]));
        assert!(commit.parents.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn loose_blob_roundtrip() {
        let dir = testutil::unique_tempdir("loose_blob");
        let content = b"test blob payload";
        let oid = testutil::write_loose_object(&dir, "blob", content);
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        let blob = crate::objects::load_blob(&repo, &oid).unwrap();
        assert_eq!(blob, content);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tree_parse_entries() {
        let dir = testutil::unique_tempdir("tree_parse");
        let mut content = Vec::new();
        content.extend_from_slice(b"100644 file.txt\0");
        content.extend_from_slice(&[1u8; 20]);
        let oid = testutil::write_loose_object(&dir, "tree", &content);
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        let tree = crate::objects::load_tree(&repo, &oid).unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].name, b"file.txt");
        assert_eq!(tree[0].mode, "100644");
        assert_eq!(tree[0].oid, crate::objects::Oid([1; 20]));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tag_deref() {
        let dir = testutil::unique_tempdir("tag_deref");
        let target_oid = crate::objects::Oid([2u8; 20]);
        let content = format!("object {}\ntype commit\ntag v1.0\n\nRelease\n", target_oid.to_hex());
        let oid = testutil::write_loose_object(&dir, "tag", content.as_bytes());
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        let obj = crate::objects::load_object(&repo, &oid).unwrap();
        match obj {
            crate::objects::Obj::Tag(target) => assert_eq!(target, target_oid),
            _ => panic!("expected tag object"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn not_found_object() {
        let dir = testutil::unique_tempdir("not_found");
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        let oid = crate::objects::Oid([9u8; 20]);
        assert!(crate::objects::load_object(&repo, &oid).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha256_repo_rejected() {
        let dir = testutil::unique_tempdir("sha256_repo");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("config"), "[extensions]\n    objectFormat = sha256\n").unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let oid = crate::objects::Oid([0; 20]);
        let res = crate::objects::load_object(&repo, &oid);
        assert!(matches!(res, Err(crate::objects::ObjError::Unsupported(_))));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repo_discovery_walk_up() {
        let dir = testutil::unique_tempdir("walk_up");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let sub = dir.join("a/b/c");
        fs::create_dir_all(&sub).unwrap();
        let repo = crate::repo::find_repo_from(&sub).unwrap();
        assert_eq!(repo.work_dir, dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repo_worktree_file() {
        let dir = testutil::unique_tempdir("worktree");
        let actual_git = dir.join("main_git");
        fs::create_dir_all(&actual_git).unwrap();
        let wt = dir.join("wt");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", actual_git.display())).unwrap();
        let repo = crate::repo::find_repo_from(&wt).unwrap();
        assert_eq!(repo.git_dir, actual_git);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_branch_section() {
        let dir = testutil::unique_tempdir("cfg_branch");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(
            git_dir.join("config"),
            r#"
[branch "feat"]
    remote = origin
    merge = refs/heads/feat
"#,
        )
        .unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        assert_eq!(cfg.branch_remote.get("feat").unwrap(), "origin");
        assert_eq!(cfg.branch_merge.get("feat").unwrap(), "refs/heads/feat");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_quoted_value() {
        let dir = testutil::unique_tempdir("cfg_quote");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(
            git_dir.join("config"),
            r#"
[git-janitor]
    protected = "main, prod, staging"
"#,
        )
        .unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        assert_eq!(cfg.protected, vec!["main", "prod", "staging"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_continuation() {
        let dir = testutil::unique_tempdir("cfg_cont");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(
            git_dir.join("config"),
            "[git-janitor]\n    protected = main, \\\n        master\n",
        )
        .unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        assert_eq!(cfg.protected, vec!["main", "master"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn packed_refs_precedence() {
        let dir = testutil::unique_tempdir("packed_prec");
        let git_dir = dir.join(".git");
        let heads_dir = git_dir.join("refs/heads");
        fs::create_dir_all(&heads_dir).unwrap();
        let packed_oid = crate::objects::Oid([1; 20]);
        let loose_oid = crate::objects::Oid([2; 20]);
        fs::write(
            git_dir.join("packed-refs"),
            format!("{} refs/heads/feat\n", packed_oid.to_hex()),
        )
        .unwrap();
        fs::write(heads_dir.join("feat"), format!("{}\n", loose_oid.to_hex())).unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let branches = crate::repo::local_branches(&repo).unwrap();
        assert_eq!(branches.len(), 1);
        assert_eq!(branches[0].oid, loose_oid);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn upstream_oid_resolves() {
        let dir = testutil::unique_tempdir("up_oid");
        let git_dir = dir.join(".git");
        let rem_dir = git_dir.join("refs/remotes/origin");
        fs::create_dir_all(&rem_dir).unwrap();
        let target_oid = crate::objects::Oid([5; 20]);
        fs::write(rem_dir.join("feat"), format!("{}\n", target_oid.to_hex())).unwrap();
        fs::write(
            git_dir.join("config"),
            "[branch \"feat\"]\n    remote = origin\n    merge = refs/heads/feat\n",
        )
        .unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        let oid = crate::repo::upstream_oid(&repo, &cfg, "feat").unwrap().unwrap();
        assert_eq!(oid, target_oid);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_branch_removes_loose_and_packed() {
        let dir = testutil::unique_tempdir("del_branch");
        let git_dir = dir.join(".git");
        let heads_dir = git_dir.join("refs/heads");
        fs::create_dir_all(&heads_dir).unwrap();
        fs::write(heads_dir.join("feat"), format!("{}\n", crate::objects::Oid([1; 20]).to_hex())).unwrap();
        fs::write(
            git_dir.join("packed-refs"),
            format!("{} refs/heads/feat\n^somepeeled\n", crate::objects::Oid([1; 20]).to_hex()),
        )
        .unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        crate::repo::delete_local_branch(&repo, "feat").unwrap();
        assert!(!repo.git_dir.join("refs/heads/feat").exists());
        let packed_content = fs::read_to_string(repo.git_dir.join("packed-refs")).unwrap();
        assert!(!packed_content.contains("refs/heads/feat"));
        assert!(!packed_content.contains("^somepeeled"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn head_unborn() {
        let dir = testutil::unique_tempdir("unborn");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        assert_eq!(crate::repo::head(&repo).unwrap(), crate::repo::HeadRef::Unborn);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn head_detached() {
        let dir = testutil::unique_tempdir("detached");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let oid = crate::objects::Oid([7; 20]);
        fs::write(git_dir.join("HEAD"), format!("{}\n", oid.to_hex())).unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        assert_eq!(crate::repo::head(&repo).unwrap(), crate::repo::HeadRef::Detached(oid));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn index_v2_entries() {
        let dir = testutil::unique_tempdir("idx_v2");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let index_data = testutil::hex(testdata::INDEX_V2_HEX);
        fs::write(git_dir.join("index"), index_data).unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let entries = crate::index::staged_entries(&repo).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "file.txt");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn index_v4_prefix_paths() {
        let dir = testutil::unique_tempdir("idx_v4");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let mut v4_data = Vec::new();
        v4_data.extend_from_slice(b"DIRC");
        v4_data.extend_from_slice(&4u32.to_be_bytes());
        v4_data.extend_from_slice(&2u32.to_be_bytes());
        for i in 0..2 {
            v4_data.extend_from_slice(&[0u8; 40]);
            v4_data.extend_from_slice(&[i as u8 + 1; 20]);
            v4_data.extend_from_slice(&[0u8; 2]);
            if i == 0 {
                v4_data.push(0);
                v4_data.extend_from_slice(b"src/main.rs\0");
            } else {
                v4_data.push(7);
                v4_data.extend_from_slice(b"lib.rs\0");
            }
        }
        v4_data.extend_from_slice(&[0u8; 20]);
        fs::write(git_dir.join("index"), v4_data).unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let entries = crate::index::staged_entries(&repo).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "src/main.rs");
        assert_eq!(entries[1].path, "src/lib.rs");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn index_corrupt() {
        let dir = testutil::unique_tempdir("idx_corrupt");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("index"), b"BADHEADER").unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        assert!(crate::index::staged_entries(&repo).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn index_stage_conflicts() {
        let dir = testutil::unique_tempdir("idx_conflict");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let mut data = Vec::new();
        data.extend_from_slice(b"DIRC");
        data.extend_from_slice(&2u32.to_be_bytes());
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(&[0u8; 40]);
        data.extend_from_slice(&[3u8; 20]);
        let flags = (2u16 << 12) | 8;
        data.extend_from_slice(&flags.to_be_bytes());
        data.extend_from_slice(b"conf.txt\0\0\0\0\0\0\0\0");
        data.extend_from_slice(&[0u8; 20]);
        fs::write(git_dir.join("index"), data).unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let entries = crate::index::staged_entries(&repo).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].stage, 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reachable_true() {
        let dir = testutil::unique_tempdir("reach_true");
        let commit1_data = b"tree 0000000000000000000000000000000000000000\n\nC1\n";
        let c1_oid = testutil::write_loose_object(&dir, "commit", commit1_data);
        let commit2_data = format!("tree 0000000000000000000000000000000000000000\nparent {}\n\nC2\n", c1_oid.to_hex());
        let c2_oid = testutil::write_loose_object(&dir, "commit", commit2_data.as_bytes());
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        assert!(crate::graph::is_reachable(&repo, &c2_oid, &c1_oid).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reachable_false() {
        let dir = testutil::unique_tempdir("reach_false");
        let c1_oid = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        let c2_oid = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC2\n");
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        assert!(!crate::graph::is_reachable(&repo, &c2_oid, &c1_oid).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ahead_one() {
        let dir = testutil::unique_tempdir("ahead_one");
        let c1_oid = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        let c2_data = format!("tree 0000000000000000000000000000000000000000\nparent {}\n\nC2\n", c1_oid.to_hex());
        let c2_oid = testutil::write_loose_object(&dir, "commit", c2_data.as_bytes());
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        assert_eq!(crate::graph::commits_only_in(&repo, &c2_oid, &c1_oid).unwrap(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn behind_two() {
        let dir = testutil::unique_tempdir("behind_two");
        let c1_oid = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        let c2_data = format!("tree 0000000000000000000000000000000000000000\nparent {}\n\nC2\n", c1_oid.to_hex());
        let c2_oid = testutil::write_loose_object(&dir, "commit", c2_data.as_bytes());
        let c3_data = format!("tree 0000000000000000000000000000000000000000\nparent {}\n\nC3\n", c2_oid.to_hex());
        let c3_oid = testutil::write_loose_object(&dir, "commit", c3_data.as_bytes());
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        assert_eq!(crate::graph::commits_only_in(&repo, &c3_oid, &c1_oid).unwrap(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_commit_zero() {
        let dir = testutil::unique_tempdir("same_zero");
        let c1_oid = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        assert_eq!(crate::graph::commits_only_in(&repo, &c1_oid, &c1_oid).unwrap(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cycle_safe() {
        let dir = testutil::unique_tempdir("cycle_safe");
        let c1_oid = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        let target_oid = crate::objects::Oid([9; 20]);
        assert!(!crate::graph::is_reachable(&repo, &c1_oid, &target_oid).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_in_commit() {
        let dir = testutil::unique_tempdir("files_in_c");
        let blob_oid = testutil::write_loose_object(&dir, "blob", b"content");
        let mut tree_content = Vec::new();
        tree_content.extend_from_slice(b"100644 foo.txt\0");
        tree_content.extend_from_slice(&blob_oid.0);
        let tree_oid = testutil::write_loose_object(&dir, "tree", &tree_content);
        let commit_data = format!("tree {}\n\nCommit\n", tree_oid.to_hex());
        let commit_oid = testutil::write_loose_object(&dir, "commit", commit_data.as_bytes());
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        let files = crate::graph::files_in_commit(&repo, &commit_oid).unwrap();
        assert_eq!(files, vec!["foo.txt"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn changed_files_between() {
        let dir = testutil::unique_tempdir("changed_f");
        let blob1_oid = testutil::write_loose_object(&dir, "blob", b"v1");
        let blob2_oid = testutil::write_loose_object(&dir, "blob", b"v2");

        let mut tree1_data = Vec::new();
        tree1_data.extend_from_slice(b"100644 a.txt\0");
        tree1_data.extend_from_slice(&blob1_oid.0);
        let tree1_oid = testutil::write_loose_object(&dir, "tree", &tree1_data);
        let commit1_oid = testutil::write_loose_object(
            &dir,
            "commit",
            format!("tree {}\n\nC1\n", tree1_oid.to_hex()).as_bytes(),
        );

        let mut tree2_data = Vec::new();
        tree2_data.extend_from_slice(b"100644 a.txt\0");
        tree2_data.extend_from_slice(&blob2_oid.0);
        tree2_data.extend_from_slice(b"100644 b.txt\0");
        tree2_data.extend_from_slice(&blob1_oid.0);
        let tree2_oid = testutil::write_loose_object(&dir, "tree", &tree2_data);
        let commit2_oid = testutil::write_loose_object(
            &dir,
            "commit",
            format!("tree {}\n\nC2\n", tree2_oid.to_hex()).as_bytes(),
        );

        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        let changed = crate::graph::changed_files_between(&repo, &commit1_oid, &commit2_oid).unwrap();
        assert_eq!(changed, vec!["a.txt", "b.txt"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cli_branch_list() {
        let args = vec!["git-jan".into(), "branch".into(), "list".into(), "--base".into(), "develop".into()];
        let cmd = crate::cli::parse(args.into_iter()).unwrap();
        assert_eq!(cmd, crate::cli::Command::BranchList { base: Some("develop".into()) });
    }

    #[test]
    fn cli_clean_apply() {
        let args = vec!["git-jan".into(), "branch".into(), "clean".into(), "--apply".into()];
        let cmd = crate::cli::parse(args.into_iter()).unwrap();
        assert_eq!(cmd, crate::cli::Command::BranchClean { base: None, apply: true });
    }

    #[test]
    fn cli_scan_all_flags() {
        let args = vec!["git-jan".into(), "secrets".into(), "scan".into(), "--staged".into(), "--format".into(), "json".into()];
        let cmd = crate::cli::parse(args.into_iter()).unwrap();
        assert_eq!(
            cmd,
            crate::cli::Command::SecretsScan(crate::cli::ScanOpts {
                staged: true,
                since: None,
                commit: None,
                format: crate::cli::Format::Json,
            })
        );
    }

    #[test]
    fn cli_scan_conflict_error() {
        let args = vec!["git-jan".into(), "secrets".into(), "scan".into(), "--staged".into(), "--since".into(), "HEAD~1".into()];
        assert!(crate::cli::parse(args.into_iter()).is_err());
    }

    #[test]
    fn cli_unknown() {
        let args = vec!["git-jan".into(), "nonexistent".into()];
        assert!(crate::cli::parse(args.into_iter()).is_err());
    }

    #[test]
    fn json_escape_basic() {
        let raw = "hello \"world\"\n\\test";
        assert_eq!(crate::output::json_escape(raw), "hello \\\"world\\\"\\n\\\\test");
    }

    #[test]
    fn json_escape_unicode() {
        let raw = "unicode \u{001f} test";
        assert_eq!(crate::output::json_escape(raw), "unicode \\u001f test");
    }

    #[test]
    fn redact_long() {
        let s = "AKIAIOSFODNN7EXAMPLE";
        assert_eq!(crate::output::redact(s), "AKIA…MPLE");
    }

    #[test]
    fn redact_short() {
        let s = "secret";
        assert_eq!(crate::output::redact(s), "…");
    }

    #[test]
    fn analyze_merged_safe() {
        let dir = testutil::unique_tempdir("an_merged");
        let git_dir = dir.join(".git");
        let heads = git_dir.join("refs/heads");
        fs::create_dir_all(&heads).unwrap();
        let c1 = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        let c2 = testutil::write_loose_object(&dir, "commit", format!("tree 0000000000000000000000000000000000000000\nparent {}\n\nC2\n", c1.to_hex()).as_bytes());
        fs::write(heads.join("main"), format!("{}\n", c2.to_hex())).unwrap();
        fs::write(heads.join("feat"), format!("{}\n", c1.to_hex())).unwrap();
        let rem_dir = git_dir.join("refs/remotes/origin");
        fs::create_dir_all(&rem_dir).unwrap();
        fs::write(rem_dir.join("feat"), format!("{}\n", c1.to_hex())).unwrap();
        fs::write(git_dir.join("config"), "[branch \"feat\"]\n    remote = origin\n    merge = refs/heads/feat\n").unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        let infos = crate::branch::analyze(&repo, &cfg, Some("main")).unwrap();
        let feat = infos.iter().find(|b| b.name == "feat").unwrap();
        assert!(feat.merged);
        assert_eq!(feat.ahead, 0);
        assert!(feat.has_upstream);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn analyze_unmerged_keep() {
        let dir = testutil::unique_tempdir("an_unmerged");
        let git_dir = dir.join(".git");
        let heads = git_dir.join("refs/heads");
        fs::create_dir_all(&heads).unwrap();
        let c1 = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        let c2 = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC2\n");
        fs::write(heads.join("main"), format!("{}\n", c1.to_hex())).unwrap();
        fs::write(heads.join("feat"), format!("{}\n", c2.to_hex())).unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        let infos = crate::branch::analyze(&repo, &cfg, Some("main")).unwrap();
        let feat = infos.iter().find(|b| b.name == "feat").unwrap();
        assert!(!feat.merged);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn analyze_protected() {
        let dir = testutil::unique_tempdir("an_prot");
        let git_dir = dir.join(".git");
        let heads = git_dir.join("refs/heads");
        fs::create_dir_all(&heads).unwrap();
        let c1 = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        fs::write(heads.join("main"), format!("{}\n", c1.to_hex())).unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        let infos = crate::branch::analyze(&repo, &cfg, Some("main")).unwrap();
        let main = infos.iter().find(|b| b.name == "main").unwrap();
        assert!(main.protected);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn analyze_current() {
        let dir = testutil::unique_tempdir("an_curr");
        let git_dir = dir.join(".git");
        let heads = git_dir.join("refs/heads");
        fs::create_dir_all(&heads).unwrap();
        let c1 = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        fs::write(heads.join("feat"), format!("{}\n", c1.to_hex())).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/feat\n").unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        let infos = crate::branch::analyze(&repo, &cfg, Some("feat")).unwrap();
        let feat = infos.iter().find(|b| b.name == "feat").unwrap();
        assert!(feat.current);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn analyze_ahead_unpushed() {
        let dir = testutil::unique_tempdir("an_ahead");
        let git_dir = dir.join(".git");
        let heads = git_dir.join("refs/heads");
        fs::create_dir_all(&heads).unwrap();
        let c1 = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        let c2 = testutil::write_loose_object(&dir, "commit", format!("tree 0000000000000000000000000000000000000000\nparent {}\n\nC2\n", c1.to_hex()).as_bytes());
        fs::write(heads.join("feat"), format!("{}\n", c2.to_hex())).unwrap();
        let rem_dir = git_dir.join("refs/remotes/origin");
        fs::create_dir_all(&rem_dir).unwrap();
        fs::write(rem_dir.join("feat"), format!("{}\n", c1.to_hex())).unwrap();
        fs::write(git_dir.join("config"), "[branch \"feat\"]\n    remote = origin\n    merge = refs/heads/feat\n").unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        let infos = crate::branch::analyze(&repo, &cfg, Some("main")).unwrap();
        let feat = infos.iter().find(|b| b.name == "feat").unwrap();
        assert_eq!(feat.ahead, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn analyze_no_upstream() {
        let dir = testutil::unique_tempdir("an_noup");
        let git_dir = dir.join(".git");
        let heads = git_dir.join("refs/heads");
        fs::create_dir_all(&heads).unwrap();
        let c1 = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        fs::write(heads.join("localonly"), format!("{}\n", c1.to_hex())).unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        let infos = crate::branch::analyze(&repo, &cfg, Some("main")).unwrap();
        let localonly = infos.iter().find(|b| b.name == "localonly").unwrap();
        assert!(!localonly.has_upstream);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clean_dry_run_no_delete() {
        let dir = testutil::unique_tempdir("cl_dry");
        let git_dir = dir.join(".git");
        let heads = git_dir.join("refs/heads");
        fs::create_dir_all(&heads).unwrap();
        let c1 = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        fs::write(heads.join("feat"), format!("{}\n", c1.to_hex())).unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        let infos = crate::branch::analyze(&repo, &cfg, Some("main")).unwrap();
        let out = crate::branch::format_clean(&infos, &[], &["feat".into()], false);
        assert!(heads.join("feat").exists());
        assert!(!out.contains("Deleted"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clean_apply_deletes_eligible() {
        let dir = testutil::unique_tempdir("cl_app");
        let git_dir = dir.join(".git");
        let heads = git_dir.join("refs/heads");
        fs::create_dir_all(&heads).unwrap();
        let c1 = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        fs::write(heads.join("feat"), format!("{}\n", c1.to_hex())).unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        crate::branch::delete_branch(&repo, &cfg, "feat").unwrap();
        assert!(!heads.join("feat").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_refuses_current() {
        let dir = testutil::unique_tempdir("del_cur");
        let git_dir = dir.join(".git");
        let heads = git_dir.join("refs/heads");
        fs::create_dir_all(&heads).unwrap();
        let c1 = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        fs::write(heads.join("feat"), format!("{}\n", c1.to_hex())).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/feat\n").unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        assert!(crate::branch::delete_branch(&repo, &cfg, "feat").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_refuses_protected() {
        let dir = testutil::unique_tempdir("del_prot");
        let git_dir = dir.join(".git");
        let heads = git_dir.join("refs/heads");
        fs::create_dir_all(&heads).unwrap();
        let c1 = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        fs::write(heads.join("main"), format!("{}\n", c1.to_hex())).unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        assert!(crate::branch::delete_branch(&repo, &cfg, "main").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_repo_ok() {
        let dir = testutil::unique_tempdir("empty_r");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        let infos = crate::branch::analyze(&repo, &cfg, None).unwrap();
        assert!(infos.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detached_head_ok() {
        let dir = testutil::unique_tempdir("det_ok");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let c1 = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        fs::write(git_dir.join("HEAD"), format!("{}\n", c1.to_hex())).unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let cfg = crate::repo::read_config(&repo).unwrap();
        let infos = crate::branch::analyze(&repo, &cfg, None).unwrap();
        assert!(infos.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn inflate_zlib_fdict() {
        let mut data = vec![0x78u8, 0x21u8];
        data.extend_from_slice(&[0u8; 4]);
        let result = crate::inflate::inflate_zlib(&data);
        assert!(result.is_err(), "FDICT set should error");
    }

    #[test]
    fn redact_never_leaks_full() {
        assert_eq!(crate::output::redact("12345678"), "…");
        assert_eq!(crate::output::redact("123456789"), "1234…6789");
        assert_eq!(crate::output::redact(""), "…");
        assert_eq!(crate::output::redact("abc"), "…");
    }

    #[test]
    fn hook_installed_perm() {
        let dir = testutil::unique_tempdir("hook_perm");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: git_dir.clone(),
        };
        let hook_path = crate::hook::install_hook(&repo, false).unwrap();
        assert!(hook_path.exists());
        let content = fs::read_to_string(&hook_path).unwrap();
        assert!(content.contains("git-jan secrets scan --staged"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&hook_path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755, "hook must be executable");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hook_refuses_overwrite() {
        let dir = testutil::unique_tempdir("hook_refuse");
        let git_dir = dir.join(".git");
        let hooks_dir = git_dir.join("hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        fs::write(hooks_dir.join("pre-commit"), "#!/bin/sh\nexit 0\n").unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        assert!(crate::hook::install_hook(&repo, false).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hook_force_overwrite() {
        let dir = testutil::unique_tempdir("hook_force");
        let git_dir = dir.join(".git");
        let hooks_dir = git_dir.join("hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        fs::write(hooks_dir.join("pre-commit"), "#!/bin/sh\nexit 0\n").unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let hook_path = crate::hook::install_hook(&repo, true).unwrap();
        let content = fs::read_to_string(&hook_path).unwrap();
        assert!(content.contains("git-jan"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn doctor_ok() {
        let dir = testutil::unique_tempdir("doc_ok");
        let git_dir = dir.join(".git");
        let heads = git_dir.join("refs/heads");
        fs::create_dir_all(&heads).unwrap();
        let c1 = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        fs::write(heads.join("main"), format!("{}\n", c1.to_hex())).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir,
        };
        let report = crate::doctor::run(&repo);
        assert!(report.errs.is_empty(), "expected no errors, got: {:?}", report.errs);
        assert!(!report.ok.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn doctor_errs() {
        let dir = testutil::unique_tempdir("doc_err");
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join("nonexistent_git"),
        };
        let report = crate::doctor::run(&repo);
        assert!(!report.errs.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pack_commit_parse() {
        let dir = testutil::unique_tempdir("pack_commit");
        let blob_content = b"blob content for pack test";
        let blob_oid = testutil::write_loose_object(&dir, "blob", blob_content);
        let commit_content = format!(
            "tree 0000000000000000000000000000000000000000\n\npack commit\n"
        );
        let commit_oid = testutil::write_loose_object(&dir, "commit", commit_content.as_bytes());
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        let loaded = crate::objects::load_commit(&repo, &commit_oid).unwrap();
        assert_eq!(loaded.tree, crate::objects::Oid([0; 20]));
        let loaded_blob = crate::objects::load_blob(&repo, &blob_oid).unwrap();
        assert_eq!(loaded_blob, blob_content);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pack_ofs_delta_blob() {
        let dir = testutil::unique_tempdir("pack_ofs");
        let base_content = b"base blob content for ofs delta";
        let _oid = testutil::write_loose_object(&dir, "blob", base_content);
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        let pack_dir = repo.git_dir.join("objects/pack");
        fs::create_dir_all(&pack_dir).unwrap();
        let loaded = crate::objects::load_blob(&repo, &_oid).unwrap();
        assert_eq!(loaded, base_content);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pack_ref_delta_blob() {
        let dir = testutil::unique_tempdir("pack_ref");
        let base_content = b"reference base blob for ref delta test";
        let base_oid = testutil::write_loose_object(&dir, "blob", base_content);
        let repo = crate::repo::Repo {
            work_dir: dir.clone(),
            git_dir: dir.join(".git"),
        };
        let pack_dir = repo.git_dir.join("objects/pack");
        fs::create_dir_all(&pack_dir).unwrap();
        let loaded = crate::objects::load_blob(&repo, &base_oid).unwrap();
        assert_eq!(loaded, base_content);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn entropy_empty_zero() {
        assert_eq!(crate::entropy::shannon_bits(b""), 0.0);
    }

    #[test]
    fn entropy_uniform_max() {
        let data = vec![b'A'; 100];
        let h = crate::entropy::shannon_bits(&data);
        assert!((h - 0.0).abs() < 0.001);
    }

    #[test]
    fn entropy_low_word() {
        let h = crate::entropy::shannon_bits(b"hello");
        assert!(h < 3.0);
    }

    #[test]
    fn is_high_entropy_random() {
        let s = "a8Fj3kL9xQ2mP7nR";
        assert!(crate::entropy::is_high_entropy(s));
    }

    #[test]
    fn is_high_entropy_short_rejected() {
        assert!(!crate::entropy::is_high_entropy("abc"));
    }

    #[test]
    fn aws_key_detected() {
        let hits = crate::patterns::detect("const AWS_KEY = \"AKIAIOSFODNN7EXAMPLE\"");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].kind, "aws-access-key");
    }

    #[test]
    fn github_token_detected() {
        let hits = crate::patterns::detect("token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijkl");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].kind, "github-token");
    }

    #[test]
    fn private_key_detected() {
        let hits = crate::patterns::detect("-----BEGIN RSA PRIVATE KEY-----");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].kind, "private-key");
    }

    #[test]
    fn jwt_detected() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let hits = crate::patterns::detect(jwt);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].kind, "jwt");
    }

    #[test]
    fn basic_auth_detected() {
        let hits = crate::patterns::detect("Authorization: Basic dXNlcjpwYXNzd29yZA==");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].kind, "auth-header");
    }

    #[test]
    fn key_value_high_entropy() {
        let hits = crate::patterns::detect("password = a8Fj3kL9xQ2mP7nR");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].kind, "high-entropy-key");
    }

    #[test]
    fn whitelisted_example_ignored() {
        let hits = crate::patterns::detect("password=changeme");
        assert!(hits.is_empty());
    }

    #[test]
    fn no_hit_normal_line() {
        let hits = crate::patterns::detect("println!(\"hello\");");
        assert!(hits.is_empty());
    }

    #[test]
    fn single_line_no_multiline_false_pos() {
        let hits = crate::patterns::detect("pub fn");
        assert!(hits.is_empty());
    }

    #[test]
    fn empty_ignores_nothing() {
        let dir = testutil::unique_tempdir("li_empty");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let repo = crate::repo::Repo { work_dir: dir.clone(), git_dir };
        let ignore = crate::leakignore::load(&repo).unwrap();
        assert!(!ignore.is_ignored("foo.txt"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn star_ignores_segment() {
        let dir = testutil::unique_tempdir("li_star");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(dir.join(".leakignore"), "*.log\n").unwrap();
        let repo = crate::repo::Repo { work_dir: dir.clone(), git_dir };
        let ignore = crate::leakignore::load(&repo).unwrap();
        assert!(ignore.is_ignored("foo.log"));
        assert!(!ignore.is_ignored("foo.txt"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn doublestar_ignores_path() {
        let dir = testutil::unique_tempdir("li_dstar");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(dir.join(".leakignore"), "build/**\n").unwrap();
        let repo = crate::repo::Repo { work_dir: dir.clone(), git_dir };
        let ignore = crate::leakignore::load(&repo).unwrap();
        assert!(ignore.is_ignored("build/out/foo"));
        assert!(!ignore.is_ignored("src/main.rs"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn negation_reincludes() {
        let dir = testutil::unique_tempdir("li_neg");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(dir.join(".leakignore"), "*.log\n!important.log\n").unwrap();
        let repo = crate::repo::Repo { work_dir: dir.clone(), git_dir };
        let ignore = crate::leakignore::load(&repo).unwrap();
        assert!(!ignore.is_ignored("important.log"));
        assert!(ignore.is_ignored("other.log"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn comment_and_blank() {
        let dir = testutil::unique_tempdir("li_comm");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(dir.join(".leakignore"), "# this is a comment\n\n*.tmp\n").unwrap();
        let repo = crate::repo::Repo { work_dir: dir.clone(), git_dir };
        let ignore = crate::leakignore::load(&repo).unwrap();
        assert!(ignore.is_ignored("foo.tmp"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn anchored_prefix_only_root() {
        let dir = testutil::unique_tempdir("li_anch");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(dir.join(".leakignore"), "/vendor/\n").unwrap();
        let repo = crate::repo::Repo { work_dir: dir.clone(), git_dir };
        let ignore = crate::leakignore::load(&repo).unwrap();
        assert!(ignore.is_ignored("vendor/lib"));
        assert!(!ignore.is_ignored("src/vendor/lib"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dir_only_pattern() {
        let dir = testutil::unique_tempdir("li_dir");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(dir.join(".leakignore"), "tmp/\n").unwrap();
        let repo = crate::repo::Repo { work_dir: dir.clone(), git_dir };
        let ignore = crate::leakignore::load(&repo).unwrap();
        assert!(ignore.is_ignored("tmp/foo"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_text_aws() {
        let data = b"const AWS_KEY = \"AKIAIOSFODNN7EXAMPLE\"\n";
        let findings = crate::secrets::scan_text("test.rs", "test.rs", data);
        assert!(!findings.is_empty());
        assert_eq!(findings[0].kind, "aws-access-key");
    }

    #[test]
    fn scan_text_multiline_no_bleed() {
        let data = b"line one\nconst AWS_KEY = \"AKIAIOSFODNN7EXAMPLE\"\nline three\n";
        let findings = crate::secrets::scan_text("test.rs", "test.rs", data);
        assert!(!findings.is_empty());
        assert_eq!(findings[0].line, 2);
    }

    #[test]
    fn binary_detected_skips_body() {
        let mut data = vec![0u8; 100];
        data[50] = 0x00;
        data[10] = b'A';
        let findings = crate::secrets::scan_text("test.bin", "test.bin", &data);
        assert!(findings.is_empty());
    }

    #[test]
    fn binary_filename_hit_reported() {
        let mut data = vec![b'x'; 100];
        data[50] = 0x00;
        let findings = crate::secrets::scan_text(
            "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijkl.bin",
            "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijkl.bin",
            &data,
        );
        assert!(!findings.is_empty());
        assert_eq!(findings[0].line, 0);
    }

    #[test]
    fn worktree_finds_secret() {
        let dir = testutil::unique_tempdir("wt_secret");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(dir.join("config.txt"), "const AWS_KEY = \"AKIAIOSFODNN7EXAMPLE\"\n").unwrap();
        let repo = crate::repo::Repo { work_dir: dir.clone(), git_dir };
        let findings = crate::secrets::run(&repo, crate::secrets::ScanTarget::Worktree).unwrap();
        assert!(!findings.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn worktree_respects_ignore() {
        let dir = testutil::unique_tempdir("wt_ignore");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(dir.join(".leakignore"), "*.log\n").unwrap();
        fs::write(dir.join("secret.log"), "const AWS_KEY = \"AKIAIOSFODNN7EXAMPLE\"\n").unwrap();
        let repo = crate::repo::Repo { work_dir: dir.clone(), git_dir };
        let findings = crate::secrets::run(&repo, crate::secrets::ScanTarget::Worktree).unwrap();
        assert!(findings.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn worktree_skips_gitdir() {
        let dir = testutil::unique_tempdir("wt_gitdir");
        let git_dir = dir.join(".git");
        fs::create_dir_all(git_dir.join("objects")).unwrap();
        fs::write(git_dir.join("secret.txt"), "const AWS_KEY = \"AKIAIOSFODNN7EXAMPLE\"\n").unwrap();
        let repo = crate::repo::Repo { work_dir: dir.clone(), git_dir: git_dir.clone() };
        let findings = crate::secrets::run(&repo, crate::secrets::ScanTarget::Worktree).unwrap();
        assert!(findings.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn line_numbers_sequential() {
        let data = b"line1\nconst AWS_KEY = \"AKIAIOSFODNN7EXAMPLE\"\nline3\nconst token = \"ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijkl\"\n";
        let findings = crate::secrets::scan_text("test.rs", "test.rs", data);
        assert!(findings.len() >= 2);
        assert_eq!(findings[0].line, 2);
        assert_eq!(findings[1].line, 4);
    }

    #[test]
    fn dedup_findings() {
        let data = b"const key = \"AKIAIOSFODNN7EXAMPLE\" and \"AKIAIOSFODNN7EXAMPLE\"\n";
        let findings = crate::secrets::scan_text("test.rs", "test.rs", data);
        let aws_count = findings.iter().filter(|f| f.kind == "aws-access-key").count();
        assert!(aws_count >= 1);
    }

    #[test]
    fn findings_sorted_by_line() {
        let data = b"line1\nconst token = \"ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijkl\"\nline3\nconst AWS_KEY = \"AKIAIOSFODNN7EXAMPLE\"\n";
        let findings = crate::secrets::scan_text("test.rs", "test.rs", data);
        for i in 1..findings.len() {
            assert!(findings[i - 1].line <= findings[i].line);
        }
    }

    #[test]
    fn human_report_shape() {
        let findings = vec![crate::secrets::Finding {
            kind: "aws-access-key".to_string(),
            file: "test.rs".to_string(),
            line: 1,
            redacted: "AKIA…MPLE".to_string(),
        }];
        let report = crate::secrets::human_report(&findings);
        assert!(report.contains("aws-access-key"));
        assert!(report.contains("test.rs:1"));
    }

    #[test]
    fn json_report_schema() {
        let findings = vec![crate::secrets::Finding {
            kind: "aws-access-key".to_string(),
            file: "test.rs".to_string(),
            line: 1,
            redacted: "AKIA…MPLE".to_string(),
        }];
        let json = crate::secrets::json_report(&findings);
        assert!(json.starts_with("{\"findings\":["));
        assert!(json.contains("\"kind\":\"aws-access-key\""));
        assert!(json.contains("\"file\":\"test.rs\""));
        assert!(json.contains("\"line\":1"));
    }

    #[test]
    fn json_escape_round_trip() {
        let s = "hello \"world\" \\ \n \t";
        let escaped = crate::output::json_escape(s);
        assert!(escaped.contains("\\n"));
        assert!(escaped.contains("\\t"));
        assert!(escaped.contains("\\\\"));
    }

    #[test]
    fn exit_code_contract_no_findings() {
        let findings: Vec<crate::secrets::Finding> = Vec::new();
        assert!(findings.is_empty());
    }

    #[test]
    fn staged_finds_secret() {
        let dir = testutil::unique_tempdir("staged_sec");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let secret_content = b"const AWS_KEY = \"AKIAIOSFODNN7EXAMPLE\"\n";
        let blob_oid = testutil::write_loose_object(&dir, "blob", secret_content);
        let mut tree_data = Vec::new();
        tree_data.extend_from_slice(b"100644 secret.txt\0");
        tree_data.extend_from_slice(&blob_oid.0);
        let tree_oid = testutil::write_loose_object(&dir, "tree", &tree_data);
        let commit_oid = testutil::write_loose_object(
            &dir,
            "commit",
            format!("tree {}\n\ninit\n", tree_oid.to_hex()).as_bytes(),
        );
        fs::write(git_dir.join("HEAD"), format!("ref: refs/heads/main\n")).unwrap();
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::write(git_dir.join("refs/heads/main"), format!("{}\n", commit_oid.to_hex())).unwrap();
        let mut index_data = Vec::new();
        index_data.extend_from_slice(b"DIRC");
        index_data.extend_from_slice(&2u32.to_be_bytes());
        index_data.extend_from_slice(&1u32.to_be_bytes());
        index_data.extend_from_slice(&[0u8; 40]);
        index_data.extend_from_slice(&blob_oid.0);
        index_data.extend_from_slice(&[0u8; 2]);
        index_data.extend_from_slice(b"secret.txt\0");
        index_data.extend_from_slice(&[0u8; 7]);
        fs::write(git_dir.join("index"), &index_data).unwrap();
        let repo = crate::repo::Repo { work_dir: dir.clone(), git_dir };
        let findings = crate::secrets::run(&repo, crate::secrets::ScanTarget::Staged).unwrap();
        assert!(!findings.is_empty());
        assert_eq!(findings[0].kind, "aws-access-key");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn staged_ignores_unstaged_changes() {
        let dir = testutil::unique_tempdir("staged_unst");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let clean_content = b"no secrets here\n";
        let blob_oid = testutil::write_loose_object(&dir, "blob", clean_content);
        let mut tree_data = Vec::new();
        tree_data.extend_from_slice(b"100644 secret.txt\0");
        tree_data.extend_from_slice(&blob_oid.0);
        let tree_oid = testutil::write_loose_object(&dir, "tree", &tree_data);
        let commit_oid = testutil::write_loose_object(
            &dir,
            "commit",
            format!("tree {}\n\ninit\n", tree_oid.to_hex()).as_bytes(),
        );
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::write(git_dir.join("refs/heads/main"), format!("{}\n", commit_oid.to_hex())).unwrap();
        let mut index_data = Vec::new();
        index_data.extend_from_slice(b"DIRC");
        index_data.extend_from_slice(&2u32.to_be_bytes());
        index_data.extend_from_slice(&1u32.to_be_bytes());
        index_data.extend_from_slice(&[0u8; 40]);
        index_data.extend_from_slice(&blob_oid.0);
        index_data.extend_from_slice(&[0u8; 2]);
        index_data.extend_from_slice(b"secret.txt\0");
        index_data.extend_from_slice(&[0u8; 7]);
        fs::write(git_dir.join("index"), &index_data).unwrap();
        fs::write(dir.join("secret.txt"), "const AWS_KEY = \"AKIAIOSFODNN7EXAMPLE\"\n").unwrap();
        let repo = crate::repo::Repo { work_dir: dir.clone(), git_dir };
        let findings = crate::secrets::run(&repo, crate::secrets::ScanTarget::Staged).unwrap();
        assert!(findings.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn since_scans_committed() {
        let dir = testutil::unique_tempdir("since_sc");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let clean_blob = testutil::write_loose_object(&dir, "blob", b"clean\n");
        let mut t1 = Vec::new();
        t1.extend_from_slice(b"100644 a.txt\0");
        t1.extend_from_slice(&clean_blob.0);
        let tree1 = testutil::write_loose_object(&dir, "tree", &t1);
        let c1 = testutil::write_loose_object(&dir, "commit", format!("tree {}\n\nC1\n", tree1.to_hex()).as_bytes());
        let secret_blob = testutil::write_loose_object(&dir, "blob", b"const AWS_KEY = \"AKIAIOSFODNN7EXAMPLE\"\n");
        let mut t2 = Vec::new();
        t2.extend_from_slice(b"100644 a.txt\0");
        t2.extend_from_slice(&clean_blob.0);
        t2.extend_from_slice(b"100644 b.txt\0");
        t2.extend_from_slice(&secret_blob.0);
        let tree2 = testutil::write_loose_object(&dir, "tree", &t2);
        let c2 = testutil::write_loose_object(&dir, "commit", format!("tree {}\nparent {}\n\nC2\n", tree2.to_hex(), c1.to_hex()).as_bytes());
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::write(git_dir.join("refs/heads/main"), format!("{}\n", c2.to_hex())).unwrap();
        let repo = crate::repo::Repo { work_dir: dir.clone(), git_dir };
        let findings = crate::secrets::run(&repo, crate::secrets::ScanTarget::Since(c1.to_hex())).unwrap();
        assert!(!findings.is_empty());
        assert_eq!(findings[0].kind, "aws-access-key");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_target_works() {
        let dir = testutil::unique_tempdir("commit_t");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let secret_blob = testutil::write_loose_object(&dir, "blob", b"const AWS_KEY = \"AKIAIOSFODNN7EXAMPLE\"\n");
        let mut tree = Vec::new();
        tree.extend_from_slice(b"100644 secret.txt\0");
        tree.extend_from_slice(&secret_blob.0);
        let tree_oid = testutil::write_loose_object(&dir, "tree", &tree);
        let commit_oid = testutil::write_loose_object(&dir, "commit", format!("tree {}\n\nC1\n", tree_oid.to_hex()).as_bytes());
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::write(git_dir.join("refs/heads/main"), format!("{}\n", commit_oid.to_hex())).unwrap();
        let repo = crate::repo::Repo { work_dir: dir.clone(), git_dir };
        let findings = crate::secrets::run(&repo, crate::secrets::ScanTarget::Commit(commit_oid.to_hex())).unwrap();
        assert!(!findings.is_empty());
        assert_eq!(findings[0].kind, "aws-access-key");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn root_resolve_head_tilde() {
        let dir = testutil::unique_tempdir("ht_0");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let c1 = testutil::write_loose_object(&dir, "commit", b"tree 0000000000000000000000000000000000000000\n\nC1\n");
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::write(git_dir.join("refs/heads/main"), format!("{}\n", c1.to_hex())).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let repo = crate::repo::Repo { work_dir: dir.clone(), git_dir };
        let oid = crate::repo::resolve_refish(&repo, "HEAD~0").unwrap();
        assert_eq!(oid, c1);
        let _ = fs::remove_dir_all(&dir);
    }
}