use crate::{Error, Result, inode::Inode};
use std::path::{Path, PathBuf};

pub struct Cache {
    inodes_dir: PathBuf,
    chunks_dir: PathBuf,
}

impl Cache {
    pub fn open(root: &Path) -> Result<Self> {
        let inodes_dir = root.join("inodes");
        let chunks_dir = root.join("chunks");
        std::fs::create_dir_all(&inodes_dir)?;
        std::fs::create_dir_all(&chunks_dir)?;
        Ok(Self {
            inodes_dir,
            chunks_dir,
        })
    }

    pub fn default_path() -> Result<PathBuf> {
        let base = dirs::config_dir()
            .ok_or_else(|| Error::Config("no config dir".into()))?;
        Ok(base.join("oubliette").join("cache"))
    }

    fn inode_path(&self, msg_id: u64) -> PathBuf {
        self.inodes_dir.join(format!("{msg_id}.json"))
    }

    fn chunk_path(&self, sha: &[u8; 32]) -> PathBuf {
        self.chunks_dir.join(format!("{}.bin", hex::encode(sha)))
    }

    pub fn get_inode(&self, msg_id: u64) -> Option<Inode> {
        let path = self.inode_path(msg_id);
        let bytes = std::fs::read(&path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn put_inode(&self, msg_id: u64, inode: &Inode) -> Result<()> {
        if !matches!(inode, Inode::File { .. }) {
            return Ok(());
        }
        let json = serde_json::to_vec(inode)?;
        write_atomic(&self.inode_path(msg_id), &json)
    }

    pub fn get_chunk(&self, sha: &[u8; 32]) -> Option<Vec<u8>> {
        std::fs::read(self.chunk_path(sha)).ok()
    }

    pub fn put_chunk(&self, sha: &[u8; 32], data: &[u8]) -> Result<()> {
        write_atomic(&self.chunk_path(sha), data)
    }

    pub fn stats(&self) -> Result<(u64, u64, u64, u64)> {
        let (ic, ib) = dir_count_bytes(&self.inodes_dir)?;
        let (cc, cb) = dir_count_bytes(&self.chunks_dir)?;
        Ok((ic, ib, cc, cb))
    }
}

fn write_atomic(target: &Path, data: &[u8]) -> Result<()> {
    let tmp = target.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, target)?;
    Ok(())
}

fn dir_count_bytes(dir: &Path) -> Result<(u64, u64)> {
    let mut count = 0u64;
    let mut bytes = 0u64;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            count += 1;
            bytes += meta.len();
        }
    }
    Ok((count, bytes))
}
