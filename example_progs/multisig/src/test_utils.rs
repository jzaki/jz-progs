//! Fixtures for the handler tests: one hand-built `ReceiveInput` per message, compared against
//! the exact expected `Response`. The conversation on a chain is covered by
//! `multisig.sh`, over spel's private calls.

pub(crate) use borsh::BorshSerialize;
pub(crate) use lee_core::{
    account::{AccountId, Actor, ActorState},
    native_token,
    program::{ReceiveInput, Response, StateReply, SyncMessage},
};
pub(crate) use multisig_core::{
    pseudonym, seed_for, Action, Config, Message, Pending, Placement, Proposal, SeatState,
    ToVault, VaultMessage, VaultState,
};

pub(crate) use crate::handle_message;

pub(crate) const MULTISIG: AccountId = AccountId::new([0xAA; 32]);
pub(crate) const OTHER_PROGRAM: AccountId = AccountId::new([0xBB; 32]);

pub(crate) const CREATE_KEY: [u8; 32] = [7; 32];
pub(crate) const VAULT: AccountId = AccountId::new([2; 32]);
pub(crate) const SUCCESSOR: AccountId = AccountId::new([3; 32]);
/// The blinded alias a private seat or vault reaches another actor under.
pub(crate) const SEAT_ALIAS: AccountId = AccountId::new([5; 32]);
pub(crate) const VAULT_ALIAS: AccountId = AccountId::new([6; 32]);

pub(crate) const ALICE_SEAT: AccountId = AccountId::new([11; 32]);
pub(crate) const BOB_SEAT: AccountId = AccountId::new([12; 32]);
pub(crate) const CAROL_SEAT: AccountId = AccountId::new([13; 32]);
pub(crate) const DAVE_SEAT: AccountId = AccountId::new([14; 32]);
pub(crate) const ALICE_SECRET: [u8; 32] = [21; 32];

pub(crate) fn alice() -> AccountId {
    pseudonym(ALICE_SEAT, ALICE_SECRET, CREATE_KEY)
}
pub(crate) fn bob() -> AccountId {
    pseudonym(BOB_SEAT, [22; 32], CREATE_KEY)
}
pub(crate) fn carol() -> AccountId {
    pseudonym(CAROL_SEAT, [23; 32], CREATE_KEY)
}
pub(crate) fn dave() -> AccountId {
    pseudonym(DAVE_SEAT, [24; 32], CREATE_KEY)
}
pub(crate) fn members3() -> Vec<AccountId> {
    vec![alice(), bob(), carol()]
}

pub(crate) fn public_vault() -> AccountId {
    AccountId::for_public_pda(&MULTISIG, &seed_for(CREATE_KEY, 0))
}

pub(crate) fn run(
    receiver: AccountId,
    from: Option<Actor>,
    is_authorized: bool,
    pre: Vec<u8>,
    message: &impl BorshSerialize,
) -> Response {
    handle_message(&ReceiveInput {
        receiver: Actor::new(receiver, MULTISIG),
        from,
        is_authorized,
        pre_state: ActorState::from(pre),
        message: borsh::to_vec(message).expect("test message encodes"),
    })
}

/// A transaction `seat`'s owner signed, to their seat.
pub(crate) fn signed_to_seat(seat: AccountId, pre: Vec<u8>, message: &Message) -> Response {
    run(seat, None, true, pre, message)
}

/// A seat's message at `vault`, from its blinded alias. `granted`: whether it granted the
/// vault's seed.
pub(crate) fn to_vault_at(
    vault: AccountId,
    granted: bool,
    pre: Vec<u8>,
    message: ToVault,
) -> Response {
    run(
        vault,
        Some(Actor::new(SEAT_ALIAS, MULTISIG)),
        granted,
        pre,
        &VaultMessage::Seat(message),
    )
}

/// A seat's message at the hidden vault `VAULT`, in state `pre`.
pub(crate) fn seat_to_vault(pre: &VaultState, message: ToVault) -> Response {
    to_vault_at(VAULT, false, pre.encode(), message)
}

/// The message a seat sends `VAULT`.
pub(crate) fn from_seat(message: ToVault) -> SyncMessage {
    SyncMessage::new(Actor::new(VAULT, MULTISIG), &VaultMessage::Seat(message))
}

pub(crate) fn seat(secret: [u8; 32]) -> Vec<u8> {
    SeatState { secret }.encode()
}

/// A fresh hidden 2-of-3 vault: alice, bob and carol.
pub(crate) fn vault3() -> VaultState {
    VaultState::new(
        Placement::Hidden,
        Config {
            create_key: CREATE_KEY,
            generation: 0,
            members: members3(),
            threshold: 2,
        },
    )
}

pub(crate) fn proposal(id: u64, action: Action, approvals: Vec<AccountId>) -> Proposal {
    Proposal {
        id,
        action,
        approvals,
    }
}
