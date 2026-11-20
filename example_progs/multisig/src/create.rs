//! `Create`: a seat sets up a vault, once.

use lee_core::{
    account::AccountId,
    program::{write_once, ReceiveInput, Response},
};
use multisig_core::{seed_for, Config, Placement, VaultState, MAX_MEMBERS};

/// At the vault.
///
/// The seat granted generation 0's seed, so `is_authorized` proves the vault is this program's
/// PDA of that seed. What the grant can't tell is whether the PDA is public or private: both are
/// granted the same way. A public PDA's address is the program and seed alone, and a private
/// one's also hashes in its keys, so the two never coincide. That separates the placements.
pub(crate) fn create(
    input: &ReceiveInput,
    member: AccountId,
    placement: Placement,
    create_key: [u8; 32],
    members: Vec<AccountId>,
    threshold: u8,
) -> Response {
    assert!(
        input.is_authorized,
        "a vault must be this program's PDA of its create key"
    );
    let public = AccountId::for_public_pda(
        &input.receiver.program_account_id,
        &seed_for(create_key, 0),
    );
    match placement {
        Placement::Public => assert_eq!(
            input.receiver.account_id, public,
            "a public vault is the public PDA of its create key"
        ),
        Placement::Hidden => assert_ne!(
            input.receiver.account_id, public,
            "a hidden vault must be a private PDA, or its state would be public"
        ),
    }
    assert!(members.contains(&member), "the creator must be a member");
    check_config(&members, threshold);
    let config = Config {
        create_key,
        generation: 0,
        members,
        threshold,
    };
    let state = VaultState::new(placement, config).encode();
    Response::set_state(write_once(&input.pre_state, state))
}

/// A valid membership: 1 to [`MAX_MEMBERS`] distinct members, and a threshold from 1 to their
/// number.
pub(crate) fn check_config(members: &[AccountId], threshold: u8) {
    assert!(
        !members.is_empty() && members.len() <= MAX_MEMBERS,
        "a vault has 1 to {MAX_MEMBERS} members"
    );
    for (i, member) in members.iter().enumerate() {
        assert!(
            !members.iter().skip(i.saturating_add(1)).any(|m| m == member),
            "members must be distinct"
        );
    }
    assert!(
        threshold >= 1 && usize::from(threshold) <= members.len(),
        "the threshold must be from 1 to the number of members"
    );
}

#[cfg(test)]
mod tests {
    use crate::test_utils::*;

    fn create_at(vault: AccountId, placement: Placement, granted: bool) -> Response {
        to_vault_at(
            vault,
            granted,
            Vec::new(),
            ToVault::Create {
                member: alice(),
                placement,
                create_key: CREATE_KEY,
                members: members3(),
                threshold: 2,
            },
        )
    }

    #[test]
    fn a_hidden_vault_is_created() {
        assert_eq!(
            create_at(VAULT, Placement::Hidden, true),
            Response::set_state(vault3().encode())
        );
    }

    #[test]
    fn a_public_vault_is_created_at_its_public_pda() {
        let public = public_vault();
        let mut expected = vault3();
        expected.placement = Placement::Public;
        assert_eq!(
            create_at(public, Placement::Public, true),
            Response::set_state(expected.encode())
        );
    }

    #[test]
    #[should_panic(expected = "a public vault is the public PDA of its create key")]
    fn a_public_vault_elsewhere_is_refused() {
        let _response = create_at(VAULT, Placement::Public, true);
    }

    #[test]
    #[should_panic(expected = "a hidden vault must be a private PDA")]
    fn a_hidden_vault_at_the_public_pda_is_refused() {
        let _response = create_at(public_vault(), Placement::Hidden, true);
    }

    #[test]
    #[should_panic(expected = "a vault must be this program's PDA of its create key")]
    fn a_vault_without_the_seed_grant_is_refused() {
        let _response = create_at(VAULT, Placement::Hidden, false);
    }

    #[test]
    #[should_panic(expected = "actor state already holds different data")]
    fn a_vault_is_created_once() {
        let mut other = vault3();
        other.config.threshold = 3;
        let _response = to_vault_at(
            VAULT,
            true,
            other.encode(),
            ToVault::Create {
                member: alice(),
                placement: Placement::Hidden,
                create_key: CREATE_KEY,
                members: members3(),
                threshold: 2,
            },
        );
    }

    #[test]
    #[should_panic(expected = "the creator must be a member")]
    fn the_creator_must_be_a_member() {
        let _response = to_vault_at(
            VAULT,
            true,
            Vec::new(),
            ToVault::Create {
                member: alice(),
                placement: Placement::Hidden,
                create_key: CREATE_KEY,
                members: vec![bob(), carol()],
                threshold: 2,
            },
        );
    }

    #[test]
    #[should_panic(expected = "the threshold must be from 1 to the number of members")]
    fn the_threshold_cant_exceed_the_members() {
        let _response = to_vault_at(
            VAULT,
            true,
            Vec::new(),
            ToVault::Create {
                member: alice(),
                placement: Placement::Hidden,
                create_key: CREATE_KEY,
                members: members3(),
                threshold: 4,
            },
        );
    }

    #[test]
    #[should_panic(expected = "members must be distinct")]
    fn members_are_distinct() {
        let _response = to_vault_at(
            VAULT,
            true,
            Vec::new(),
            ToVault::Create {
                member: alice(),
                placement: Placement::Hidden,
                create_key: CREATE_KEY,
                members: vec![alice(), bob(), alice()],
                threshold: 2,
            },
        );
    }
}
