//! Fixtures for the handler tests: one hand-built `ReceiveInput` per message, compared against
//! the exact expected `Response`. The conversation on a chain is covered by
//! `jz-progs/payroll-runner`.

pub(crate) use borsh::BorshSerialize;
pub(crate) use lee_core::{
    account::{AccountId, Actor, ActorState},
    native_token,
    program::{AsyncMessage, ReceiveInput, Response, StateReply, SyncMessage},
};
pub(crate) use payroll_core::{Employee, Internal, Message, PayrollState, SeatState, Withdrawal};

pub(crate) use crate::{handle_message, seed_of};

pub(crate) const PAYROLL: AccountId = AccountId::new([0xAA; 32]);
pub(crate) const OTHER_PROGRAM: AccountId = AccountId::new([0xBB; 32]);

pub(crate) const SEAT: AccountId = AccountId::new([1; 32]);
pub(crate) const TREASURY: AccountId = AccountId::new([2; 32]);
pub(crate) const ALICE: AccountId = AccountId::new([3; 32]);
pub(crate) const BOB: AccountId = AccountId::new([4; 32]);
/// The blinded alias a private seat reaches the treasury under.
pub(crate) const SEAT_ALIAS: AccountId = AccountId::new([5; 32]);

pub(crate) const PERIOD_MS: u64 = 10_000;

pub(crate) fn payroll(account_id: AccountId) -> Actor {
    Actor::new(account_id, PAYROLL)
}

pub(crate) fn run(
    receiver: AccountId,
    from: Option<Actor>,
    is_authorized: bool,
    pre: Vec<u8>,
    message: &impl BorshSerialize,
) -> Response {
    handle_message(&ReceiveInput {
        receiver: payroll(receiver),
        from,
        is_authorized,
        pre_state: ActorState::from(pre),
        message: borsh::to_vec(message).expect("test message encodes"),
    })
}

/// A transaction the seat's owner signed, to the seat.
pub(crate) fn signed_to_seat(pre: Vec<u8>, message: &Message) -> Response {
    run(SEAT, None, true, pre, message)
}

/// The seat's seed-granted message, at the treasury.
pub(crate) fn granted_to_treasury(pre: &PayrollState, message: &Internal) -> Response {
    run(
        TREASURY,
        Some(payroll(SEAT_ALIAS)),
        true,
        pre.encode(),
        message,
    )
}

/// The seat's state once it opened `TREASURY`.
pub(crate) fn seat_state() -> Vec<u8> {
    SeatState { treasury: TREASURY }.encode()
}

/// The message the seat forwards to the treasury, with the treasury's seed.
pub(crate) fn forwarded(message: &Internal) -> SyncMessage {
    SyncMessage::new(payroll(TREASURY), message).with_pda_seeds([seed_of(SEAT)])
}

pub(crate) fn employee(account: AccountId, amount: u128, owed: u128, paid: u128) -> Employee {
    Employee {
        account,
        amount,
        owed,
        paid,
    }
}

/// A treasury with `employees`, whose owed and paid totals add up.
pub(crate) fn treasury_with(last_accrued: Option<u64>, employees: Vec<Employee>) -> PayrollState {
    let mut state = PayrollState::new(seed_of(SEAT), PERIOD_MS);
    state.last_accrued = last_accrued;
    state.total_owed = employees.iter().map(|e| e.owed).sum();
    state.total_paid = employees.iter().map(|e| e.paid).sum();
    state.employees = employees;
    state
}

pub(crate) fn write(state: &PayrollState) -> Response {
    Response::set_state(state.encode())
}
