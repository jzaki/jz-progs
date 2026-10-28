//! The engine, run locally against chain state, recording what a transaction is missing.
//!
//! The sequencer executes a public transaction as `WholeExecution` over an `ExecutionEnvironment`
//! that reads its state. `DryRun` is such an environment: it reads state over RPC, runs guests in
//! the risc0 executor, and, instead of refusing an account that isn't admitted, notes it and lets
//! the run go on. An undeclared actor or a guest's panic still stops a run; the planner adds what
//! is missing and runs again.

use std::collections::BTreeSet;

use lee::{AccountId, program::Program};
use lee_core::{
    account::{Actor, ActorState},
    execution_state::{ExecutionEnvironment, ExecutionError, Placement, TransitionView},
    from_frame, native_token,
    native_token::NATIVE_TOKEN_PROGRAM_ID,
    program::{MessageBody, PROGRAM_LOADER_ACCOUNT_ID, ReceiveInput, Transition},
};
use risc0_zkvm::{ExecutorEnv, ExitCode, default_executor};

use crate::chain::Chain;

/// What one delivery saw: who received it, and whether it was authorized.
#[derive(Clone, Copy, Debug)]
pub struct Delivered {
    pub receiver: Actor,
    pub is_authorized: bool,
}

#[derive(Debug)]
pub enum Failure {
    /// The engine refused the transaction's shape, e.g. `UndeclaredActor`.
    Engine(ExecutionError),
    /// A program panicked or ran out of cycles: the last delivery's receiver.
    Guest(String),
    /// Reading the chain or running the executor failed.
    Other(anyhow::Error),
}

impl From<ExecutionError> for Failure {
    fn from(error: ExecutionError) -> Self {
        Self::Engine(error)
    }
}

pub struct DryRun<'chain> {
    pub chain: &'chain Chain,
    /// The cycles the whole transaction may use.
    pub budget: u64,
    /// Accounts admitted without being on chain: signers, the payer, and planned evidence.
    pub admitted: BTreeSet<AccountId>,
    pub deliveries: Vec<Delivered>,
    /// Accounts that would need evidence: not on chain, not admitted, reached by a delivery.
    pub unadmitted: BTreeSet<AccountId>,
    pub cycles: u64,
}

impl<'chain> DryRun<'chain> {
    pub const fn new(
        chain: &'chain Chain,
        budget: u64,
        admitted: BTreeSet<AccountId>,
    ) -> Self {
        Self {
            chain,
            budget,
            admitted,
            deliveries: Vec::new(),
            unadmitted: BTreeSet::new(),
            cycles: 0,
        }
    }

    fn run_guest(
        &mut self,
        program: &Program,
        input: &ReceiveInput,
    ) -> Result<Transition, Failure> {
        // The guest's stderr holds its panic message; keep it out of ours.
        let mut stderr = Vec::new();
        let executed = {
            let mut builder = ExecutorEnv::builder();
            builder
                .session_limit(Some(self.budget.saturating_sub(self.cycles)))
                .stderr(&mut stderr);
            Program::write_receive_input(input, &mut builder)
                .map_err(|error| Failure::Other(error.into()))?;
            let env = builder.build().map_err(Failure::Other)?;
            default_executor().execute(env, program.elf())
        };
        let info = executed.map_err(|error| Failure::Guest(guest_error(&error, &stderr)))?;
        self.cycles = self.cycles.saturating_add(info.cycles());
        if !matches!(info.exit_code, ExitCode::Halted(0)) {
            return Err(Failure::Guest(format!("exited with {:?}", info.exit_code)));
        }
        let payload = from_frame(&info.journal.bytes)
            .ok_or_else(|| Failure::Other(anyhow::anyhow!("malformed program journal frame")))?;
        borsh::from_slice(payload).map_err(|error| Failure::Other(error.into()))
    }
}

impl ExecutionEnvironment for DryRun<'_> {
    type Error = Failure;

    fn handle_message(
        &mut self,
        input: &ReceiveInput,
        _view: &TransitionView<'_>,
    ) -> Result<Transition, Failure> {
        self.deliveries.push(Delivered {
            receiver: input.receiver,
            is_authorized: input.is_authorized,
        });
        let program_id = input.receiver.program_account_id;
        if program_id == PROGRAM_LOADER_ACCOUNT_ID {
            return Err(Failure::Other(anyhow::anyhow!(
                "a dry run doesn't run the program loader: deploy with `wallet program-loader`"
            )));
        }
        if program_id == NATIVE_TOKEN_PROGRAM_ID {
            return native_token::handle_message(input)
                .map_err(|error| Failure::Guest(error.to_string()));
        }
        let program = self
            .chain
            .program(program_id)
            .map_err(Failure::Other)?
            .ok_or_else(|| {
                Failure::Other(anyhow::anyhow!("no program is deployed at {program_id}"))
            })?;
        self.run_guest(&program, input)
    }

    fn public_actor_state(&mut self, actor: Actor) -> Result<ActorState, Failure> {
        self.chain.actor_state(actor).map_err(Failure::Other)
    }

    fn admits(&mut self, account_id: AccountId) -> Result<bool, Failure> {
        if !self.admitted.contains(&account_id)
            && !self.chain.on_chain(account_id).map_err(Failure::Other)?
        {
            self.unadmitted.insert(account_id);
        }
        Ok(true)
    }

    fn promote(
        &mut self,
        _placement: Placement,
        _index: u64,
        _body: &MessageBody,
    ) -> Result<bool, Failure> {
        Ok(false)
    }
}

/// A guest's panic message: from what it wrote to stderr (`panicked at <file>:<line>:<col>:` and
/// the message), or else the executor's error without its wrapping.
fn guest_error(error: &anyhow::Error, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    if let Some((_, rest)) = stderr.rsplit_once("panicked at ") {
        let message = rest.split_once(":\n").map_or(rest, |(_, message)| message);
        let message = message.split("\nnote:").next().unwrap_or(message);
        return message.trim().to_owned();
    }
    let text = format!("{error:#}");
    text.trim()
        .trim_start_matches("Guest panicked: ")
        .to_owned()
}
