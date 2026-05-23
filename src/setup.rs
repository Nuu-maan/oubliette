use crate::{
    Result,
    discord::DiscordClient,
    error::Error,
    store::Store,
};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::runtime::Runtime;

const HORIZONTAL: &str = "================================================================";
const STEPS_TOTAL: u8 = 4;

#[cfg(windows)]
const WINFSP_SEARCH_PATHS: &[&str] = &[
    r"C:\Program Files (x86)\WinFsp\bin\winfsp-x64.dll",
    r"C:\Program Files\WinFsp\bin\winfsp-x64.dll",
];

pub fn run(runtime: Arc<Runtime>, cfg_path: &Path) -> anyhow::Result<()> {
    print_banner();

    // Drop helper batch files next to the binary up-front so the user always
    // has them, even if they abort the wizard.
    let _ = write_helper_scripts();

    if cfg_path.exists() {
        println!("⚠  An existing config was found at:");
        println!("    {}", cfg_path.display());
        println!();
        if !confirm("Re-run setup and overwrite? [y/N]: ", false)? {
            println!("Setup cancelled.");
            return Ok(());
        }
        println!();
    }

    step_header(1, "Check WinFSP")?;
    let dll_src = check_winfsp()?;

    step_header(2, "Set up your Discord bot")?;
    let token = collect_bot_token()?;

    step_header(3, "Find your server")?;
    let guild_id = collect_guild_id()?;

    println!("Connecting to Discord to verify…");
    let disc = DiscordClient::new(&token, guild_id);
    let bot_name = runtime
        .block_on(disc.verify_token())
        .map_err(|e| anyhow::anyhow!("could not authenticate with Discord: {e}\n\nDouble-check the token was copied fully (it's long), and that the bot has been invited to your server."))?;
    println!("  ✓ Authenticated as bot: {bot_name}");
    println!();

    step_header(4, "Initialize your oubliette")?;
    println!("This will create the following in your Discord server:");
    println!("  - 1 category named \"oubliette\"");
    println!("  - 1 metadata channel (#fs-metadata)");
    println!("  - 4 data channels (#fs-data-0 through #fs-data-3)");
    println!();
    if !confirm("Proceed? [Y/n]: ", true)? {
        println!("Setup cancelled.");
        return Ok(());
    }
    println!();

    let cfg = runtime
        .block_on(Store::init(token, guild_id, 4))
        .map_err(|e| anyhow::anyhow!("channel creation failed: {e}"))?;
    cfg.save(cfg_path)?;
    println!();
    println!("  ✓ Config saved to {}", cfg_path.display());

    if let Some(src) = dll_src {
        if let Err(e) = copy_winfsp_dll(&src) {
            println!("  ⚠  could not copy WinFSP DLL automatically: {e}");
            println!("     (You may need to copy it manually — see below.)");
        } else {
            println!("  ✓ Copied winfsp-x64.dll next to oubliette.exe");
        }
    }

    if write_helper_scripts().is_ok() {
        println!("  ✓ Helper batch files refreshed next to oubliette.exe");
    }

    print_finish_banner();
    Ok(())
}

fn print_banner() {
    println!("{HORIZONTAL}");
    println!("              OUBLIETTE — First-time Setup Wizard");
    println!("{HORIZONTAL}");
    println!();
    println!("This wizard will set up a personal Discord-backed encrypted");
    println!("filesystem that mounts as a Windows drive letter.");
    println!();
    println!("Reading and writing are transparent — drop files in, drag them");
    println!("back out, like any other drive. Behind the scenes everything");
    println!("is chunked, encrypted with AES-256, and stored across Discord");
    println!("messages in a private server you own.");
    println!();
}

fn print_finish_banner() {
    println!();
    println!("{HORIZONTAL}");
    println!("                       Setup complete!");
    println!("{HORIZONTAL}");
    println!();
    println!("To mount your oubliette as drive Z:");
    println!("  Double-click \"Mount Oubliette.bat\" next to oubliette.exe");
    println!();
    println!("To unmount: close the window that opens (or press Ctrl+C in it).");
    println!();
    println!("Files copied INTO Z:\\ get encrypted + uploaded to Discord.");
    println!("Files copied OUT of Z:\\ get downloaded + decrypted on the fly.");
    println!();
    println!("Enjoy.");
    println!();
}

fn step_header(n: u8, title: &str) -> anyhow::Result<()> {
    println!();
    println!("─── Step {n}/{STEPS_TOTAL} — {title} {}", "─".repeat(46_usize.saturating_sub(title.len())));
    println!();
    Ok(())
}

#[cfg(windows)]
fn check_winfsp() -> anyhow::Result<Option<PathBuf>> {
    loop {
        if let Some(found) = WINFSP_SEARCH_PATHS.iter().map(PathBuf::from).find(|p| p.exists()) {
            println!("  ✓ Found WinFSP at:");
            println!("    {}", found.display());
            return Ok(Some(found));
        }

        println!("  ✗ WinFSP not detected.");
        println!();
        println!("  WinFSP is the kernel driver that lets us mount the oubliette");
        println!("  as a Windows drive. It's free + open source (MIT licensed).");
        println!();
        println!("  Download + install it from:");
        println!("    https://winfsp.dev");
        println!();
        println!("  (the installer is ~5 MB and takes about 30 seconds.)");
        println!();
        if !confirm("Press Enter when done (or type 'skip' to skip): ", true)? {
            println!("  ⚠  Skipping — mounts won't work until WinFSP is installed.");
            return Ok(None);
        }
        println!();
    }
}

#[cfg(not(windows))]
fn check_winfsp() -> anyhow::Result<Option<PathBuf>> {
    println!("  (WinFSP is Windows-only — skipping)");
    Ok(None)
}

fn collect_bot_token() -> anyhow::Result<String> {
    println!("You need a Discord bot for this. One-time setup, ~2 minutes.");
    println!();
    println!("  1. Open this URL in your browser:");
    println!("       https://discord.com/developers/applications");
    println!("  2. Click \"New Application\". Pick any name (e.g. \"My Oubliette\").");
    println!("  3. Accept the developer terms.");
    println!("  4. In the left sidebar, click \"Bot\".");
    println!("  5. Click \"Reset Token\" → \"Yes, do it!\".");
    println!("  6. Click \"Copy\" to copy the token.");
    println!();
    println!("⚠  This token is a password. Don't share it. Don't post it.");
    println!();

    loop {
        print!("Paste the bot token here and press Enter: ");
        std::io::stdout().flush().ok();
        let token = read_line()?;
        let token = token.trim();
        if token.is_empty() {
            println!("  (empty input — try again)");
            continue;
        }
        if !token.contains('.') || token.len() < 50 {
            println!("  ⚠  That doesn't look like a bot token. Tokens are long and");
            println!("     contain dots, e.g. MTUw...XYZ.abc.123...");
            if !confirm("Use it anyway? [y/N]: ", false)? {
                continue;
            }
        }
        println!();
        println!("Now invite your bot to a Discord server you own:");
        println!();
        println!("  7. In the dev portal, click \"OAuth2\" → \"URL Generator\".");
        println!("  8. Under \"Scopes\", tick \"bot\".");
        println!("  9. Under \"Bot Permissions\", tick:");
        println!("        Manage Channels, Send Messages, Manage Messages,");
        println!("        Read Message History, Attach Files, View Channels.");
        println!(" 10. Copy the URL at the bottom and open it in your browser.");
        println!(" 11. Pick your server (or create a fresh one) and click Authorize.");
        println!();
        if !confirm("Press Enter once your bot is in your server: ", true)? {
            println!("Skipped — please return when ready.");
            continue;
        }
        return Ok(token.to_string());
    }
}

fn collect_guild_id() -> anyhow::Result<u64> {
    println!("Now we need the ID of the Discord server where your bot lives.");
    println!();
    println!("  1. In Discord, enable Developer Mode if you haven't already:");
    println!("       User Settings → Advanced → Developer Mode = ON");
    println!("  2. Right-click on your server's icon → Copy Server ID.");
    println!();

    loop {
        print!("Paste the server ID here and press Enter: ");
        std::io::stdout().flush().ok();
        let line = read_line()?;
        let line = line.trim();
        match line.parse::<u64>() {
            Ok(0) => println!("  (zero isn't a server ID — try again)"),
            Ok(id) => return Ok(id),
            Err(_) => {
                println!("  ⚠  That doesn't look like a server ID. It should be a long number");
                println!("     like 1234567890123456789. Make sure Developer Mode is on, then");
                println!("     right-click the server icon → Copy Server ID.");
            }
        }
    }
}

fn read_line() -> anyhow::Result<String> {
    let stdin = std::io::stdin();
    let mut s = String::new();
    stdin.lock().read_line(&mut s)?;
    Ok(s)
}

fn confirm(prompt: &str, default_yes: bool) -> anyhow::Result<bool> {
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let line = read_line()?;
    let answer = line.trim().to_ascii_lowercase();
    if answer == "skip" {
        return Ok(false);
    }
    if answer.is_empty() {
        return Ok(default_yes);
    }
    Ok(answer.starts_with('y'))
}

#[cfg(windows)]
fn copy_winfsp_dll(src: &Path) -> Result<()> {
    let exe_dir = std::env::current_exe()
        .map_err(|e| Error::Other(format!("current_exe: {e}")))?
        .parent()
        .ok_or_else(|| Error::Other("exe has no parent".into()))?
        .to_path_buf();
    let dest = exe_dir.join("winfsp-x64.dll");
    if dest.exists() {
        return Ok(());
    }
    std::fs::copy(src, &dest).map_err(Error::Io)?;
    Ok(())
}

#[cfg(not(windows))]
fn copy_winfsp_dll(_src: &Path) -> Result<()> {
    Ok(())
}

fn write_helper_scripts() -> Result<()> {
    let exe = std::env::current_exe()
        .map_err(|e| Error::Other(format!("current_exe: {e}")))?;
    let exe_dir = exe
        .parent()
        .ok_or_else(|| Error::Other("exe has no parent".into()))?
        .to_path_buf();
    let exe_path = exe.display().to_string();

    let mount = format!(
        "@echo off\r\n\
         title Oubliette - mounted at Z:\\\r\n\
         echo Mounting your oubliette at Z:\\ ...\r\n\
         echo.\r\n\
         echo Leave this window open while you use the drive.\r\n\
         echo Close it (or press Ctrl+C) to unmount.\r\n\
         echo.\r\n\
         \"{exe_path}\" mount Z:\r\n\
         echo.\r\n\
         echo Unmounted. You can close this window.\r\n\
         pause >nul\r\n"
    );
    std::fs::write(exe_dir.join("Mount Oubliette.bat"), mount).map_err(Error::Io)?;

    let setup = format!(
        "@echo off\r\n\
         title Oubliette - first-time setup\r\n\
         \"{exe_path}\" setup\r\n\
         pause\r\n"
    );
    std::fs::write(exe_dir.join("Setup Oubliette.bat"), setup).map_err(Error::Io)?;

    Ok(())
}
