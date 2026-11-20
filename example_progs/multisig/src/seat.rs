//! The seat: the program's actor on a member's own account. Its owner signs a transaction to it;
//! it forwards their word to a vault under their pseudonym.
//!
//! This is the only code that sends a vault a [`ToVault`], and it only ever names its own
//! pseudonym. That is what lets the vault trust the name (see the crate docs).

use lee_core::{
    account::{AccountId, Actor},
    program::{write_once, ReceiveInput, Response, SyncMessage},
};
use multisig_core::{pseudonym, seed_for, Message, SeatState, ToVault, VaultMessage};

use crate::seat_state;

/// At the seat: `input` is a transaction its owner signed.
pub(crate) fn handle(input: &ReceiveInput, message: Message) -> Response {
    match message {
        Message::InitSeat { secret } => {
            let state = SeatState { secret }.encode();
            Response::set_state(write_once(&input.pre_state, state))
        }
        Message::Create {
            vault,
            placement,
            create_key,
            members,
            threshold,
        } => {
            let message = ToVault::Create {
                member: me(input, create_key),
                placement,
                create_key,
                members,
                threshold,
            };
            // The grant proves to the vault that it is this program's PDA of generation 0's seed.
            Response::keep_state()
                .send(to(input, vault, message).with_pda_seeds([seed_for(create_key, 0)]))
        }
        Message::Propose {
            vault,
            create_key,
            action,
        } => {
            let member = me(input, create_key);
            Response::keep_state().send(to(input, vault, ToVault::Propose { member, action }))
        }
        Message::Approve {
            vault,
            create_key,
            id,
        } => {
            let member = me(input, create_key);
            Response::keep_state().send(to(input, vault, ToVault::Approve { member, id }))
        }
        Message::Sweep { vault, create_key } => {
            let member = me(input, create_key);
            Response::keep_state().send(to(input, vault, ToVault::Sweep { member }))
        }
    }
}

/// This seat's pseudonym at the vaults of `create_key`.
fn me(input: &ReceiveInput, create_key: [u8; 32]) -> AccountId {
    let secret = seat_state(input).secret;
    pseudonym(input.receiver.account_id, secret, create_key)
}

/// `message` to `vault`'s program actor.
fn to(input: &ReceiveInput, vault: AccountId, message: ToVault) -> SyncMessage {
    SyncMessage::new(
        Actor::new(vault, input.receiver.program_account_id),
        &VaultMessage::Seat(message),
    )
}

#[cfg(test)]
mod tests {
    use crate::test_utils::*;

    #[test]
    fn init_writes_the_secret_once() {
        assert_eq!(
            signed_to_seat(ALICE_SEAT, Vec::new(), &Message::InitSeat { secret: ALICE_SECRET }),
            Response::set_state(seat(ALICE_SECRET))
        );
    }

    #[test]
    #[should_panic(expected = "actor state already holds different data")]
    fn the_secret_cant_be_replaced() {
        let _response = signed_to_seat(
            ALICE_SEAT,
            seat(ALICE_SECRET),
            &Message::InitSeat { secret: [9; 32] },
        );
    }

    #[test]
    fn the_seat_proposes_under_its_own_pseudonym() {
        let action = Action::Transfer {
            to: CAROL_SEAT,
            amount: 5,
        };
        assert_eq!(
            signed_to_seat(
                ALICE_SEAT,
                seat(ALICE_SECRET),
                &Message::Propose {
                    vault: VAULT,
                    create_key: CREATE_KEY,
                    action: action.clone()
                }
            ),
            Response::keep_state().send(from_seat(ToVault::Propose {
                member: alice(),
                action
            }))
        );
    }

    #[test]
    fn creating_grants_the_vaults_seed() {
        let response = signed_to_seat(
            ALICE_SEAT,
            seat(ALICE_SECRET),
            &Message::Create {
                vault: VAULT,
                placement: Placement::Hidden,
                create_key: CREATE_KEY,
                members: members3(),
                threshold: 2,
            },
        );
        assert_eq!(
            response,
            Response::keep_state().send(
                from_seat(ToVault::Create {
                    member: alice(),
                    placement: Placement::Hidden,
                    create_key: CREATE_KEY,
                    members: members3(),
                    threshold: 2,
                })
                .with_pda_seeds([seed_for(CREATE_KEY, 0)])
            )
        );
    }

    #[test]
    fn a_pseudonym_depends_on_the_secret_and_the_vaults_create_key() {
        assert_ne!(alice(), pseudonym(ALICE_SEAT, [9; 32], CREATE_KEY));
        assert_ne!(alice(), pseudonym(ALICE_SEAT, ALICE_SECRET, [9; 32]));
        assert_ne!(alice(), ALICE_SEAT);
    }

    #[test]
    #[should_panic(expected = "this seat isn't initialised")]
    fn an_uninitialised_seat_cant_speak() {
        let _response = signed_to_seat(
            ALICE_SEAT,
            Vec::new(),
            &Message::Approve {
                vault: VAULT,
                create_key: CREATE_KEY,
                id: 0,
            },
        );
    }

    #[test]
    #[should_panic(expected = "only the seat's owner calls")]
    fn an_unsigned_transaction_cant_use_a_seat() {
        let _response = run(
            ALICE_SEAT,
            None,
            false,
            seat(ALICE_SECRET),
            &Message::Approve {
                vault: VAULT,
                create_key: CREATE_KEY,
                id: 0,
            },
        );
    }

    #[test]
    #[should_panic(expected = "only the seat's owner calls")]
    fn another_program_cant_use_a_seat() {
        let _response = run(
            ALICE_SEAT,
            Some(Actor::new(BOB_SEAT, OTHER_PROGRAM)),
            true,
            seat(ALICE_SECRET),
            &Message::Approve {
                vault: VAULT,
                create_key: CREATE_KEY,
                id: 0,
            },
        );
    }
}
