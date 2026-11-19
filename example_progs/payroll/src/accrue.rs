//! `Accrue`: add each period's pay to what every employee is owed. The seat forwards it; this is
//! the treasury's step. No funds move, so nothing can be stranded.

use lee_core::program::{ReceiveInput, Response};

use crate::treasury_state;

/// At the treasury: accrue every period after the last one accrued, up to and including `period`.
/// The first accrual counts one period.
///
/// The response's timestamp window opens at `period`'s start, so the engine refuses the
/// transaction in any block before it. The program trusts `period` only because of that window.
pub(crate) fn accrue_to(input: &ReceiveInput, period: u64) -> Response {
    let mut state = treasury_state(input);
    let periods = match state.last_accrued {
        None => 1,
        Some(last) => {
            assert!(
                period > last,
                "period {period} is already accrued (the last is {last})"
            );
            period.checked_sub(last).expect("period > last")
        }
    };
    for employee in &mut state.employees {
        let pay = employee
            .amount
            .checked_mul(u128::from(periods))
            .expect("pay overflows");
        employee.owed = employee.owed.checked_add(pay).expect("owed overflows");
        state.total_owed = state
            .total_owed
            .checked_add(pay)
            .expect("total owed overflows");
    }
    state.last_accrued = Some(period);
    let start = period
        .checked_mul(state.period_ms)
        .expect("the period's start fits in u64 ms");
    Response::set_state(state.encode()).timestamp_window(start..)
}

#[cfg(test)]
mod tests {
    use crate::test_utils::*;

    #[test]
    fn the_seat_forwards_the_accrual_to_its_treasury() {
        assert_eq!(
            signed_to_seat(seat_state(), &Message::Accrue { period: 7 }),
            Response::keep_state().send(forwarded(&Internal::AccrueTo { period: 7 }))
        );
    }

    #[test]
    fn the_first_accrual_counts_one_period_and_waits_for_its_start() {
        let response = granted_to_treasury(
            &treasury_with(
                None,
                vec![employee(ALICE, 100, 0, 0), employee(BOB, 250, 0, 0)],
            ),
            &Internal::AccrueTo { period: 7 },
        );
        assert_eq!(
            response,
            write(&treasury_with(
                Some(7),
                vec![employee(ALICE, 100, 100, 0), employee(BOB, 250, 250, 0)]
            ))
            .timestamp_window(7 * PERIOD_MS..)
        );
    }

    #[test]
    fn a_later_accrual_counts_every_period_since_the_last() {
        let response = granted_to_treasury(
            &treasury_with(
                Some(7),
                vec![employee(ALICE, 100, 100, 0), employee(BOB, 0, 40, 60)],
            ),
            &Internal::AccrueTo { period: 10 },
        );
        assert_eq!(
            response,
            write(&treasury_with(
                Some(10),
                vec![employee(ALICE, 100, 400, 0), employee(BOB, 0, 40, 60)]
            ))
            .timestamp_window(10 * PERIOD_MS..)
        );
    }

    #[test]
    #[should_panic(expected = "period 7 is already accrued")]
    fn a_period_is_accrued_once() {
        let _response = granted_to_treasury(
            &treasury_with(Some(7), vec![employee(ALICE, 100, 100, 0)]),
            &Internal::AccrueTo { period: 7 },
        );
    }

    #[test]
    #[should_panic(expected = "period 6 is already accrued")]
    fn an_earlier_period_is_refused() {
        let _response = granted_to_treasury(
            &treasury_with(Some(7), vec![employee(ALICE, 100, 100, 0)]),
            &Internal::AccrueTo { period: 6 },
        );
    }

    #[test]
    #[should_panic(expected = "the payroll isn't open")]
    fn an_unopened_treasury_accrues_nothing() {
        let _response = run(
            TREASURY,
            Some(payroll(SEAT_ALIAS)),
            true,
            Vec::new(),
            &Internal::AccrueTo { period: 7 },
        );
    }
}
