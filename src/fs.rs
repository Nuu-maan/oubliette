#![cfg(windows)]

use crate::{Error as OubError, crypto, inode::Inode, store::Store};
use std::ffi::{OsString, c_void};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::runtime::Runtime;
use windows::Win32::Foundation::{
    STATUS_ACCESS_DENIED, STATUS_DIRECTORY_NOT_EMPTY, STATUS_INVALID_DEVICE_REQUEST,
    STATUS_NOT_A_DIRECTORY, STATUS_OBJECT_NAME_NOT_FOUND,
};
use winfsp::U16CStr;
use winfsp::constants::FspCleanupFlags;
use winfsp::filesystem::{
    DirBuffer, DirInfo, DirMarker, FileInfo, FileSecurity, FileSystemContext, OpenFileInfo,
    VolumeInfo, WideNameInfo,
};
use winfsp_sys::FILE_ACCESS_RIGHTS;

const UNIX_EPOCH_TO_FILETIME_SECS: u64 = 11_644_473_600;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;

static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_writes_dir() -> std::io::Result<PathBuf> {
    let base = dirs::config_dir()
        .ok_or_else(|| std::io::Error::other("no config dir"))?
        .join("oubliette")
        .join("tmp_writes");
    std::fs::create_dir_all(&base)?;
    Ok(base)
}

fn next_temp_path() -> std::io::Result<PathBuf> {
    let n = WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    Ok(temp_writes_dir()?.join(format!("write-{pid}-{n}.bin")))
}

pub struct OublietteFs {
    pub store: Arc<Store>,
    pub runtime: Arc<Runtime>,
}

pub enum FileCtx {
    File(FileHandle),
    Dir(DirHandle),
}

pub struct FileHandle {
    pub msg_id: Option<u64>,
    pub remote_path: String,
    pub state: Mutex<FileState>,
}

#[derive(Default)]
pub struct FileState {
    pub writer: Option<FileWriter>,
    pub will_delete: bool,
}

pub struct FileWriter {
    pub temp_path: PathBuf,
    pub temp_file: std::fs::File,
    pub current_size: u64,
}

pub struct DirHandle {
    pub msg_id: u64,
    pub remote_path: String,
    pub buffer: DirBuffer,
    pub will_delete: Mutex<bool>,
}

fn unix_to_filetime(unix: i64) -> u64 {
    let u = unix.max(0) as u64;
    (u + UNIX_EPOCH_TO_FILETIME_SECS) * 10_000_000
}

fn now_filetime() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    unix_to_filetime(s)
}

fn win_path_to_oub(p: &U16CStr) -> String {
    let os: OsString = OsString::from_wide(p.as_slice());
    let s = os.to_string_lossy().replace('\\', "/");
    if s.is_empty() { "/".into() } else { s }
}

fn split_parent_name(path: &str) -> Option<(String, String)> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let parts: Vec<&str> = trimmed.split('/').collect();
    let name = parts.last().unwrap().to_string();
    let parent = if parts.len() == 1 {
        "/".to_string()
    } else {
        format!("/{}", parts[..parts.len() - 1].join("/"))
    };
    Some((parent, name))
}

fn fill_file_info_from_inode(file_info: &mut FileInfo, inode: &Inode) {
    let (size, ctime, mtime) = match inode {
        Inode::File { size, created_at, modified_at, .. } => (*size, *created_at, *modified_at),
        Inode::Dir { created_at, modified_at, .. } => (0u64, *created_at, *modified_at),
    };
    file_info.file_attributes = match inode {
        Inode::Dir { .. } => FILE_ATTRIBUTE_DIRECTORY,
        Inode::File { .. } => FILE_ATTRIBUTE_NORMAL,
    };
    file_info.reparse_tag = 0;
    file_info.allocation_size = (size + 4095) & !4095;
    file_info.file_size = size;
    file_info.creation_time = unix_to_filetime(ctime);
    file_info.last_access_time = unix_to_filetime(mtime);
    file_info.last_write_time = unix_to_filetime(mtime);
    file_info.change_time = unix_to_filetime(mtime);
    file_info.index_number = 0;
    file_info.hard_links = 0;
    file_info.ea_size = 0;
}

fn fill_file_info_for_writer(file_info: &mut FileInfo, size: u64, is_dir: bool) {
    file_info.file_attributes = if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL };
    file_info.reparse_tag = 0;
    file_info.allocation_size = (size + 4095) & !4095;
    file_info.file_size = size;
    let now = now_filetime();
    file_info.creation_time = now;
    file_info.last_access_time = now;
    file_info.last_write_time = now;
    file_info.change_time = now;
    file_info.index_number = 0;
    file_info.hard_links = 0;
    file_info.ea_size = 0;
}

impl OublietteFs {
    fn current_file_info(&self, handle: &FileHandle, file_info: &mut FileInfo) -> winfsp::Result<()> {
        let st = handle.state.lock().unwrap();
        if let Some(w) = st.writer.as_ref() {
            fill_file_info_for_writer(file_info, w.current_size, false);
            return Ok(());
        }
        let msg_id = handle.msg_id.ok_or(STATUS_OBJECT_NAME_NOT_FOUND)?;
        let inode = self
            .runtime
            .block_on(self.store.read_inode(msg_id))
            .map_err(|_| STATUS_OBJECT_NAME_NOT_FOUND)?;
        fill_file_info_from_inode(file_info, &inode);
        Ok(())
    }
}

impl FileSystemContext for OublietteFs {
    type FileContext = FileCtx;

    fn get_security_by_name(
        &self,
        file_name: &U16CStr,
        _security_descriptor: Option<&mut [c_void]>,
        _resolver: impl FnOnce(&U16CStr) -> Option<FileSecurity>,
    ) -> winfsp::Result<FileSecurity> {
        let path = win_path_to_oub(file_name);
        let inode = self
            .runtime
            .block_on(async {
                let msg = self.store.resolve_path(&path).await?;
                self.store.read_inode(msg).await
            })
            .map_err(|_| STATUS_OBJECT_NAME_NOT_FOUND)?;

        let attrs = match inode {
            Inode::Dir { .. } => FILE_ATTRIBUTE_DIRECTORY,
            Inode::File { .. } => FILE_ATTRIBUTE_NORMAL,
        };
        Ok(FileSecurity {
            reparse: false,
            sz_security_descriptor: 0,
            attributes: attrs,
        })
    }

    fn open(
        &self,
        file_name: &U16CStr,
        _create_options: u32,
        _granted_access: FILE_ACCESS_RIGHTS,
        file_info: &mut OpenFileInfo,
    ) -> winfsp::Result<Self::FileContext> {
        let path = win_path_to_oub(file_name);
        let (msg_id, inode) = self
            .runtime
            .block_on(async {
                let msg = self.store.resolve_path(&path).await?;
                let inode = self.store.read_inode(msg).await?;
                Ok::<_, OubError>((msg, inode))
            })
            .map_err(|_| STATUS_OBJECT_NAME_NOT_FOUND)?;

        fill_file_info_from_inode(file_info.as_mut(), &inode);

        Ok(match &inode {
            Inode::Dir { .. } => FileCtx::Dir(DirHandle {
                msg_id,
                remote_path: path,
                buffer: DirBuffer::new(),
                will_delete: Mutex::new(false),
            }),
            Inode::File { .. } => FileCtx::File(FileHandle {
                msg_id: Some(msg_id),
                remote_path: path,
                state: Mutex::new(FileState::default()),
            }),
        })
    }

    fn create(
        &self,
        file_name: &U16CStr,
        create_options: u32,
        _granted_access: FILE_ACCESS_RIGHTS,
        _file_attributes: winfsp_sys::FILE_FLAGS_AND_ATTRIBUTES,
        _security_descriptor: Option<&[c_void]>,
        _allocation_size: u64,
        _extra_buffer: Option<&[u8]>,
        _extra_buffer_is_reparse_point: bool,
        file_info: &mut OpenFileInfo,
    ) -> winfsp::Result<Self::FileContext> {
        let path = win_path_to_oub(file_name);
        let (parent_path, name) =
            split_parent_name(&path).ok_or(STATUS_OBJECT_NAME_NOT_FOUND)?;

        let is_dir = (create_options & FILE_DIRECTORY_FILE) != 0;

        if is_dir {
            let new_msg = self
                .runtime
                .block_on(self.store.mkdir_p(&path))
                .map_err(|_| STATUS_ACCESS_DENIED)?;
            fill_file_info_for_writer(file_info.as_mut(), 0, true);
            return Ok(FileCtx::Dir(DirHandle {
                msg_id: new_msg,
                remote_path: path,
                buffer: DirBuffer::new(),
                will_delete: Mutex::new(false),
            }));
        }

        // ensure parent exists; mkdir_p is idempotent
        self.runtime
            .block_on(self.store.mkdir_p(&parent_path))
            .map_err(|_| STATUS_ACCESS_DENIED)?;
        let _ = name;

        let temp_path = next_temp_path().map_err(|_| STATUS_ACCESS_DENIED)?;
        let temp_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&temp_path)
            .map_err(|_| STATUS_ACCESS_DENIED)?;

        fill_file_info_for_writer(file_info.as_mut(), 0, false);

        Ok(FileCtx::File(FileHandle {
            msg_id: None,
            remote_path: path,
            state: Mutex::new(FileState {
                writer: Some(FileWriter {
                    temp_path,
                    temp_file,
                    current_size: 0,
                }),
                will_delete: false,
            }),
        }))
    }

    fn overwrite(
        &self,
        context: &Self::FileContext,
        _file_attributes: winfsp_sys::FILE_FLAGS_AND_ATTRIBUTES,
        _replace_file_attributes: bool,
        _allocation_size: u64,
        _extra_buffer: Option<&[u8]>,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        // Promote this handle to writable + truncate.
        let handle = match context {
            FileCtx::File(h) => h,
            _ => return Err(STATUS_NOT_A_DIRECTORY.into()),
        };
        let mut st = handle.state.lock().unwrap();
        if st.writer.is_none() {
            let temp_path = next_temp_path().map_err(|_| STATUS_ACCESS_DENIED)?;
            let temp_file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(&temp_path)
                .map_err(|_| STATUS_ACCESS_DENIED)?;
            st.writer = Some(FileWriter {
                temp_path,
                temp_file,
                current_size: 0,
            });
        } else if let Some(w) = st.writer.as_mut() {
            w.temp_file.set_len(0).map_err(|_| STATUS_ACCESS_DENIED)?;
            w.current_size = 0;
        }
        fill_file_info_for_writer(file_info, 0, false);
        Ok(())
    }

    fn close(&self, context: Self::FileContext) {
        // Best-effort: clean up any leftover temp file (cleanup should've handled it).
        if let FileCtx::File(h) = context {
            if let Ok(st) = h.state.into_inner() {
                if let Some(w) = st.writer {
                    let _ = std::fs::remove_file(&w.temp_path);
                }
            }
        }
    }

    fn cleanup(&self, context: &Self::FileContext, _file_name: Option<&U16CStr>, flags: u32) {
        let want_delete = FspCleanupFlags::FspCleanupDelete.is_flagged(flags);

        match context {
            FileCtx::File(handle) => {
                let mut st = handle.state.lock().unwrap();
                if want_delete {
                    st.will_delete = true;
                }
                let will_delete = st.will_delete;
                if will_delete {
                    if let Some(w) = st.writer.take() {
                        drop(w.temp_file);
                        let _ = std::fs::remove_file(&w.temp_path);
                    }
                    if handle.msg_id.is_some() {
                        let _ = self
                            .runtime
                            .block_on(self.store.unlink(&handle.remote_path));
                    }
                    return;
                }
                if let Some(w) = st.writer.take() {
                    let temp_path = w.temp_path.clone();
                    drop(w.temp_file);
                    let res = self
                        .runtime
                        .block_on(self.store.put_file(&temp_path, &handle.remote_path));
                    if let Err(e) = res {
                        tracing::error!("commit failed for {}: {e}", handle.remote_path);
                    }
                    let _ = std::fs::remove_file(&temp_path);
                }
            }
            FileCtx::Dir(handle) => {
                let mut wd = handle.will_delete.lock().unwrap();
                if want_delete {
                    *wd = true;
                }
                if *wd {
                    let _ = self
                        .runtime
                        .block_on(self.store.unlink(&handle.remote_path));
                }
            }
        }
    }

    fn set_delete(
        &self,
        context: &Self::FileContext,
        _file_name: &U16CStr,
        delete_file: bool,
    ) -> winfsp::Result<()> {
        match context {
            FileCtx::File(h) => {
                h.state.lock().unwrap().will_delete = delete_file;
            }
            FileCtx::Dir(h) => {
                if delete_file {
                    let children_count = self
                        .runtime
                        .block_on(async {
                            let inode = self.store.read_inode(h.msg_id).await?;
                            match inode {
                                Inode::Dir { children, .. } => Ok::<_, OubError>(children.len()),
                                _ => Err(OubError::Other("not a dir".into())),
                            }
                        })
                        .map_err(|_| STATUS_INVALID_DEVICE_REQUEST)?;
                    if children_count > 0 {
                        return Err(STATUS_DIRECTORY_NOT_EMPTY.into());
                    }
                }
                *h.will_delete.lock().unwrap() = delete_file;
            }
        }
        Ok(())
    }

    fn flush(
        &self,
        _context: Option<&Self::FileContext>,
        _file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        Ok(())
    }

    fn get_file_info(
        &self,
        context: &Self::FileContext,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        match context {
            FileCtx::File(h) => self.current_file_info(h, file_info),
            FileCtx::Dir(h) => {
                let inode = self
                    .runtime
                    .block_on(self.store.read_inode(h.msg_id))
                    .map_err(|_| STATUS_OBJECT_NAME_NOT_FOUND)?;
                fill_file_info_from_inode(file_info, &inode);
                Ok(())
            }
        }
    }

    fn set_file_size(
        &self,
        context: &Self::FileContext,
        new_size: u64,
        _set_allocation_size: bool,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        let handle = match context {
            FileCtx::File(h) => h,
            _ => return Err(STATUS_NOT_A_DIRECTORY.into()),
        };
        let mut st = handle.state.lock().unwrap();
        let w = st.writer.as_mut().ok_or(STATUS_ACCESS_DENIED)?;
        w.temp_file
            .set_len(new_size)
            .map_err(|_| STATUS_ACCESS_DENIED)?;
        w.current_size = new_size;
        fill_file_info_for_writer(file_info, new_size, false);
        Ok(())
    }

    fn read(
        &self,
        context: &Self::FileContext,
        buffer: &mut [u8],
        offset: u64,
    ) -> winfsp::Result<u32> {
        let handle = match context {
            FileCtx::File(h) => h,
            _ => return Err(STATUS_INVALID_DEVICE_REQUEST.into()),
        };

        // If we have a writer (just-written file), read straight from temp.
        {
            let mut st = handle.state.lock().unwrap();
            if let Some(w) = st.writer.as_mut() {
                if offset >= w.current_size {
                    return Ok(0);
                }
                w.temp_file
                    .seek(SeekFrom::Start(offset))
                    .map_err(|_| STATUS_INVALID_DEVICE_REQUEST)?;
                let n = w
                    .temp_file
                    .read(buffer)
                    .map_err(|_| STATUS_INVALID_DEVICE_REQUEST)?;
                return Ok(n as u32);
            }
        }

        let msg_id = handle.msg_id.ok_or(STATUS_OBJECT_NAME_NOT_FOUND)?;

        let (chunks, total_size) = self
            .runtime
            .block_on(async {
                let inode = self.store.read_inode(msg_id).await?;
                match inode {
                    Inode::File { chunks, size, .. } => Ok((chunks, size)),
                    _ => Err(OubError::Other("not a file".into())),
                }
            })
            .map_err(|_| STATUS_INVALID_DEVICE_REQUEST)?;

        if offset >= total_size {
            return Ok(0);
        }

        let chunk_target = self.store.cfg.chunk_target as u64;
        let mut bytes_written = 0u32;
        let mut cur = offset;
        let end = std::cmp::min(offset + buffer.len() as u64, total_size);

        while cur < end {
            let idx = (cur / chunk_target) as usize;
            if idx >= chunks.len() {
                break;
            }
            let ch = chunks[idx].clone();
            let chunk_start = idx as u64 * chunk_target;
            let in_chunk = (cur - chunk_start) as usize;

            let plain = self
                .runtime
                .block_on(async {
                    if let Some(c) = self.store.cache.get_chunk(&ch.sha256) {
                        let actual = crypto::sha256(&c);
                        if actual == ch.sha256 {
                            return Ok::<_, OubError>(c);
                        }
                    }
                    let cipher = self
                        .store
                        .disc
                        .download_chunk(ch.channel_id, ch.message_id)
                        .await?;
                    let plain = crypto::decrypt_chunk(
                        &self.store.cfg.master_key,
                        &ch.nonce,
                        &cipher,
                    )?;
                    let actual = crypto::sha256(&plain);
                    if actual != ch.sha256 {
                        return Err(OubError::IntegrityFailure(hex::encode(ch.sha256)));
                    }
                    let _ = self.store.cache.put_chunk(&ch.sha256, &plain);
                    Ok(plain)
                })
                .map_err(|_| STATUS_INVALID_DEVICE_REQUEST)?;

            let available = plain.len().saturating_sub(in_chunk);
            let space = buffer.len() - bytes_written as usize;
            let to_copy = std::cmp::min(available, space);
            if to_copy == 0 {
                break;
            }
            let bw = bytes_written as usize;
            buffer[bw..bw + to_copy].copy_from_slice(&plain[in_chunk..in_chunk + to_copy]);
            bytes_written += to_copy as u32;
            cur += to_copy as u64;
        }

        Ok(bytes_written)
    }

    fn write(
        &self,
        context: &Self::FileContext,
        buffer: &[u8],
        offset: u64,
        write_to_eof: bool,
        constrained_io: bool,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<u32> {
        let handle = match context {
            FileCtx::File(h) => h,
            _ => return Err(STATUS_NOT_A_DIRECTORY.into()),
        };
        let mut st = handle.state.lock().unwrap();
        let w = st.writer.as_mut().ok_or(STATUS_ACCESS_DENIED)?;

        if constrained_io {
            if offset >= w.current_size {
                fill_file_info_for_writer(file_info, w.current_size, false);
                return Ok(0);
            }
            let space = w.current_size - offset;
            let to_write = std::cmp::min(buffer.len() as u64, space) as usize;
            w.temp_file
                .seek(SeekFrom::Start(offset))
                .map_err(|_| STATUS_INVALID_DEVICE_REQUEST)?;
            w.temp_file
                .write_all(&buffer[..to_write])
                .map_err(|_| STATUS_INVALID_DEVICE_REQUEST)?;
            fill_file_info_for_writer(file_info, w.current_size, false);
            return Ok(to_write as u32);
        }

        let pos = if write_to_eof { w.current_size } else { offset };
        w.temp_file
            .seek(SeekFrom::Start(pos))
            .map_err(|_| STATUS_INVALID_DEVICE_REQUEST)?;
        w.temp_file
            .write_all(buffer)
            .map_err(|_| STATUS_INVALID_DEVICE_REQUEST)?;

        let written_end = pos + buffer.len() as u64;
        if written_end > w.current_size {
            w.current_size = written_end;
        }
        fill_file_info_for_writer(file_info, w.current_size, false);
        Ok(buffer.len() as u32)
    }

    fn rename(
        &self,
        _context: &Self::FileContext,
        file_name: &U16CStr,
        new_file_name: &U16CStr,
        replace_if_exists: bool,
    ) -> winfsp::Result<()> {
        let old = win_path_to_oub(file_name);
        let new = win_path_to_oub(new_file_name);
        self.runtime
            .block_on(self.store.rename(&old, &new, replace_if_exists))
            .map_err(|e| {
                tracing::warn!("rename {old} -> {new} failed: {e}");
                STATUS_ACCESS_DENIED
            })?;
        Ok(())
    }

    fn read_directory(
        &self,
        context: &Self::FileContext,
        _pattern: Option<&U16CStr>,
        marker: DirMarker,
        buffer: &mut [u8],
    ) -> winfsp::Result<u32> {
        let handle = match context {
            FileCtx::Dir(h) => h,
            _ => return Err(STATUS_NOT_A_DIRECTORY.into()),
        };

        if marker.is_none() {
            let children = self
                .runtime
                .block_on(async {
                    let inode = self.store.read_inode(handle.msg_id).await?;
                    match inode {
                        Inode::Dir { children, .. } => Ok::<_, OubError>(children),
                        _ => Err(OubError::Other("not a dir".into())),
                    }
                })
                .map_err(|_| STATUS_NOT_A_DIRECTORY)?;

            let lock = handle.buffer.acquire(true, Some(children.len() as u32))?;
            for (name, child_msg) in &children {
                let child_inode = self
                    .runtime
                    .block_on(self.store.read_inode(*child_msg))
                    .map_err(|_| STATUS_OBJECT_NAME_NOT_FOUND)?;
                let mut entry: DirInfo<255> = DirInfo::new();
                fill_file_info_from_inode(entry.file_info_mut(), &child_inode);
                entry.set_name(name)?;
                lock.write(&mut entry)?;
            }
        }

        Ok(handle.buffer.read(marker, buffer))
    }

    fn get_volume_info(&self, info: &mut VolumeInfo) -> winfsp::Result<()> {
        info.total_size = 1 << 40;
        info.free_size = 1 << 40;
        info.set_volume_label("Oubliette");
        Ok(())
    }
}
