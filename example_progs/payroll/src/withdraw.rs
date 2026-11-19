//! `Withdraw`: return funds nobody is owed to the seat. The seat forwards it as `Recover`.
//!
//! The treasury doesn't know its own balance: native funds are the native token program's state.
//! So it asks: it stores the withdrawal, sends `ReadState` to its own native balance, and decides
//! on the reply, which comes back synchronously in the same transaction (the `live_payout`
//! pattern). It pays out only `balance - total_owed`, so a withdrawal can never take an employee's
//! pay.

use lee_core::{
    account::{AccountId, Actor},
    native_token::{self, custody_transfer, decode_balance},
    program::{ReceiveInput, Response, StateReply},
};
use payroll_core::Withdrawal;

use crate::treasury_state;

/// At the treasury: hold the withdrawal and read the balance.
pub(crate) fn recover(input: &ReceiveInput, amount: u128, to: AccountId) -> Response {
    let mut state = treasury_state(input);
    assert!(
        state.pending_withdraw.is_none(),
        "a withdrawal is already waiting for the balance"
    );
    state.pending_withdraw = Some(Withdrawal { amount, to });
    Response::set_state(state.encode()).send_sync(
        Actor::native_balance(input.receiver.account_id),
        &native_token::Message::ReadState,
    )
}

/// At the treasury: its native balance's answer. Only the treasury's own native balance reaches
/// here (`handle_message` dispatches on the sender).
pub(crate) fn on_balance(input: &ReceiveInput, reply: &StateReply) -> Response {
    let mut state = treasury_state(input);
    let Withdrawal { amount, to } = state
        .pending_withdraw
        .take()
        .expect("no withdrawal is waiting for the balance");
    let balance = decode_balance(&reply.state).expect("a native balance decodes");
    let free = balance.saturating_sub(state.total_owed);
    assert!(
        amount <= free,
        "withdrawal of {amount} exceeds the {free} nobody is owed"
    );
    let transfer = custody_transfer(input.receiver.account_id, state.seed, to, amount);
    Response::set_state(state.encode()).send(transfer)
}

#[cfg(test)]
mod tests {
    use crate::test_utils::*;

    fn waiting(amount: u128) -> PayrollState {
        let mut state = treasury_with(Some(3), vec![employee(ALICE, 100, 300, 0)]);
        state.pending_withdraw = Some(Withdrawal { amount, to: SEAT });
        state
    }

    /// The treasury's native balance answering `ReadState` with `balance`.
    fn balance_reply(pre: &PayrollState, balance: u128) -> Response {
        run(
            TREASURY,
            Some(Actor::native_balance(TREASURY)),
            false,
            pre.encode(),
            &StateReply {
                state: native_token::encode_balance(balance),
            },
        )
    }

    #[test]
    fn the_seat_withdraws_to_itself() {
        assert_eq!(
            signed_to_seat(seat_state(), &Message::Withdraw { amount: 40 }),
            Response::keep_state().send(forwarded(&Internal::Recover {
                amount: 40,
                to: SEAT
            }))
        );
    }

    #[test]
    fn recovering_first_reads_the_treasury_balance() {
        let response = granted_to_treasury(
            &treasury_with(Some(3), vec![employee(ALICE, 100, 300, 0)]),
            &Internal::Recover {
                amount: 40,
                to: SEAT,
            },
        );
        assert_eq!(
            response,
            write(&waiting(40)).send_sync(
                Actor::native_balance(TREASURY),
                &native_token::Message::ReadState
            )
        );
    }

    #[test]
    fn what_nobody_is_owed_is_paid_out() {
        let response = balance_reply(&waiting(200), 500);
        assert_eq!(
            response,
            write(&treasury_with(Some(3), vec![employee(ALICE, 100, 300, 0)])).send(
                native_token::custody_transfer(TREASURY, seed_of(SEAT), SEAT, 200)
            )
        );
    }

    #[test]
    #[should_panic(expected = "withdrawal of 201 exceeds the 200 nobody is owed")]
    fn what_employees_are_owed_stays() {
        let _response = balance_reply(&waiting(201), 500);
    }

    #[test]
    #[should_panic(expected = "withdrawal of 1 exceeds the 0 nobody is owed")]
    fn an_underfunded_treasury_pays_out_nothing() {
        let _response = balance_reply(&waiting(1), 100);
    }

    #[test]
    #[should_panic(expected = "no withdrawal is waiting for the balance")]
    fn a_balance_nobody_asked_for_is_refused() {
        let _response = balance_reply(
            &treasury_with(Some(3), vec![employee(ALICE, 100, 300, 0)]),
            1_000,
        );
    }

    #[test]
    #[should_panic(expected = "only the seat's owner calls")]
    fn a_balance_from_another_account_is_not_a_reply() {
        // Another account's native balance isn't the treasury's, so the message is decoded as a
        // seat message, from a sender: refused.
        let _response = run(
            TREASURY,
            Some(Actor::native_balance(ALICE)),
            false,
            waiting(200).encode(),
            &StateReply {
                state: native_token::encode_balance(1_000_000),
            },
        );
    }
}
