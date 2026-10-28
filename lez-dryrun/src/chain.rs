//! Chain reads for the dry run, over the sequencer's RPC. The engine's environment is synchronous,
//! so each read blocks on a small runtime of its own.

use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    rc::Rc,
};

use anyhow::Context as _;
use lee::{AccountId, program::Program};
use lee_core::{
    account::{Account, Actor, ActorState},
    program::{MAX_PROGRAM_SEGMENTS, PROGRAM_LOADER_ACCOUNT_ID, ProgramHeader, ProgramSegment},
};
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use tokio::runtime::Runtime;

/// The chain as the sequencer has it, cached: a plan reads the same accounts on every run.
pub struct Chain {
    client: SequencerClient,
    rt: Runtime,
    /// The public accounts whose keys the wallet holds: they can sign.
    signers: BTreeSet<AccountId>,
    views: RefCell<HashMap<Actor, Option<Account>>>,
    programs: RefCell<HashMap<AccountId, Option<Rc<Program>>>>,
}

impl Chain {
    pub fn new(rpc: &str, signers: BTreeSet<AccountId>) -> anyhow::Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let client = {
            let _entered = rt.enter();
            SequencerClientBuilder::default()
                .build(rpc)
                .with_context(|| format!("sequencer at {rpc}"))?
        };
        Ok(Self {
            client,
            rt,
            signers,
            views: RefCell::new(HashMap::new()),
            programs: RefCell::new(HashMap::new()),
        })
    }

    /// The account with `actor`'s state selected, or `None` when it isn't on chain.
    pub fn view(&self, actor: Actor) -> anyhow::Result<Option<Account>> {
        if let Some(view) = self.views.borrow().get(&actor) {
            return Ok(view.clone());
        }
        // RPC projections include empty actor states; the wallet omits them, and so does this.
        let view = self
            .rt
            .block_on(self.client.get_account_view(actor))?
            .map(|mut account| {
                account
                    .data
                    .actor_states
                    .retain(|_, actor_state| !actor_state.is_empty());
                account
            });
        self.views.borrow_mut().insert(actor, view.clone());
        Ok(view)
    }

    pub fn actor_state(&self, actor: Actor) -> anyhow::Result<ActorState> {
        Ok(self
            .view(actor)?
            .map(|account| account.data.actor_state(actor.program_account_id).clone())
            .unwrap_or_default())
    }

    pub fn on_chain(&self, account_id: AccountId) -> anyhow::Result<bool> {
        Ok(self.view(Actor::native_balance(account_id))?.is_some())
    }

    /// Whether an async message can wait in the mailbox for `account_id`: it has a recovery
    /// binding.
    pub fn recovery_bound(&self, account_id: AccountId) -> anyhow::Result<bool> {
        Ok(self
            .rt
            .block_on(self.client.get_recovery_binding(account_id))?
            .is_some())
    }

    /// Whether the wallet can sign for `account_id`.
    pub fn holds_key(&self, account_id: AccountId) -> bool {
        self.signers.contains(&account_id)
    }

    /// The header of the program deployed at `program`.
    pub fn header(&self, program: AccountId) -> anyhow::Result<Option<ProgramHeader>> {
        let state = self.actor_state(Actor::new(program, PROGRAM_LOADER_ACCOUNT_ID))?;
        Ok(ProgramHeader::from_bytes(state.as_ref()))
    }

    /// The program deployed at `program`, read from its loader states.
    pub fn program(&self, program: AccountId) -> anyhow::Result<Option<Rc<Program>>> {
        if let Some(loaded) = self.programs.borrow().get(&program) {
            return Ok(loaded.clone());
        }
        let loader = |account_id| Actor::new(account_id, PROGRAM_LOADER_ACCOUNT_ID);
        let mut states = HashMap::new();
        let header_state = self.actor_state(loader(program))?;
        let loaded = match ProgramHeader::from_bytes(header_state.as_ref()) {
            None => None,
            Some(header) => {
                states.insert(program, header_state);
                let mut next = Some(header.program_first_segment);
                while let Some(segment_id) = next {
                    anyhow::ensure!(
                        states.len() <= MAX_PROGRAM_SEGMENTS,
                        "program {program} has more than {MAX_PROGRAM_SEGMENTS} segments"
                    );
                    let state = self.actor_state(loader(segment_id))?;
                    let segment = ProgramSegment::from_bytes(state.as_ref())
                        .with_context(|| format!("segment {segment_id} of program {program}"))?;
                    next = segment.next_segment;
                    states.insert(segment_id, state);
                }
                lee::program::load_program(program, |account_id| states.get(&account_id))
                    .map(Rc::new)
            }
        };
        self.programs.borrow_mut().insert(program, loaded.clone());
        Ok(loaded)
    }
}
