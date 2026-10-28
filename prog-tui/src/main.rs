//! prog-tui: deploy and test the LEZ example programs locally.
//! See README.md for what each pane and key does.

mod app;
mod builtin;
mod guest_bin;
mod jobs;
mod lez;
mod lez_call;
mod rpc;
mod sequencer;
mod state;
mod tools;
mod ui;
mod userprogs;
mod wallet;

use std::time::{Duration, Instant};

use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyEventKind},
};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::{app::App, jobs::Msg};

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let (tx, mut rx) = unbounded_channel();
    let mut app = App::new(runtime.handle().clone(), tx);
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, &mut rx);
    ratatui::restore();
    // Jobs still running (a build, a call) are dropped with the runtime; their child processes
    // are killed. A sequencer prog-tui started keeps running.
    runtime.shutdown_timeout(Duration::from_millis(500));
    result
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
