use crate::{
    Error, Result,
    cache::Cache,
    chunker,
    config::{Config, DEFAULT_CHUNK_TARGET},
    crypto,
    discord::DiscordClient,
    inode::{ChunkRef, Inode, RootPointer},
};
use futures::stream::{self, StreamExt, TryStreamExt};
use serenity::all::MessageId;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const INODE_INLINE_LIMIT: usize = 1900;
const UPLOAD_PARALLELISM: usize = 4;
const DOWNLOAD_PARALLELISM: usize = 4;

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn parse_path(s: &str) -> Result<Vec<String>> {
    let trimmed = s.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let parts: Vec<String> = trimmed.split('/').map(String::from).collect();
    for p in &parts {
        if p.is_empty() {
            return Err(Error::Other(format!("empty path component in {s:?}")));
        }
        if p == "." || p == ".." {
            return Err(Error::Other(format!("invalid path component {p:?}")));
        }
    }
    Ok(parts)
}

pub struct Store {
    pub cfg: Config,
    pub disc: DiscordClient,
    pub cache: Arc<Cache>,
}

impl Store {
    pub fn open(cfg: Config) -> Result<Self> {
        let disc = DiscordClient::new(&cfg.bot_token, cfg.guild_id);
        let cache = Arc::new(Cache::open(&Cache::default_path()?)?);
        Ok(Self { cfg, disc, cache })
    }

    pub async fn init(token: String, guild_id: u64, data_channels: u8) -> Result<Config> {
        let disc = DiscordClient::new(&token, guild_id);
        let bot_name = disc.verify_token().await?;
        tracing::info!("authenticated as bot: {bot_name}");

        let category = disc.create_category("oubliette").await?;
        tracing::info!("category created: {category}");

        let metadata = disc.create_text_channel("fs-metadata", Some(category)).await?;
        tracing::info!("metadata channel: {metadata}");

        let mut data_ids = Vec::with_capacity(data_channels as usize);
        for i in 0..data_channels {
            let id = disc
                .create_text_channel(&format!("fs-data-{i}"), Some(category))
                .await?;
            tracing::info!("data channel {i}: {id}");
            data_ids.push(id);
        }

        let now = now_unix();
        let root = Inode::Dir {
            name: "/".to_string(),
            children: BTreeMap::new(),
            created_at: now,
            modified_at: now,
        };
        let root_json = serde_json::to_string(&root)?;
        let root_msg = disc.post_text(metadata, root_json).await?;

        let pointer = RootPointer {
            version: 1,
            root_inode_msg_id: root_msg.get(),
            generation: 0,
        };
        let pointer_json = serde_json::to_string(&pointer)?;
        let pointer_msg = disc.post_text(metadata, pointer_json).await?;

        Ok(Config {
            bot_token: token,
            guild_id,
            metadata_channel_id: metadata,
            data_channel_ids: data_ids,
            root_pointer_message_id: Some(pointer_msg.get()),
            master_key: crypto::random_master_key(),
            chunk_target: DEFAULT_CHUNK_TARGET,
        })
    }

    async fn post_inode(&self, inode: &Inode) -> Result<MessageId> {
        let json = serde_json::to_string(inode)?;
        if json.len() <= INODE_INLINE_LIMIT {
            self.disc.post_text(self.cfg.metadata_channel_id, json).await
        } else {
            self.disc
                .upload_chunk(self.cfg.metadata_channel_id, "inode.json", json.into_bytes())
                .await
        }
    }

    pub async fn read_inode(&self, msg_id: u64) -> Result<Inode> {
        if let Some(cached) = self.cache.get_inode(msg_id) {
            return Ok(cached);
        }
        let body = self
            .disc
            .fetch_message_body(self.cfg.metadata_channel_id, msg_id)
            .await?;
        let inode: Inode = serde_json::from_slice(&body)?;
        let _ = self.cache.put_inode(msg_id, &inode);
        Ok(inode)
    }

    pub async fn resolve_path(&self, path: &str) -> Result<u64> {
        let parts = parse_path(path)?;
        let root_ptr = self.load_root().await?;
        let mut current = root_ptr.root_inode_msg_id;
        for part in &parts {
            let inode = self.read_inode(current).await?;
            let children = match inode {
                Inode::Dir { children, .. } => children,
                _ => {
                    return Err(Error::Other(format!(
                        "path traverses through a file at {part:?}"
                    )));
                }
            };
            current = *children
                .get(part)
                .ok_or_else(|| Error::InodeNotFound(path.to_string()))?;
        }
        Ok(current)
    }

    pub async fn mkdir_p(&self, path: &str) -> Result<u64> {
        let parts = parse_path(path)?;
        let root_ptr = self.load_root().await?;
        let mut current_msg = root_ptr.root_inode_msg_id;

        for part in &parts {
            let mut dir = self.read_inode(current_msg).await?;
            let children = match &mut dir {
                Inode::Dir { children, .. } => children,
                _ => {
                    return Err(Error::Other(format!(
                        "cannot mkdir under a file at {part:?}"
                    )));
                }
            };

            if let Some(&existing) = children.get(part) {
                let existing_inode = self.read_inode(existing).await?;
                if !matches!(existing_inode, Inode::Dir { .. }) {
                    return Err(Error::Other(format!(
                        "{part:?} exists but is a file"
                    )));
                }
                current_msg = existing;
                continue;
            }

            let now = now_unix();
            let new_dir = Inode::Dir {
                name: part.clone(),
                children: BTreeMap::new(),
                created_at: now,
                modified_at: now,
            };
            let new_msg = self.post_inode(&new_dir).await?;
            children.insert(part.clone(), new_msg.get());
            if let Inode::Dir { modified_at, .. } = &mut dir {
                *modified_at = now;
            }
            self.write_dir(current_msg, &dir).await?;
            current_msg = new_msg.get();
        }
        Ok(current_msg)
    }

    async fn write_dir(&self, msg_id: u64, dir: &Inode) -> Result<()> {
        let json = serde_json::to_string(dir)?;
        if json.len() > INODE_INLINE_LIMIT {
            return Err(Error::Other(
                "directory grew too large to inline; multi-message dirs not yet implemented".into(),
            ));
        }
        self.disc
            .edit_text(self.cfg.metadata_channel_id, msg_id, json)
            .await?;
        Ok(())
    }

    pub async fn load_root(&self) -> Result<RootPointer> {
        let msg_id = self
            .cfg
            .root_pointer_message_id
            .ok_or_else(|| Error::Config("no root pointer; run `oubliette init`".into()))?;
        let raw = self
            .disc
            .fetch_text(self.cfg.metadata_channel_id, msg_id)
            .await?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub async fn put_file(&self, local: &Path, remote: &str) -> Result<()> {
        self.put_file_inner(local, remote, None).await
    }

    pub async fn put_file_with_progress(
        &self,
        local: &Path,
        remote: &str,
        progress: Arc<std::sync::atomic::AtomicU64>,
    ) -> Result<()> {
        self.put_file_inner(local, remote, Some(progress)).await
    }

    async fn put_file_inner(
        &self,
        local: &Path,
        remote: &str,
        progress: Option<Arc<std::sync::atomic::AtomicU64>>,
    ) -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};

        let parts = parse_path(remote)?;
        if parts.is_empty() {
            return Err(Error::Other("cannot put to /".into()));
        }
        let name = parts.last().unwrap().clone();
        let parent_path: String = if parts.len() == 1 {
            "/".to_string()
        } else {
            format!("/{}", parts[..parts.len() - 1].join("/"))
        };

        let meta = tokio::fs::metadata(local).await?;
        let file_size = meta.len();
        tracing::info!("file: {} bytes ({})", file_size, local.display());

        let chunk_specs = chunker::plan(file_size, self.cfg.chunk_target as u64);
        let total_chunks = chunk_specs.len();
        tracing::info!(
            "split into {} chunks, parallel upload x{}",
            total_chunks,
            UPLOAD_PARALLELISM
        );

        let data_channels = Arc::new(self.cfg.data_channel_ids.clone());
        let master_key = self.cfg.master_key;
        if data_channels.is_empty() {
            return Err(Error::Config("no data channels configured".into()));
        }

        let path: Arc<Path> = Arc::from(local);
        let cache = self.cache.clone();
        let progress_arc = progress;

        let chunk_results: Vec<(usize, ChunkRef)> = stream::iter(chunk_specs.into_iter().enumerate())
            .map(|(i, spec)| {
                let path = path.clone();
                let data_channels = data_channels.clone();
                let disc = self.disc.clone();
                let cache = cache.clone();
                let progress = progress_arc.clone();
                async move {
                    let mut file = tokio::fs::File::open(&*path).await?;
                    file.seek(std::io::SeekFrom::Start(spec.offset)).await?;
                    let mut buf = vec![0u8; spec.length as usize];
                    file.read_exact(&mut buf).await?;

                    let sha = crypto::sha256(&buf);
                    let _ = cache.put_chunk(&sha, &buf);
                    let (cipher, nonce) = crypto::encrypt_chunk(&master_key, &buf)?;
                    drop(buf);

                    let idx = u32::from_be_bytes([sha[0], sha[1], sha[2], sha[3]]) as usize
                        % data_channels.len();
                    let channel = data_channels[idx];
                    let filename = format!("c{}", hex::encode(&sha[..8]));
                    let cipher_len = cipher.len() as u32;
                    let msg_id = disc.upload_chunk(channel, &filename, cipher).await?;
                    if let Some(p) = progress.as_ref() {
                        p.fetch_add(spec.length as u64, std::sync::atomic::Ordering::Relaxed);
                    }
                    tracing::info!(
                        "chunk {}/{}: {} bytes -> msg {}",
                        i + 1,
                        total_chunks,
                        spec.length,
                        msg_id.get()
                    );
                    Ok::<(usize, ChunkRef), Error>((
                        i,
                        ChunkRef {
                            channel_id: channel,
                            message_id: msg_id.get(),
                            sha256: sha,
                            nonce,
                            size_cipher: cipher_len,
                            size_plain: spec.length,
                        },
                    ))
                }
            })
            .buffer_unordered(UPLOAD_PARALLELISM)
            .try_collect()
            .await?;

        let mut chunk_refs: Vec<Option<ChunkRef>> = vec![None; total_chunks];
        for (i, cref) in chunk_results {
            chunk_refs[i] = Some(cref);
        }
        let chunk_refs: Vec<ChunkRef> = chunk_refs
            .into_iter()
            .map(|c| c.expect("buffer_unordered yielded fewer chunks than expected"))
            .collect();

        let now = now_unix();
        let file_inode = Inode::File {
            name: name.clone(),
            size: file_size,
            chunks: chunk_refs,
            created_at: now,
            modified_at: now,
        };
        let file_msg = self.post_inode(&file_inode).await?;
        let _ = self.cache.put_inode(file_msg.get(), &file_inode);
        tracing::info!("file inode posted: msg {}", file_msg.get());

        let parent_msg = self.mkdir_p(&parent_path).await?;
        let mut parent = self.read_inode(parent_msg).await?;
        match &mut parent {
            Inode::Dir { children, modified_at, .. } => {
                children.insert(name, file_msg.get());
                *modified_at = now;
            }
            _ => return Err(Error::Other("parent is not a directory".into())),
        }
        self.write_dir(parent_msg, &parent).await?;
        tracing::info!("parent dir updated");

        Ok(())
    }

    pub async fn get_file(&self, remote: &str, local: &Path) -> Result<()> {
        let file_msg_id = self.resolve_path(remote).await?;
        let file_inode = self.read_inode(file_msg_id).await?;
        let chunks = match file_inode {
            Inode::File { chunks, .. } => chunks,
            _ => return Err(Error::Other(format!("{remote} is not a file"))),
        };
        let total = chunks.len();
        tracing::info!(
            "downloading {} chunks, parallelism x{}",
            total,
            DOWNLOAD_PARALLELISM
        );

        let master_key = self.cfg.master_key;
        let cache = self.cache.clone();
        let mut stream = stream::iter(chunks.into_iter().enumerate())
            .map(|(i, ch)| {
                let disc = self.disc.clone();
                let cache = cache.clone();
                async move {
                    if let Some(cached) = cache.get_chunk(&ch.sha256) {
                        let actual = crypto::sha256(&cached);
                        if actual == ch.sha256 {
                            tracing::debug!("chunk {} cache HIT", i + 1);
                            return Ok::<(usize, Vec<u8>), Error>((i, cached));
                        }
                        tracing::warn!("chunk {} cache corrupt, refetching", i + 1);
                    }
                    let cipher = disc.download_chunk(ch.channel_id, ch.message_id).await?;
                    let plain = crypto::decrypt_chunk(&master_key, &ch.nonce, &cipher)?;
                    let actual = crypto::sha256(&plain);
                    if actual != ch.sha256 {
                        return Err(Error::IntegrityFailure(hex::encode(ch.sha256)));
                    }
                    let _ = cache.put_chunk(&ch.sha256, &plain);
                    Ok::<(usize, Vec<u8>), Error>((i, plain))
                }
            })
            .buffered(DOWNLOAD_PARALLELISM);

        use tokio::io::AsyncWriteExt;
        let mut out = tokio::fs::File::create(local).await?;
        while let Some(result) = stream.next().await {
            let (i, plain) = result?;
            out.write_all(&plain).await?;
            tracing::info!("chunk {}/{} ok", i + 1, total);
        }
        out.flush().await?;
        Ok(())
    }

    pub async fn rename(&self, old: &str, new: &str, replace_if_exists: bool) -> Result<()> {
        let old_parts = parse_path(old)?;
        let new_parts = parse_path(new)?;
        if old_parts.is_empty() || new_parts.is_empty() {
            return Err(Error::Other("cannot rename /".into()));
        }
        let old_name = old_parts.last().unwrap().clone();
        let new_name = new_parts.last().unwrap().clone();
        let old_parent_path = if old_parts.len() == 1 {
            "/".to_string()
        } else {
            format!("/{}", old_parts[..old_parts.len() - 1].join("/"))
        };
        let new_parent_path = if new_parts.len() == 1 {
            "/".to_string()
        } else {
            format!("/{}", new_parts[..new_parts.len() - 1].join("/"))
        };

        let old_parent_msg = self.resolve_path(&old_parent_path).await?;
        let old_parent_inode = self.read_inode(old_parent_msg).await?;
        let entry_msg = match &old_parent_inode {
            Inode::Dir { children, .. } => *children
                .get(&old_name)
                .ok_or_else(|| Error::InodeNotFound(old.to_string()))?,
            _ => return Err(Error::Other("old parent is not a directory".into())),
        };

        let new_parent_msg = self.mkdir_p(&new_parent_path).await?;
        let now = now_unix();

        if old_parent_msg == new_parent_msg {
            let mut parent = self.read_inode(old_parent_msg).await?;
            match &mut parent {
                Inode::Dir { children, modified_at, .. } => {
                    if children.contains_key(&new_name) && new_name != old_name {
                        if !replace_if_exists {
                            return Err(Error::Other("target exists".into()));
                        }
                        children.remove(&new_name);
                    }
                    children.remove(&old_name);
                    children.insert(new_name, entry_msg);
                    *modified_at = now;
                }
                _ => return Err(Error::Other("not a directory".into())),
            }
            self.write_dir(old_parent_msg, &parent).await?;
        } else {
            let mut new_parent = self.read_inode(new_parent_msg).await?;
            match &mut new_parent {
                Inode::Dir { children, modified_at, .. } => {
                    if children.contains_key(&new_name) {
                        if !replace_if_exists {
                            return Err(Error::Other("target exists".into()));
                        }
                        children.remove(&new_name);
                    }
                    children.insert(new_name, entry_msg);
                    *modified_at = now;
                }
                _ => return Err(Error::Other("new parent not a directory".into())),
            }
            self.write_dir(new_parent_msg, &new_parent).await?;

            let mut old_parent = self.read_inode(old_parent_msg).await?;
            match &mut old_parent {
                Inode::Dir { children, modified_at, .. } => {
                    children.remove(&old_name);
                    *modified_at = now;
                }
                _ => return Err(Error::Other("old parent not a directory".into())),
            }
            self.write_dir(old_parent_msg, &old_parent).await?;
        }
        Ok(())
    }

    pub async fn unlink(&self, remote: &str) -> Result<()> {
        let parts = parse_path(remote)?;
        if parts.is_empty() {
            return Err(Error::Other("cannot unlink /".into()));
        }
        let name = parts.last().unwrap().clone();
        let parent_path = if parts.len() == 1 {
            "/".to_string()
        } else {
            format!("/{}", parts[..parts.len() - 1].join("/"))
        };
        let parent_msg = self.resolve_path(&parent_path).await?;
        let mut parent = self.read_inode(parent_msg).await?;
        match &mut parent {
            Inode::Dir { children, modified_at, .. } => {
                if children.remove(&name).is_none() {
                    return Err(Error::InodeNotFound(remote.to_string()));
                }
                *modified_at = now_unix();
            }
            _ => return Err(Error::Other("parent is not a directory".into())),
        }
        self.write_dir(parent_msg, &parent).await?;
        Ok(())
    }

    pub async fn list(&self, remote: &str) -> Result<Vec<Inode>> {
        let dir_msg = self.resolve_path(remote).await?;
        let dir = self.read_inode(dir_msg).await?;
        let children = match dir {
            Inode::Dir { children, .. } => children,
            _ => return Err(Error::Other(format!("{remote} is not a directory"))),
        };
        let mut out = Vec::with_capacity(children.len());
        for (_, msg_id) in children {
            out.push(self.read_inode(msg_id).await?);
        }
        Ok(out)
    }
}
