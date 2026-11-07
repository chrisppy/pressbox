// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

use std::{path::PathBuf, process, time::Duration};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use ratatui::crossterm::event::{self, Event, KeyEventKind};

mod app;
mod client;
mod ui;

use app::{Action, App};
use client::{Client, ClientConfig};

/// Terminal client for the pressbox feed aggregator.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Server URL, e.g. `http://127.0.0.1:8470`. Overrides the config file.
    #[arg(short, long, env = "PRESSBOX_SERVER", global = true)]
    server: Option<String>,
    /// File holding the API token. Overrides the config file.
    #[arg(short, long, env = "PRESSBOX_TOKEN_FILE", global = true)]
    token_file: Option<PathBuf>,
    /// Client config (JSON). Defaults to `~/.config/pressbox/client.json`.
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Pair this machine with the server: shows a code to approve, then
    /// saves this device's token.
    Pair {
        /// Name for this device. Defaults to the hostname.
        #[arg(long)]
        name: Option<String>,
    },
    /// Approve another device's pairing code (or list pending ones).
    Approve {
        code: Option<String>,
        /// Add the device to this user instead of yours (admins only).
        #[arg(long)]
        user: Option<String>,
    },
    /// Show which user and device this machine is signed in as.
    Whoami,
}

fn main() {
    if let Err(e) = run(Args::parse()) {
        eprintln!("pressbox: {e:#}");
        std::process::exit(1);
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "pressbox".to_owned())
}

fn pair(server: &str, token_path: &std::path::Path, name: Option<String>) -> Result<()> {
    let client = Client::new(server, None)?;
    let name = name.unwrap_or_else(hostname);
    let start = client.pair_start(&name)?;
    println!("Pairing {name:?} with {server}.\n");
    println!("    {}\n", start.code);
    println!(
        "Approve it on the server with `pressboxctl approve {}`,",
        start.code
    );
    println!(
        "or from a paired device (`pressbox approve {}`, or the iOS app).",
        start.code
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(start.expires_in);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(2));
        if let Some(approved) = client.pair_poll(&start.poll_id)? {
            if let Some(dir) = token_path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            write_private(token_path, &approved.token)?;
            println!(
                "\nPaired as {:?}. Token saved to {}.",
                approved.name,
                token_path.display()
            );
            return Ok(());
        }
    }
    anyhow::bail!("the code expired before it was approved; run `pressbox pair` again")
}

/// Writes a file only the owner can read.
fn write_private(path: &std::path::Path, contents: &str) -> Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    writeln!(file, "{contents}")?;
    Ok(())
}

fn run(args: Args) -> Result<()> {
    let config = match args.config.or_else(ClientConfig::default_path) {
        Some(path) => ClientConfig::load(&path)?,
        None => ClientConfig::default(),
    };
    let server = args
        .server
        .or(config.server)
        .unwrap_or_else(|| "http://127.0.0.1:8470".to_owned());
    let token_path = args
        .token_file
        .or(config.token_file)
        .or_else(ClientConfig::default_token_path)
        .context("no token file configured and no state directory")?;

    if let Some(Command::Pair { name }) = args.command {
        return pair(&server, &token_path, name);
    }
    let token = match std::fs::read_to_string(&token_path) {
        Ok(t) => t.trim().to_owned(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => anyhow::bail!(
            "no token at {}: run `pressbox pair` to pair this machine",
            token_path.display()
        ),
        Err(e) => return Err(e).with_context(|| format!("reading {}", token_path.display())),
    };
    let client = Client::new(&server, Some(token))?;

    if let Some(Command::Whoami) = args.command {
        let me = client.me()?;
        let admin = if me.admin { " (admin)" } else { "" };
        let device = me.device.unwrap_or_else(|| "the static token".to_owned());
        println!("{}{admin} on {device}, at {server}", me.user);
        match client.version() {
            Ok(v) => println!("pressboxd {} (API v{})", v.version, v.api),
            // Servers from before the endpoint existed.
            Err(_) => println!("pressboxd version unknown"),
        }
        return Ok(());
    }
    if let Some(Command::Approve { code, user }) = args.command {
        match code {
            Some(code) => {
                let approved = client.approve(&code, user)?;
                println!("paired {} for {}", approved.name, approved.user);
            }
            None => {
                let pending = client.pairings()?;
                if pending.is_empty() {
                    println!("no pairing requests");
                }
                for p in pending {
                    println!("{}  {}", p.code, p.name);
                }
            }
        }
        return Ok(());
    }

    let mut app = App::new();
    app.reload(&client);
    ratatui::run(|terminal| -> Result<()> {
        loop {
            terminal.draw(|frame| ui::draw(frame, &app))?;
            if !event::poll(Duration::from_secs(60))? {
                // Idle for a minute: pick up whatever the server fetched.
                app.reload(&client);
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match app.handle(key.code, &client) {
                Action::None => {}
                Action::Quit => return Ok(()),
                Action::Open(url) => open(&url, &mut app),
            }
        }
    })
}

fn open(url: &str, app: &mut App) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let spawned = process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    app.status = match spawned {
        Ok(_) => format!("opened {url}"),
        Err(e) => format!("error: could not run {opener}: {e}"),
    };
}
