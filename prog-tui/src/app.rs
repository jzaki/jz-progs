//! UI state and key handling. Long work goes to [`crate::jobs`]; results come back as [`Msg`]s.

use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    path::PathBuf,
    time::{Duration, Instant},
};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::{runtime::Handle, sync::mpsc::UnboundedSender};

use crate::{
    builtin::{self, Builtin},
    jobs::{self, Ctx, Msg, Status, StepState},
    lez::Lez,
    rpc::{AccountView, Rpc},
    sequencer::{self, StopTarget},
    state::{self, Deployment, Saved},
    userprogs,
    wallet::WalletAccount,
};

const LOG_LINES: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Accounts,
    Guests,
    Deployments,
    Builtin,
}

pub struct Form {
    pub title: String,
    /// `(label, value)`. For a call, each label is also the `prog_cli` flag.
    pub fields: Vec<(String, String)>,
    pub selected: usize,
    pub action: FormAction,
}

impl Form {
    /// Starts on the first empty field: prefilled ones usually need no change.
    fn new(title: String, fields: Vec<(String, String)>, action: FormAction) -> Self {
        let selected = fields
            .iter()
            .position(|(_, value)| value.is_empty())
            .unwrap_or(0);
        Self {
            title,
            fields,
            selected,
            action,
        }
    }
}

pub enum FormAction {
    NewAccount,
    /// `wallet auth-transfer send`: fields `from`, `to`, `amount`.
    Transfer,
    /// Build and deploy a guest source file: field `file`.
    BuildDeploy,
    LezDir,
    /// `prog_cli <subcommand> --<label> <value>… <extra>…`
    Call {
        subcommand: String,
        extra: Vec<String>,
    },
}

#[derive(Clone, Copy)]
pub enum Confirmed {
    Reset(Option<StopTarget>),
    Walkthrough(Option<StopTarget>),
    StopExternal(i32),
}

pub enum Modal {
    None,
    Help,
    Form(Form),
    Confirm(String, Confirmed),
}

pub struct App {
    rt: Handle,
    tx: UnboundedSender<Msg>,
    crate_dir: PathBuf,
    pub state_dir: PathBuf,
    pub saved: Saved,
    pub lez: Lez,
    pub rpc: Rpc,
    pub status: Status,
    pub accounts: Vec<WalletAccount>,
    pub views: BTreeMap<String, Option<AccountView>>,
    pub guests: Vec<String>,
    pub focus: Focus,
    pub selected_account: usize,
    pub selected_guest: usize,
    pub selected_deployment: usize,
    pub selected_builtin: usize,
    pub builtins: Vec<Builtin>,
    /// Hide the header and segment accounts that deploys create.
    pub hide_loader: bool,
    pub log: VecDeque<String>,
    pub busy: Option<(String, Instant)>,
    pub checklist: Vec<(String, StepState)>,
    pub modal: Modal,
    pub quit: bool,
    pub ticks: usize,
    refreshing: Refreshing,
    last_status: Option<Instant>,
    last_accounts: Option<Instant>,
    last_views: Option<Instant>,
}

#[derive(Default)]
struct Refreshing {
    status: bool,
    accounts: bool,
    views: bool,
}

impl App {
    pub fn new(rt: Handle, tx: UnboundedSender<Msg>) -> Self {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let state_dir = crate_dir.join(".state");
        let saved = Saved::load(&state_dir);
        let lez = Lez::new(state::resolve_lez_dir(&crate_dir, &saved.lez_dir));
        let rpc = Rpc::new(lez.sequencer_url());
        let mut app = Self {
            rt,
            tx,
            crate_dir,
            state_dir,
            saved,
            lez,
            rpc,
            status: Status::default(),
            accounts: Vec::new(),
            views: BTreeMap::new(),
            guests: Vec::new(),
            focus: Focus::Accounts,
            selected_account: 0,
            selected_guest: 0,
            selected_deployment: 0,
            selected_builtin: 0,
            builtins: builtin::builtins(),
            hide_loader: true,
            log: VecDeque::new(),
            busy: None,
            checklist: Vec::new(),
            modal: Modal::None,
            quit: false,
            ticks: 0,
            refreshing: Refreshing::default(),
            last_status: None,
            last_accounts: None,
            last_views: None,
        };
        app.guests = app.lez.guests();
        app.save();
        app.push_log(format!(
            "prog-tui · LEZ at {} · press ? for keys",
            app.lez.dir.display()
        ));
        app.push_log(format!(
            "env vars for your shell: source {}",
            state::env_sh_path(&app.state_dir).display()
        ));
        app
    }

    fn save(&mut self) {
        if let Err(error) = self.saved.save(&self.state_dir, &self.lez) {
            self.push_log(format!("✗ couldn't save state: {error:#}"));
        }
    }

    pub fn push_log(&mut self, line: String) {
        self.log.push_back(line);
        while self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
    }

    // ---- What the panes show ----

    /// Header and segment accounts that deploys create: the ones holding only program-loader state.
    fn is_loader_account(&self, id: &str) -> bool {
        if self.saved.deployments.iter().any(|d| d.program_id == id) {
            return true;
        }
        let loader = builtin::program_loader_id();
        self.views
            .get(id)
            .and_then(Option::as_ref)
            .is_some_and(|view| {
                !view.actor_states.is_empty() && view.actor_states.keys().all(|key| *key == loader)
            })
    }

    pub fn visible_accounts(&self) -> Vec<&WalletAccount> {
        self.accounts
            .iter()
            .filter(|account| {
                !self.hide_loader
                    || self.saved.payer.as_deref() == Some(account.id.as_str())
                    || self.saved.account.as_deref() == Some(account.id.as_str())
                    || !self.is_loader_account(&account.id)
            })
            .collect()
    }

    pub fn selected_account(&self) -> Option<&WalletAccount> {
        self.visible_accounts().get(self.selected_account).copied()
    }

    pub fn program_label(&self, program_id: &str) -> String {
        if let Some(name) = self.saved.program_name(program_id) {
            return name.to_owned();
        }
        match self.builtins.iter().find(|builtin| builtin.id == program_id) {
            Some(builtin) => builtin.name.to_owned(),
            None => jobs::short(program_id),
        }
    }

    // ---- Periodic refreshes ----

    pub fn on_tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        let due = |last: Option<Instant>, every: u64| {
            last.is_none_or(|last| last.elapsed() >= Duration::from_secs(every))
        };

        if !self.refreshing.status && due(self.last_status, 2) {
            self.refreshing.status = true;
            self.last_status = Some(Instant::now());
            self.guests = self.lez.guests();
            let (lez, state_dir, rpc, tx) = (
                self.lez.clone(),
                self.state_dir.clone(),
                self.rpc.clone(),
                self.tx.clone(),
            );
            self.rt.spawn(async move {
                let status = jobs::gather_status(lez, state_dir, rpc).await;
                let _ = tx.send(Msg::Status(status));
            });
        }

        // The wallet rewrites storage.json during jobs, so only list accounts between them. With
        // no storage.json, any wallet command creates a new wallet, so leave that to `i`.
        let wallet_exists = self.lez.storage_json().is_file();
        if !wallet_exists {
            self.accounts.clear();
        }
        if self.busy.is_none()
            && !self.refreshing.accounts
            && self.status.repo.wallet_built
            && wallet_exists
            && due(self.last_accounts, 10)
        {
            self.refreshing.accounts = true;
            self.last_accounts = Some(Instant::now());
            let (lez, tx) = (self.lez.clone(), self.tx.clone());
            self.rt.spawn(async move {
                let accounts = jobs::list_accounts(&lez).await.unwrap_or_default();
                let _ = tx.send(Msg::Accounts(accounts));
            });
        }

        if self.status.seq.healthy && !self.refreshing.views && due(self.last_views, 3) {
            self.refreshing.views = true;
            self.last_views = Some(Instant::now());
            let mut ids: Vec<String> = self
                .accounts
                .iter()
                .filter(|account| account.public)
                .map(|account| account.id.clone())
                .collect();
            ids.extend(self.saved.deployments.iter().map(|d| d.program_id.clone()));
            // Engine programs have no account to look up; the others are deployed at genesis.
            ids.extend(
                self.builtins
                    .iter()
                    .filter(|builtin| !builtin.engine)
                    .map(|builtin| builtin.id.clone()),
            );
            ids.sort();
            ids.dedup();
            let (rpc, tx) = (self.rpc.clone(), self.tx.clone());
            self.rt.spawn(async move {
                let views = jobs::fetch_views(&rpc, ids).await;
                let _ = tx.send(Msg::Views(views));
            });
        }
    }

    fn refresh_now(&mut self) {
        self.last_status = None;
        self.last_accounts = None;
        self.last_views = None;
    }

    pub fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Log(line) => self.push_log(line),
            Msg::JobDone { name, log, result } => {
                let seconds = self.busy.take().map_or(0, |(_, since)| since.elapsed().as_secs());
                match result {
                    Ok(()) => self.push_log(format!("✓ {name} ({seconds}s)")),
                    Err(error) => {
                        self.push_log(format!("✗ {name} failed: {error}"));
                        self.push_log(format!("  full output: {}", log.display()));
                    }
                }
                self.refresh_now();
            }
            Msg::Status(status) => {
                self.refreshing.status = false;
                if !status.seq.healthy {
                    self.views.clear();
                }
                self.status = status;
            }
            Msg::Accounts(accounts) => {
                self.refreshing.accounts = false;
                self.accounts = accounts;
                self.clamp_selection();
            }
            Msg::Views(views) => {
                self.refreshing.views = false;
                let mut changed = false;
                for deployment in &mut self.saved.deployments {
                    if let Some(view) = views.get(&deployment.program_id) {
                        let stale = view.is_none();
                        changed |= deployment.stale != stale;
                        deployment.stale = stale;
                    }
                }
                self.views.extend(views);
                if changed {
                    self.save();
                }
                self.clamp_selection();
            }
            Msg::SetPayer(id) => {
                self.saved.payer = Some(id);
                self.save();
            }
            Msg::SetAccount(id) => {
                self.saved.account = Some(id);
                self.save();
                self.last_accounts = None;
            }
            Msg::Deployed(deployment) => {
                self.saved.deployments.push(deployment);
                self.save();
            }
            Msg::ChainReset => {
                self.saved.clear_chain();
                self.accounts.clear();
                self.views.clear();
                self.selected_account = 0;
                self.selected_deployment = 0;
                self.save();
            }
            Msg::Checklist(steps) => {
                self.checklist = steps
                    .into_iter()
                    .map(|step| (step, StepState::Pending))
                    .collect();
            }
            Msg::Step(index, state) => {
                if let Some(step) = self.checklist.get_mut(index) {
                    step.1 = state;
                }
            }
        }
    }

    fn clamp_selection(&mut self) {
        let accounts = self.visible_accounts().len();
        self.selected_account = self.selected_account.min(accounts.saturating_sub(1));
        self.selected_guest = self
            .selected_guest
            .min(self.guests.len().saturating_sub(1));
        self.selected_deployment = self
            .selected_deployment
            .min(self.saved.deployments.len().saturating_sub(1));
        self.selected_builtin = self
            .selected_builtin
            .min(self.builtins.len().saturating_sub(1));
    }

    // ---- Jobs ----

    fn spawn_job<F, Fut>(&mut self, name: &str, job: F)
    where
        F: FnOnce(Ctx) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        if let Some((busy, _)) = &self.busy {
            let busy = busy.clone();
            self.push_log(format!("busy: wait for `{busy}` to finish"));
            return;
        }
        if !self.lez.present() {
            self.push_log(format!(
                "✗ no LEZ checkout at {} (key L to change it)",
                self.lez.dir.display()
            ));
            return;
        }
        let ctx = Ctx::new(
            self.tx.clone(),
            self.lez.clone(),
            self.state_dir.clone(),
            self.rpc.clone(),
            self.saved.wallet_password.clone(),
            name,
        );
        let log = ctx.log_path.clone();
        let tx = self.tx.clone();
        let name = name.to_owned();
        self.busy = Some((name.clone(), Instant::now()));
        self.push_log(format!("▶ {name}"));
        self.rt.spawn(async move {
            let result = job(ctx).await.map_err(|error| format!("{error:#}"));
            let _ = tx.send(Msg::JobDone { name, log, result });
        });
    }

    /// How to stop whatever sequencer is running now, if any.
    fn running_sequencer(&self) -> Option<StopTarget> {
        let seq = &self.status.seq;
        if let Some(pgid) = seq.managed {
            return Some(StopTarget::Managed(pgid));
        }
        seq.listener
            .filter(|_| seq.port_open)
            .map(StopTarget::External)
    }

    fn need(&mut self, what: &str, value: Option<&String>) -> Option<String> {
        if value.is_none() {
            self.push_log(format!("✗ set {what} first"));
        }
        value.cloned()
    }

    fn prog_cli_ready(&mut self) -> bool {
        if self.lez.release_bin("prog_cli").is_file() {
            return true;
        }
        if self.lez.prog_cli_src().is_file() {
            self.push_log("✗ prog_cli isn't built yet (key b)".to_owned());
        } else {
            self.push_log(format!(
                "✗ {} has no examples/program_deployment/src/bin/prog_cli.rs: deploy/call need it; key r runs the stock runners",
                self.lez.dir.display()
            ));
        }
        false
    }

    // ---- Keys ----

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::None => self.on_main_key(key),
            Modal::Help => {}
            Modal::Confirm(text, action) => {
                if matches!(key.code, KeyCode::Char('y' | 'Y')) {
                    self.confirmed(action);
                } else if key.code != KeyCode::Esc && !matches!(key.code, KeyCode::Char(_)) {
                    self.modal = Modal::Confirm(text, action);
                }
            }
            Modal::Form(form) => self.on_form_key(form, key),
        }
    }

    fn on_main_key(&mut self, key: KeyEvent) {
        if self.focus == Focus::Builtin
            && matches!(key.code, KeyCode::Char('d' | 'r' | 'c' | 'm' | 'x'))
        {
            self.push_log("built-in programs are read-only: you can't deploy, call or forget them".to_owned());
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.modal = Modal::Help,
            KeyCode::Tab | KeyCode::Right => self.cycle_focus(true),
            KeyCode::BackTab | KeyCode::Left => self.cycle_focus(false),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Char('g') => {
                self.refresh_now();
                self.push_log("refreshing".to_owned());
            }
            KeyCode::Char('h') => {
                self.hide_loader = !self.hide_loader;
                self.clamp_selection();
            }
            KeyCode::Char('e') => {
                for line in sequencer::sequencer_help(&self.state_dir, &self.rpc.url, &self.status.seq) {
                    self.push_log(line);
                }
            }
            KeyCode::Char('L') => {
                self.modal = Modal::Form(Form::new(
                    "LEZ checkout (absolute, ~/…, or relative to prog-tui)".to_owned(),
                    vec![("dir".to_owned(), self.saved.lez_dir.clone())],
                    FormAction::LezDir,
                ));
            }
            KeyCode::Char('s') => self.toggle_sequencer(),
            KeyCode::Char('R') => {
                let stop = self.running_sequencer();
                self.modal = Modal::Confirm(
                    format!(
                        "Reset the chain?{} `just clean` wipes the chain and the wallet, then the sequencer starts fresh. y/n",
                        stop_note(stop)
                    ),
                    Confirmed::Reset(stop),
                );
            }
            KeyCode::Char('W') => {
                let stop = self.running_sequencer();
                self.modal = Modal::Confirm(
                    format!(
                        "Run the walkthrough? It builds, then resets the chain and wallet.{} y/n",
                        stop_note(stop)
                    ),
                    Confirmed::Walkthrough(stop),
                );
            }
            KeyCode::Char('b') => self.spawn_job("build", |mut ctx| async move {
                jobs::build(&mut ctx).await
            }),
            KeyCode::Char('i') => self.spawn_job("wallet init", |mut ctx| async move {
                jobs::wallet_init(&mut ctx).await
            }),
            KeyCode::Char('n') => {
                self.modal = Modal::Form(Form::new(
                    "New public account".to_owned(),
                    vec![("label".to_owned(), String::new())],
                    FormAction::NewAccount,
                ));
            }
            KeyCode::Char('t') => self.open_transfer(),
            KeyCode::Char('u') => {
                self.modal = Modal::Form(Form::new(
                    "Build and deploy a guest (path relative to jz-progs/example_progs)".to_owned(),
                    vec![("file".to_owned(), "hello_world.rs".to_owned())],
                    FormAction::BuildDeploy,
                ));
            }
            KeyCode::Char('P') => self.set_role(true),
            KeyCode::Char('A') => self.set_role(false),
            KeyCode::Char('d') => self.deploy_selected(),
            KeyCode::Char('r') => self.run_runner(),
            KeyCode::Char('c') => self.open_call(false),
            KeyCode::Char('m') => self.open_call(true),
            KeyCode::Char('x')
                if self.focus == Focus::Deployments
                    && self.selected_deployment < self.saved.deployments.len() =>
            {
                let removed = self.saved.deployments.remove(self.selected_deployment);
                self.push_log(format!("forgot {} {}", removed.name, removed.program_id));
                self.save();
                self.clamp_selection();
            }
            _ => {}
        }
    }

    fn cycle_focus(&mut self, forward: bool) {
        self.focus = match (self.focus, forward) {
            (Focus::Accounts, true) | (Focus::Deployments, false) => Focus::Guests,
            (Focus::Guests, true) | (Focus::Builtin, false) => Focus::Deployments,
            (Focus::Deployments, true) | (Focus::Accounts, false) => Focus::Builtin,
            (Focus::Builtin, true) | (Focus::Guests, false) => Focus::Accounts,
        };
    }

    fn move_selection(&mut self, delta: isize) {
        let visible = self.visible_accounts().len();
        let (selected, len) = match self.focus {
            Focus::Accounts => (&mut self.selected_account, visible),
            Focus::Guests => (&mut self.selected_guest, self.guests.len()),
            Focus::Deployments => (&mut self.selected_deployment, self.saved.deployments.len()),
            Focus::Builtin => (&mut self.selected_builtin, self.builtins.len()),
        };
        *selected = selected.saturating_add_signed(delta).min(len.saturating_sub(1));
        self.clamp_selection();
    }

    fn toggle_sequencer(&mut self) {
        match self.running_sequencer() {
            Some(StopTarget::Managed(pgid)) => self.spawn_job("stop sequencer", move |mut ctx| async move {
                jobs::stop_sequencer(&mut ctx, StopTarget::Managed(pgid)).await
            }),
            Some(StopTarget::External(pid)) => {
                self.modal = Modal::Confirm(
                    format!("Stop the sequencer prog-tui didn't start (pid {pid})? y/n"),
                    Confirmed::StopExternal(pid),
                );
            }
            None => self.spawn_job("start sequencer", |mut ctx| async move {
                jobs::start_sequencer(&mut ctx).await
            }),
        }
    }

    fn confirmed(&mut self, action: Confirmed) {
        match action {
            Confirmed::Reset(stop) => self.spawn_job("reset chain", move |mut ctx| async move {
                jobs::reset(&mut ctx, stop).await
            }),
            Confirmed::Walkthrough(stop) => {
                self.spawn_job("walkthrough", move |mut ctx| async move {
                    jobs::walkthrough(&mut ctx, stop).await
                });
            }
            Confirmed::StopExternal(pid) => {
                self.spawn_job("stop sequencer", move |mut ctx| async move {
                    jobs::stop_sequencer(&mut ctx, StopTarget::External(pid)).await
                });
            }
        }
    }

    fn set_role(&mut self, payer: bool) {
        if self.focus != Focus::Accounts {
            self.push_log("select an account in the Accounts pane first".to_owned());
            return;
        }
        let Some(account) = self.selected_account().cloned() else {
            return;
        };
        if !account.public {
            self.push_log("✗ only public accounts can be PAYER or ACCOUNT".to_owned());
            return;
        }
        let (role, slot) = if payer {
            ("PAYER", &mut self.saved.payer)
        } else {
            ("ACCOUNT", &mut self.saved.account)
        };
        *slot = Some(account.id.clone());
        self.push_log(format!("{role}={}", account.id));
        self.save();
    }

    /// The transfer form: from `PAYER`, to the selected account (when the Accounts pane has
    /// focus and it isn't the payer) or else `ACCOUNT`.
    fn open_transfer(&mut self) {
        let payer = self.saved.payer.clone().unwrap_or_default();
        let selected = (self.focus == Focus::Accounts)
            .then(|| self.selected_account())
            .flatten()
            .filter(|account| account.public && account.id != payer)
            .map(|account| account.id.clone());
        let to = selected
            .or_else(|| self.saved.account.clone())
            .unwrap_or_default();
        self.modal = Modal::Form(Form::new(
            "Native token transfer → wallet auth-transfer send".to_owned(),
            vec![
                ("from".to_owned(), payer),
                ("to".to_owned(), to),
                ("amount".to_owned(), "1000".to_owned()),
            ],
            FormAction::Transfer,
        ));
    }

    fn deploy_selected(&mut self) {
        if self.focus != Focus::Guests {
            self.push_log("select a guest in the Programs pane to deploy".to_owned());
            return;
        }
        let Some(guest) = self.guests.get(self.selected_guest).cloned() else {
            return;
        };
        if !self.prog_cli_ready() {
            return;
        }
        let Some(payer) = self.need("PAYER (key P)", self.saved.payer.clone().as_ref()) else {
            return;
        };
        self.spawn_job(&format!("deploy {guest}"), move |mut ctx| async move {
            jobs::deploy(&mut ctx, &guest, &payer).await.map(|_| ())
        });
    }

    /// The stock `run_*` runner for the selected guest: deploys again and calls once.
    fn run_runner(&mut self) {
        if self.focus != Focus::Guests {
            self.push_log("select a guest in the Programs pane to run its runner".to_owned());
            return;
        }
        let Some(guest) = self.guests.get(self.selected_guest).cloned() else {
            return;
        };
        let Some(payer) = self.need("PAYER (key P)", self.saved.payer.clone().as_ref()) else {
            return;
        };
        let account = self.saved.account.clone();
        let bin = |name: &str| self.lez.guest_bin(name).display().to_string();
        let args: Vec<String> = match guest.as_str() {
            "hello_world" | "hello_world_with_authorization" => {
                let Some(account) = account else {
                    self.push_log("✗ set ACCOUNT first (key A)".to_owned());
                    return;
                };
                vec![format!("run_{guest}"), bin(&guest), account, payer]
            }
            "hello_world_with_move_function" => {
                let Some(account) = account else {
                    self.push_log("✗ set ACCOUNT first (key A)".to_owned());
                    return;
                };
                vec![
                    "run_hello_world_with_move_function".into(),
                    bin(&guest),
                    payer,
                    "write-public".into(),
                    account,
                    "Hola mundo!".into(),
                ]
            }
            "simple_tail_call" => {
                let Some(account) = account else {
                    self.push_log("✗ set ACCOUNT first (key A)".to_owned());
                    return;
                };
                vec![
                    "run_hello_world_through_tail_call".into(),
                    bin(&guest),
                    bin("hello_world"),
                    account,
                    payer,
                ]
            }
            "tail_call_with_pda" => vec![
                "run_hello_world_with_authorization_through_tail_call_with_pda".into(),
                bin(&guest),
                bin("hello_world_with_authorization"),
                payer,
            ],
            _ => {
                self.push_log(format!("no runner for {guest}"));
                return;
            }
        };
        self.spawn_job(&args[0].clone(), move |mut ctx| async move {
            jobs::runner(&mut ctx, &args).await
        });
    }

    fn open_call(&mut self, alternate: bool) {
        if self.focus != Focus::Deployments {
            self.push_log("select a deployment in the Programs pane to call it".to_owned());
            return;
        }
        let Some(deployment) = self.saved.deployments.get(self.selected_deployment).cloned() else {
            return;
        };
        match call_form(&self.saved, &deployment, alternate) {
            Some(form) => self.modal = Modal::Form(form),
            None => self.push_log(format!(
                "no call for {}{}",
                deployment.name,
                if alternate { " with m" } else { "" }
            )),
        }
    }

    fn on_form_key(&mut self, mut form: Form, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Enter => {
                self.submit(form);
                return;
            }
            KeyCode::Down | KeyCode::Tab => {
                form.selected = (form.selected + 1) % form.fields.len();
            }
            KeyCode::Up | KeyCode::BackTab => {
                form.selected = (form.selected + form.fields.len() - 1) % form.fields.len();
            }
            KeyCode::Backspace => {
                form.fields[form.selected].1.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                form.fields[form.selected].1.clear();
            }
            KeyCode::Char(c) => form.fields[form.selected].1.push(c),
            _ => {}
        }
        self.modal = Modal::Form(form);
    }

    fn submit(&mut self, form: Form) {
        let values: Vec<String> = form
            .fields
            .iter()
            .map(|(_, value)| value.trim().to_owned())
            .collect();
        match form.action {
            FormAction::NewAccount => {
                let label = values[0].clone();
                self.spawn_job("new account", move |mut ctx| async move {
                    let id = jobs::new_account(&mut ctx, &label).await?;
                    ctx.log(format!("new account {id}"));
                    Ok(())
                });
            }
            FormAction::BuildDeploy => {
                let file = userprogs::resolve(&values[0]);
                if let Err(error) = userprogs::guest_name(&file) {
                    self.push_log(format!("✗ {error}"));
                    return;
                }
                if !self.prog_cli_ready() {
                    return;
                }
                let Some(payer) = self.need("PAYER (key P)", self.saved.payer.clone().as_ref()) else {
                    return;
                };
                let label = format!(
                    "build+deploy {}",
                    file.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                );
                self.spawn_job(&label, move |mut ctx| async move {
                    jobs::build_and_deploy(&mut ctx, &file, &payer).await
                });
            }
            FormAction::Transfer => {
                let from = jobs::bare_account_id(&values[0]).to_owned();
                let to = jobs::bare_account_id(&values[1]).to_owned();
                if from.is_empty() || to.is_empty() {
                    self.push_log("✗ from and to are both needed".to_owned());
                    return;
                }
                let Some(amount) = jobs::parse_amount(&values[2]) else {
                    self.push_log(format!("✗ amount must be a whole number above 0, not {:?}", values[2]));
                    return;
                };
                self.spawn_job("transfer", move |mut ctx| async move {
                    jobs::transfer(&mut ctx, &from, &to, amount).await
                });
            }
            FormAction::LezDir => {
                self.saved.lez_dir.clone_from(&values[0]);
                self.lez = Lez::new(state::resolve_lez_dir(&self.crate_dir, &values[0]));
                self.rpc = Rpc::new(self.lez.sequencer_url());
                self.accounts.clear();
                self.views.clear();
                self.guests = self.lez.guests();
                self.save();
                self.refresh_now();
                self.push_log(format!(
                    "LEZ at {}{}",
                    self.lez.dir.display(),
                    if self.lez.present() { "" } else { " (not present)" }
                ));
            }
            FormAction::Call { subcommand, extra } => {
                if let Some((label, _)) = form.fields.iter().zip(&values).find(|(_, v)| v.is_empty()).map(|(f, _)| f) {
                    self.push_log(format!("✗ {label} is empty"));
                    return;
                }
                if !self.prog_cli_ready() {
                    return;
                }
                let Some(payer) = self.need("PAYER (key P)", self.saved.payer.clone().as_ref()) else {
                    return;
                };
                let mut args = vec![subcommand.clone()];
                for ((label, _), value) in form.fields.iter().zip(values) {
                    args.push(format!("--{label}"));
                    args.push(value);
                }
                args.extend(extra);
                self.spawn_job(&format!("call {subcommand}"), move |mut ctx| async move {
                    jobs::call(&mut ctx, &payer, &args).await.map(|_| ())
                });
            }
        }
    }
}

fn stop_note(stop: Option<StopTarget>) -> String {
    match stop {
        Some(StopTarget::Managed(_)) => " It stops the running sequencer.".to_owned(),
        Some(StopTarget::External(pid)) => {
            format!(" It stops the sequencer prog-tui didn't start (pid {pid}).")
        }
        None => String::new(),
    }
}

/// The call form for a deployment, prefilled from state. `alternate` is the move program's
/// `MoveData` (key m).
fn call_form(saved: &Saved, deployment: &Deployment, alternate: bool) -> Option<Form> {
    let id = deployment.program_id.clone();
    let account = saved.account.clone().unwrap_or_default();
    let latest = |name: &str| {
        saved
            .latest(name)
            .map(|d| d.program_id.clone())
            .unwrap_or_default()
    };
    let field = |label: &str, value: String| (label.to_owned(), value);
    // A copy of an example in example_progs (`jz_hello_world`) takes the example's message.
    let name = deployment
        .name
        .strip_prefix(userprogs::PREFIX)
        .unwrap_or(&deployment.name);
    let (subcommand, fields, extra) = match (name, alternate) {
        ("hello_world", false) => (
            "hello",
            vec![
                field("program", id),
                field("account", account),
                field("text", "Hola mundo!".into()),
            ],
            vec![],
        ),
        ("hello_world_with_authorization", false) => (
            "hello",
            vec![
                field("program", id),
                field("account", account),
                field("text", "Hola mundo!".into()),
            ],
            vec!["--sign".to_owned()],
        ),
        ("hello_world_with_move_function", false) => (
            "move-write",
            vec![
                field("program", id),
                field("account", account),
                field("text", "Hola mundo!".into()),
            ],
            vec![],
        ),
        ("hello_world_with_move_function", true) => (
            "move-data",
            vec![
                field("program", id),
                field("from", account),
                field("to", String::new()),
            ],
            vec![],
        ),
        ("simple_tail_call", false) => (
            "tail",
            vec![
                field("caller", id),
                field("callee", latest("hello_world")),
                field("account", account),
            ],
            vec![],
        ),
        ("tail_call_with_pda", false) => (
            "pda",
            vec![
                field("caller", id),
                field("callee", latest("hello_world_with_authorization")),
            ],
            vec![],
        ),
        _ => return None,
    };
    Some(Form::new(
        format!(
            "{} → prog_cli {subcommand}{}",
            deployment.name,
            if extra.is_empty() { "" } else { " --sign" }
        ),
        fields,
        FormAction::Call {
            subcommand: subcommand.to_owned(),
            extra,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subcommand_for(name: &str) -> Option<String> {
        let deployment = Deployment {
            name: name.to_owned(),
            program_id: "PROG".to_owned(),
            stale: false,
        };
        match call_form(&Saved::default(), &deployment, false)?.action {
            FormAction::Call { subcommand, .. } => Some(subcommand),
            _ => None,
        }
    }

    #[test]
    fn an_example_progs_copy_gets_the_examples_call_form() {
        assert_eq!(subcommand_for("hello_world").as_deref(), Some("hello"));
        assert_eq!(subcommand_for("jz_hello_world").as_deref(), Some("hello"));
        assert_eq!(subcommand_for("jz_tail_call_with_pda").as_deref(), Some("pda"));
        assert_eq!(subcommand_for("jz_my_own_program"), None);
    }
}
