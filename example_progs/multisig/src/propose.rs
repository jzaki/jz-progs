//! `Propose` and `Approve`: the vault stores a proposal with its approvals, and the approval that
//! meets the threshold runs the action in the same transaction.
//!
//! A transfer that fails (an insufficient balance) reverts the whole transaction, approval
//! included, so the proposal stays pending with the approvals it had.

use lee_core::{
    account::AccountId,
    native_token::custody_transfer,
    program::{ReceiveInput, Response},
};
use multisig_core::{Action, Proposal, VaultState, MAX_PENDING};

use crate::members;

/// At the vault: `member` proposes `action`, approving it.
pub(crate) fn propose(
    input: &ReceiveInput,
    mut state: VaultState,
    member: AccountId,
    action: Action,
) -> Response {
    assert!(
        state.proposals.len() < MAX_PENDING,
        "the vault already holds {MAX_PENDING} pending proposals"
    );
    let id = state.next_id;
    state.next_id = id.checked_add(1).expect("proposal ids overflow");
    if state.config.threshold <= 1 {
        return execute(input, state, action);
    }
    state.proposals.push(Proposal {
        id,
        action,
        approvals: vec![member],
    });
    Response::set_state(state.encode())
}

/// At the vault: `member` approves proposal `id`.
pub(crate) fn approve(
    input: &ReceiveInput,
    mut state: VaultState,
    member: AccountId,
    id: u64,
) -> Response {
    let threshold = usize::from(state.config.threshold);
    let index = state
        .proposals
        .iter()
        .position(|p| p.id == id)
        .expect("no such pending proposal");
    let proposal = state
        .proposals
        .get_mut(index)
        .expect("the index was just found");
    assert!(
        !proposal.approvals.contains(&member),
        "this member already approved the proposal"
    );
    proposal.approvals.push(member);
    if proposal.approvals.len() < threshold {
        return Response::set_state(state.encode());
    }
    let proposal = state.proposals.remove(index);
    execute(input, state, proposal.action)
}

/// At the vault: run an approved `action`.
fn execute(input: &ReceiveInput, state: VaultState, action: Action) -> Response {
    match action {
        Action::Transfer { to, amount } => {
            assert!(amount > 0, "a transfer moves at least 1");
            let transfer = custody_transfer(input.receiver.account_id, state.seed, to, amount);
            Response::set_state(state.encode()).send(transfer)
        }
        Action::AddMember {
            member,
            threshold,
            successor,
        } => members::add(input, state, member, threshold, successor),
        Action::RemoveMember {
            member,
            threshold,
            successor,
        } => members::remove(input, state, member, threshold, successor),
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::*;

    fn pay_carol() -> Action {
        Action::Transfer {
            to: CAROL_SEAT,
            amount: 300,
        }
    }

    #[test]
    fn a_proposal_is_stored_with_the_proposers_approval() {
        let mut expected = vault3();
        expected.next_id = 1;
        expected.proposals = vec![proposal(0, pay_carol(), vec![alice()])];
        assert_eq!(
            seat_to_vault(
                &vault3(),
                ToVault::Propose {
                    member: alice(),
                    action: pay_carol()
                }
            ),
            Response::set_state(expected.encode())
        );
    }

    #[test]
    fn the_approval_that_meets_the_threshold_pays() {
        let mut pre = vault3();
        pre.next_id = 1;
        pre.proposals = vec![proposal(0, pay_carol(), vec![alice()])];
        let mut post = pre.clone();
        post.proposals.clear();
        assert_eq!(
            seat_to_vault(
                &pre,
                ToVault::Approve {
                    member: bob(),
                    id: 0
                }
            ),
            Response::set_state(post.encode()).send(native_token::custody_transfer(
                VAULT,
                seed_for(CREATE_KEY, 0),
                CAROL_SEAT,
                300
            ))
        );
    }

    #[test]
    fn an_approval_below_the_threshold_is_stored() {
        let mut pre = vault3();
        pre.config.threshold = 3;
        pre.next_id = 1;
        pre.proposals = vec![proposal(0, pay_carol(), vec![alice()])];
        let mut post = pre.clone();
        post.proposals = vec![proposal(0, pay_carol(), vec![alice(), carol()])];
        assert_eq!(
            seat_to_vault(
                &pre,
                ToVault::Approve {
                    member: carol(),
                    id: 0
                }
            ),
            Response::set_state(post.encode())
        );
    }

    #[test]
    fn a_one_of_n_vault_acts_on_the_proposal() {
        let mut pre = vault3();
        pre.config.threshold = 1;
        let mut post = pre.clone();
        post.next_id = 1;
        assert_eq!(
            seat_to_vault(
                &pre,
                ToVault::Propose {
                    member: bob(),
                    action: pay_carol()
                }
            ),
            Response::set_state(post.encode()).send(native_token::custody_transfer(
                VAULT,
                seed_for(CREATE_KEY, 0),
                CAROL_SEAT,
                300
            ))
        );
    }

    #[test]
    #[should_panic(expected = "this member already approved the proposal")]
    fn a_member_approves_once() {
        let mut pre = vault3();
        pre.next_id = 1;
        pre.proposals = vec![proposal(0, pay_carol(), vec![alice()])];
        let _response = seat_to_vault(
            &pre,
            ToVault::Approve {
                member: alice(),
                id: 0,
            },
        );
    }

    #[test]
    #[should_panic(expected = "not a member of this vault")]
    fn a_stranger_cant_approve() {
        let mut pre = vault3();
        pre.next_id = 1;
        pre.proposals = vec![proposal(0, pay_carol(), vec![alice()])];
        let _response = seat_to_vault(
            &pre,
            ToVault::Approve {
                member: dave(),
                id: 0,
            },
        );
    }

    #[test]
    #[should_panic(expected = "no such pending proposal")]
    fn an_unknown_proposal_cant_be_approved() {
        let _response = seat_to_vault(
            &vault3(),
            ToVault::Approve {
                member: bob(),
                id: 7,
            },
        );
    }

    #[test]
    #[should_panic(expected = "only the seat's owner calls")]
    fn a_transaction_cant_speak_to_the_vault_directly() {
        // Not from this program, so read as a seat message, and the vault isn't a signer.
        let _response = run(
            VAULT,
            None,
            false,
            vault3().encode(),
            &VaultMessage::Seat(ToVault::Approve {
                member: bob(),
                id: 0,
            }),
        );
    }

    #[test]
    #[should_panic(expected = "only the seat's owner calls")]
    fn another_program_cant_speak_for_a_seat() {
        let _response = run(
            VAULT,
            Some(Actor::new(BOB_SEAT, OTHER_PROGRAM)),
            false,
            vault3().encode(),
            &VaultMessage::Seat(ToVault::Approve {
                member: bob(),
                id: 0,
            }),
        );
    }
}
