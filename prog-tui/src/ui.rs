//! Drawing. Reads [`App`], never changes it.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize as _},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::{
    app::{App, Focus, Modal},
    jobs::{StepState, short},
    rpc::{NATIVE_TOKEN, describe_bytes, format_balance},
};

const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

pub fn draw(frame: &mut Frame, app: &App) {
    let [top, main, bottom, keys] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(8),
        Constraint::Length(12),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    draw_status(frame, top, app);

    let [accounts, detail, programs] = Layout::horizontal([
        Constraint::Percentage(30),
        Constraint::Percentage(40),
        Constraint::Percentage(30),
    ])
    .areas(main);
    draw_accounts(frame, accounts, app);
    draw_detail(frame, detail, app);
    draw_programs(frame, programs, app);

    if app.checklist.is_empty() {
        draw_log(frame, bottom, app);
    } else {
        let [log, checklist] =
            Layout::horizontal([Constraint::Min(40), Constraint::Length(46)]).areas(bottom);
        draw_log(frame, log, app);
        draw_checklist(frame, checklist, app);
    }
    draw_keys(frame, keys, app);

    match &app.modal {
        Modal::None => {}
        Modal::Help => draw_help(frame),
        Modal::Confirm(text, _) => {
            let area = centered(frame.area(), 70, 5);
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(text.as_str())
                    .wrap(Wrap { trim: true })
                    .block(Block::bordered().title(" Confirm ").border_style(Color::Yellow)),
                area,
            );
        }
        Modal::Form(form) => {
            let height = u16::try_from(form.fields.len()).unwrap_or(4) + 4;
            let area = centered(frame.area(), 90, height);
            frame.render_widget(Clear, area);
            let width = form.fields.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
            let mut lines: Vec<Line> = form
                .fields
                .iter()
                .enumerate()
                .map(|(i, (label, value))| {
                    let selected = i == form.selected;
                    Line::from(vec![
                        Span::styled(
                            format!("{label:>width$}: "),
                            Style::new().fg(Color::DarkGray),
                        ),
                        Span::styled(
                            value.clone(),
                            if selected {
                                Style::new().fg(Color::Yellow)
                            } else {
                                Style::new()
                            },
                        ),
                        Span::raw(if selected { "▏" } else { "" }),
                    ])
                })
                .collect();
            lines.push(Line::default());
            lines.push(
                Line::from("Enter run · Tab/↑↓ field · Ctrl-U clear · Esc cancel").dark_gray(),
            );
            frame.render_widget(
                Paragraph::new(lines).block(
                    Block::bordered()
                        .title(format!(" {} ", form.title))
                        .border_style(Color::Cyan),
                ),
                area,
            );
        }
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn pane(title: &str, focused: bool) -> Block<'_> {
    let block = Block::bordered().title(format!(" {title} "));
    if focused {
        block.border_style(Color::Cyan)
    } else {
        block.border_style(Color::DarkGray)
    }
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let repo = &app.status.repo;
    let mut lez = vec![Span::raw("LEZ ").bold(), Span::raw(app.saved.lez_dir.clone())];
    if repo.present {
        lez.push(Span::raw("  "));
        lez.push(Span::raw(repo.branch.clone().unwrap_or_else(|| "(detached)".into())).green());
        lez.push(Span::raw(format!(" {}", repo.head.clone().unwrap_or_default())));
        if repo.dirty {
            lez.push(Span::raw(" dirty").yellow());
        }
        lez.push(Span::raw("  prog_cli "));
        lez.push(match (repo.has_prog_cli_src, repo.prog_cli_built) {
            (_, true) => Span::raw("✓").green(),
            (true, false) => Span::raw("not built (b)").yellow(),
            (false, false) => Span::raw("missing (runners only)").red(),
        });
    } else if app.lez.present() {
        lez.push(Span::raw("  …"));
    } else {
        lez.push(Span::raw("  not present (L to change)").red());
    }

    let seq = &app.status.seq;
    let (dot, state) = match (seq.healthy, seq.managed, seq.port_open) {
        (true, Some(pgid), _) => (Span::raw("●").green(), format!("running (prog-tui, pgid {pgid})")),
        (true, None, _) => (
            Span::raw("●").green(),
            format!(
                "running (external{})",
                seq.listener.map(|pid| format!(", pid {pid}")).unwrap_or_default()
            ),
        ),
        (false, Some(_), _) | (false, None, true) => (Span::raw("◐").yellow(), "starting…".into()),
        (false, None, false) => (Span::raw("○").red(), "stopped".into()),
    };
    let mut second = vec![Span::raw("SEQ ").bold(), dot, Span::raw(format!(" {state}"))];
    if let Some(block) = seq.block {
        second.push(Span::raw(format!(" · block {block}")));
    }
    second.push(Span::raw(format!(" · {}", app.rpc.url)).dark_gray());
    second.push(Span::raw("   PAYER ").bold());
    second.push(Span::raw(app.saved.payer.as_deref().map_or("-".into(), short)));
    second.push(Span::raw("  ACCOUNT ").bold());
    second.push(Span::raw(app.saved.account.as_deref().map_or("-".into(), short)));
    if let Some((job, since)) = &app.busy {
        second.push(Span::raw(format!(
            "   {} {job} ({}s)",
            SPINNER[app.ticks % SPINNER.len()],
            since.elapsed().as_secs()
        ))
        .cyan());
    }
    frame.render_widget(
        Paragraph::new(vec![Line::from(lez), Line::from(second)]),
        area,
    );
}

fn draw_accounts(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .visible_accounts()
        .into_iter()
        .map(|account| {
            let role = if app.saved.payer.as_deref() == Some(&account.id) {
                Span::raw("★ ").yellow()
            } else if app.saved.account.as_deref() == Some(&account.id) {
                Span::raw("▶ ").cyan()
            } else {
                Span::raw("  ")
            };
            let kind = if account.public { "" } else { "priv " };
            let view = app.views.get(&account.id);
            let chain = match view {
                Some(Some(view)) => Span::raw(format!(" {}", format_balance(view.balance()))).green(),
                Some(None) => Span::raw(" off-chain").dark_gray(),
                None => Span::raw(""),
            };
            let label = account
                .label
                .as_deref()
                .map(|label| format!(" [{label}]"))
                .unwrap_or_default();
            ListItem::new(Line::from(vec![
                role,
                Span::raw(format!("{kind}{}", short(&account.id))),
                Span::raw(label).magenta(),
                chain,
            ]))
        })
        .collect();
    let title = format!(
        "Accounts{}",
        if app.hide_loader { " (h: show loader accts)" } else { " (h: hide loader accts)" }
    );
    let list = List::new(items)
        .block(pane(&title, app.focus == Focus::Accounts))
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(app.selected_account));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_builtin_detail(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = Vec::new();
    if let Some(builtin) = app.builtins.get(app.selected_builtin) {
        lines.push(Line::from(builtin.name).bold());
        lines.push(Line::from(builtin.id.clone()));
        lines.push(Line::default());
        lines.push(Line::from(builtin.origin).dark_gray());
        lines.push(Line::default());
        if builtin.engine {
            lines.push(Line::from("built into the engine: always available").green());
            lines.push(
                Line::from("It has no account of its own to look up; accounts hold its state.")
                    .dark_gray(),
            );
        } else {
            match app.views.get(&builtin.id) {
                Some(Some(view)) => {
                    lines.push(Line::from("deployed on this chain").green());
                    for (program, bytes) in &view.actor_states {
                        lines.push(Line::from(format!(
                            "  {} state, {} B",
                            app.program_label(program),
                            bytes.len()
                        )));
                    }
                }
                Some(None) => {
                    lines.push(Line::from("not deployed on this chain").yellow());
                    lines.push(
                        Line::from("The standalone dev chain deploys only some built-ins.")
                            .dark_gray(),
                    );
                }
                None if !app.status.seq.healthy => {
                    lines.push(Line::from("sequencer not running").dark_gray());
                }
                None => lines.push(Line::from("loading…").dark_gray()),
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(pane("Built-in program", false)),
        area,
    );
}

fn draw_detail(frame: &mut Frame, area: Rect, app: &App) {
    if app.focus == Focus::Builtin {
        draw_builtin_detail(frame, area, app);
        return;
    }
    let mut lines = Vec::new();
    if let Some(account) = app.selected_account() {
        let kind = if account.public { "Public" } else { "Private" };
        lines.push(Line::from(format!("{kind}/{}", account.id)).bold());
        let mut meta = Vec::new();
        if let Some(label) = &account.label {
            meta.push(format!("label {label}"));
        }
        if let Some(path) = &account.path {
            meta.push(format!("path {path}"));
        }
        if app.saved.payer.as_deref() == Some(&account.id) {
            meta.push("PAYER".into());
        }
        if app.saved.account.as_deref() == Some(&account.id) {
            meta.push("ACCOUNT".into());
        }
        if !meta.is_empty() {
            lines.push(Line::from(meta.join(" · ")).dark_gray());
        }
        lines.push(Line::default());
        match app.views.get(&account.id) {
            _ if !account.public => {
                lines.push(Line::from("private: its state isn't on the public chain").dark_gray());
            }
            None if !app.status.seq.healthy => {
                lines.push(Line::from("sequencer not running").dark_gray());
            }
            None => lines.push(Line::from("loading…").dark_gray()),
            Some(None) => {
                lines.push(Line::from("not on chain").yellow());
                lines.push(
                    Line::from("A successful call creates it. If a call left it here, it reverted.")
                        .dark_gray(),
                );
            }
            Some(Some(view)) => {
                lines.push(Line::from(format!(
                    "nonce {} · balance {}",
                    view.nonce,
                    view.balance()
                )));
                lines.push(Line::default());
                lines.push(Line::from("actor states").bold());
                if view.actor_states.is_empty() {
                    lines.push(Line::from("  (none)").dark_gray());
                }
                for (program, bytes) in &view.actor_states {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::raw(app.program_label(program)).cyan(),
                        Span::raw(format!("  {}  {} B", short(program), bytes.len())).dark_gray(),
                    ]));
                    let shown = if program == NATIVE_TOKEN {
                        format!("balance {}", crate::rpc::decode_u128_le(bytes))
                    } else {
                        describe_bytes(bytes)
                    };
                    lines.push(Line::from(format!("    {shown}")));
                }
            }
        }
    } else {
        lines.push(Line::from("No accounts yet.").dark_gray());
        lines.push(Line::from("W runs the whole walkthrough; i initialises the wallet.").dark_gray());
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(pane("Account", false)),
        area,
    );
}

fn draw_programs(frame: &mut Frame, area: Rect, app: &App) {
    let [guests_area, deployments_area, builtin_area] = Layout::vertical([
        Constraint::Length(7),
        Constraint::Min(5),
        Constraint::Length(14),
    ])
    .areas(area);

    let guests: Vec<ListItem> = app
        .guests
        .iter()
        .map(|guest| {
            let count = app
                .saved
                .deployments
                .iter()
                .filter(|d| &d.name == guest && !d.stale)
                .count();
            let mut spans = vec![Span::raw(guest.clone())];
            if guest.starts_with(crate::userprogs::PREFIX) {
                spans.push(Span::raw(" (example_progs)").dark_gray());
            }
            if count > 0 {
                spans.push(Span::raw(format!(" ×{count}")).green());
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let guests_title = if app.guests.is_empty() {
        "Guests ($BIN): none built, press b"
    } else {
        "Guests ($BIN)"
    };
    frame.render_stateful_widget(
        List::new(guests)
            .block(pane(guests_title, app.focus == Focus::Guests))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        guests_area,
        &mut ListState::default().with_selected(Some(app.selected_guest)),
    );

    let deployments: Vec<ListItem> = app
        .saved
        .deployments
        .iter()
        .map(|deployment| {
            let line = Line::from(vec![
                Span::raw(deployment.name.clone()),
                Span::raw(format!(" {}", short(&deployment.program_id))).dark_gray(),
            ]);
            if deployment.stale {
                ListItem::new(line.crossed_out().dark_gray())
            } else {
                ListItem::new(line)
            }
        })
        .collect();
    frame.render_stateful_widget(
        List::new(deployments)
            .block(pane("Deployed (c call, x forget)", app.focus == Focus::Deployments))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        deployments_area,
        &mut ListState::default().with_selected(Some(app.selected_deployment)),
    );

    // Dim the guests this chain didn't deploy at genesis. Engine programs are always there.
    let builtins: Vec<ListItem> = app
        .builtins
        .iter()
        .map(|builtin| {
            let line = Line::from(vec![
                Span::raw(builtin.name),
                Span::raw(format!(" {}", short(&builtin.id))).dark_gray(),
            ]);
            let missing = !builtin.engine && matches!(app.views.get(&builtin.id), Some(None));
            if missing {
                ListItem::new(line.dark_gray())
            } else {
                ListItem::new(line)
            }
        })
        .collect();
    frame.render_stateful_widget(
        List::new(builtins)
            .block(pane("Built-in (read-only)", app.focus == Focus::Builtin))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        builtin_area,
        &mut ListState::default().with_selected(Some(app.selected_builtin)),
    );
}

fn draw_log(frame: &mut Frame, area: Rect, app: &App) {
    let height = usize::from(area.height.saturating_sub(2));
    let start = app.log.len().saturating_sub(height);
    let lines: Vec<Line> = app
        .log
        .iter()
        .skip(start)
        .map(|line| {
            let styled = Line::from(line.clone());
            if line.starts_with('✗') || line.starts_with("error") || line.starts_with("Error") {
                styled.red()
            } else if line.starts_with('✓') {
                styled.green()
            } else if line.starts_with("$ ") || line.starts_with('▶') {
                styled.cyan()
            } else if line.starts_with("sequencer: ") {
                styled.yellow()
            } else {
                styled
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(pane("Log", false)), area);
}

fn draw_checklist(frame: &mut Frame, area: Rect, app: &App) {
    let lines: Vec<Line> = app
        .checklist
        .iter()
        .enumerate()
        .map(|(i, (step, state))| {
            let mark = match state {
                StepState::Pending => Span::raw("·").dark_gray(),
                StepState::Running => Span::raw(SPINNER[app.ticks % SPINNER.len()]).cyan(),
                StepState::Done => Span::raw("✓").green(),
                StepState::Failed => Span::raw("✗").red(),
            };
            Line::from(vec![mark, Span::raw(format!(" {}. {step}", i + 1))])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).block(pane("Walkthrough", false)),
        area,
    );
}

fn draw_keys(frame: &mut Frame, area: Rect, app: &App) {
    let context = match app.focus {
        Focus::Accounts => "P payer · A account · n new · t transfer",
        Focus::Guests => "d deploy · r stock runner · u build+deploy file",
        Focus::Deployments => "c call · m move-data · x forget",
        Focus::Builtin => "read-only: details in the middle pane",
    };
    frame.render_widget(
        Paragraph::new(format!(
            " {context} │ W walkthrough · s seq start/stop · e seq cmds · R reset · b build · i wallet init · Tab pane · ? help · q quit"
        ))
        .dark_gray(),
        area,
    );
}

fn draw_help(frame: &mut Frame) {
    let keys = [
        ("W", "walkthrough: build, reset, wallet, PAYER/ACCOUNT, deploy and call hello_world"),
        ("s", "start the sequencer, or stop it (asks first if prog-tui didn't start it)"),
        ("R", "reset: stop, `just clean`, start (wipes the chain and the wallet)"),
        ("b", "build wallet, runners, prog_cli and the example guests"),
        ("i", "initialise the wallet and import the two funded test accounts"),
        ("n", "new public account"),
        ("P / A", "make the selected account PAYER / ACCOUNT"),
        ("t", "native token transfer (wallet auth-transfer send), checked by both balances"),
        ("d", "deploy the selected guest with prog_cli (Guests pane)"),
        ("r", "run the guest's stock run_* runner: deploys again and calls once"),
        ("u", "build and deploy a guest from jz-progs/example_progs (as jz_<name>)"),
        ("c", "call the selected deployment (Deployed pane)"),
        ("m", "move-data, for hello_world_with_move_function"),
        ("x", "forget the selected deployment"),
        ("h", "show or hide the header/segment accounts that deploys create"),
        ("e", "shell commands for the sequencer (log, wallet, stop) and the env.sh to source"),
        ("L", "change the LEZ checkout"),
        ("g", "refresh now"),
        ("Tab / ←→", "switch pane (Accounts, Guests, Deployed, Built-in); ↑↓ or j/k select"),
        ("q", "quit (the sequencer keeps running)"),
    ];
    let lines: Vec<Line> = keys
        .iter()
        .map(|(key, what)| {
            Line::from(vec![
                Span::raw(format!("{key:>9}  ")).cyan(),
                Span::raw(*what),
            ])
        })
        .collect();
    let area = centered(frame.area(), 92, u16::try_from(keys.len()).unwrap_or(20) + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Keys (any key closes) ")),
        area,
    );
}
