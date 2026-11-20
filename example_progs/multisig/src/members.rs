//! `AddMember` and `RemoveMember`, once approved: in place, or by rotating a hidden vault.

use lee_core::{
    account::AccountId,
    program::{ReceiveInput, Response},
};
use multisig_core::{Config, VaultState};

use crate::{create::check_config, is_hidden, rotate};

pub(crate) fn add(
    input: &ReceiveInput,
    state: VaultState,
    member: AccountId,
    threshold: u8,
    successor: Option<AccountId>,
) -> Response {
    assert!(!state.is_member(member), "already a member of this vault");
    let mut members = state.config.members.clone();
    members.push(member);
    apply(input, state, members, threshold, successor)
}

pub(crate) fn remove(
    input: &ReceiveInput,
    state: VaultState,
    member: AccountId,
    threshold: u8,
    successor: Option<AccountId>,
) -> Response {
    assert!(state.is_member(member), "not a member of this vault");
    assert!(
        successor.is_some() || !is_hidden(&state),
        "a hidden vault removes a member by rotating: they hold its keys"
    );
    let mut members = state.config.members.clone();
    members.retain(|m| *m != member);
    apply(input, state, members, threshold, successor)
}

fn apply(
    input: &ReceiveInput,
    mut state: VaultState,
    members: Vec<AccountId>,
    threshold: u8,
    successor: Option<AccountId>,
) -> Response {
    check_config(&members, threshold);
    match successor {
        None => {
            // Approvals by a removed member no longer count.
            for proposal in &mut state.proposals {
                proposal.approvals.retain(|m| members.contains(m));
            }
            state.config.members = members;
            state.config.threshold = threshold;
            Response::set_state(state.encode())
        }
        Some(successor) => {
            assert!(is_hidden(&state), "a public vault changes members in place");
            assert_ne!(
                successor, input.receiver.account_id,
                "a vault can't succeed itself"
            );
            let config = Config {
                create_key: state.config.create_key,
                generation: state
                    .config
                    .generation
                    .checked_add(1)
                    .expect("generations overflow"),
                members,
                threshold,
            };
            rotate::begin(input, state, successor, config)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::*;

    /// `action` proposed by alice, at a 2-of-3 vault of `placement`.
    fn approve_by_bob(placement: Placement, action: Action) -> Response {
        let mut pre = vault3();
        pre.placement = placement;
        pre.next_id = 1;
        pre.proposals = vec![proposal(0, action, vec![alice()])];
        seat_to_vault(
            &pre,
            ToVault::Approve {
                member: bob(),
                id: 0,
            },
        )
    }

    fn approved(placement: Placement, members: Vec<AccountId>, threshold: u8) -> VaultState {
        let mut state = vault3();
        state.placement = placement;
        state.next_id = 1;
        state.config.members = members;
        state.config.threshold = threshold;
        state
    }

    #[test]
    fn a_member_is_added_in_place() {
        assert_eq!(
            approve_by_bob(
                Placement::Hidden,
                Action::AddMember {
                    member: dave(),
                    threshold: 3,
                    successor: None
                }
            ),
            Response::set_state(
                approved(
                    Placement::Hidden,
                    vec![alice(), bob(), carol(), dave()],
                    3
                )
                .encode()
            )
        );
    }

    #[test]
    fn a_public_vault_removes_in_place_and_drops_their_approvals() {
        let mut pre = vault3();
        pre.placement = Placement::Public;
        pre.next_id = 2;
        let pay = Action::Transfer {
            to: DAVE_SEAT,
            amount: 1,
        };
        pre.proposals = vec![
            proposal(
                0,
                Action::RemoveMember {
                    member: carol(),
                    threshold: 2,
                    successor: None,
                },
                vec![alice()],
            ),
            proposal(1, pay.clone(), vec![carol()]),
        ];
        let mut post = approved(Placement::Public, vec![alice(), bob()], 2);
        post.next_id = 2;
        post.proposals = vec![proposal(1, pay, vec![])];
        assert_eq!(
            seat_to_vault(
                &pre,
                ToVault::Approve {
                    member: bob(),
                    id: 0
                }
            ),
            Response::set_state(post.encode())
        );
    }

    #[test]
    #[should_panic(expected = "a hidden vault removes a member by rotating")]
    fn a_hidden_vault_cant_remove_in_place() {
        let _response = approve_by_bob(
            Placement::Hidden,
            Action::RemoveMember {
                member: carol(),
                threshold: 2,
                successor: None,
            },
        );
    }

    #[test]
    #[should_panic(expected = "a public vault changes members in place")]
    fn a_public_vault_cant_rotate() {
        let _response = approve_by_bob(
            Placement::Public,
            Action::AddMember {
                member: dave(),
                threshold: 2,
                successor: Some(SUCCESSOR),
            },
        );
    }

    #[test]
    #[should_panic(expected = "the threshold must be from 1 to the number of members")]
    fn removing_cant_leave_the_threshold_unreachable() {
        let _response = approve_by_bob(
            Placement::Public,
            Action::RemoveMember {
                member: carol(),
                threshold: 3,
                successor: None,
            },
        );
    }

    #[test]
    #[should_panic(expected = "already a member of this vault")]
    fn a_member_isnt_added_twice() {
        let _response = approve_by_bob(
            Placement::Public,
            Action::AddMember {
                member: carol(),
                threshold: 2,
                successor: None,
            },
        );
    }
}
