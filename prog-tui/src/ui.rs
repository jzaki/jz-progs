//! Drawing. Reads [`App`], never changes it.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize as _},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::{
    app::{AccountRow, App, Focus, Modal, View},
    jobs::{StepState, short},
    l1::{self, L1Status, MODULE},
    network::{NETWORKS_FILE, Network},
    rpc::{NATIVE_TOKEN, describe_bytes, format_balance},
    state::Origin,
};

const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

pub fn draw(frame: &mut Frame, app: &App) {
    let [top, main, bottom, keys] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(12),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    draw_status(frame, top, app);

    let [left, middle, right] = Layout::horizontal([
        Constraint::Percentage(30),
        Constraint::Percentage(40),
        Constraint::Percentage(30),
    ])
    .areas(main);
    match app.view {
        View::Lez => {
            draw_accounts(frame, left, app);
            draw_detail(frame, middle, app);
            draw_programs(frame, right, app);
        }
        View::L1 => {
            draw_l1_daemon(frame, left, app);
            draw_l1_node(frame, middle, app);
            draw_l1_wallets(frame, right, app);
        }
    }

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
        Modal::Text(title, text) => {
            let height = u16::try_from(text.len()).unwrap_or(20) + 2;
            let area = centered(frame.area(), 100, height);
            frame.render_widget(Clear, area);
            let lines: Vec<Line> = text.iter().map(|line| Line::from(line.as_str())).collect();
            frame.render_widget(
                Paragraph::new(lines).wrap(Wrap { trim: false }).block(
                    Block::bordered()
                        .title(format!(" {title} (any key closes) "))
                        .border_style(Color::Cyan),
                ),
                area,
            );
        }
        Modal::Plan(view) => {
            // At least 12 rows; a longer plan scrolls.
            let width = 120;
            let inner = usize::from(width.min(frame.area().width.saturating_sub(2)).saturating_sub(2)).max(1);
            let wrapped: usize = view
                .lines
                .iter()
                .map(|line| line.chars().count().div_ceil(inner).max(1))
                .sum();
            let height = u16::try_from(wrapped).unwrap_or(u16::MAX).saturating_add(2).max(12);
            let area = centered(frame.area(), width, height);
            frame.render_widget(Clear, area);
            let visible = usize::from(area.height.saturating_sub(2));
            let scrolls = wrapped > visible;
            let scroll = view
                .scroll
                .get()
                .min(u16::try_from(wrapped.saturating_sub(visible)).unwrap_or(u16::MAX));
            view.scroll.set(scroll);
            let lines: Vec<Line> = view
                .lines
                .iter()
                .map(|line| {
                    let styled = Line::from(line.as_str());
                    if line.starts_with('✗') || line.starts_with("Error") {
                        styled.red()
                    } else {
                        styled
                    }
                })
                .collect();
            let keys = if scrolls { " · ↑↓ PgUp/PgDn scroll" } else { "" };
            let (footer, color) = match &view.call {
                None if view.read_only => (format!(" read-only · Esc closes{keys} "), Color::Cyan),
                Some(call) => (format!(" Enter {} ({}) · Esc cancel{keys} ", call.verb(), call.sender()), Color::Yellow),
                None => (format!(" nothing to send (see above) · Esc closes{keys} "), Color::Red),
            };
            frame.render_widget(
                Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((scroll, 0)).block(
                    Block::bordered()
                        .title(if view.read_only {
                            format!(" {} ", view.title)
                        } else {
                            format!(" {}: plan ", view.title)
                        })
                        .title_bottom(footer)
                        .border_style(color),
                ),
                area,
            );
        }
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
        Modal::Pick(pick) => {
            let height = u16::try_from(pick.options.len()).unwrap_or(20) + 2;
            let area = centered(frame.area(), 90, height);
            frame.render_widget(Clear, area);
            let items: Vec<ListItem> = pick
                .options
                .iter()
                .map(|(label, _)| ListItem::new(label.as_str()))
                .collect();
            let block = Block::bordered()
                .title(format!(" {} ", pick.title))
                .title_bottom(" Enter choose · ↑↓ select · Esc cancel ")
                .border_style(Color::Cyan);
            frame.render_stateful_widget(
                List::new(items)
                    .block(block)
                    .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
                area,
                &mut ListState::default().with_selected(Some(pick.selected)),
            );
        }
        Modal::Form(form) => {
            // A label is a name, and optionally a description after a newline, drawn dimmed on
            // its own line under the field so long text doesn't push every value right.
            let split = |label: &str| -> (String, Option<String>) {
                match label.split_once('\n') {
                    Some((name, about)) => (name.to_owned(), Some(about.to_owned())),
                    None => (label.to_owned(), None),
                }
            };
            let width = form
                .fields
                .iter()
                .map(|(label, _)| split(label).0.chars().count())
                .max()
                .unwrap_or(0);
            let mut lines: Vec<Line> = Vec::new();
            for (i, (label, value)) in form.fields.iter().enumerate() {
                let selected = i == form.selected;
                let (name, about) = split(label);
                lines.push(Line::from(vec![
                    Span::styled(format!("{name:>width$}: "), Style::new().fg(Color::DarkGray)),
                    Span::styled(
                        value.clone(),
                        if selected {
                            Style::new().fg(Color::Yellow)
                        } else {
                            Style::new()
                        },
                    ),
                    Span::raw(if selected { "▏" } else { "" }),
                ]));
                if let Some(about) = about {
                    lines.push(
                        Line::from(format!("{:width$}  {about}", "")).fg(Color::DarkGray).italic(),
                    );
                }
            }
            let height = u16::try_from(lines.len()).unwrap_or(8) + 4;
            let area = centered(frame.area(), 90, height);
            frame.render_widget(Clear, area);
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

/// `[1 LEZ] [2 L1 ●]`: the views, the shown one highlighted, and the L1 daemon's state.
fn tabs(app: &App) -> Vec<Span<'static>> {
    let tab = |label: &'static str, shown: bool| {
        if shown {
            Span::raw(label).black().on_cyan()
        } else {
            Span::raw(label).dark_gray()
        }
    };
    let l1 = &app.l1;
    let dot = if !l1.checked {
        Span::raw(" ·").dark_gray()
    } else if l1.logosctl.is_none() {
        Span::raw(" -").dark_gray()
    } else if l1.running() && l1.module_loaded() && l1.node_log.as_ref().is_some_and(|log| !log.online) {
        Span::raw(" ◐").yellow()
    } else if l1.running() && l1.module_loaded() {
        Span::raw(" ●").green()
    } else if l1.running() {
        Span::raw(" ◐").yellow()
    } else {
        Span::raw(" ○").red()
    };
    vec![
        tab("[1 LEZ]", app.view == View::Lez),
        Span::raw(" "),
        tab("[2 L1", app.view == View::L1),
        dot,
        tab("]", app.view == View::L1),
        Span::raw("  "),
    ]
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let repo = &app.status.repo;
    let mut lez = tabs(app);
    lez.extend([Span::raw("LEZ ").bold(), Span::raw(app.saved.lez_dir.clone())]);
    if repo.present {
        lez.push(Span::raw("  "));
        lez.push(Span::raw(repo.branch.clone().unwrap_or_else(|| "(detached)".into())).green());
        lez.push(Span::raw(format!(" {}", repo.head.clone().unwrap_or_default())));
        if repo.dirty {
            lez.push(Span::raw(" dirty").yellow());
        }
    } else if app.lez.present() {
        lez.push(Span::raw("  …"));
    } else {
        lez.push(Span::raw("  not present (L to change)").red());
    }

    let info = &app.status.programs;
    let mut progs = vec![Span::raw("PROGS ").bold(), Span::raw(app.saved.programs_dir.clone())];
    if info.present {
        progs.push(Span::raw("  "));
        progs.push(Span::raw(info.branch.clone().unwrap_or_else(|| "(detached)".into())).green());
        progs.push(Span::raw(format!(" {}", info.head.clone().unwrap_or_default())));
        if info.dirty {
            progs.push(Span::raw(" dirty").yellow());
        }
    } else if app.programs.present() {
        progs.push(Span::raw("  …"));
    } else {
        progs.push(Span::raw("  not present (L to change)").red());
    }
    let tool = |built: bool, checkout: bool, missing: &'static str| match (built, checkout) {
        (true, _) => Span::raw("✓").green(),
        (false, true) => Span::raw("not built (b)").yellow(),
        (false, false) => Span::raw(missing).red(),
    };
    if repo.present {
        progs.push(Span::raw("  spel "));
        progs.push(tool(repo.spel, repo.spel_checkout, "missing (no token/nft calls)"));
        progs.push(Span::raw("  lez-call "));
        progs.push(tool(repo.lez_call, repo.lez_call_checkout, "missing (no jz calls)"));
        progs.push(Span::raw("  dry run "));
        progs.push(match (repo.lez_dryrun, repo.lez_dryrun_checkout) {
            (true, _) => Span::raw("✓").green(),
            (false, true) => Span::raw("not built (b)").yellow(),
            (false, false) => Span::raw("off").yellow(),
        });
    }

    let seq = &app.status.seq;
    let (dot, state) = match (seq.healthy, seq.managed, seq.port_open) {
        _ if !app.network.is_local() => match (seq.healthy, seq.port_open) {
            (true, _) => (Span::raw("●").green(), "reachable".to_owned()),
            (false, true) => (Span::raw("◐").yellow(), "answers, but not healthy".to_owned()),
            (false, false) => (Span::raw("○").red(), "unreachable".to_owned()),
        },
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
    let mut second = match &app.network {
        Network::Local => vec![Span::raw("SEQ ").bold(), dot, Span::raw(format!(" {state}"))],
        Network::Remote { name, .. } => vec![
            Span::raw("NET ").bold(),
            Span::raw(format!("{name} ")).magenta().bold(),
            dot,
            Span::raw(format!(" {state}")),
        ],
    };
    if let Some(block) = seq.block {
        second.push(Span::raw(format!(" · block {block}")));
    }
    second.push(Span::raw(format!(" · {}", app.rpc.url)).dark_gray());
    if let Network::Remote { channel_id: Some(expected), .. } = &app.network {
        match &seq.channel_id {
            Some(reported) if reported == expected => second.push(Span::raw(" · channel ✓").green()),
            Some(reported) => second.push(
                Span::raw(format!(" · channel {}… ≠ networks.json", &reported[..8.min(reported.len())])).red(),
            ),
            None => {}
        }
    }
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
        Paragraph::new(vec![Line::from(lez), Line::from(progs), Line::from(second)]),
        area,
    );
}

/// An account's native balance, `off-chain`, or nothing before its view loads.
fn chain_span(app: &App, id: &str) -> Span<'static> {
    match app.views.get(id) {
        Some(Some(view)) => Span::raw(format!(" {}", format_balance(view.balance()))).green(),
        Some(None) => Span::raw(" off-chain").dark_gray(),
        None => Span::raw(""),
    }
}

fn draw_accounts(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .account_rows()
        .into_iter()
        .map(|row| {
            let account = match row {
                AccountRow::Wallet(account) => account,
                AccountRow::Watched(watched) => {
                    return ListItem::new(Line::from(vec![
                        Span::raw("◇ ").magenta(),
                        Span::raw(short(&watched.id)),
                        Span::raw(format!(" [{}]", watched.note)).cyan(),
                        chain_span(app, &watched.id),
                    ]));
                }
            };
            let role = if app.saved.payer.as_deref() == Some(&account.id) {
                Span::raw("★ ").yellow()
            } else if app.saved.account.as_deref() == Some(&account.id) {
                Span::raw("▶ ").cyan()
            } else {
                Span::raw("  ")
            };
            let kind = if account.public { "" } else { "priv " };
            let chain = chain_span(app, &account.id);
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
    if let Some(AccountRow::Watched(watched)) = app.selected_row() {
        lines.push(Line::from(format!("Public/{}", watched.id)).bold());
        lines.push(Line::from(format!("{} · watched, not in the wallet (x forgets it)", watched.note)).dark_gray());
        lines.push(Line::default());
        push_view(&mut lines, app, &watched.id);
    } else if let Some(account) = app.selected_account() {
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
        if account.public {
            push_view(&mut lines, app, &account.id);
        } else {
            lines.push(Line::from("private: its state isn't on the public chain").dark_gray());
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

/// The chain view of a public account: nonce, balance and each actor state.
fn push_view(lines: &mut Vec<Line<'static>>, app: &App, id: &str) {
    match app.views.get(id) {
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
                let decoded = app
                    .decoded
                    .get(&(id.to_owned(), program.clone(), bytes.clone()))
                    .filter(|text| !text.is_empty());
                let shown = if program == NATIVE_TOKEN {
                    format!("balance {}", crate::rpc::decode_u128_le(bytes))
                } else if let Some(text) = decoded {
                    format!("{text}  (spel)")
                } else {
                    describe_bytes(bytes)
                };
                lines.push(Line::from(format!("    {shown}")));
            }
        }
    }
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
                .filter(|d| d.name == guest.name && !d.stale)
                .count();
            let mut spans = vec![Span::raw(guest.name.clone())];
            spans.push(
                Span::raw(match guest.origin {
                    Origin::Program(_) => " (lez-programs)",
                    _ => " (example_progs)",
                })
                .dark_gray(),
            );
            if !guest.built() {
                spans.push(Span::raw(" not built (b)").yellow());
            }
            if count > 0 {
                spans.push(Span::raw(format!(" ×{count}")).green());
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let guests_title = if app.guests.is_empty() {
        "Guests: none found, check lez-programs (L) and press b"
    } else {
        "Guests"
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
            .block(pane("Deployed (c call, v messages, x forget)", app.focus == Focus::Deployments))
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
    if app.view == View::L1 {
        frame.render_widget(
            Paragraph::new(
                " s start/stop node · m mining · a auto-claim · p PoW settings · b bridge to LEZ · t L1 transfer · j/k wallet · c/C config · g refresh │ 1 LEZ · 2 L1 · N network · ? help · q quit",
            )
            .dark_gray(),
            area,
        );
        return;
    }
    let context = match app.focus {
        Focus::Accounts => "P payer · A account · n new · t transfer · w watch · x forget watched",
        Focus::Guests => "v messages · d deploy · u build+deploy an example_progs file",
        Focus::Deployments => "c call (pick message) · v messages · x forget",
        Focus::Builtin => "read-only: details in the middle pane",
    };
    frame.render_widget(
        Paragraph::new(format!(
            " {context} │ W walkthrough · s seq start/stop · e seq cmds · R reset · b build · i wallet init │ 1 LEZ · 2 L1 · N network · Tab pane · ? help · q quit"
        ))
        .dark_gray(),
        area,
    );
}

fn draw_help(frame: &mut Frame) {
    let keys = [
        ("W", "walkthrough: build, reset, wallet, PAYER/ACCOUNT, deploy token, create a token"),
        ("s", "start the sequencer, or stop it (asks first if prog-tui didn't start it); local only"),
        ("R", "reset: stop, `just clean`, start (wipes the chain and the wallet); local only"),
        ("b", "build the wallet, lez-programs' programs + IDLs, the jz guests, spel, lez-call, lez-dryrun"),
        ("i", "initialise the wallet and import the two funded test accounts"),
        ("n", "new public account"),
        ("w / x", "watch an account outside the wallet (a PDA, say) / forget the selected one"),
        ("P / A", "make the selected account PAYER / ACCOUNT"),
        ("t", "native token transfer (wallet auth-transfer send), checked by both balances"),
        ("v", "the messages the selected guest or deployment accepts (spel IDL, or lez-call iface)"),
        ("d", "deploy the selected guest (wallet program-loader deploy; Guests pane)"),
        ("u", "build and deploy a guest from jz-progs/example_progs (as jz_<name>)"),
        ("c", "call the selected deployment: pick a message, fill in its fields, check the plan"),
        ("x", "forget the selected deployment"),
        ("h", "show or hide the header/segment accounts that deploys create"),
        ("e", "shell commands for the sequencer (log, wallet, stop) and the env.sh to source"),
        ("L", "change the LEZ and lez-programs checkouts"),
        ("N", "back to the network selector: local standalone, or one from networks.json"),
        ("g", "refresh now"),
        ("1 / 2", "the LEZ view / the L1 view: the logosctl daemon, the node, mining and its wallets"),
        ("L1: s", "start the L1 node (daemon, module, node), or stop it and the daemon (asks first)"),
        ("L1: b", "bridge L1 funds to a LEZ account (channel_deposit): pick the account, the amount, check the plan"),
        ("L1: t", "transfer between L1 addresses (wallet_transfer_funds): from, to (hex or a key name), amount, change"),
        ("L1: m / a", "switch mining / auto-claim on the running node (not kept: every start begins with mining off)"),
        ("L1: p", "PoW settings: the config's pow section (threads, claim tick, claim targets), read at node start; mine after start"),
        ("L1: c / C", "show the node's config (secrets hidden) / set which config file the node uses"),
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

/// The network selector prog-tui opens on: local standalone, then each sequencer in
/// `networks.json`.
pub fn draw_selector(frame: &mut Frame, choices: &[Network], selected: usize, error: Option<&str>) {
    let mut items: Vec<ListItem> = Vec::new();
    for (index, network) in choices.iter().enumerate() {
        let number = if index < 9 { format!("{} ", index + 1) } else { "  ".to_owned() };
        let line = match network {
            Network::Local => Line::from(vec![
                Span::raw(number),
                Span::raw("local standalone").bold(),
                Span::raw("  127.0.0.1:3040; prog-tui starts, stops and resets it").dark_gray(),
            ]),
            Network::Remote { name, url, channel_id } => Line::from(vec![
                Span::raw(number),
                Span::raw(name.clone()).magenta().bold(),
                Span::raw(format!("  {url}")),
                Span::raw(match channel_id {
                    Some(id) => format!("  channel {}…{}", &id[..8], &id[56..]),
                    None => String::new(),
                })
                .dark_gray(),
            ]),
        };
        items.push(ListItem::new(line));
    }
    let mut notes = vec![
        Line::from(format!(
            "Remote networks come from prog-tui/{NETWORKS_FILE}; each keeps its own wallet and state."
        ))
        .dark_gray(),
    ];
    if choices.len() == 1 && error.is_none() {
        notes.push(Line::from(format!("No {NETWORKS_FILE}: only the local chain to choose.")).dark_gray());
    }
    if let Some(error) = error {
        notes.push(Line::from(format!("✗ {error}")).red());
    }
    let list_height = u16::try_from(items.len()).unwrap_or(10);
    let notes_height = u16::try_from(notes.len()).unwrap_or(2);
    let area = centered(frame.area(), 100, list_height + notes_height + 4);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(" prog-tui: which network? ")
        .title_bottom(" Enter or 1–9 choose · ↑↓ select · q quit ")
        .border_style(Color::Cyan);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [list_area, _, notes_area] = Layout::vertical([
        Constraint::Length(list_height),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(inner);
    frame.render_stateful_widget(
        List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        list_area,
        &mut ListState::default().with_selected(Some(selected)),
    );
    frame.render_widget(Paragraph::new(notes).wrap(Wrap { trim: false }), notes_area);
}

// ---- The L1 view ----

fn uptime(seconds: u64) -> String {
    match seconds {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m{:02}s", s / 60, s % 60),
        s if s < 86_400 => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
        s => format!("{}d{:02}h", s / 86_400, (s % 86_400) / 3600),
    }
}

/// A field of one of the node's reports, as text: `-` when it's missing.
fn field(value: Option<&serde_json::Value>, key: &str) -> String {
    match value.and_then(|value| value.get(key)) {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Null) | None => "-".to_owned(),
        Some(other) => other.to_string(),
    }
}

fn start_hint(lines: &mut Vec<Line<'static>>, l1: &L1Status, config: Option<&std::path::Path>) {
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::raw("s").cyan(),
        Span::raw(" starts it here; by hand, that's:").dark_gray(),
    ]));
    let config = config.map_or("<user_config.yaml>".to_owned(), |path| path.display().to_string());
    let mut commands = Vec::new();
    if !l1.running() {
        commands.push("logosctl daemon start --detach".to_owned());
    }
    if !l1.module_loaded() {
        commands.push(format!("logosctl module load {MODULE}"));
    }
    commands.push(format!("logosctl call {MODULE} start str:{config} str:"));
    for command in commands {
        lines.push(Line::from(format!("  {command}")).cyan());
    }
}

fn draw_l1_daemon(frame: &mut Frame, area: Rect, app: &App) {
    let l1 = &app.l1;
    let mut lines: Vec<Line> = Vec::new();
    if !l1.checked {
        lines.push(Line::from("asking logosctl…").dark_gray());
    } else if let Some(program) = &l1.logosctl {
        lines.push(Line::from(vec![
            Span::raw("logosctl ").dark_gray(),
            Span::raw(program.display().to_string()),
        ]));
        lines.push(Line::from(vec![
            Span::raw("session  ").dark_gray(),
            Span::raw(l1.config_dir.display().to_string()),
        ]));
        lines.push(Line::default());
        match &l1.daemon {
            Some(Ok(daemon)) => {
                let state = Span::raw(daemon.status.clone());
                lines.push(Line::from(vec![
                    Span::raw("daemon ").bold(),
                    if daemon.status == "running" { state.green() } else { state.yellow() },
                    Span::raw(format!(
                        "{}{}",
                        daemon.pid.map(|pid| format!(" · pid {pid}")).unwrap_or_default(),
                        daemon.version.as_deref().map(|v| format!(" · v{v}")).unwrap_or_default(),
                    ))
                    .dark_gray(),
                ]));
            }
            Some(Err(error)) => {
                lines.push(Line::from(vec![Span::raw("daemon ").bold(), Span::raw("not answering").red()]));
                lines.push(Line::from(error.clone()).dark_gray());
            }
            None => {}
        }
        if !l1.modules.is_empty() {
            lines.push(Line::default());
            lines.push(Line::from("modules").bold());
        }
        for module in &l1.modules {
            let state = Span::raw(format!(" {}", module.status));
            let mut spans = vec![
                Span::raw(format!("  {}", module.name)).fg(if module.name == MODULE {
                    Color::Cyan
                } else {
                    Color::Reset
                }),
                if module.status == "loaded" { state.green() } else { state.yellow() },
            ];
            let mut meta = Vec::new();
            if !module.version.is_empty() {
                meta.push(format!("v{}", module.version));
            }
            if let Some(seconds) = module.uptime_seconds {
                meta.push(format!("up {}", uptime(seconds)));
            }
            if let Some(memory) = module.memory_mb {
                meta.push(format!("{memory:.0} MB"));
            }
            if let Some(cpu) = module.cpu_percent {
                meta.push(format!("{cpu:.1}% cpu"));
            }
            spans.push(Span::raw(format!(" {}", meta.join(" · "))).dark_gray());
            lines.push(Line::from(spans));
        }
        if !l1.running() || !l1.module_loaded() {
            start_hint(&mut lines, l1, app.l1_config().as_deref());
        }
    } else {
        lines.push(Line::from("logosctl isn't on PATH").red());
        lines.push(
            Line::from("Install Logos Core's CLI and put it on PATH; this view runs it.").dark_gray(),
        );
        start_hint(&mut lines, l1, app.l1_config().as_deref());
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(pane("L1 daemon (logosctl)", false)),
        area,
    );
}

fn draw_l1_node(frame: &mut Frame, area: Rect, app: &App) {
    let l1 = &app.l1;
    let mut lines: Vec<Line> = Vec::new();
    let label = |text: &'static str| Span::raw(format!("{text:<11}")).dark_gray();
    let node_log = l1.node_log.as_ref().filter(|_| l1.module_loaded());
    match &l1.node {
        None if !l1.checked => lines.push(Line::from("…").dark_gray()),
        None if node_log.is_some_and(|log| log.started() && !log.online) => {
            let log = node_log.expect("checked above");
            lines.push(Line::from(vec![label("mode"), Span::raw("Bootstrapping").yellow()]));
            lines.push(Line::from(vec![
                label("phase"),
                Span::raw(log.phase.clone().unwrap_or_default()),
                Span::raw(format!(" since {}", log.since.as_deref().unwrap_or("?"))).dark_gray(),
            ]));
            if let Some(slot) = log.slot {
                lines.push(Line::from(vec![
                    label("slot"),
                    Span::raw(slot.to_string()),
                    Span::raw(format!("  latest block at {}", log.slot_at.as_deref().unwrap_or("?"))).dark_gray(),
                ]));
            }
            if let Some(at) = log.online_expected() {
                lines.push(Line::from(vec![label("online"), Span::raw(format!("about {at}")).cyan()]));
            }
            lines.push(
                Line::from("It doesn't answer calls until Online, so wallets and balances wait; this is read from its log.")
                    .dark_gray(),
            );
        }
        None if node_log.is_some_and(|log| log.shut_down.is_some()) => {
            let why = node_log.and_then(|log| log.shut_down.clone()).unwrap_or_default();
            lines.push(Line::from("the node shut itself down").red());
            lines.push(Line::from(why).dark_gray());
            lines.push(Line::from("s starts it again").dark_gray());
        }
        None if l1.module_loaded() && node_log.is_some_and(|log| !log.started()) => {
            lines.push(Line::from("the node isn't started (s starts it)").yellow());
        }
        None if l1.module_loaded() => lines.push(Line::from("…").dark_gray()),
        None => lines.push(Line::from(format!("{MODULE} isn't loaded")).dark_gray()),
        Some(node) => {
            let chain = node.chain.as_ref();
            let mode = field(chain, "mode");
            let mode_span = Span::raw(mode.clone());
            lines.push(Line::from(vec![
                label("mode"),
                if mode == "Online" { mode_span.green() } else { mode_span.yellow() },
            ]));
            lines.push(Line::from(vec![label("height"), Span::raw(field(chain, "height"))]));
            lines.push(Line::from(vec![
                label("slot"),
                Span::raw(field(chain, "slot")),
                Span::raw(format!("  lib slot {}", field(chain, "lib_slot"))).dark_gray(),
            ]));
            let tip = field(chain, "tip");
            lines.push(Line::from(vec![label("tip"), Span::raw(short(&tip))]));
            let network = node.network.as_ref();
            lines.push(Line::from(vec![
                label("peers"),
                Span::raw(field(network, "n_peers")),
                Span::raw(format!("  connections {}", field(network, "n_connections"))).dark_gray(),
            ]));
            let time = node.time.as_ref();
            lines.push(Line::from(vec![
                label("epoch"),
                Span::raw(field(time, "current_epoch")),
                Span::raw(format!("  genesis {}", field(time, "genesis_time"))).dark_gray(),
            ]));
            let pow = node.pow.as_ref();
            let (mining, armed) = pow.map(l1::pow_flags).unwrap_or_default();
            let switch = |flag: Option<bool>, on: &str, off: &str| match flag {
                Some(true) => Span::raw(on.to_owned()).green(),
                Some(false) => Span::raw(off.to_owned()).yellow(),
                None => Span::raw("-"),
            };
            lines.push(Line::from(vec![
                label("mining"),
                switch(mining, "on", "off"),
                Span::raw("  m switches").dark_gray(),
            ]));
            let claim = pow.and_then(|pow| pow.get("auto_claim"));
            let tick = claim.and_then(|claim| claim.get("tick"));
            let every = match (tick.and_then(|t| t.get("unit")).and_then(|u| u.as_str()), tick.and_then(|t| t.get("value"))) {
                (Some("slots"), Some(value)) => format!(" every {value} slots"),
                (_, Some(value)) => format!(" every {value}s"),
                _ => String::new(),
            };
            lines.push(Line::from(vec![
                label("auto-claim"),
                switch(armed, "armed", "off"),
                Span::raw(format!("{every}  a switches")).dark_gray(),
            ]));
            let names: std::collections::BTreeMap<&str, &str> = l1
                .wallets
                .iter()
                .filter_map(|wallet| Some((wallet.address.as_str(), wallet.name.as_deref()?)))
                .collect();
            let targets = claim.and_then(|claim| claim.get("targets")).and_then(|t| t.as_array());
            for target in targets.into_iter().flatten() {
                let key = field(Some(target), "public_key");
                let name = names.get(key.as_str()).map_or_else(|| short(&key), |name| (*name).to_owned());
                lines.push(Line::from(vec![
                    Span::raw(format!("  → {name} ≥ {}", field(Some(target), "threshold"))),
                    Span::raw(format!("  balance {}", field(Some(target), "balance"))).dark_gray(),
                ]));
            }
            if targets.is_some_and(Vec::is_empty) {
                lines.push(Line::from("  no claim targets: p sets them").dark_gray());
            }
            let rewards = node.rewards.as_ref();
            let enabled = pow.and_then(|pow| pow.get("are_rewards_enabled")).and_then(|v| v.as_bool());
            lines.push(Line::from(vec![
                label("tickets"),
                Span::raw(field(rewards, "claimable_tickets")),
                Span::raw(match enabled {
                    Some(false) => "  claimable · rewards off on this network".to_owned(),
                    _ => "  claimable".to_owned(),
                })
                .dark_gray(),
            ]));
            let vouchers = node.vouchers.as_ref();
            lines.push(Line::from(vec![
                label("vouchers"),
                Span::raw(field(vouchers, "total_claimable")),
                Span::raw(format!("  claimable · reward amount {}", field(vouchers, "reward_amount"))).dark_gray(),
            ]));
            if let Some(error) = &node.error {
                lines.push(Line::from(format!("✗ {error}")).red());
            }
        }
    }

    lines.push(Line::default());
    match app.l1_config() {
        Some(path) => {
            let exists = path.is_file();
            lines.push(Line::from(vec![
                label("config"),
                Span::raw(path.display().to_string()).fg(if exists { Color::Reset } else { Color::Red }),
            ]));
            if !exists {
                lines.push(Line::from("  no such file (C changes it)").red());
            } else if app.saved.l1_config.trim().is_empty() {
                lines.push(Line::from("  Basecamp's (C sets another) · c shows it").dark_gray());
            } else {
                lines.push(Line::from("  c shows it · C changes it").dark_gray());
            }
            match &l1.pow_config {
                Some(Ok(settings)) => {
                    let names = l1::names_beside(Some(&path));
                    lines.push(Line::from(vec![label("pow"), Span::raw(settings.summary(&names))]));
                    let mine = if app.saved.l1_mine_after_start { "mine after start · " } else { "" };
                    lines.push(Line::from(format!("  {mine}read at node start · p changes")).dark_gray());
                }
                Some(Err(error)) => lines.push(Line::from(vec![label("pow"), Span::raw(format!("✗ {error}")).red()])),
                None => {}
            }
        }
        None => lines.push(Line::from(vec![label("config"), Span::raw("not set (C sets it)").yellow()])),
    }

    if let Some(wallet) = l1.wallets.get(app.selected_l1_wallet) {
        lines.push(Line::default());
        lines.push(Line::from(wallet.name.clone().unwrap_or_else(|| "wallet".to_owned())).bold());
        lines.push(Line::from(wallet.address.clone()));
        match &wallet.balance {
            Ok(balance) => lines.push(Line::from(format!("balance {balance}")).green()),
            Err(_) if wallet.holds_no_funds() => lines.push(
                Line::from("the wallet tracks no funds for this key (a blend, network or stake key)")
                    .dark_gray(),
            ),
            Err(error) => lines.push(Line::from(format!("✗ {error}")).red()),
        }
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(pane("L1 node", false)),
        area,
    );
}

fn draw_l1_wallets(frame: &mut Frame, area: Rect, app: &App) {
    let l1 = &app.l1;
    let width = l1
        .wallets
        .iter()
        .map(|wallet| wallet.name.as_deref().map_or(0, str::len))
        .max()
        .unwrap_or(0);
    let items: Vec<ListItem> = l1
        .wallets
        .iter()
        .map(|wallet| {
            let name = wallet.name.clone().unwrap_or_default();
            let mut spans = vec![
                Span::raw(format!("{name:<width$} ")).magenta(),
                Span::raw(short(&wallet.address)),
            ];
            spans.push(match &wallet.balance {
                Ok(balance) => Span::raw(format!(" {balance}")).green(),
                Err(_) if wallet.holds_no_funds() => Span::raw(" no funds key").dark_gray(),
                Err(_) => Span::raw(" ✗").red(),
            });
            ListItem::new(Line::from(spans))
        })
        .collect();
    let title = match (l1.total(), &l1.wallets_error) {
        (_, Some(error)) => format!("L1 wallets: ✗ {error}"),
        (Some(total), None) => format!("L1 wallets · total {total}"),
        (None, None) if l1.node.is_none() => "L1 wallets".to_owned(),
        (None, None) => "L1 wallets · no balances".to_owned(),
    };
    frame.render_stateful_widget(
        List::new(items)
            .block(pane(&title, true))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        area,
        &mut ListState::default().with_selected(Some(app.selected_l1_wallet)),
    );
}
