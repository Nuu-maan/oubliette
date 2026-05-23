#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use oubliette::config::Config;
        use std::sync::Arc;

        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?,
        );
        let cfg_path = Config::default_path()?;
        oubliette::gui::run(runtime, cfg_path)
    }
    #[cfg(not(windows))]
    {
        anyhow::bail!("GUI is Windows-only");
    }
}
