use anyhow::Context;
use clap::Parser;
use oubliette::{
    cache::Cache,
    cli::{Cli, Cmd},
    config::Config,
    setup,
    store::Store,
};
use std::sync::Arc;
use tokio::runtime::Runtime;
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cli.log)),
        )
        .init();

    let cfg_path = match cli.config {
        Some(p) => p,
        None => Config::default_path()?,
    };

    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
    );

    match cli.cmd {
        Cmd::Init { token, guild_id, data_channels } => {
            runtime.block_on(async {
                let cfg = Store::init(token, guild_id, data_channels)
                    .await
                    .context("initializing oubliette")?;
                cfg.save(&cfg_path).context("saving config")?;
                println!("config saved to {}", cfg_path.display());
                println!("metadata channel  : {}", cfg.metadata_channel_id);
                println!("data channels     : {}", cfg.data_channel_ids.len());
                println!("root pointer msg  : {:?}", cfg.root_pointer_message_id);
                Ok::<_, anyhow::Error>(())
            })?;
        }
        Cmd::Put { local, remote } => {
            runtime.block_on(async {
                let cfg = Config::load(&cfg_path).context("loading config")?;
                let store = Store::open(cfg)?;
                store.put_file(&local, &remote).await?;
                Ok::<_, anyhow::Error>(())
            })?;
        }
        Cmd::Get { remote, local } => {
            runtime.block_on(async {
                let cfg = Config::load(&cfg_path).context("loading config")?;
                let store = Store::open(cfg)?;
                store.get_file(&remote, &local).await?;
                Ok::<_, anyhow::Error>(())
            })?;
        }
        Cmd::Ls { remote } => {
            runtime.block_on(async {
                let cfg = Config::load(&cfg_path).context("loading config")?;
                let store = Store::open(cfg)?;
                let entries = store.list(&remote).await?;
                for e in entries {
                    let kind = if e.is_dir() { "d" } else { "f" };
                    println!("{kind} {}", e.name());
                }
                Ok::<_, anyhow::Error>(())
            })?;
        }
        Cmd::Mkdir { remote } => {
            runtime.block_on(async {
                let cfg = Config::load(&cfg_path).context("loading config")?;
                let store = Store::open(cfg)?;
                let msg = store.mkdir_p(&remote).await?;
                println!("dir ready: {remote} (msg {msg})");
                Ok::<_, anyhow::Error>(())
            })?;
        }
        #[cfg(windows)]
        Cmd::Mount { mountpoint, label } => {
            mount_filesystem(runtime, &cfg_path, &mountpoint, &label)?;
        }
        Cmd::Setup => {
            setup::run(runtime, &cfg_path)?;
        }
        Cmd::Info => {
            let cfg = Config::load(&cfg_path).context("loading config")?;
            println!("guild         : {}", cfg.guild_id);
            println!("data channels : {}", cfg.data_channel_ids.len());
            println!("chunk target  : {} bytes", cfg.chunk_target);
            println!("root pointer  : {:?}", cfg.root_pointer_message_id);

            let cache = Cache::open(&Cache::default_path()?)?;
            let (ic, ib, cc, cb) = cache.stats()?;
            println!("cache inodes  : {ic} files, {} KB", ib / 1024);
            println!("cache chunks  : {cc} files, {} MB", cb / (1024 * 1024));
        }
    }

    Ok(())
}

#[cfg(windows)]
fn mount_filesystem(
    runtime: Arc<Runtime>,
    cfg_path: &std::path::Path,
    mountpoint: &str,
    label: &str,
) -> anyhow::Result<()> {
    use oubliette::fs::OublietteFs;
    use winfsp::host::{FileSystemHost, FineGuard, VolumeParams};

    eprintln!("[mount] loading config…");
    let cfg = Config::load(cfg_path).context("loading config")?;
    let store = Arc::new(Store::open(cfg)?);

    eprintln!("[mount] initializing WinFSP…");
    let _init = winfsp::winfsp_init_or_die();
    eprintln!("[mount] WinFSP initialized");

    let mut params = VolumeParams::new();
    params
        .sector_size(4096)
        .sectors_per_allocation_unit(1)
        .max_component_length(255)
        .file_info_timeout(10_000)
        .case_sensitive_search(true)
        .case_preserved_names(true)
        .unicode_on_disk(true)
        .filesystem_name("oubliette");

    let fs = OublietteFs {
        store,
        runtime: runtime.clone(),
    };

    eprintln!("[mount] constructing FileSystemHost…");
    let mut host: FileSystemHost<OublietteFs, FineGuard> =
        FileSystemHost::new(params, fs).context("FileSystemHost::new")?;
    eprintln!("[mount] calling mount(\"{mountpoint}\")…");
    host.mount(mountpoint).context("mount")?;
    eprintln!("[mount] starting dispatcher…");
    host.start().context("start dispatcher")?;

    println!("mounted at {mountpoint}  (label: {label})");
    println!("press Ctrl+C to unmount");

    runtime.block_on(async {
        tokio::signal::ctrl_c().await.context("ctrl_c handler")
    })?;

    println!("\nunmounting...");
    host.stop();
    host.unmount();
    println!("done");
    Ok(())
}
