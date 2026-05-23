use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRef {
    pub channel_id: u64,
    pub message_id: u64,
    #[serde(with = "hex::serde")]
    pub sha256: [u8; 32],
    #[serde(with = "hex::serde")]
    pub nonce: [u8; 12],
    pub size_cipher: u32,
    pub size_plain: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Inode {
    File {
        name: String,
        size: u64,
        chunks: Vec<ChunkRef>,
        created_at: i64,
        modified_at: i64,
    },
    Dir {
        name: String,
        children: BTreeMap<String, u64>,
        created_at: i64,
        modified_at: i64,
    },
}

impl Inode {
    pub fn name(&self) -> &str {
        match self {
            Inode::File { name, .. } | Inode::Dir { name, .. } => name,
        }
    }

    pub fn is_dir(&self) -> bool {
        matches!(self, Inode::Dir { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootPointer {
    pub version: u64,
    pub root_inode_msg_id: u64,
    pub generation: u64,
}
