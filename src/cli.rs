use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "oubliette", version, about = "Discord-backed encrypted filesystem")]
pub struct Cli {
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    #[arg(long, global = true, default_value = "info")]
    pub log: String,

    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// First-time setup: write config, verify bot token, create channels.
    Init {
        #[arg(long)]
        token: String,
        #[arg(long)]
        guild_id: u64,
        #[arg(long, default_value_t = 8)]
        data_channels: u8,
    },

    /// Upload a local file into the oubliette.
    Put {
        local: PathBuf,
        remote: String,
    },

    /// Download a file from the oubliette to a local path.
    Get {
        remote: String,
        local: PathBuf,
    },

    /// List entries under a remote path.
    Ls {
        #[arg(default_value = "/")]
        remote: String,
    },

    /// Create a directory (mkdir -p semantics).
    Mkdir {
        remote: String,
    },

    /// Mount the oubliette as a Windows drive. Ctrl+C to unmount.
    #[cfg(windows)]
    Mount {
        /// Drive letter (e.g. Z:) or directory mount point
        mountpoint: String,
        /// Volume label shown in Explorer
        #[arg(long, default_value = "Oubliette")]
        label: String,
    },

    /// Friendly first-time setup wizard (recommended for new users).
    Setup,

    /// Show storage stats: total files, chunks, bytes.
    Info,
}
