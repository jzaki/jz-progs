//! UI state and key handling. Long work goes to [`crate::jobs`]; results come back as [`Msg`]s.

use std::{
    cell::Cell,
    collections::{BTreeMap, VecDeque},
    future::Future,
    path::PathBuf,
    time::{Duration, Instant},
};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::{runtime::Handle, sync::mpsc::UnboundedSender};

use crate::{
    builtin::{self, Builtin},
    jobs::{self, CallRequest, Ctx, Msg, Planned, Status, StepState},
    l1::{self, L1Status},
    lez::Lez,
    lez_call::{self, Method, Seed},
    network::Network,
    programs::{Guest, Programs},
    rpc::{AccountView, Rpc},
    sequencer::{self, StopTarget},
    spel::{self, SpelCall},
    state::{self, Deployment, Origin, Saved, Watched},
    tools::{LEZ_CALL, SPEL},
    userprogs,
    wallet::WalletAccount,
};

const LOG_LINES: usize = 2000;

/// The two screens: LEZ (accounts, programs, calls) and the L1 node run by logosctl.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Lez,
    L1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Accounts,
    Guests,
    Deployments,
    Builtin,
}

pub struct Form {
    pub title: String,
    /// `(label, value)`. A label is a name, then optionally a description after a newline.
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
    /// The checkouts: fields `LEZ`, `lez-programs`.
    Dirs,
    /// The L1 node's config: field `config`.
    L1Config,
    /// Bridge L1 funds into a LEZ account: fields `to`, `amount`, `from (L1)`, `channel`,
    /// `max fee`, `metadata`.
    Bridge,
    /// L1 funds between L1 addresses: fields `from`, `to`, `amount`, `change`.
    L1Transfer,
    /// The L1 config's `pow:` section, and prog-tui's mine-after-start: fields `threads`,
    /// `tickets per block`, `claim every`, `claim targets`, `mine after start`.
    PowSettings,
    /// Watch an account outside the wallet: fields `id`, `note`.
    Watch,
    /// Plan one message: fields `to`, one per param, then `sign`, `actors` and `pda seeds`.
    /// `single`: the program takes one message type, so it's given no name.
    Plan {
        program_id: String,
        name: String,
        method: Method,
        single: bool,
        /// The guest's source, for `lez-call`.
        source: PathBuf,
        seeds: Vec<Seed>,
    },
    /// Plan one message of a lez-programs program with `spel send --dry-run`: fields `to`, one
    /// per IDL field, then `signers` and `actors`.
    SpelPlan {
        program_id: String,
        name: String,
        method: Method,
        idl: PathBuf,
    },
}

/// A list to choose one entry from.
pub struct Pick {
    pub title: String,
    pub options: Vec<(String, PickAction)>,
    pub selected: usize,
}

pub enum PickAction {
    /// Ask for this message's params, then plan it.
    Method {
        deployment: Deployment,
        method: Method,
        single: bool,
        source: PathBuf,
        /// The source's `PdaSeed` constants.
        seeds: Vec<Seed>,
    },
    /// Ask for this message's fields (from the program's spel IDL), then plan it with spel.
    SpelMethod {
        deployment: Deployment,
        method: Method,
        idl: PathBuf,
    },
    /// Bridge L1 funds into this LEZ account (base58): ask the amount next.
    BridgeTo(String),
}

/// A call's plan on screen: Enter sends it (`wallet program call` or `spel send`), when it can be
/// sent.
pub struct PlanView {
    pub title: String,
    pub lines: Vec<String>,
    pub call: Option<Planned>,
    /// The first line shown. The drawing clamps it to what fits, so it's a `Cell`.
    pub scroll: Cell<u16>,
    /// Text to read, not a call: no "nothing to send".
    pub read_only: bool,
}

#[derive(Clone, Copy)]
pub enum Confirmed {
    Reset(Option<StopTarget>),
    Walkthrough(Option<StopTarget>),
    StopExternal(i32),
    /// Stop the L1 node and the logosctl daemon.
    L1Stop,
}

pub enum Modal {
    None,
    Help,
    /// Read-only text, `(title, lines)`; any key closes it.
    Text(String, Vec<String>),
    Pick(Pick),
    Form(Form),
    Plan(PlanView),
    Confirm(String, Confirmed),
}

/// One row of the Accounts pane: a wallet account, or a watched one such as a PDA.
#[derive(Clone, Copy)]
pub enum AccountRow<'a> {
    Wallet(&'a WalletAccount),
    Watched(&'a Watched),
}

pub struct App {
    rt: Handle,
    tx: UnboundedSender<Msg>,
    crate_dir: PathBuf,
    /// The chain prog-tui works against, chosen in the selector at startup.
    pub network: Network,
    /// `.state/`: the local chain's state, and the settings every network shares.
    root_state_dir: PathBuf,
    /// This network's state: `root_state_dir` for local, `.state/networks/<key>/` otherwise.
    pub state_dir: PathBuf,
    pub saved: Saved,
    pub lez: Lez,
    pub programs: Programs,
    pub rpc: Rpc,
    pub status: Status,
    pub accounts: Vec<WalletAccount>,
    pub views: BTreeMap<String, Option<AccountView>>,
    pub guests: Vec<Guest>,
    /// Actor states decoded through their spel IDLs, by `(account, program, bytes)`. Showing these
    /// in the Account pane instead of the raw bytes.
    pub decoded: BTreeMap<(String, String, Vec<u8>), String>,
    decoding: std::collections::BTreeSet<(String, String, Vec<u8>)>,
    pub view: View,
    /// The L1 node, its daemon and wallets, refreshed from logosctl.
    pub l1: L1Status,
    pub selected_l1_wallet: usize,
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
    /// Quit back to the network selector rather than out of prog-tui (key N).
    pub switch_network: bool,
    pub ticks: usize,
    refreshing: Refreshing,
    last_status: Option<Instant>,
    last_accounts: Option<Instant>,
    last_views: Option<Instant>,
    last_l1: Option<Instant>,
    /// `s` started the L1 node with mine-after-start on: start mining once it's Online.
    l1_mine_pending: bool,
}

#[derive(Default)]
struct Refreshing {
    status: bool,
    accounts: bool,
    views: bool,
    l1: bool,
}

impl App {
    pub fn new(rt: Handle, tx: UnboundedSender<Msg>, network: Network) -> Self {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root_state_dir = crate_dir.join(".state");
        let state_dir = state::network_state_dir(&root_state_dir, &network.key());
        let mut root = Saved::load(&root_state_dir);
        root.last_network = Some(network.key());
        let saved = if network.is_local() {
            root.clone()
        } else {
            let mut saved = Saved::load(&state_dir);
            saved.share_settings_from(&root);
            saved
        };
        let programs = Programs::new(state::resolve_dir(&crate_dir, &saved.programs_dir));
        if !network.is_local() {
            // Remember the choice in the root state; the local chain's own state is untouched.
            let local = Lez::new(state::resolve_dir(&crate_dir, &root.lez_dir));
            let _ = root.save(&root_state_dir, &local, &programs);
        }
        let (lez, rpc, wallet_error) = connect(&network, &crate_dir, &saved.lez_dir, &state_dir);
        let mut app = Self {
            rt,
            tx,
            crate_dir,
            network,
            root_state_dir,
            state_dir,
            saved,
            lez,
            programs,
            rpc,
            status: Status::default(),
            accounts: Vec::new(),
            views: BTreeMap::new(),
            guests: Vec::new(),
            decoded: BTreeMap::new(),
            decoding: std::collections::BTreeSet::new(),
            view: View::Lez,
            l1: L1Status::default(),
            selected_l1_wallet: 0,
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
            switch_network: false,
            ticks: 0,
            refreshing: Refreshing::default(),
            last_status: None,
            last_accounts: None,
            last_views: None,
            last_l1: None,
            l1_mine_pending: false,
        };
        app.guests = app.programs.guests();
        app.save();
        app.push_log(format!(
            "prog-tui · {} · LEZ at {} · lez-programs at {} · press ? for keys",
            app.network.label(),
            app.lez.dir.display(),
            app.programs.dir.display()
        ));
        if !app.network.is_local() {
            app.push_log(format!(
                "remote network: its own wallet at {}; prog-tui doesn't start, stop or reset its sequencer",
                app.lez.wallet_home().display()
            ));
        }
        if let Some(error) = wallet_error {
            app.push_log(format!("✗ couldn't set up the network's wallet: {error:#}"));
        }
        app.push_log(format!(
            "env vars for your shell: source {}",
            state::env_sh_path(&app.state_dir).display()
        ));
        app
    }

    /// Writes this network's state. On a remote network, the shared settings (the checkouts, the
    /// wallet password) also go to the root state, where every network reads them.
    fn save(&mut self) {
        let mut result = self.saved.save(&self.state_dir, &self.lez, &self.programs);
        if result.is_ok() && !self.network.is_local() {
            let mut root = Saved::load(&self.root_state_dir);
            root.share_settings_from(&self.saved);
            let local = Lez::new(self.lez.dir.clone());
            result = root.save(&self.root_state_dir, &local, &self.programs);
        }
        if let Err(error) = result {
            self.push_log(format!("✗ couldn't save state: {error:#}"));
        }
    }

    /// Refuses `what` on a remote network, whose sequencer isn't prog-tui's to run.
    fn local_only(&mut self, what: &str) -> bool {
        if self.network.is_local() {
            return true;
        }
        self.push_log(format!(
            "✗ {what}: only on the local chain. {} is someone else's sequencer (N switches network)",
            self.network.label()
        ));
        false
    }

    /// The L1 node's config: the one set with `C`, else Basecamp's.
    pub fn l1_config(&self) -> Option<PathBuf> {
        let entered = Some(self.saved.l1_config.trim())
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .or_else(l1::basecamp_config)?;
        Some(state::resolve_dir(&self.crate_dir, &entered))
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

    /// The Accounts pane's rows: the wallet's accounts, then the watched ones it doesn't hold.
    pub fn account_rows(&self) -> Vec<AccountRow<'_>> {
        let mut rows: Vec<AccountRow> = self
            .visible_accounts()
            .into_iter()
            .map(AccountRow::Wallet)
            .collect();
        rows.extend(
            self.saved
                .watched
                .iter()
                .filter(|watched| !self.accounts.iter().any(|account| account.id == watched.id))
                .map(AccountRow::Watched),
        );
        rows
    }

    pub fn selected_row(&self) -> Option<AccountRow<'_>> {
        self.account_rows().get(self.selected_account).copied()
    }

    pub fn selected_account(&self) -> Option<&WalletAccount> {
        match self.selected_row()? {
            AccountRow::Wallet(account) => Some(account),
            AccountRow::Watched(_) => None,
        }
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
            self.guests = self.programs.guests();
            let (network, lez, programs, state_dir, rpc, tx) = (
                self.network.clone(),
                self.lez.clone(),
                self.programs.clone(),
                self.state_dir.clone(),
                self.rpc.clone(),
                self.tx.clone(),
            );
            self.rt.spawn(async move {
                let status = jobs::gather_status(network, lez, programs, state_dir, rpc).await;
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
            let password = self.saved.wallet_password.clone();
            self.rt.spawn(async move {
                let accounts = jobs::list_accounts(&lez, &password).await.unwrap_or_default();
                let _ = tx.send(Msg::Accounts(accounts));
            });
        }

        self.decode_selected();

        // The L1 view refreshes often, and asks the node itself; otherwise only the daemon, for
        // the status bar's dot. A node that didn't answer (still syncing) is asked again after a
        // minute, not every 5s.
        let full = self.view == View::L1;
        let unanswered = self.l1.node.as_ref().is_some_and(|node| node.chain.is_none());
        let every = match (full, unanswered) {
            (true, false) => 5,
            (true, true) => 60,
            (false, _) => 10,
        };
        if !self.refreshing.l1 && due(self.last_l1, every) {
            self.refreshing.l1 = true;
            self.last_l1 = Some(Instant::now());
            let (config, tx) = (self.l1_config(), self.tx.clone());
            self.rt.spawn(async move {
                let _ = tx.send(Msg::L1(Box::new(l1::gather(config, full).await)));
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
            ids.extend(self.saved.watched.iter().map(|watched| watched.id.clone()));
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

    /// Decodes the selected account's actor states that have a spel IDL, one at a time and only
    /// between jobs (spel opens the wallet, which a running job may be writing). Each result is
    /// cached by the state's bytes, so a changed state is decoded again and an unchanged one never.
    fn decode_selected(&mut self) {
        if self.busy.is_some() || !self.status.seq.healthy || !self.decoding.is_empty() {
            return;
        }
        let id = match self.selected_row() {
            Some(AccountRow::Wallet(account)) if account.public => account.id.clone(),
            Some(AccountRow::Watched(watched)) => watched.id.clone(),
            _ => return,
        };
        let Some(Some(view)) = self.views.get(&id) else {
            return;
        };
        for (program, bytes) in &view.actor_states {
            let key = (id.clone(), program.clone(), bytes.clone());
            if bytes.is_empty() || self.decoded.contains_key(&key) {
                continue;
            }
            let Some(deployment) = self
                .saved
                .deployments
                .iter()
                .find(|d| d.program_id == *program && !d.stale)
            else {
                continue;
            };
            let Some(idl) = self.spel_idl(&deployment.name, &deployment.origin).filter(|idl| idl.is_file()) else {
                continue;
            };
            self.decoding.insert(key.clone());
            let (lez, tx, password, program) = (
                self.lez.clone(),
                self.tx.clone(),
                self.saved.wallet_password.clone(),
                program.clone(),
            );
            self.rt.spawn(async move {
                let text = jobs::spel_decode(&lez, &password, &idl, &program, &key.0).await;
                // A state spel can't decode is remembered as such, so it isn't tried again.
                let _ = tx.send(Msg::Decoded(key, text.unwrap_or_default()));
            });
            return;
        }
    }

    fn refresh_now(&mut self) {
        self.last_status = None;
        self.last_accounts = None;
        self.last_views = None;
        self.last_l1 = None;
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
            Msg::Decoded(key, text) => {
                self.decoding.remove(&key);
                self.decoded.insert(key, text);
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
            Msg::Plan { title, lines, call } => {
                self.modal = Modal::Plan(PlanView {
                    title,
                    lines,
                    call,
                    scroll: Cell::new(0),
                    read_only: false,
                });
            }
            Msg::L1(status) => {
                self.refreshing.l1 = false;
                self.l1 = *status;
                self.selected_l1_wallet = self
                    .selected_l1_wallet
                    .min(self.l1.wallets.len().saturating_sub(1));
                self.mine_if_pending();
            }
            Msg::L1Config(path, text) => {
                let lines = match text {
                    Ok(text) => text.lines().map(str::to_owned).collect(),
                    Err(error) => vec![format!("✗ {error}")],
                };
                self.modal = Modal::Plan(PlanView {
                    title: format!("L1 config {}", path.display()),
                    lines,
                    call: None,
                    scroll: Cell::new(0),
                    read_only: true,
                });
            }
            Msg::Watch(id, note, pda) => {
                if self.saved.watch(id.clone(), note, pda) {
                    self.push_log(format!("watching {id} in the Accounts pane"));
                    self.last_views = None;
                }
                self.save();
            }
            Msg::ChainReset => {
                self.saved.clear_chain();
                self.decoded.clear();
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
        let accounts = self.account_rows().len();
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
            self.network.clone(),
            self.lez.clone(),
            self.programs.clone(),
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
        if !self.network.is_local() {
            return None;
        }
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

    fn lez_call_ready(&mut self) -> bool {
        if LEZ_CALL.path().is_some() {
            return true;
        }
        if LEZ_CALL.has_checkout() {
            self.push_log("✗ lez-call isn't built yet (key b)".to_owned());
        } else {
            self.push_log(format!(
                "✗ no lez-call: check it out at {} or put it on PATH",
                LEZ_CALL.checkout().display()
            ));
        }
        false
    }

    fn spel_ready(&mut self) -> bool {
        if SPEL.path().is_some() {
            return true;
        }
        if SPEL.has_checkout() {
            self.push_log("✗ spel isn't built yet (key b)".to_owned());
        } else {
            self.push_log(format!(
                "✗ no spel: check out its refactor/lez-v0.4.x branch at {} or put it on PATH",
                SPEL.checkout().display()
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
            Modal::Help | Modal::Text(..) => {}
            Modal::Confirm(text, action) => {
                if matches!(key.code, KeyCode::Char('y' | 'Y')) {
                    self.confirmed(action);
                } else if key.code != KeyCode::Esc && !matches!(key.code, KeyCode::Char(_)) {
                    self.modal = Modal::Confirm(text, action);
                }
            }
            Modal::Pick(pick) => self.on_pick_key(pick, key),
            Modal::Form(form) => self.on_form_key(form, key),
            Modal::Plan(view) => self.on_plan_key(view, key),
        }
    }

    fn on_plan_key(&mut self, view: PlanView, key: KeyEvent) {
        let scroll = view.scroll.get();
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => view.scroll.set(scroll.saturating_add(1)),
            KeyCode::Up | KeyCode::Char('k') => view.scroll.set(scroll.saturating_sub(1)),
            KeyCode::PageDown | KeyCode::Char(' ') => view.scroll.set(scroll.saturating_add(8)),
            KeyCode::PageUp => view.scroll.set(scroll.saturating_sub(8)),
            KeyCode::Home | KeyCode::Char('g') => view.scroll.set(0),
            KeyCode::End | KeyCode::Char('G') => view.scroll.set(u16::MAX),
            KeyCode::Enter | KeyCode::Char('y') => {
                if let Some(call) = view.call {
                    self.spawn_job(&format!("call {}", call.name()), move |mut ctx| async move {
                        match call {
                            Planned::Wallet(call) => jobs::call(&mut ctx, &call).await,
                            Planned::Spel(call) => jobs::spel_call(&mut ctx, &call).await,
                            Planned::Bridge(call) => l1::bridge(&mut ctx, &call).await,
                            Planned::L1Transfer(call) => l1::transfer(&mut ctx, &call).await,
                            Planned::PowConfig(write) => l1::write_pow(&mut ctx, &write).await,
                        }
                    });
                }
                return;
            }
            _ => return,
        }
        self.modal = Modal::Plan(view);
    }

    fn on_pick_key(&mut self, mut pick: Pick, key: KeyEvent) {
        let last = pick.options.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                pick.selected = (pick.selected + 1).min(last);
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                pick.selected = pick.selected.saturating_sub(1);
            }
            KeyCode::Enter => {
                if pick.selected < pick.options.len() {
                    let (_, action) = pick.options.swap_remove(pick.selected);
                    self.modal = Modal::Form(match action {
                        PickAction::Method {
                            deployment,
                            method,
                            single,
                            source,
                            seeds,
                        } => send_form(&self.saved, &deployment, method, single, source, seeds),
                        PickAction::SpelMethod {
                            deployment,
                            method,
                            idl,
                        } => spel_form(&self.saved, &deployment, method, idl),
                        PickAction::BridgeTo(to) => self.bridge_form(to),
                    });
                }
                return;
            }
            _ => {}
        }
        self.modal = Modal::Pick(pick);
    }

    fn on_main_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('1') => {
                self.view = View::Lez;
                return;
            }
            KeyCode::Char('2') => {
                if self.view != View::L1 {
                    self.view = View::L1;
                    self.last_l1 = None;
                }
                return;
            }
            _ => {}
        }
        if self.view == View::L1 {
            self.on_l1_key(key);
            return;
        }
        if self.focus == Focus::Builtin
            && matches!(key.code, KeyCode::Char('d' | 'c' | 'x'))
        {
            self.push_log("built-in programs are read-only: you can't deploy, call or forget them".to_owned());
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('N') => {
                if let Some((busy, _)) = &self.busy {
                    let busy = busy.clone();
                    self.push_log(format!("busy: wait for `{busy}` to finish before switching network"));
                } else {
                    self.switch_network = true;
                    self.quit = true;
                }
            }
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
                if self.network.is_local() {
                    for line in sequencer::sequencer_help(&self.state_dir, &self.rpc.url, &self.status.seq) {
                        self.push_log(line);
                    }
                } else {
                    self.push_log(format!(
                        "{}: someone else's sequencer, at {}; its log is theirs",
                        self.network.label(),
                        self.rpc.url
                    ));
                    self.push_log(format!(
                        "  use its wallet: source {}",
                        state::env_sh_path(&self.state_dir).display()
                    ));
                }
            }
            KeyCode::Char('L') => {
                self.modal = Modal::Form(Form::new(
                    "Checkouts (absolute, ~/…, or relative to prog-tui)".to_owned(),
                    vec![
                        (
                            "LEZ\nthe sequencer and the wallet".to_owned(),
                            self.saved.lez_dir.clone(),
                        ),
                        (
                            "lez-programs\nthe programs, and the jz package for example_progs".to_owned(),
                            self.saved.programs_dir.clone(),
                        ),
                    ],
                    FormAction::Dirs,
                ));
            }
            KeyCode::Char('s') => {
                if self.local_only("starting or stopping the sequencer") {
                    self.toggle_sequencer();
                }
            }
            KeyCode::Char('R') if !self.local_only("resetting the chain") => {}
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
                let text = if self.network.is_local() {
                    format!(
                        "Run the walkthrough? It builds, then resets the chain and wallet.{} y/n",
                        stop_note(stop)
                    )
                } else {
                    format!(
                        "Run the walkthrough on {}? It builds, checks the sequencer, then deploys token and creates a token there, paid by a test account. Nothing is reset. y/n",
                        self.network.label()
                    )
                };
                self.modal = Modal::Confirm(text, Confirmed::Walkthrough(stop));
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
            KeyCode::Char('v') => self.show_messages(),
            KeyCode::Char('w') => {
                self.modal = Modal::Form(Form::new(
                    "Watch an account outside the wallet (shown in the Accounts pane)".to_owned(),
                    vec![
                        ("id".to_owned(), String::new()),
                        ("note".to_owned(), String::new()),
                    ],
                    FormAction::Watch,
                ));
            }
            KeyCode::Char('x') if self.focus == Focus::Accounts => {
                match self.selected_row() {
                    Some(AccountRow::Watched(watched)) => {
                        let id = watched.id.clone();
                        self.saved.watched.retain(|watched| watched.id != id);
                        self.push_log(format!("stopped watching {id}"));
                        self.save();
                        self.clamp_selection();
                    }
                    _ => self.push_log("x forgets watched accounts only, not wallet accounts".to_owned()),
                }
            }
            KeyCode::Char('c') => self.open_pick(),
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

    /// The channel a deposit goes through: the network's, from `networks.json`, else the one its
    /// sequencer reports.
    fn bridge_channel(&self) -> Option<String> {
        match &self.network {
            Network::Remote { channel_id: Some(id), .. } => Some(id.clone()),
            _ => self.status.seq.channel_id.clone(),
        }
    }

    /// `b` on the L1 view: pick the LEZ account to credit, `ACCOUNT` first selected: the wallet's
    /// public accounts, then the watched ones, then any other account, typed in the form.
    fn open_bridge(&mut self) {
        if !self.l1.running() || self.l1.node.as_ref().and_then(|node| node.chain.as_ref()).is_none() {
            self.push_log("✗ bridging needs the L1 node running (s starts it)".to_owned());
            return;
        }
        let accounts: Vec<WalletAccount> =
            self.accounts.iter().filter(|account| account.public).cloned().collect();
        let mut options: Vec<(String, PickAction)> = accounts
            .iter()
            .map(|account| {
                let mut label = account.id.clone();
                if let Some(name) = &account.label {
                    label.push_str(&format!(" [{name}]"));
                }
                if self.saved.account.as_deref() == Some(&account.id) {
                    label.push_str(" ▶ ACCOUNT");
                }
                if self.saved.payer.as_deref() == Some(&account.id) {
                    label.push_str(" ★ PAYER");
                }
                if let Some(Some(view)) = self.views.get(&account.id) {
                    label.push_str(&format!("  balance {}", view.balance()));
                }
                (label, PickAction::BridgeTo(account.id.clone()))
            })
            .collect();
        // A watched PDA a call made is named by its evidence; any other watched account by id,
        // which the deposit can name only with the `account` metadata form.
        for watched in &self.saved.watched {
            if accounts.iter().any(|account| account.id == watched.id) {
                continue;
            }
            let mut label = format!("{} [{}] (watched)", watched.id, watched.note);
            if let Some(Some(view)) = self.views.get(&watched.id) {
                label.push_str(&format!("  balance {}", view.balance()));
            }
            let to = watched
                .pda
                .as_ref()
                .map_or_else(|| watched.id.clone(), |pda| format!("pda:{pda}"));
            options.push((label, PickAction::BridgeTo(to)));
        }
        options.push((
            "another account: its public key (64 hex), pda:<program>/<seed>, or id".to_owned(),
            PickAction::BridgeTo(String::new()),
        ));
        let selected = accounts
            .iter()
            .position(|account| self.saved.account.as_deref() == Some(&account.id))
            .unwrap_or(0);
        self.modal = Modal::Pick(Pick {
            title: format!("Bridge L1 funds to which LEZ account? ({})", self.network.label()),
            options,
            selected,
        });
    }

    /// `t` on the L1 view: a transfer between L1 addresses, from the wallet holding the most.
    fn open_l1_transfer(&mut self) {
        let online = self.l1.node.as_ref().is_some_and(|node| node.chain.is_some());
        if !online {
            self.push_log("✗ L1 transfers need the node Online (it answers no calls before)".to_owned());
            return;
        }
        let from = self.funded_l1_wallet().unwrap_or_default();
        self.modal = Modal::Form(Form::new(
            "L1 transfer (L1 module wallet_transfer_funds)".to_owned(),
            vec![
                ("from\nthe address that pays: hex, or a key name such as PoWClaim".to_owned(), from.clone()),
                ("to\nhex, or a key name from the keystore".to_owned(), String::new()),
                ("amount".to_owned(), String::new()),
                ("change\nwhere the rest goes back to".to_owned(), from),
            ],
            FormAction::L1Transfer,
        ));
    }

    /// The L1 wallet holding the most: the natural payer, since a deposit's or transfer's fee
    /// comes from notes beside the ones it spends (`PoWClaim`, where mining rewards land, as a rule).
    fn funded_l1_wallet(&self) -> Option<String> {
        self.l1
            .wallets
            .iter()
            .filter_map(|wallet| {
                let balance = wallet.balance.as_ref().ok()?.parse::<u128>().ok()?;
                (balance > 0).then_some((balance, &wallet.address))
            })
            .max_by_key(|(balance, _)| *balance)
            .map(|(_, address)| address.clone())
    }

    /// A watched PDA typed by id, named by the evidence its call recorded.
    fn with_watched_evidence(&self, to: l1::LezRecipient) -> l1::LezRecipient {
        let l1::LezRecipient::Id(id) = &to else { return to };
        self.saved
            .watched
            .iter()
            .find(|watched| &watched.id == id)
            .and_then(|watched| watched.pda.as_ref())
            .and_then(|pda| l1::LezRecipient::parse(&format!("pda:{pda}")).ok())
            .unwrap_or(to)
    }

    /// The deposit's details, after the LEZ account is picked. The L1 address defaults to the
    /// wallet holding the most.
    fn bridge_form(&self, to: String) -> Form {
        let from = self.funded_l1_wallet().unwrap_or_default();
        Form::new(
            "Bridge L1 → LEZ (L1 module channel_deposit_with_notes)".to_owned(),
            vec![
                (
                    "to\nthe LEZ account credited: a wallet account's id, a public key (64 hex), or pda:<program>/<seed>".to_owned(),
                    to,
                ),
                ("amount".to_owned(), String::new()),
                ("from (L1)\nthe L1 address that pays the note, the fees and takes the change, hex".to_owned(), from),
                (
                    "channel\nthe LEZ network's channel on L1".to_owned(),
                    self.bridge_channel().unwrap_or_default(),
                ),
                ("max fee\nper transaction; a transfer paid about 10,000 on the devnet".to_owned(), "50000".to_owned()),
                ("metadata\nkey: 01 00 ‖ pk (LEZ v0.4) · account: the 32-byte account id".to_owned(), "key".to_owned()),
            ],
            FormAction::Bridge,
        )
    }

    /// The L1 view's keys. The LEZ keys don't apply here.
    fn on_l1_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('N') | KeyCode::Char('?') => {
                // The same as on the LEZ view.
                self.view = View::Lez;
                self.on_main_key(key);
                self.view = View::L1;
            }
            KeyCode::Char('g') => {
                self.last_l1 = None;
                self.push_log("refreshing the L1".to_owned());
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected_l1_wallet = (self.selected_l1_wallet + 1)
                    .min(self.l1.wallets.len().saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected_l1_wallet = self.selected_l1_wallet.saturating_sub(1);
            }
            KeyCode::Char('s') => {
                if self.l1.running() {
                    self.modal = Modal::Confirm(
                        "Stop the L1 node and the logosctl daemon? (`blockchain_module stop`, then `logosctl daemon stop`) y/n".to_owned(),
                        Confirmed::L1Stop,
                    );
                } else {
                    let config = self.l1_config();
                    let idle = self.busy.is_none();
                    self.spawn_job("start L1 node", move |mut ctx| async move {
                        l1::start(&mut ctx, config).await
                    });
                    if idle && self.saved.l1_mine_after_start {
                        self.l1_mine_pending = true;
                        self.push_log("mining starts once the node is Online (p turns that off)".to_owned());
                    }
                }
            }
            KeyCode::Char('b') => self.open_bridge(),
            KeyCode::Char('t') => self.open_l1_transfer(),
            KeyCode::Char('m') => self.toggle_pow(true),
            KeyCode::Char('a') => self.toggle_pow(false),
            KeyCode::Char('p') => self.open_pow_settings(),
            KeyCode::Char('C') => {
                self.modal = Modal::Form(Form::new(
                    "The L1 node's config (absolute, ~/…, or relative to prog-tui)".to_owned(),
                    vec![(
                        "config\nthe user_config.yaml the node was started with; empty: Basecamp's".to_owned(),
                        self.saved.l1_config.clone(),
                    )],
                    FormAction::L1Config,
                ));
            }
            KeyCode::Char('c') => {
                let Some(config) = self.l1_config() else {
                    self.push_log("✗ no L1 config set: C sets it".to_owned());
                    return;
                };
                if !config.is_file() {
                    self.push_log(format!("✗ no L1 config at {} (C changes it)", config.display()));
                    return;
                }
                let tx = self.tx.clone();
                self.rt.spawn(async move {
                    let text = l1::show_config(&config).await;
                    let _ = tx.send(Msg::L1Config(config, text));
                });
            }
            KeyCode::Char(_) | KeyCode::Tab | KeyCode::BackTab => {
                self.push_log("L1 view keys: s start/stop · m mining · a auto-claim · p PoW settings · b bridge · t transfer · c/C config · j/k · g; 1 switches back to LEZ".to_owned());
            }
            _ => {}
        }
    }

    /// `m` (mining) or `a` (auto-claim) on the L1 view: flips it on the running node.
    fn toggle_pow(&mut self, mining: bool) {
        let Some(pow) = self.l1.node.as_ref().filter(|node| node.chain.is_some()).and_then(|node| node.pow.as_ref()) else {
            self.push_log("✗ mining and auto-claim switch on an Online node (it answers no calls before)".to_owned());
            return;
        };
        let (is_mining, is_armed) = l1::pow_flags(pow);
        let switch = match (mining, is_mining, is_armed) {
            (true, Some(true), _) => l1::PowSwitch::StopMining,
            (true, _, _) => l1::PowSwitch::StartMining,
            (false, _, Some(true)) => l1::PowSwitch::StopAutoClaim,
            (false, _, _) => l1::PowSwitch::StartAutoClaim,
        };
        if switch == l1::PowSwitch::StopMining {
            // Stopped by hand: don't start it again behind the user's back.
            self.l1_mine_pending = false;
        }
        self.spawn_job(switch.label(), move |mut ctx| async move { l1::pow_switch(&mut ctx, switch).await });
    }

    /// After `s` started the node with mine-after-start on: once the node is Online and answers
    /// `pow_status`, start mining (once).
    fn mine_if_pending(&mut self) {
        if !self.l1_mine_pending || self.busy.is_some() {
            return;
        }
        let Some(pow) = self.l1.node.as_ref().filter(|node| node.chain.is_some()).and_then(|node| node.pow.as_ref()) else {
            return;
        };
        self.l1_mine_pending = false;
        if l1::pow_flags(pow).0 == Some(false) {
            self.push_log("L1 node Online: starting mining (mine after start, p)".to_owned());
            let switch = l1::PowSwitch::StartMining;
            self.spawn_job(switch.label(), move |mut ctx| async move { l1::pow_switch(&mut ctx, switch).await });
        }
    }

    /// `p` on the L1 view: the config's `pow:` section, and mine-after-start.
    fn open_pow_settings(&mut self) {
        let Some(config) = self.l1_config().filter(|path| path.is_file()) else {
            self.push_log("✗ no L1 config file: C sets it".to_owned());
            return;
        };
        let current = match &self.l1.pow_config {
            Some(Ok(settings)) => settings.clone(),
            Some(Err(error)) => {
                self.push_log(format!("✗ the config's pow section: {error}"));
                return;
            }
            None => l1::PowSettings::default(),
        };
        let names = l1::names_beside(Some(&config));
        let suggestion = self.funded_l1_wallet().map(|hex| {
            format!(" (e.g. {}=1000000)", names.get(&hex).cloned().unwrap_or(hex))
        });
        self.modal = Modal::Form(Form::new(
            "L1 PoW settings (the config's pow section, read when the node starts)".to_owned(),
            vec![
                (
                    "threads\nticket-search threads; auto: one per CPU".to_owned(),
                    current.max_threads.map_or("auto".to_owned(), |n| n.to_string()),
                ),
                (
                    "tickets per block\nsearch attempts in flight per block".to_owned(),
                    current.max_tickets_per_block.to_string(),
                ),
                ("claim every\nauto-claim's tick: 10s, or 5 slots".to_owned(), current.tick.label()),
                (
                    format!(
                        "claim targets\nkey=balance to reach, comma-separated; a keystore name or hex{}; empty: auto-claim off",
                        suggestion.unwrap_or_default()
                    ),
                    current.targets_text(&names),
                ),
                (
                    "mine after start\nyes: once a node s started is Online, prog-tui starts mining (no node resumes mining by itself)".to_owned(),
                    if self.saved.l1_mine_after_start { "yes" } else { "no" }.to_owned(),
                ),
            ],
            FormAction::PowSettings,
        ));
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
        let visible = self.account_rows().len();
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
            Confirmed::L1Stop => {
                self.spawn_job("stop L1 node", |mut ctx| async move { l1::stop(&mut ctx).await });
                self.last_l1 = None;
            }
        }
    }

    fn set_role(&mut self, payer: bool) {
        if self.focus != Focus::Accounts {
            self.push_log("select an account in the Accounts pane first".to_owned());
            return;
        }
        if matches!(self.selected_row(), Some(AccountRow::Watched(_))) {
            self.push_log("✗ a watched account isn't in the wallet: it can't be PAYER or ACCOUNT".to_owned());
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
                ("to\nan account id, or a new account's public key (64 hex)".to_owned(), to),
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
        let Some(payer) = self.need("PAYER (key P)", self.saved.payer.clone().as_ref()) else {
            return;
        };
        self.spawn_job(&format!("deploy {}", guest.name), move |mut ctx| async move {
            jobs::deploy(&mut ctx, &guest, &payer).await.map(|_| ())
        });
    }

    /// The messages the selected guest (or deployment) accepts: from its spel IDL for one of
    /// lez-programs' programs, else `lez-call iface`, which reads a jz guest's source.
    fn show_messages(&mut self) {
        let target = match self.focus {
            Focus::Guests => self
                .guests
                .get(self.selected_guest)
                .map(|guest| (guest.name.clone(), guest.origin.clone(), None)),
            Focus::Deployments => self
                .saved
                .deployments
                .get(self.selected_deployment)
                .map(|d| (d.name.clone(), d.origin.clone(), Some(d.program_id.clone()))),
            _ => None,
        };
        let Some((name, origin, program_id)) = target else {
            self.push_log("select a guest or deployment in the Programs pane to see its messages".to_owned());
            return;
        };
        let result = if let Some(idl) = self.spel_idl(&name, &origin) {
            spel::methods(&idl).map(|methods| {
                let from = match &origin {
                    Origin::Program(_) => "lez-programs",
                    _ => "example_progs",
                };
                let mut lines = vec![
                    format!("program: {name} ({from}; called with spel)"),
                    format!("idl: {}", idl.display()),
                    String::new(),
                ];
                for method in methods {
                    lines.push(format!("  {}", method.label()));
                    if let Some(doc) = &method.doc {
                        lines.push(format!("      {doc}"));
                    }
                }
                lines
            })
        } else {
            if !self.lez_call_ready() {
                return;
            }
            self.jz_source(&name, &origin, program_id.as_deref())
                .and_then(|source| lez_call::text(&self.lez, &self.saved.wallet_password, &source))
        };
        match result {
            Ok(lines) => self.modal = Modal::Text(format!("{name}: messages"), lines),
            Err(error) => self.push_log(format!("✗ {error:#}")),
        }
    }

    /// The spel IDL a guest is called with: a lez-programs program's, from `make idl`, or a jz
    /// guest's, from `spel generate-idl` when its message is an `enum Message`. `None` sends the
    /// call through `lez-call` instead.
    fn spel_idl(&self, name: &str, origin: &Origin) -> Option<PathBuf> {
        match origin {
            Origin::Program(program) => Some(self.programs.idl(program)),
            Origin::Jz => Some(self.programs.jz_idl(name)).filter(|idl| idl.is_file()),
            Origin::Unknown => None,
        }
    }

    /// The source `lez-call` reads for a jz guest: its own, by name, when there is one; else the
    /// jz guest whose build has the image id in the program's header (`lez-dryrun source-of`).
    fn jz_source(&self, name: &str, origin: &Origin, program_id: Option<&str>) -> anyhow::Result<PathBuf> {
        let source = lez_call::source_of(&self.programs, name);
        if source.is_file() {
            return Ok(source);
        }
        let Some(program_id) = program_id.filter(|_| *origin == Origin::Unknown) else {
            anyhow::bail!("no source for {name} at {}: build first (key b)", source.display());
        };
        let guest = lez_call::guest_of(&self.lez, &self.programs, program_id)?;
        Ok(lez_call::source_of(&self.programs, &guest))
    }

    /// `c`: pick one of the deployment's messages, from its spel IDL or `lez-call iface`.
    fn open_pick(&mut self) {
        if self.focus != Focus::Deployments {
            self.push_log("select a deployment in the Programs pane to call it".to_owned());
            return;
        }
        let Some(deployment) = self.saved.deployments.get(self.selected_deployment).cloned() else {
            return;
        };
        if let Some(idl) = self.spel_idl(&deployment.name, &deployment.origin) {
            if !self.spel_ready() {
                return;
            }
            let methods = match spel::methods(&idl) {
                Ok(methods) => methods,
                Err(error) => {
                    self.push_log(format!("✗ no messages for {}: {error:#}", deployment.name));
                    return;
                }
            };
            let options = methods
                .into_iter()
                .map(|method| {
                    let mut label = method.label();
                    if let Some(doc) = &method.doc {
                        label.push_str(&format!(" — {doc}"));
                    }
                    let action = PickAction::SpelMethod {
                        deployment: deployment.clone(),
                        method,
                        idl: idl.clone(),
                    };
                    (label, action)
                })
                .collect();
            self.modal = Modal::Pick(Pick {
                title: format!("{}: pick a message (spel)", deployment.name),
                options,
                selected: 0,
            });
            return;
        }
        if !self.lez_call_ready() {
            return;
        }
        let found = self
            .jz_source(&deployment.name, &deployment.origin, Some(&deployment.program_id))
            .and_then(|source| {
                let interface = lez_call::interface(&self.lez, &self.saved.wallet_password, &source)?;
                Ok((source, interface))
            });
        let (source, interface) = match found {
            Ok(found) => found,
            Err(error) => {
                self.push_log(format!("✗ no messages for {}: {error:#}", deployment.name));
                return;
            }
        };
        let single = interface.single();
        let seeds = interface.seeds.clone();
        let options = interface
            .methods
            .into_iter()
            .map(|method| {
                let mut label = method.label();
                if method.internal() {
                    label.push_str(" (another program sends it)");
                }
                if let Some(doc) = &method.doc {
                    label.push_str(&format!(" — {doc}"));
                }
                let action = PickAction::Method {
                    deployment: deployment.clone(),
                    method,
                    single,
                    source: source.clone(),
                    seeds: seeds.clone(),
                };
                (label, action)
            })
            .collect();
        self.modal = Modal::Pick(Pick {
            title: format!("{}: pick a message", deployment.name),
            options,
            selected: 0,
        });
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
                // The wallet refuses these too: they separate the parts of a call's actor.
                if let Some(reserved) = label.chars().find(|c| ['@', ':', '/'].contains(c)) {
                    self.push_log(format!("✗ a label can't contain {reserved:?}: @ : / are reserved"));
                    self.modal = Modal::Form(form);
                    return;
                }
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
                let to = jobs::Recipient::parse(&values[1]);
                if from.is_empty() || to.id().is_empty() {
                    self.push_log("✗ from and to are both needed".to_owned());
                    return;
                }
                let Some(amount) = jobs::parse_amount(&values[2]) else {
                    self.push_log(format!("✗ amount must be a whole number above 0, not {:?}", values[2]));
                    return;
                };
                if let jobs::Recipient::Key { id, .. } = &to {
                    self.push_log(format!("to: that pk's account is Public/{id}"));
                }
                self.spawn_job("transfer", move |mut ctx| async move {
                    jobs::transfer(&mut ctx, &from, &to, amount).await
                });
            }
            FormAction::Dirs => {
                self.saved.lez_dir.clone_from(&values[0]);
                self.saved.programs_dir.clone_from(&values[1]);
                let (lez, rpc, wallet_error) =
                    connect(&self.network, &self.crate_dir, &values[0], &self.state_dir);
                self.lez = lez;
                self.rpc = rpc;
                if let Some(error) = wallet_error {
                    self.push_log(format!("✗ couldn't set up the network's wallet: {error:#}"));
                }
                self.programs = Programs::new(state::resolve_dir(&self.crate_dir, &values[1]));
                self.accounts.clear();
                self.views.clear();
                self.guests = self.programs.guests();
                self.save();
                self.refresh_now();
                let missing = |present: bool| if present { "" } else { " (not present)" };
                self.push_log(format!(
                    "LEZ at {}{} · lez-programs at {}{}",
                    self.lez.dir.display(),
                    missing(self.lez.present()),
                    self.programs.dir.display(),
                    missing(self.programs.present()),
                ));
            }
            FormAction::L1Transfer => {
                let Some(amount) = values[2].parse::<u64>().ok().filter(|amount| *amount > 0) else {
                    self.push_log(format!("✗ amount must be a whole number above 0, not {:?}", values[2]));
                    self.modal = Modal::Form(form);
                    return;
                };
                let change = if values[3].is_empty() { values[0].clone() } else { values[3].clone() };
                let call = l1::TransferCall { from: values[0].clone(), to: values[1].clone(), amount, change };
                let config = self.l1_config();
                self.spawn_job("plan L1 transfer", move |mut ctx| async move {
                    l1::plan_transfer(&mut ctx, call, config).await
                });
            }
            FormAction::Bridge => {
                let to = match l1::LezRecipient::parse(&values[0]) {
                    Ok(to) if !to.id().is_empty() => self.with_watched_evidence(to),
                    Ok(_) => {
                        self.push_log("✗ to: which LEZ account? An id, a public key or pda:<program>/<seed>".to_owned());
                        self.modal = Modal::Form(form);
                        return;
                    }
                    Err(error) => {
                        self.push_log(format!("✗ to: {error}"));
                        self.modal = Modal::Form(form);
                        return;
                    }
                };
                let Some(amount) = values[1].parse::<u64>().ok().filter(|amount| *amount > 0) else {
                    self.push_log(format!("✗ amount must be a whole number above 0, not {:?}", values[1]));
                    self.modal = Modal::Form(form);
                    return;
                };
                let Some(max_fee) = values[4].parse::<u64>().ok() else {
                    self.push_log(format!("✗ max fee must be a whole number, not {:?}", values[4]));
                    self.modal = Modal::Form(form);
                    return;
                };
                let metadata = match l1::MetadataForm::parse(&values[5]) {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        self.push_log(format!("✗ {error}"));
                        self.modal = Modal::Form(form);
                        return;
                    }
                };
                let request = l1::BridgeRequest {
                    from: values[2].clone(),
                    to,
                    amount,
                    channel: values[3].clone(),
                    max_fee,
                    metadata,
                };
                self.spawn_job("plan bridge", move |mut ctx| async move {
                    l1::plan_bridge(&mut ctx, request).await
                });
            }
            FormAction::PowSettings => {
                let mine = match values[4].to_ascii_lowercase().as_str() {
                    "yes" | "y" | "on" | "true" => true,
                    "no" | "n" | "off" | "false" | "" => false,
                    other => {
                        self.push_log(format!("✗ mine after start: yes or no, not {other:?}"));
                        self.modal = Modal::Form(form);
                        return;
                    }
                };
                let Some(config) = self.l1_config() else {
                    self.push_log("✗ no L1 config: C sets it".to_owned());
                    return;
                };
                let request = l1::PowForm {
                    threads: values[0].clone(),
                    tickets: values[1].clone(),
                    tick: values[2].clone(),
                    targets: values[3].clone(),
                };
                // The node's wallet keys, when it's Online to say.
                let known: Option<Vec<String>> = self
                    .l1
                    .node
                    .as_ref()
                    .filter(|node| node.chain.is_some() && self.l1.wallets_error.is_none())
                    .map(|_| self.l1.wallets.iter().map(|wallet| wallet.address.clone()).collect());
                let plan = match l1::plan_pow(&config, &request, known.as_deref()) {
                    Ok(plan) => plan,
                    Err(error) => {
                        self.push_log(format!("✗ {error:#}"));
                        self.modal = Modal::Form(form);
                        return;
                    }
                };
                if mine != self.saved.l1_mine_after_start {
                    self.saved.l1_mine_after_start = mine;
                    self.save();
                    self.push_log(format!(
                        "mine after start: {}",
                        if mine { "on (s starts the node, then mining once Online)" } else { "off" }
                    ));
                }
                match plan {
                    (_, None) => self.push_log("the config's pow section already says that".to_owned()),
                    (lines, Some(write)) => {
                        self.modal = Modal::Plan(PlanView {
                            title: "L1 PoW settings".to_owned(),
                            lines,
                            call: Some(Planned::PowConfig(write)),
                            scroll: Cell::new(0),
                            read_only: false,
                        });
                    }
                }
            }
            FormAction::L1Config => {
                self.saved.l1_config.clone_from(&values[0]);
                self.save();
                self.last_l1 = None;
                match self.l1_config() {
                    Some(path) if path.is_file() => self.push_log(format!("L1 config {}", path.display())),
                    Some(path) => self.push_log(format!("✗ no file at {}", path.display())),
                    None => self.push_log("L1 config cleared, and Basecamp has none".to_owned()),
                }
            }
            FormAction::Watch => {
                let id = jobs::bare_account_id(&values[0]).to_owned();
                if bs58::decode(&id).into_vec().map_or(true, |bytes| bytes.len() != 32) {
                    self.push_log(format!("✗ not a base58 account id: {id:?}"));
                    self.modal = Modal::Form(form);
                    return;
                }
                let note = if values[1].is_empty() { "watched".to_owned() } else { values[1].clone() };
                self.on_msg(Msg::Watch(id, note, None));
            }
            FormAction::Plan {
                ref program_id,
                ref name,
                ref method,
                single,
                ref source,
                ref seeds,
            } => {
                if !self.lez_call_ready() {
                    return;
                }
                let Some(payer) = self.need("PAYER (key P)", self.saved.payer.clone().as_ref()) else {
                    return;
                };
                let request = match call_request(&payer, program_id, method, single, &values) {
                    Ok(request) => CallRequest {
                        name: name.clone(),
                        source: source.clone(),
                        seeds: seeds.clone(),
                        ..request
                    },
                    Err(error) => {
                        self.push_log(format!("✗ {error}"));
                        self.modal = Modal::Form(form);
                        return;
                    }
                };
                let title = format!("{name}: {}", method.name);
                self.spawn_job(&format!("plan {title}"), move |mut ctx| async move {
                    jobs::plan(&mut ctx, title, request).await
                });
            }
            FormAction::SpelPlan {
                ref program_id,
                ref name,
                ref method,
                ref idl,
            } => {
                if !self.spel_ready() {
                    return;
                }
                let Some(payer) = self.need("PAYER (key P)", self.saved.payer.clone().as_ref()) else {
                    return;
                };
                let call = match spel_call(&payer, program_id, name, idl, method, &values) {
                    Ok(call) => call,
                    Err(error) => {
                        self.push_log(format!("✗ {error}"));
                        self.modal = Modal::Form(form);
                        return;
                    }
                };
                let title = format!("{name}: {}", method.name);
                self.spawn_job(&format!("plan {title}"), move |mut ctx| async move {
                    jobs::spel_plan(&mut ctx, title, call).await
                });
            }
        }
    }
}

/// The form for one message of a lez-programs program: the actor it goes to, each IDL field (an
/// account field prefilled with `ACCOUNT`), then who signs and more actors to declare. spel never
/// works out signers, so `signers` starts as `ACCOUNT`.
fn spel_form(saved: &Saved, deployment: &Deployment, method: Method, idl: PathBuf) -> Form {
    let account = saved.account.clone().unwrap_or_default();
    let mut fields = vec![(
        "to\nthe actor it goes to: an account (on this program) or account@program; a wallet label or base58; pda:<account>".to_owned(),
        account.clone(),
    )];
    for param in &method.params {
        let value = if param.ty == "account_id" {
            account.clone()
        } else {
            String::new()
        };
        fields.push((format!("{}: {}\n{}", param.name, param.ty, param.hint), value));
    }
    let params = method.params.len();
    fields.push((
        "signers\ncomma-separated accounts that authorize: each actor of theirs in the call".to_owned(),
        account,
    ));
    fields.push((
        "actors\noptional, comma-separated actors spel can't see from the fields: account@program; pda:<account> (this program's PDA seeded by that account); clock".to_owned(),
        String::new(),
    ));
    let mut form = Form::new(
        format!("{}: {} → spel send --dry-run", deployment.name, method.name),
        fields,
        FormAction::SpelPlan {
            program_id: deployment.program_id.clone(),
            name: deployment.name.clone(),
            method,
            idl,
        },
    );
    form.selected = form.fields[1..=params]
        .iter()
        .position(|(_, value)| value.is_empty())
        .map_or(0, |i| i + 1);
    form
}

/// The `spel send` the form's values ask for: `to`, the fields, `signers` and `actors`.
fn spel_call(
    payer: &str,
    program_id: &str,
    name: &str,
    idl: &std::path::Path,
    method: &Method,
    values: &[String],
) -> Result<SpelCall, String> {
    let params = method.params.len();
    let (to, rest) = (&values[0], &values[params + 1..]);
    if to.is_empty() {
        return Err("to is empty".to_owned());
    }
    Ok(SpelCall {
        name: name.to_owned(),
        idl: idl.to_path_buf(),
        program_id: program_id.to_owned(),
        payer: payer.to_owned(),
        message: method.name.clone(),
        root: to.clone(),
        signers: list(&rest[0]),
        actors: list(&rest[1]),
        fields: method
            .params
            .iter()
            .zip(&values[1..=params])
            .map(|(param, value)| (param.name.clone(), value.clone()))
            .collect(),
    })
}

/// A comma-separated list, without empty items.
fn list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The form for one message: where it goes, and each param (an account id prefilled with
/// `ACCOUNT`). Then what the dry run otherwise works out itself: whether `to` signs, more actors
/// to declare, and seeds to try for a new PDA.
fn send_form(
    saved: &Saved,
    deployment: &Deployment,
    method: Method,
    single: bool,
    source: PathBuf,
    seeds: Vec<Seed>,
) -> Form {
    let account = saved.account.clone().unwrap_or_default();
    let pdas: String = seeds.iter().map(|seed| format!(", or pda:{}", seed.name)).collect();
    let mut fields = vec![(
        format!("to\nthe account the message goes to: base58 or a wallet label{pdas}"),
        account.clone(),
    )];
    for param in &method.params {
        let value = if param.ty.ends_with("AccountId") {
            account.clone()
        } else {
            String::new()
        };
        fields.push((format!("{}: {}\n{}", param.name, param.ty, param.hint), value));
    }
    let params = method.params.len();
    fields.push((
        "sign\nauto: the dry run finds out; y: `to`'s account signs; n: it never does".to_owned(),
        "auto".to_owned(),
    ));
    fields.push((
        "actors\noptional, comma-separated: more actors to declare (account@program, pda:<seed>, pk:<hex>)".to_owned(),
        String::new(),
    ));
    fields.push((
        "pda seeds\noptional, comma-separated: seeds to try for a PDA that isn't on chain yet".to_owned(),
        String::new(),
    ));
    let mut form = Form::new(
        format!("{}: {} → plan", deployment.name, method.name),
        fields,
        FormAction::Plan {
            program_id: deployment.program_id.clone(),
            name: deployment.name.clone(),
            method,
            single,
            source,
            seeds,
        },
    );
    // Start on the first empty param, not on an optional field.
    form.selected = form.fields[1..=params]
        .iter()
        .position(|(_, value)| value.is_empty())
        .map_or(0, |i| i + 1);
    form
}

/// The call the form's values ask for: `to`, the params, `sign`, `actors` and `pda seeds`. Its
/// program's name, source and seeds are the caller's to fill in.
fn call_request(
    payer: &str,
    program_id: &str,
    method: &Method,
    single: bool,
    values: &[String],
) -> Result<CallRequest, String> {
    let params = method.params.len();
    let (to, rest) = (&values[0], &values[params + 1..]);
    if to.is_empty() {
        return Err("to is empty".to_owned());
    }
    let sign = match rest[0].as_str() {
        "" | "auto" => None,
        "y" | "yes" | "true" => Some(true),
        "n" | "no" | "false" => Some(false),
        other => return Err(format!("sign is auto, y or n, not {other:?}")),
    };
    Ok(CallRequest {
        payer: payer.to_owned(),
        program_id: program_id.to_owned(),
        to: to.clone(),
        sign,
        hinted_signed: method.signed(),
        actors: list(&rest[1]),
        pda_seeds: list(&rest[2]),
        method: (!single).then(|| method.name.clone()),
        values: method
            .params
            .iter()
            .zip(&values[1..=params])
            .map(|(param, value)| format!("{}={value}", param.name))
            .collect(),
        ..CallRequest::default()
    })
}

/// The LEZ checkout and the RPC for `network`. A remote network gets its own wallet home,
/// `<state_dir>/wallet`, made from LEZ's debug `wallet_config.json` with `sequencer_addr` set to
/// the network's sequencer; its RPC is that sequencer. The error is the wallet home's, if setting it
/// up failed (no LEZ checkout yet, say).
fn connect(
    network: &Network,
    crate_dir: &std::path::Path,
    lez_dir: &str,
    state_dir: &std::path::Path,
) -> (Lez, Rpc, Option<anyhow::Error>) {
    let lez = Lez::new(state::resolve_dir(crate_dir, lez_dir));
    match network {
        Network::Local => {
            let rpc = Rpc::new(lez.sequencer_url());
            (lez, rpc, None)
        }
        Network::Remote { url, poll_timeout, .. } => {
            let home = state_dir.join("wallet");
            let error = lez.prepare_wallet_home(&home, url, poll_timeout.as_deref()).err();
            (lez.with_wallet_home(home), Rpc::new(url.clone()), error)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lez_call::Param;

    fn method(name: &str, params: &[(&str, &str)]) -> Method {
        Method {
            name: name.to_owned(),
            index: Some(2),
            fields: String::new(),
            params: params
                .iter()
                .map(|(name, ty)| Param {
                    name: (*name).to_owned(),
                    ty: (*ty).to_owned(),
                    hint: String::new(),
                })
                .collect(),
            doc: None,
            hints: Vec::new(),
        }
    }

    fn values(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn the_form_gives_the_call() {
        let mut mint = method("Mint", &[("to", "AccountId"), ("amount", "u128")]);
        mint.hints = vec!["signed".to_owned()];
        let request =
            call_request("PAY", "PROG", &mint, false, &values(&["ACC", "BOB", "30", "auto", "", ""])).unwrap();
        assert_eq!(request.to, "ACC");
        assert_eq!(request.sign, None);
        assert!(request.hinted_signed);
        assert_eq!(request.method.as_deref(), Some("Mint"));
        assert_eq!(request.values, ["to=BOB", "amount=30"]);

        let hello = method("Vec<u8>", &[("#0", "Vec<u8>")]);
        let request =
            call_request("PAY", "PROG", &hello, true, &values(&["pda:SEED", "Hi", "y", "X@P, pk:ab", "0x01"])).unwrap();
        assert_eq!(request.sign, Some(true));
        assert_eq!(request.method, None);
        assert_eq!(request.actors, ["X@P", "pk:ab"]);
        assert_eq!(request.pda_seeds, ["0x01"]);
        assert_eq!(request.values, ["#0=Hi"]);

        assert!(call_request("PAY", "PROG", &hello, true, &values(&["", "Hi", "auto", "", ""])).is_err());
        assert!(call_request("PAY", "PROG", &hello, true, &values(&["ACC", "Hi", "maybe", "", ""])).is_err());
    }

    #[test]
    fn the_spel_form_gives_the_send() {
        let new = method("NewDefinition", &[("name", "string"), ("total_supply", "u128"), ("holding", "account_id")]);
        let call = spel_call(
            "PAY",
            "PROG",
            "token",
            std::path::Path::new("/idl.json"),
            &new,
            &values(&["DEF", "JZ", "1000", "ACC", "DEF, ACC", ""]),
        )
        .unwrap();
        assert_eq!(call.root, "DEF");
        assert_eq!(call.signers, ["DEF", "ACC"]);
        assert!(call.actors.is_empty());
        assert_eq!(call.message, "NewDefinition");
        assert_eq!(
            call.fields,
            [
                ("name".to_owned(), "JZ".to_owned()),
                ("total_supply".to_owned(), "1000".to_owned()),
                ("holding".to_owned(), "ACC".to_owned())
            ]
        );
        assert!(spel_call("PAY", "PROG", "token", std::path::Path::new("/i"), &new, &values(&["", "JZ", "1", "ACC", "", ""])).is_err());
    }
}
