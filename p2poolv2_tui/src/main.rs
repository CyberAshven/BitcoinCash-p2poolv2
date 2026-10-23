// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `p2poolv2_tui`: a terminal UI for a Bitcoin Cash P2Poolv2 node and its
//! BCHN, ported from P2Poolv2 PDM. Its main loop is PDM's: take what the
//! fetches sent, draw, wait up to 100 ms for a key.

#![forbid(unsafe_code)]

use clap::Parser;
use p2poolv2_tui::app::App;
use p2poolv2_tui::input::handle_key;
use p2poolv2_tui::settings;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};
use rustls::crypto::aws_lc_rs;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};
use tokio::runtime::Builder;

/// How long to wait for a key before drawing again.
const FRAME_INTERVAL: Duration = Duration::from_millis(100);

/// Terminal UI for a Bitcoin Cash P2Poolv2 node and the BCHN it mines on.
///
/// Point it at the node's config.toml: the node API, BCHN's JSON-RPC and the
/// network (mainnet, chipnet or regtest) are read from it.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// The node's config.toml to open for this run, instead of the one in
    /// the settings. It is not remembered.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    /// The directory holding this tool's settings.toml.
    #[arg(long, value_name = "DIR", env = settings::CONFIG_DIR_VARIABLE)]
    settings_dir: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("p2poolv2_tui: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    // The workspace builds rustls with two crypto providers, so it needs
    // one chosen before the live feed opens a wss:// connection. An error
    // means one is installed already, which is as good.
    let _ = aws_lc_rs::default_provider().install_default();
    let runtime = Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("Cannot start the runtime: {error}"))?;
    let default_settings_dir = match cli.settings_dir {
        Some(dir) => dir,
        None => settings::default_dir()?,
    };
    let saved = settings::load(&default_settings_dir)?;
    let node_config = cli.config.or_else(|| saved.node_config_path.clone());
    let mut app = App::new(saved, default_settings_dir, runtime.handle().clone());
    if let Some(path) = node_config {
        app.open_node_config(path);
    }

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result.map_err(|error| format!("Terminal error: {error}"))
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    let mut running = true;
    while running {
        let now = Instant::now();
        app.drain(now);
        app.tick(now);
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(FRAME_INTERVAL)?
            && let Event::Key(key) = event::read()?
        {
            running = handle_key(app, key, Instant::now()).is_continue();
        }
    }
    Ok(())
}
