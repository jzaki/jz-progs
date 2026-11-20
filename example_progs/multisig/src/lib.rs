//! An M-of-N multisig on LEZ's actor interface. Members are equal: there are no executors.
//!
//! # Actors, all of this program
//!
//! - **A seat**: the program's actor on a member's own *private* account. The member signs a
//!   transaction to it, and it forwards their word to a vault, naming the member by a
//!   *pseudonym* ([`multisig_core::pseudonym`]). It holds one secret ([`SeatState`]).
//! - **The vault**: the program's actor on a PDA. It holds the members' pseudonyms, the threshold,
//!   and the pending proposals with their approvals ([`VaultState`]). The PDA's native balance
//!   holds the funds, and only this program can move them, by granting the vault's seed.
//!
//! A vault is one of two [`Placement`]s, chosen when it is created. The code is the same for both.
//! - **Public**: `for_public_pda(program, seed)`. Its state, balance and payouts are public.
//!   Approvals are public too, under pseudonyms.
//! - **Hidden**: `for_private_pda(program, seed, npk, vpk)`, with keys derived from a group secret
//!   every member holds (`wallet group`). Only members can see it or put it in a transaction.
//!   The secret lets a member *operate* the vault, not spend from it: only the program grants the
//!   seed, and only when the threshold is met.
//!
//! # How agreement becomes an action
//!
//! Like a Safe: a member **proposes** an action, which the vault stores with their approval. They
//! tell the other members out of band. Each member who agrees **approves** it by a transaction of
//! their own. The approval that meets the threshold runs the action in that same transaction.
//!
//! Every message reaches the vault through its sender's seat:
//! `member's transaction → their seat → the vault`, as a sync message in that one private
//! transaction. A hidden vault takes part in it like the seat. A public vault takes the message
//! across the private-to-public boundary, so its content (pseudonym, action) is published.
//!
//! # How the vault knows who approved
//!
//! A private sender reaches the vault under a fresh blinded alias for each message, so `from`
//! can't identify a seat. Instead, the seat names itself in the message, and the vault trusts that
//! name because of *who can send it*:
//! - The vault accepts a seat's message only from this program (`from_own_program`).
//! - The only code of this program that sends one is the seat's. It runs only on its owner's
//!   signed transaction, and it names only its own pseudonym, computed from its own address and its
//!   own private state.
//!
//! So a member can only ever approve as themselves. No signatures are needed, and nobody can link
//! a pseudonym to an account without the seat's secret.
//!
//! # Membership changes, and what a removed member keeps
//!
//! Adding and removing members are actions like any other: proposed, then approved by M.
//! - **Public vault:** the member is added or removed in place. A removed member loses control at
//!   once, but keeps view, as everyone has it.
//! - **Hidden vault:** a removed member still holds the group secret, so the vault **rotates**.
//!   The members make a new group secret, which the removed member never receives, and with it a
//!   new vault: the next generation of the same `create_key`. In the transaction that approves the
//!   removal, the old vault retires, hands its configuration to the new vault (`Adopt`), and moves
//!   its whole balance there. The removed member can still read the old vault, up to the line
//!   "retired, succeeded by X". They can't read or touch X.
//!
//!   Adding a member can rotate too, if the members don't want them to read the vault's history.

use lee_core::{
    account::Actor,
    program::{ReceiveInput, Response, StateReply},
};
pub use multisig_core as core;
pub use multisig_core::{pseudonym, seed_for};
use multisig_core::{Message, Placement, SeatState, ToVault, VaultMessage, VaultState};

mod create;
mod members;
mod propose;
mod rotate;
mod seat;
#[cfg(test)]
mod test_utils;

/// The guest entry point. Dispatches on the sender:
/// - the vault's own native balance: its answer to `ReadState`, during a rotation or a sweep;
/// - this program: a [`VaultMessage`], from a seat or from the vault this one succeeds;
/// - anyone else (a transaction): a [`Message`] to a seat.
pub fn handle_message(input: &ReceiveInput) -> Response {
    if input.from == Some(Actor::native_balance(input.receiver.account_id)) {
        let reply: StateReply =
            borsh::from_slice(&input.message).expect("the native balance answers a StateReply");
        return rotate::on_balance(input, &reply);
    }
    if input.from_own_program() {
        let message: VaultMessage =
            borsh::from_slice(&input.message).expect("vault message must decode");
        return match message {
            VaultMessage::Seat(message) => to_vault(input, message),
            VaultMessage::Adopt(config) => rotate::adopt(input, config),
        };
    }
    assert!(
        input.is_authorized && input.from.is_none(),
        "only the seat's owner calls, by a transaction they sign"
    );
    let message: Message = borsh::from_slice(&input.message).expect("seat message must decode");
    seat::handle(input, message)
}

/// At the vault: a seat's message.
fn to_vault(input: &ReceiveInput, message: ToVault) -> Response {
    if let ToVault::Create {
        member,
        placement,
        create_key,
        members,
        threshold,
    } = message
    {
        return create::create(input, member, placement, create_key, members, threshold);
    }
    let state = VaultState::decode(&input.pre_state);
    assert!(
        state.pending.is_none(),
        "the vault is waiting for its balance"
    );
    match message {
        ToVault::Create { .. } => unreachable!("handled above"),
        ToVault::Propose { member, action } => {
            propose::propose(input, active(state, member), member, action)
        }
        ToVault::Approve { member, id } => {
            propose::approve(input, active(state, member), member, id)
        }
        ToVault::Sweep { member } => rotate::sweep(input, member_of(state, member)),
    }
}

/// The state of a vault that hasn't retired, checked that `member` is one of its members.
fn active(state: VaultState, member: lee_core::account::AccountId) -> VaultState {
    assert!(
        state.retired.is_none(),
        "this vault is retired: it only forwards funds to its successor"
    );
    member_of(state, member)
}

fn member_of(state: VaultState, member: lee_core::account::AccountId) -> VaultState {
    assert!(state.is_member(member), "not a member of this vault");
    state
}

/// The seat's state.
fn seat_state(input: &ReceiveInput) -> SeatState {
    SeatState::decode(&input.pre_state)
}

/// Whether the vault's placement is hidden.
const fn is_hidden(state: &VaultState) -> bool {
    matches!(state.placement, Placement::Hidden)
}
