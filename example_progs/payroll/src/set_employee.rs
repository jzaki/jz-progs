//! `SetEmployee`: add an employee, or change their pay per period. The seat forwards it; this is
//! the treasury's step.

use lee_core::{
    account::AccountId,
    program::{ReceiveInput, Response},
};
use payroll_core::Employee;

use crate::treasury_state;

/// At the treasury. An existing employee keeps what they are owed and were paid: setting their pay
/// to 0 stops future accrual, not past pay.
pub(crate) fn configure(input: &ReceiveInput, employee: AccountId, amount: u128) -> Response {
    let mut state = treasury_state(input);
    match state.employees.iter_mut().find(|e| e.account == employee) {
        Some(existing) => existing.amount = amount,
        None => state.employees.push(Employee {
            account: employee,
            amount,
            owed: 0,
            paid: 0,
        }),
    }
    Response::set_state(state.encode())
}

#[cfg(test)]
mod tests {
    use crate::test_utils::*;

    #[test]
    fn the_seat_forwards_the_setting_to_its_treasury() {
        let set = Message::SetEmployee {
            employee: ALICE,
            amount: 100,
        };
        assert_eq!(
            signed_to_seat(seat_state(), &set),
            Response::keep_state().send(forwarded(&Internal::Configure {
                employee: ALICE,
                amount: 100,
            }))
        );
    }

    #[test]
    fn a_new_employee_starts_owed_nothing() {
        let response = granted_to_treasury(
            &treasury_with(None, vec![employee(ALICE, 100, 0, 0)]),
            &Internal::Configure {
                employee: BOB,
                amount: 250,
            },
        );
        assert_eq!(
            response,
            write(&treasury_with(
                None,
                vec![employee(ALICE, 100, 0, 0), employee(BOB, 250, 0, 0)]
            ))
        );
    }

    #[test]
    fn changing_pay_keeps_what_is_owed_and_paid() {
        let response = granted_to_treasury(
            &treasury_with(Some(2), vec![employee(ALICE, 100, 300, 500)]),
            &Internal::Configure {
                employee: ALICE,
                amount: 0,
            },
        );
        assert_eq!(
            response,
            write(&treasury_with(Some(2), vec![employee(ALICE, 0, 300, 500)]))
        );
    }

    #[test]
    #[should_panic(expected = "this seat hasn't opened a payroll")]
    fn a_seat_without_a_payroll_has_nowhere_to_forward() {
        let _response = signed_to_seat(
            Vec::new(),
            &Message::SetEmployee {
                employee: ALICE,
                amount: 100,
            },
        );
    }

    #[test]
    #[should_panic(expected = "the treasury acts only on its seat's seed grant")]
    fn a_setting_without_the_seed_grant_is_refused() {
        let _response = run(
            TREASURY,
            Some(payroll(SEAT_ALIAS)),
            false,
            treasury_with(None, Vec::new()).encode(),
            &Internal::Configure {
                employee: ALICE,
                amount: 1_000_000,
            },
        );
    }
}
