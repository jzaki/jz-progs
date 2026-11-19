//! `Open`: the seat records its treasury, once, and the treasury writes its initial state, once.

use lee_core::{
    account::AccountId,
    program::{write_once, PdaSeed, ReceiveInput, Response},
};
use payroll_core::{Internal, PayrollState, SeatState};

use crate::{seed_of, treasury_message};

/// At the seat.
pub(crate) fn open(input: &ReceiveInput, treasury: AccountId, period_ms: u64) -> Response {
    assert!(period_ms > 0, "a pay period lasts at least 1 ms");
    let seat = SeatState { treasury }.encode();
    let init = Internal::Init {
        seed: seed_of(input.receiver.account_id),
        period_ms,
    };
    Response::set_state(write_once(&input.pre_state, seat))
        .send(treasury_message(input, treasury, &init))
}

/// At the treasury.
///
/// The seed grant already proves the treasury is this program's PDA of `seed`. What it can't
/// prove is that the PDA is *private*: the public PDA of the same seed is granted the same way,
/// and its state and payments would be public. So that address is refused. A private PDA's
/// address hashes the keys in as well, so it never equals the public one.
pub(crate) fn init(input: &ReceiveInput, seed: PdaSeed, period_ms: u64) -> Response {
    assert_ne!(
        input.receiver.account_id,
        AccountId::for_public_pda(&input.receiver.program_account_id, &seed),
        "the treasury must be a private PDA, or its amounts would be public"
    );
    let state = PayrollState::new(seed, period_ms).encode();
    Response::set_state(write_once(&input.pre_state, state))
}

#[cfg(test)]
mod tests {
    use crate::test_utils::*;

    #[test]
    fn opening_records_the_treasury_and_initialises_it() {
        let open = signed_to_seat(
            Vec::new(),
            &Message::Open {
                treasury: TREASURY,
                period_ms: PERIOD_MS,
            },
        );
        assert_eq!(
            open,
            Response::set_state(seat_state()).send(forwarded(&Internal::Init {
                seed: seed_of(SEAT),
                period_ms: PERIOD_MS,
            }))
        );

        let init = run(
            TREASURY,
            Some(payroll(SEAT_ALIAS)),
            true,
            Vec::new(),
            &Internal::Init {
                seed: seed_of(SEAT),
                period_ms: PERIOD_MS,
            },
        );
        assert_eq!(init, write(&treasury_with(None, Vec::new())));
    }

    #[test]
    #[should_panic(expected = "only the seat's owner calls")]
    fn an_unsigned_open_is_refused() {
        let _response = run(
            SEAT,
            None,
            false,
            Vec::new(),
            &Message::Open {
                treasury: TREASURY,
                period_ms: PERIOD_MS,
            },
        );
    }

    #[test]
    #[should_panic(expected = "only the seat's owner calls")]
    fn an_open_relayed_by_another_program_is_refused() {
        let _response = run(
            SEAT,
            Some(Actor::new(ALICE, OTHER_PROGRAM)),
            true,
            Vec::new(),
            &Message::Open {
                treasury: TREASURY,
                period_ms: PERIOD_MS,
            },
        );
    }

    #[test]
    #[should_panic(expected = "actor state already holds different data")]
    fn a_seat_opens_only_one_treasury() {
        let _response = signed_to_seat(
            seat_state(),
            &Message::Open {
                treasury: BOB,
                period_ms: PERIOD_MS,
            },
        );
    }

    #[test]
    #[should_panic(expected = "the treasury acts only on its seat's seed grant")]
    fn an_init_without_the_seed_grant_is_refused() {
        let _response = run(
            TREASURY,
            Some(payroll(SEAT_ALIAS)),
            false,
            Vec::new(),
            &Internal::Init {
                seed: seed_of(SEAT),
                period_ms: PERIOD_MS,
            },
        );
    }

    #[test]
    #[should_panic(expected = "the treasury must be a private PDA")]
    fn the_public_pda_of_the_seed_is_refused_as_a_treasury() {
        let _response = run(
            AccountId::for_public_pda(&PAYROLL, &seed_of(SEAT)),
            Some(payroll(SEAT_ALIAS)),
            true,
            Vec::new(),
            &Internal::Init {
                seed: seed_of(SEAT),
                period_ms: PERIOD_MS,
            },
        );
    }

    #[test]
    #[should_panic(expected = "actor state already holds different data")]
    fn the_treasury_is_initialised_once() {
        let _response = granted_to_treasury(
            &treasury_with(Some(3), vec![employee(ALICE, 10, 10, 0)]),
            &Internal::Init {
                seed: seed_of(SEAT),
                period_ms: PERIOD_MS,
            },
        );
    }
}
