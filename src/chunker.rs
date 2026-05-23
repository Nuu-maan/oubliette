#[derive(Debug, Clone, Copy)]
pub struct ChunkSpec {
    pub offset: u64,
    pub length: u32,
}

pub fn plan(file_size: u64, target: u64) -> Vec<ChunkSpec> {
    if file_size == 0 || target == 0 {
        return Vec::new();
    }
    let n = file_size.div_ceil(target) as usize;
    let mut out = Vec::with_capacity(n);
    let mut offset = 0u64;
    while offset < file_size {
        let length = std::cmp::min(target, file_size - offset) as u32;
        out.push(ChunkSpec { offset, length });
        offset += length as u64;
    }
    out
}
