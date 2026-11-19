//! `Pay`: pay one employee everything they are owed. The seat forwards it; this is the treasury's
//! step.
//!
//! The bookkeeping and the payment are one transaction: `owed` moves to `paid` in the same response
//! that sends the custody transfer. The transfer debits the treasury synchronously, and credits the
//! employee by an async message, which is stored sealed to them. If anything fails before the
//! credit is stored (an insufficient balance, a seal the wallet can't build), the transaction
//! reverts. The funds stay in the treasury and the pay stays owed, so a retry pays it. Paying one
//! employee per transaction keeps one failure from holding up the others.

use lee_core::{
    account::AccountId,
    native_token::custody_transfer,
    program::{ReceiveInput, Response},
};

use crate::treasury_state;

/// At the treasury.
pub(crate) fn release(input: &ReceiveInput, employee: AccountId) -> Response {
    let mut state = treasury_state(input);
    let entry = state
        .employees
        .iter_mut()
        .find(|e| e.account == employee)
        .expect("not an employee of this payroll");
    let owed = entry.owed;
    assert!(owed > 0, "nothing is owed to this employee");
    entry.owed = 0;
    entry.paid = entry.paid.checked_add(owed).expect("paid overflows");
    state.total_owed = state
        .total_owed
        .checked_sub(owed)
        .expect("total owed covers each employee's");
    state.total_paid = state
        .total_paid
        .checked_add(owed)
        .expect("total paid overflows");
    let transfer = custody_transfer(input.receiver.account_id, state.seed, employee, owed);
    Response::set_state(state.encode()).send(transfer)
}

#[cfg(test)]
mod tests {
    use crate::test_utils::*;

    #[test]
    fn the_seat_forwards_the_payment_to_its_treasury() {
        assert_eq!(
            signed_to_seat(seat_state(), &Message::Pay { employee: ALICE }),
            Response::keep_state().send(forwarded(&Internal::Release { employee: ALICE }))
        );
    }

    #[test]
    fn paying_moves_owed_to_paid_and_transfers_it_from_the_treasury() {
        let response = granted_to_treasury(
            &treasury_with(
                Some(3),
                vec![employee(ALICE, 100, 300, 50), employee(BOB, 250, 500, 0)],
            ),
            &Internal::Release { employee: ALICE },
        );
        assert_eq!(
            response,
            write(&treasury_with(
                Some(3),
                vec![employee(ALICE, 100, 0, 350), employee(BOB, 250, 500, 0)]
            ))
            .send(native_token::custody_transfer(
                TREASURY,
                seed_of(SEAT),
                ALICE,
                300
            ))
        );
    }

    #[test]
    fn the_transfer_debits_the_treasury_and_credits_the_employee_asynchronously() {
        // What the custody transfer does at the treasury's native balance, granted by its seed.
        let transition = native_token::handle_message(&ReceiveInput {
            receiver: Actor::native_balance(TREASURY),
            from: Some(payroll(TREASURY)),
            is_authorized: true,
            pre_state: native_token::encode_balance(1_000),
            message: borsh::to_vec(&native_token::Message::Transfer {
                to: ALICE,
                amount: 300,
            })
            .expect("encodes"),
        })
        .expect("the transfer succeeds");
        assert_eq!(
            transition.response.post_state,
            Some(native_token::encode_balance(700))
        );
        assert_eq!(
            transition.response.async_messages,
            [AsyncMessage::new(
                Actor::native_balance(ALICE),
                &native_token::Message::Credit(300)
            )]
        );
    }

    #[test]
    #[should_panic(expected = "nothing is owed to this employee")]
    fn paying_with_nothing_owed_is_refused() {
        let _response = granted_to_treasury(
            &treasury_with(Some(3), vec![employee(ALICE, 100, 0, 300)]),
            &Internal::Release { employee: ALICE },
        );
    }

    #[test]
    #[should_panic(expected = "not an employee of this payroll")]
    fn paying_a_stranger_is_refused() {
        let _response = granted_to_treasury(
            &treasury_with(Some(3), vec![employee(ALICE, 100, 100, 0)]),
            &Internal::Release { employee: BOB },
        );
    }

    #[test]
    #[should_panic(expected = "the treasury acts only on its seat's seed grant")]
    fn a_release_without_the_seed_grant_is_refused() {
        let _response = run(
            TREASURY,
            Some(payroll(BOB)),
            false,
            treasury_with(Some(3), vec![employee(BOB, 100, 100, 0)]).encode(),
            &Internal::Release { employee: BOB },
        );
    }

    #[test]
    #[should_panic(expected = "only the seat's owner calls")]
    fn a_release_from_another_program_is_a_seat_message_and_unsigned() {
        // Not this program, so not decoded as `Internal` at all.
        let _response = run(
            TREASURY,
            Some(Actor::new(BOB, OTHER_PROGRAM)),
            false,
            treasury_with(Some(3), vec![employee(BOB, 100, 100, 0)]).encode(),
            &Internal::Release { employee: BOB },
        );
    }
}
