//! prog-tui: deploy and test lez-programs' programs and your own guests on LEZ: the local
//! standalone sequencer, or a network from `networks.json` (a devnet), chosen when it starts.
//! See README.md for what each pane and key does.

mod app;
mod builtin;
mod guest_bin;
mod jobs;
mod l1;
mod lez;
mod lez_call;
mod network;
mod programs;
mod rpc;
mod sequencer;
mod spel;
mod state;
mod tools;
mod ui;
mod userprogs;
mod wallet;

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyCode, KeyEventKind},
};
use tokio::{
    runtime::Runtime,
    sync::mpsc::{UnboundedReceiver, unbounded_channel},
};

use crate::{app::App, jobs::Msg, network::Network, state::Saved};

/// `cargo run --release [-- <network key>]`: the key (`local`, or one the selector shows, such
/// as `devnet-127.0.0.1-3040`) skips the selector.
fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let preset = std::env::args().nth(1);
    let mut terminal = ratatui::init();
    let result = session(&mut terminal, &runtime, &crate_dir, preset);
    ratatui::restore();
    // Jobs still running (a build, a call) are dropped with the runtime; their child processes
    // are killed. A sequencer prog-tui started keeps running.
    runtime.shutdown_timeout(Duration::from_millis(500));
    result
}

/// Choose a network, run prog-tui on it, and come back to the selector when the user switches
/// network (key N) rather than quitting.
fn session(
    terminal: &mut DefaultTerminal,
    runtime: &Runtime,
    crate_dir: &Path,
    mut preset: Option<String>,
) -> anyhow::Result<()> {
    loop {
        let (choices, error) = match network::choices(crate_dir) {
            Ok(choices) => (choices, None),
            Err(error) => (vec![Network::Local], Some(format!("{error:#}"))),
        };
        let network = match preset.take() {
            Some(key) => choices
                .iter()
                .find(|network| network.key() == key)
                .cloned()
                .ok_or_else(|| {
                    let keys: Vec<String> = choices.iter().map(Network::key).collect();
                    anyhow::anyhow!("no network {key:?}; the choices are: {}", keys.join(", "))
                })?,
            None => {
                let last = Saved::load(&crate_dir.join(".state")).last_network;
                match select(terminal, &choices, error.as_deref(), last.as_deref())? {
                    Some(network) => network,
                    None => return Ok(()),
                }
            }
        };
        let (tx, mut rx) = unbounded_channel();
        let mut app = App::new(runtime.handle().clone(), tx, network);
        run(terminal, &mut app, &mut rx)?;
        if !app.switch_network {
            return Ok(());
        }
    }
}

/// The network selector. `None` when the user quits instead of choosing.
fn select(
    terminal: &mut DefaultTerminal,
    choices: &[Network],
    error: Option<&str>,
    last: Option<&str>,
) -> anyhow::Result<Option<Network>> {
    let mut selected = last
        .and_then(|key| choices.iter().position(|network| network.key() == key))
        .unwrap_or(0);
    loop {
        terminal.draw(|frame| ui::draw_selector(frame, choices, selected, error))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
            KeyCode::Char('c') if key.modifiers.contains(ratatui::crossterm::event::KeyModifiers::CONTROL) => {
                return Ok(None);
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                selected = (selected + 1).min(choices.len() - 1);
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => selected = selected.saturating_sub(1),
            KeyCode::Char(digit @ '1'..='9') => {
                let index = usize::from(digit as u8 - b'1');
                if index < choices.len() {
                    return Ok(Some(choices[index].clone()));
                }
            }
            KeyCode::Enter => return Ok(Some(choices[selected].clone())),
            _ => {}
        }
    }
}

fn run(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    rx: &mut UnboundedReceiver<Msg>,
) -> anyhow::Result<()> {
    let mut last_tick = Instant::now();
    app.on_tick();
    while !app.quit {
        while let Ok(msg) = rx.try_recv() {
            app.on_msg(msg);
        }
        if last_tick.elapsed() >= Duration::from_millis(250) {
            app.on_tick();
            last_tick = Instant::now();
        }
        terminal.draw(|frame| ui::draw(frame, app))?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.on_key(key);
        }
    }
    Ok(())
}
