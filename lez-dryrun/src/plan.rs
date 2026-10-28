//! A call's plan: dry runs until the transaction has every actor, signer and piece of evidence
//! it needs, and what that shows.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Display,
};

use lee::{AccountId, PublicExecutionContext, PublicKey, TransactionEntry};
use lee_core::{
    RootCall,
    account::Actor,
    execution_state::{ExecutionError, PublicOutcome, WholeExecution},
    native_token::{self, NATIVE_TOKEN_PROGRAM_ID},
    program::PdaSeed,
};
use serde::{Serialize, Serializer};

use crate::{
    chain::Chain,
    dry_run::{Delivered, DryRun, Failure},
};

/// Runs before the plan gives up: each one adds at least one actor or signer.
const MAX_RUNS: usize = 12;

/// How an account of the transaction is admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    OnChain,
    /// It signs, or pays the fee.
    Signer,
    /// Not on chain; admitted by its public key: one the wallet holds, or one given as `pk:…`.
    Key,
    /// Not on chain; a PDA of `program` for `seed`.
    Pda {
        program: AccountId,
        seed: PdaSeed,
    },
    /// Not on chain; admitted inside the transaction, by a seed grant or another actor on it.
    InTransaction,
}

/// One actor given from the outside: `--to`, or an `--actor`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub actor: Actor,
    pub sign: bool,
    /// The PDA's program and seed, when it's given as `pda:…`.
    pub pda: Option<(AccountId, PdaSeed)>,
    /// The account's public key, when it's given as `pk:…`.
    pub key: Option<PublicKey>,
}

pub struct Request {
    /// The program `--program` names.
    pub program: AccountId,
    pub root: Spec,
    pub actors: Vec<Spec>,
    pub message: Vec<u8>,
    pub payer: AccountId,
    /// `Some(false)`: the root never signs (`--no-sign`). `Some(true)`: it does (`--sign`).
    pub sign: Option<bool>,
    /// The source hints that the root's message is signed: sign first, saving a run.
    pub hinted_signed: bool,
    /// Seeds to try for a PDA that isn't on chain: the source's constants and `--pda-seed`.
    pub seeds: Vec<PdaSeed>,
    /// Account ids the message names, from its params.
    pub mentioned: Vec<AccountId>,
    pub leave_pending: bool,
    /// The cycles the transaction may use: the wallet's gas limit.
    pub budget: u64,
}

#[derive(Serialize)]
pub struct Plan {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(serialize_with = "display")]
    pub program: AccountId,
    #[serde(serialize_with = "display")]
    pub payer: AccountId,
    #[serde(serialize_with = "hex_of")]
    pub message: Vec<u8>,
    /// One line per dry run: what it found, and what was added.
    pub runs: Vec<String>,
    /// The root first.
    pub actors: Vec<Planned>,
    pub changes: Vec<Change>,
    /// Async messages left in the mailbox.
    pub pending: Vec<Pending>,
    pub events: Vec<Event>,
    pub cycles: u64,
    /// `wallet program call …`: the same transaction, with nothing left to infer.
    pub command: String,
    /// `command`'s arguments after `wallet`, to run it without a shell.
    pub wallet_args: Vec<String>,
    /// The `wallet` command for the same call, when the wallet has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallet_command: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct Planned {
    #[serde(serialize_with = "display")]
    pub account: AccountId,
    #[serde(serialize_with = "display")]
    pub program: AccountId,
    pub root: bool,
    pub signs: bool,
    #[serde(serialize_with = "admission_name")]
    pub admission: Admission,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "pda_of")]
    pub pda: Option<(AccountId, PdaSeed)>,
    /// The public key that admits it, when the wallet doesn't hold it.
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "key_of")]
    pub key: Option<PublicKey>,
    /// Why it's declared.
    pub why: String,
    /// Why it signs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signs_because: Option<String>,
}

#[derive(Serialize)]
pub struct Change {
    #[serde(serialize_with = "display")]
    pub account: AccountId,
    #[serde(serialize_with = "display")]
    pub program: AccountId,
    #[serde(serialize_with = "hex_of")]
    pub before: Vec<u8>,
    #[serde(serialize_with = "hex_of")]
    pub after: Vec<u8>,
}

#[derive(Serialize)]
pub struct Pending {
    #[serde(serialize_with = "actor_of")]
    pub from: Actor,
    #[serde(serialize_with = "actor_of")]
    pub to: Actor,
    #[serde(serialize_with = "hex_of")]
    pub message: Vec<u8>,
}

#[derive(Serialize)]
pub struct Event {
    #[serde(serialize_with = "actor_of")]
    pub actor: Actor,
    #[serde(serialize_with = "hex_of")]
    pub selector: Vec<u8>,
    #[serde(serialize_with = "hex_of")]
    pub data: Vec<u8>,
}

/// What the runs so far have found the transaction needs.
struct Draft<'plan> {
    chain: &'plan Chain,
    request: &'plan Request,
    declared: Vec<Actor>,
    why: BTreeMap<Actor, String>,
    signers: BTreeSet<AccountId>,
    sign_why: BTreeMap<AccountId, String>,
    evidence: BTreeMap<AccountId, Admission>,
    /// Public keys given as `pk:…`.
    keys: BTreeMap<AccountId, PublicKey>,
    runs: Vec<String>,
}

impl Plan {
    /// What `lez-dryrun plan` prints. `name` names a program, for reading.
    pub fn lines(&self, name: &dyn Fn(AccountId) -> String) -> Vec<String> {
        let label = |account: AccountId, program: AccountId| {
            format!("{}@{}", short(account), name(program))
        };
        let mut lines = vec![
            format!(
                "message 0x{} ({} bytes)   paid by {}",
                hex::encode(&self.message),
                self.message.len(),
                short(self.payer)
            ),
            String::new(),
        ];
        for (index, run) in self.runs.iter().enumerate() {
            lines.push(format!("run {}  {run}", index.saturating_add(1)));
        }
        lines.push(String::new());
        if let Some(error) = &self.error {
            lines.push(format!("✗ {error}"));
            lines.push(String::new());
        }
        // `(kind, actor, the rest)`, laid out in columns at the end.
        let mut rows: Vec<(&str, String, String)> = Vec::new();
        for planned in &self.actors {
            let admission = match planned.admission {
                Admission::OnChain => "on chain".to_owned(),
                Admission::Signer => planned.signs_because.as_ref().map_or_else(
                    || "signs".to_owned(),
                    |because| format!("signs ({because})"),
                ),
                Admission::Key => "new: key evidence".to_owned(),
                Admission::Pda { program, seed } => {
                    format!(
                        "new: PDA of {} for 0x{}",
                        name(program),
                        hex::encode(seed.as_bytes())
                    )
                }
                Admission::InTransaction => "new: admitted in the transaction".to_owned(),
            };
            rows.push((
                if planned.root { "to" } else { "actor" },
                label(planned.account, planned.program),
                format!("{admission}: {}", planned.why),
            ));
        }
        for change in &self.changes {
            rows.push((
                "after",
                label(change.account, change.program),
                format!("{} → {}", describe(&change.before), describe(&change.after)),
            ));
        }
        for pending in &self.pending {
            rows.push((
                "mail",
                label(pending.to.account_id, pending.to.program_account_id),
                format!(
                    "pending from {}: 0x{} (wallet pending receive)",
                    label(pending.from.account_id, pending.from.program_account_id),
                    hex::encode(&pending.message)
                ),
            ));
        }
        for event in &self.events {
            rows.push((
                "event",
                label(event.actor.account_id, event.actor.program_account_id),
                format!(
                    "0x{}: 0x{}",
                    hex::encode(&event.selector),
                    hex::encode(&event.data)
                ),
            ));
        }
        let width = rows
            .iter()
            .map(|(_, actor, _)| actor.chars().count())
            .max()
            .unwrap_or(0);
        for (kind, actor, rest) in rows {
            lines.push(format!("{kind:<6}{actor:<width$}  {rest}"));
        }
        if self.ok {
            lines.push(format!("{} cycles", self.cycles));
        }
        lines.push(String::new());
        lines.push("same call by hand:".to_owned());
        lines.push(format!("  {}", self.command));
        if let Some(command) = &self.wallet_command {
            lines.push(format!("  {command}"));
        }
        lines
    }
}

impl Planned {
    /// This actor as `wallet program call` takes it: `--to`/`--actor`'s value. A new account the
    /// wallet holds the key of is given plainly: the wallet adds its key evidence itself.
    pub fn spec(&self, default_program: AccountId) -> String {
        let account = match (self.admission, &self.key) {
            (Admission::Pda { program, seed }, _) if !self.signs => {
                format!("pda:{program}/0x{}", hex::encode(seed.as_bytes()))
            }
            (_, Some(key)) if !self.signs => format!("pk:{key}"),
            (
                Admission::OnChain
                | Admission::Signer
                | Admission::Key
                | Admission::Pda { .. }
                | Admission::InTransaction,
                _,
            ) => format!("Public/{}", self.account),
        };
        let at = if self.program == default_program {
            String::new()
        } else {
            format!("@{}", self.program)
        };
        let sign = if self.signs { ":sign" } else { "" };
        format!("{account}{at}{sign}")
    }
}

impl<'plan> Draft<'plan> {
    fn new(chain: &'plan Chain, request: &'plan Request) -> Self {
        let mut draft = Self {
            chain,
            request,
            declared: Vec::new(),
            why: BTreeMap::new(),
            signers: BTreeSet::new(),
            sign_why: BTreeMap::new(),
            evidence: BTreeMap::new(),
            keys: BTreeMap::new(),
            runs: Vec::new(),
        };
        let root = &request.root;
        let root_account = root.actor.account_id;
        draft.declare(root.actor, "the message goes to it".to_owned());
        let hinted = request.sign.is_none()
            && request.hinted_signed
            && root.pda.is_none()
            && chain.holds_key(root_account);
        if request.sign == Some(true) || root.sign {
            draft.sign(root_account, "--sign".to_owned());
        } else if hinted {
            draft.sign(root_account, "the source checks is_authorized".to_owned());
        } else {
            // The dry run finds out whether it has to.
        }
        for spec in std::iter::once(root).chain(&request.actors) {
            if spec.actor != root.actor {
                draft.declare(spec.actor, "--actor".to_owned());
            }
            if spec.sign {
                draft.sign(spec.actor.account_id, "--actor …:sign".to_owned());
            }
            if let Some((program, seed)) = spec.pda {
                draft
                    .evidence
                    .insert(spec.actor.account_id, Admission::Pda { program, seed });
            }
            if let Some(key) = &spec.key {
                draft.keys.insert(spec.actor.account_id, key.clone());
                draft.evidence.insert(spec.actor.account_id, Admission::Key);
            }
        }
        draft
    }

    fn declare(&mut self, actor: Actor, why: String) {
        if !self.declared.contains(&actor) {
            self.declared.push(actor);
            self.why.insert(actor, why);
        }
    }

    fn sign(&mut self, account: AccountId, why: String) {
        self.signers.insert(account);
        self.sign_why.entry(account).or_insert(why);
    }

    /// The accounts the transaction's nonces name: the signers and the payer.
    fn authorized(&self) -> BTreeSet<AccountId> {
        let mut authorized = self.signers.clone();
        authorized.insert(self.request.payer);
        authorized
    }

    fn admitted(&self) -> BTreeSet<AccountId> {
        let mut admitted = self.authorized();
        admitted.extend(self.evidence.keys().copied());
        admitted
    }

    /// Whether a delivery that panicked might pass if its receiver signed.
    fn may_sign(&self, at: Delivered) -> bool {
        let account = at.receiver.account_id;
        let root_refused =
            account == self.request.root.actor.account_id && self.request.sign == Some(false);
        !at.is_authorized
            && !root_refused
            && !self.signers.contains(&account)
            && self.chain.holds_key(account)
    }

    /// Whether an async destination can be declared: it's on chain, or something admits it.
    fn admissible(&self, account: AccountId) -> anyhow::Result<bool> {
        Ok(self.authorized().contains(&account)
            || self.chain.on_chain(account)?
            || self.evidence_for(account).is_some())
    }

    fn evidence_for(&self, account: AccountId) -> Option<Admission> {
        if self.chain.holds_key(account) || self.keys.contains_key(&account) {
            return Some(Admission::Key);
        }
        self.seed_for(account)
            .map(|(program, seed)| Admission::Pda { program, seed })
    }

    /// A program and seed whose PDA is `account`: the seeds given, then every account id the
    /// transaction names.
    fn seed_for(&self, account: AccountId) -> Option<(AccountId, PdaSeed)> {
        let programs: BTreeSet<AccountId> = self
            .declared
            .iter()
            .map(|actor| actor.program_account_id)
            .chain([self.request.program])
            .collect();
        let ids = self
            .declared
            .iter()
            .map(|actor| actor.account_id)
            .chain(self.authorized())
            .chain(self.request.mentioned.iter().copied())
            .chain(programs.iter().copied());
        let seeds: Vec<PdaSeed> = self
            .request
            .seeds
            .iter()
            .copied()
            .chain(ids.map(|id| PdaSeed::new(*id.value())))
            .collect();
        programs.iter().find_map(|program| {
            seeds
                .iter()
                .find(|seed| AccountId::for_public_pda(program, seed) == account)
                .map(|seed| (*program, *seed))
        })
    }

    fn actor(&self, actor: Actor) -> String {
        let program = if actor.program_account_id == self.request.program {
            "program".to_owned()
        } else if actor.program_account_id == NATIVE_TOKEN_PROGRAM_ID {
            "native".to_owned()
        } else {
            short(actor.program_account_id)
        };
        format!("{}@{program}", short(actor.account_id))
    }

    fn finish(self, outcome: Result<PublicOutcome, String>, cycles: u64) -> anyhow::Result<Plan> {
        let request = self.request;
        let mut actors = Vec::new();
        for &actor in &self.declared {
            let account = actor.account_id;
            let signs = self.signers.contains(&account) || account == request.payer;
            let admission = if signs {
                Admission::Signer
            } else if let Some(&evidence) = self.evidence.get(&account) {
                evidence
            } else if self.chain.on_chain(account)? {
                Admission::OnChain
            } else {
                Admission::InTransaction
            };
            let signs_because = self
                .sign_why
                .get(&account)
                .cloned()
                .or_else(|| (account == request.payer).then(|| "it pays the fee".to_owned()));
            actors.push(Planned {
                account,
                program: actor.program_account_id,
                root: actor == request.root.actor,
                signs,
                admission,
                pda: match admission {
                    Admission::Pda { program, seed } => Some((program, seed)),
                    Admission::OnChain
                    | Admission::Signer
                    | Admission::Key
                    | Admission::InTransaction => None,
                },
                key: self.keys.get(&account).filter(|_| !signs).cloned(),
                why: self.why.get(&actor).cloned().unwrap_or_default(),
                signs_because,
            });
        }
        let wallet_args = wallet_args(request, &actors);
        let command = format!("wallet {}", wallet_args.join(" "));
        let wallet_command = wallet_command(request);
        let (ok, error, changes, pending, events) = match outcome {
            Err(error) => (false, Some(error), Vec::new(), Vec::new(), Vec::new()),
            Ok(outcome) => {
                let mut changes = Vec::new();
                for (account, data) in outcome.accounts {
                    for (program, after) in data.actor_states {
                        let before = self.chain.actor_state(Actor::new(account, program))?;
                        if before != after {
                            changes.push(Change {
                                account,
                                program,
                                before: before.to_vec(),
                                after: after.to_vec(),
                            });
                        }
                    }
                }
                let pending = outcome
                    .async_messages
                    .into_iter()
                    .map(|body| Pending {
                        from: body.from,
                        to: body.to,
                        message: body.message,
                    })
                    .collect();
                let events = outcome
                    .events
                    .into_iter()
                    .map(|(actor, event)| Event {
                        actor,
                        selector: event.selector.to_vec(),
                        data: event.data,
                    })
                    .collect();
                (true, None, changes, pending, events)
            }
        };
        Ok(Plan {
            ok,
            error,
            program: request.program,
            payer: request.payer,
            message: request.message.clone(),
            runs: self.runs,
            actors,
            changes,
            pending,
            events,
            cycles,
            command,
            wallet_args,
            wallet_command,
        })
    }
}

/// Dry-runs `request` until nothing is missing, or something can't be fixed.
pub fn plan(chain: &Chain, request: &Request) -> anyhow::Result<Plan> {
    let mut draft = Draft::new(chain, request);
    let mut sign_attempt: Option<(AccountId, String)> = None;
    for _ in 0..MAX_RUNS {
        let mut env = DryRun::new(chain, request.budget, draft.admitted());
        let context =
            PublicExecutionContext::new(draft.declared.iter().copied(), draft.authorized());
        let root = RootCall {
            to: request.root.actor,
            message: request.message.clone(),
        };
        let result = WholeExecution::new(context, TransactionEntry::Sync(root), &[])
            .map_err(Failure::Engine)
            .and_then(|execution| execution.execute(&mut env))
            .map(|outcome| outcome.public);
        let last = env.deliveries.last().copied();
        match result {
            Err(Failure::Engine(ExecutionError::UndeclaredActor { actor }))
                if !draft.declared.contains(&actor) =>
            {
                let from = last.map_or_else(
                    || "the transaction".to_owned(),
                    |at| draft.actor(at.receiver),
                );
                draft.runs.push(format!(
                    "{from} sends to {}: declare it",
                    draft.actor(actor)
                ));
                draft.declare(actor, format!("{from} sends to it"));
            }
            Err(Failure::Guest(message)) => {
                let at = last.ok_or_else(|| anyhow::anyhow!("{message}"))?;
                let account = at.receiver.account_id;
                let first_line = message.lines().next().unwrap_or_default().to_owned();
                if sign_attempt.as_ref() == Some(&(account, message.clone())) {
                    // Signing didn't change what it panics on: it wasn't for want of a signature.
                    draft.signers.remove(&account);
                    draft.sign_why.remove(&account);
                    draft.runs.push(format!(
                        "{} panics the same signed: a signature isn't what's missing",
                        draft.actor(at.receiver)
                    ));
                } else if draft.may_sign(at) {
                    draft.runs.push(format!(
                        "{} panics unsigned ({first_line}): try with {} signing",
                        draft.actor(at.receiver),
                        short(account)
                    ));
                    draft.sign(account, format!("unsigned, it panics: {first_line}"));
                    sign_attempt = Some((account, message));
                    continue;
                } else {
                    // Its receiver can't sign, or already does: the panic is the answer.
                }
                let receiver = draft.actor(at.receiver);
                draft.runs.push(format!("{receiver} panics: {first_line}"));
                return draft.finish(Err(format!("{receiver} panics: {message}")), 0);
            }
            Err(Failure::Engine(error)) => {
                draft.runs.push(format!("the engine refuses: {error}"));
                return draft.finish(Err(error.to_string()), 0);
            }
            Err(Failure::Other(error)) => return Err(error),
            Ok(outcome) => {
                for &account in &env.unadmitted {
                    let Some(admission) = draft.evidence_for(account) else {
                        let error = format!(
                            "{} isn't on chain and nothing admits it: the wallet has no key for \
                             it, and no seed tried gives this PDA (pass --seed)",
                            short(account)
                        );
                        draft.runs.push(format!("ok, but {error}"));
                        return draft.finish(Err(error), env.cycles);
                    };
                    draft.evidence.insert(account, admission);
                }
                let mut added = Vec::new();
                if !request.leave_pending {
                    for body in &outcome.async_messages {
                        if !draft.declared.contains(&body.to)
                            && draft.admissible(body.to.account_id)?
                        {
                            let from = draft.actor(body.from);
                            draft.declare(
                                body.to,
                                format!("async from {from}, delivered in this transaction"),
                            );
                            added.push(draft.actor(body.to));
                        }
                    }
                }
                if added.is_empty() {
                    // Settlement keeps an async message for later only at an address with a
                    // recovery binding; anywhere else, the transaction fails.
                    for body in &outcome.async_messages {
                        if !chain.recovery_bound(body.to.account_id)? {
                            let to = draft.actor(body.to);
                            let fix = if draft.admissible(body.to.account_id)? {
                                "drop --leave-pending to deliver it in this transaction"
                            } else {
                                "it can't be declared either (it isn't on chain and the wallet \
                                 has no key for it): give its public key as --actor pk:<hex>"
                            };
                            let error = format!(
                                "{} sends an async message to {to}, which can't wait in the \
                                 mailbox (it has no recovery binding): {fix}",
                                draft.actor(body.from)
                            );
                            draft
                                .runs
                                .push(format!("ok, but {to} can't receive its async message"));
                            return draft.finish(Err(error), env.cycles);
                        }
                    }
                    draft.runs.push(format!(
                        "ok: {} deliveries, {} cycles",
                        env.deliveries.len(),
                        env.cycles
                    ));
                    return draft.finish(Ok(outcome), env.cycles);
                }
                draft.runs.push(format!(
                    "ok; async messages to {}: declare them",
                    added.join(", ")
                ));
            }
        }
    }
    draft.finish(Err(format!("still incomplete after {MAX_RUNS} runs")), 0)
}

/// `program call …`: the words after `wallet`. None needs quoting.
fn wallet_args(request: &Request, actors: &[Planned]) -> Vec<String> {
    let mut words = vec![
        "program".to_owned(),
        "call".to_owned(),
        "--payer".to_owned(),
        format!("Public/{}", request.payer),
        "--program".to_owned(),
        request.program.to_string(),
        "--message-hex".to_owned(),
        format!("0x{}", hex::encode(&request.message)),
    ];
    for planned in actors {
        words.push(if planned.root { "--to" } else { "--actor" }.to_owned());
        words.push(planned.spec(request.program));
    }
    words
}

/// `wallet auth-transfer send`, for a native token transfer.
fn wallet_command(request: &Request) -> Option<String> {
    if request.program != NATIVE_TOKEN_PROGRAM_ID {
        return None;
    }
    match borsh::from_slice::<native_token::Message>(&request.message).ok()? {
        native_token::Message::Transfer { to, amount } => Some(format!(
            "wallet auth-transfer send --from Public/{} --to Public/{to} --amount {amount}",
            request.root.actor.account_id
        )),
        native_token::Message::Credit(_) | native_token::Message::ReadState => None,
    }
}

/// The start of an account id, for reading.
pub fn short(account: AccountId) -> String {
    let text = account.to_string();
    let head: String = text.chars().take(6).collect();
    format!("{head}…")
}

/// Bytes as text when they're printable, hex otherwise.
pub fn describe(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "(empty)".to_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(text) if text.chars().all(|c| !c.is_control()) => format!("{text:?}"),
        _ => format!("0x{}", hex::encode(bytes)),
    }
}

fn display<T: Display, S: Serializer>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(value)
}

fn hex_of<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&hex::encode(bytes))
}

fn actor_of<S: Serializer>(actor: &Actor, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(&format_args!(
        "{}@{}",
        actor.account_id, actor.program_account_id
    ))
}

#[expect(
    clippy::ref_option,
    reason = "serde's serialize_with passes the field by reference"
)]
fn key_of<S: Serializer>(key: &Option<PublicKey>, serializer: S) -> Result<S::Ok, S::Error> {
    match key {
        Some(key) => serializer.collect_str(key),
        None => serializer.serialize_none(),
    }
}

fn admission_name<S: Serializer>(admission: &Admission, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(match admission {
        Admission::OnChain => "on chain",
        Admission::Signer => "signer",
        Admission::Key => "key",
        Admission::Pda { .. } => "pda",
        Admission::InTransaction => "in transaction",
    })
}

#[expect(
    clippy::ref_option,
    reason = "serde's serialize_with passes the field by reference"
)]
fn pda_of<S: Serializer>(
    pda: &Option<(AccountId, PdaSeed)>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match pda {
        Some((program, seed)) => serializer.collect_str(&format_args!(
            "{program}/0x{}",
            hex::encode(seed.as_bytes())
        )),
        None => serializer.serialize_none(),
    }
}
