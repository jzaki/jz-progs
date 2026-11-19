//! Payroll on LEZ's actor interface: a private treasury pays employees private amounts.
//!
//! # Actors, all of this program
//!
//! - **The seat**: the program's actor on the seat's own *private* account. Its owner signs (the
//!   private account's `ask` key), so the seat sees `is_authorized` with no sender. Its state is
//!   the treasury's address ([`SeatState`]).
//! - **The treasury**: the program's actor on a *private PDA*,
//!   `AccountId::for_private_pda(program, seed, npk, vpk)`. The seed is the seat's account id, and
//!   `npk`, `vpk` are the seat's own keys. It holds the native funds, the employees, their pay, and
//!   what each is owed and was paid ([`PayrollState`]).
//! - **Employees**: private accounts, paid with native transfers out of the treasury.
//!
//! # Why everything is private
//!
//! A public actor's state is public, and so is a message from public execution: an async message
//! it emits is published *clear*, amount included. A private actor's state is a commitment and an
//! encrypted note, and an async message from private execution is published *sealed*: only the
//! recipient can read it. So the seat, the treasury and the employees are all private accounts,
//! and every call is a private transaction. The chain then sees nullifiers, commitments and sealed
//! notes, and no amount.
//!
//! The treasury's address is derived from the seat's keys, so only the holder of those keys can
//! put it in a transaction (its nullifier needs the seat's `nsk`). Nobody else can see its state
//! or touch its funds, whatever they send.
//!
//! This program emits no events: an event would publish the amounts.
//!
//! # Authorization
//!
//! - **The seat** accepts a [`Message`] only from a transaction its owner signed:
//!   `is_authorized` and no sender.
//! - **The treasury** accepts an [`Internal`] message only with the seed grant. The seat's
//!   messages to it carry the treasury's seed (`with_pda_seeds`), and the engine turns that into
//!   `is_authorized` only when the sender runs this program and the PDA is bound to
//!   `(this program, seed)`.
//!
//!   The treasury can't check *which* seat sent it. A private sender reaches it under a fresh
//!   blinded alias each time, so `from` is this program at an unrelated address. The keys are the
//!   real gate: a seed grant from anyone else's seat only reaches *their* treasury.
//!
//! # Paying without losing funds
//!
//! A native transfer debits the treasury at once, and credits the employee by an *async* message.
//! The employee is private and not in the transaction, so the credit is stored, sealed to them,
//! until they collect it (`wallet pending receive`). LEZ can't bounce a stored message, so once
//! the credit is stored it is the employee's for good.
//!
//! Everything that can fail fails *before* that point, and then the whole transaction reverts:
//! debit, credit and bookkeeping together. An unknown employee, nothing owed, an insufficient
//! balance or a missing seal all revert. So pay is kept as *owed* until it is paid:
//! - `Accrue` adds each period's pay to `owed`. No funds move, so it can't strand any.
//! - `Pay` moves one employee's `owed` to `paid` in the same transaction as the transfer that
//!   pays it. If the payment fails, the funds stay in the treasury and the pay stays owed, ready
//!   to retry. One employee's failed payment never touches another's.
//! - `Withdraw` returns only what nobody is owed: the treasury reads its own balance first.
//!
//! # Periods
//!
//! `Accrue { period }` sets a timestamp validity window starting at `period * period_ms`, so the
//! transaction can only land once that period has begun. The program never reads the clock: the
//! engine checks the window against the block's timestamp (the `time_locked_transfer` pattern).

use lee_core::{
    account::{AccountId, Actor},
    program::{ReceiveInput, Response, StateReply, SyncMessage},
};
pub use payroll_core as core;
pub use payroll_core::seed_of;
use payroll_core::{Internal, Message, PayrollState, SeatState};

mod accrue;
mod open;
mod pay;
mod set_employee;
#[cfg(test)]
mod test_utils;
mod withdraw;

/// The guest entry point. Dispatches on the sender:
/// - the treasury's own native balance: its answer to `ReadState`, for a withdrawal;
/// - this program: an [`Internal`] message, the seat to its treasury;
/// - anyone else (a transaction): a [`Message`] to the seat.
pub fn handle_message(input: &ReceiveInput) -> Response {
    if input.from == Some(Actor::native_balance(input.receiver.account_id)) {
        let reply: StateReply =
            borsh::from_slice(&input.message).expect("the native balance answers a StateReply");
        return withdraw::on_balance(input, &reply);
    }
    if input.from_own_program() {
        assert!(
            input.is_authorized,
            "the treasury acts only on its seat's seed grant"
        );
        let message: Internal =
            borsh::from_slice(&input.message).expect("internal payroll message must decode");
        return match message {
            Internal::Init { seed, period_ms } => open::init(input, seed, period_ms),
            Internal::Configure { employee, amount } => {
                set_employee::configure(input, employee, amount)
            }
            Internal::AccrueTo { period } => accrue::accrue_to(input, period),
            Internal::Release { employee } => pay::release(input, employee),
            Internal::Recover { amount, to } => withdraw::recover(input, amount, to),
        };
    }
    assert!(
        input.is_authorized && input.from.is_none(),
        "only the seat's owner calls, by a transaction they sign"
    );
    let message: Message = borsh::from_slice(&input.message).expect("payroll message must decode");
    match message {
        Message::Open {
            treasury,
            period_ms,
        } => open::open(input, treasury, period_ms),
        Message::SetEmployee { employee, amount } => {
            to_treasury(input, &Internal::Configure { employee, amount })
        }
        Message::Accrue { period } => to_treasury(input, &Internal::AccrueTo { period }),
        Message::Pay { employee } => to_treasury(input, &Internal::Release { employee }),
        Message::Withdraw { amount } => to_treasury(
            input,
            &Internal::Recover {
                amount,
                to: input.receiver.account_id,
            },
        ),
    }
}

/// The seat's `message` to the treasury it opened, granting the treasury's seed.
fn treasury_message(input: &ReceiveInput, treasury: AccountId, message: &Internal) -> SyncMessage {
    SyncMessage::new(
        Actor::new(treasury, input.receiver.program_account_id),
        message,
    )
    .with_pda_seeds([seed_of(input.receiver.account_id)])
}

/// At the seat: forward `message` to its treasury. The seat's state doesn't change.
fn to_treasury(input: &ReceiveInput, message: &Internal) -> Response {
    let seat = SeatState::decode(&input.pre_state);
    Response::keep_state().send(treasury_message(input, seat.treasury, message))
}

/// At the treasury: its state.
fn treasury_state(input: &ReceiveInput) -> PayrollState {
    PayrollState::decode(&input.pre_state)
}
