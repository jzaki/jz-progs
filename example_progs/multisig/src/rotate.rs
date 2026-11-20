//! Rotating a hidden vault, and sweeping a retired one.
//!
//! A vault doesn't know its own balance: native funds are the native token program's state. So it
//! asks. It stores what it will do in `pending`, sends `ReadState` to its own native balance, and
//! finishes on the reply, which comes back in the same transaction (payroll's `Withdraw` pattern).
//!
//! On the reply, a rotation hands the new configuration to the successor (`Adopt`, granting the
//! successor's seed) and moves the whole balance there. The old vault stays retired: it refuses
//! everything but `Sweep`, which forwards anything deposited to it later.

use lee_core::{
    account::{AccountId, Actor},
    native_token::{self, custody_transfer, decode_balance},
    program::{write_once, ReceiveInput, Response, StateReply, SyncMessage},
};
use multisig_core::{seed_for, Config, Pending, Placement, VaultMessage, VaultState};

use crate::create::check_config;

/// At the vault: retire to `successor`, which takes `config`, once the balance answers.
pub(crate) fn begin(
    input: &ReceiveInput,
    mut state: VaultState,
    successor: AccountId,
    config: Config,
) -> Response {
    state.retired = Some(successor);
    state.pending = Some(Pending::Rotate(config));
    // The successor starts with no proposals: approvals are re-given under the new membership.
    state.proposals.clear();
    read_balance(input, &state)
}

/// At a retired vault: `member` asks to forward its balance to the successor.
pub(crate) fn sweep(input: &ReceiveInput, mut state: VaultState) -> Response {
    assert!(state.retired.is_some(), "only a retired vault sweeps");
    state.pending = Some(Pending::Sweep);
    read_balance(input, &state)
}

fn read_balance(input: &ReceiveInput, state: &VaultState) -> Response {
    Response::set_state(state.encode()).send_sync(
        Actor::native_balance(input.receiver.account_id),
        &native_token::Message::ReadState,
    )
}

/// At the vault: its native balance's answer. Only the vault's own native balance reaches here
/// (`handle_message` dispatches on the sender).
pub(crate) fn on_balance(input: &ReceiveInput, reply: &StateReply) -> Response {
    let mut state = VaultState::decode(&input.pre_state);
    let pending = state
        .pending
        .take()
        .expect("nothing is waiting for the balance");
    let successor = state.retired.expect("only a retired vault reads its balance");
    let balance = decode_balance(&reply.state).expect("a native balance decodes");
    let mut response = Response::set_state(state.encode());
    match pending {
        Pending::Rotate(config) => {
            let seed = seed_for(config.create_key, config.generation);
            response = response.send(
                SyncMessage::new(
                    Actor::new(successor, input.receiver.program_account_id),
                    &VaultMessage::Adopt(config),
                )
                .with_pda_seeds([seed]),
            );
        }
        Pending::Sweep => assert!(balance > 0, "nothing to sweep"),
    }
    if balance > 0 {
        response = response.send(custody_transfer(
            input.receiver.account_id,
            state.seed,
            successor,
            balance,
        ));
    }
    response
}

/// At the successor: take the configuration its predecessor hands over, once.
///
/// The predecessor granted this vault's seed, so `is_authorized` proves it is this program's PDA of
/// that seed. As at `Create`, it must not be the public PDA.
pub(crate) fn adopt(input: &ReceiveInput, config: Config) -> Response {
    assert!(
        input.is_authorized,
        "only a predecessor, granting this vault's seed, hands over a vault"
    );
    let seed = seed_for(config.create_key, config.generation);
    assert_ne!(
        input.receiver.account_id,
        AccountId::for_public_pda(&input.receiver.program_account_id, &seed),
        "a hidden vault's successor must be a private PDA"
    );
    check_config(&config.members, config.threshold);
    let state = VaultState::new(Placement::Hidden, config).encode();
    Response::set_state(write_once(&input.pre_state, state))
}

#[cfg(test)]
mod tests {
    use crate::test_utils::*;

    fn remove_carol() -> Action {
        Action::RemoveMember {
            member: carol(),
            threshold: 2,
            successor: Some(SUCCESSOR),
        }
    }

    fn next_config() -> Config {
        Config {
            create_key: CREATE_KEY,
            generation: 1,
            members: vec![alice(), bob()],
            threshold: 2,
        }
    }

    /// The vault once the removal is approved, waiting for its balance.
    fn retiring() -> VaultState {
        let mut state = vault3();
        state.next_id = 2;
        state.retired = Some(SUCCESSOR);
        state.pending = Some(Pending::Rotate(next_config()));
        state
    }

    fn balance_reply(pre: &VaultState, balance: u128) -> Response {
        run(
            VAULT,
            Some(Actor::native_balance(VAULT)),
            false,
            pre.encode(),
            &StateReply {
                state: native_token::encode_balance(balance),
            },
        )
    }

    #[test]
    fn approving_a_removal_retires_the_vault_and_reads_its_balance() {
        let mut pre = vault3();
        pre.next_id = 2;
        pre.proposals = vec![
            proposal(0, remove_carol(), vec![alice()]),
            proposal(
                1,
                Action::Transfer {
                    to: DAVE_SEAT,
                    amount: 1,
                },
                vec![carol()],
            ),
        ];
        assert_eq!(
            seat_to_vault(
                &pre,
                ToVault::Approve {
                    member: bob(),
                    id: 0
                }
            ),
            Response::set_state(retiring().encode()).send_sync(
                Actor::native_balance(VAULT),
                &native_token::Message::ReadState
            )
        );
    }

    #[test]
    fn the_balance_moves_with_the_configuration_to_the_successor() {
        let mut post = retiring();
        post.pending = None;
        assert_eq!(
            balance_reply(&retiring(), 1_000),
            Response::set_state(post.encode())
                .send(
                    SyncMessage::new(
                        Actor::new(SUCCESSOR, MULTISIG),
                        &VaultMessage::Adopt(next_config())
                    )
                    .with_pda_seeds([seed_for(CREATE_KEY, 1)])
                )
                .send(native_token::custody_transfer(
                    VAULT,
                    seed_for(CREATE_KEY, 0),
                    SUCCESSOR,
                    1_000
                ))
        );
    }

    #[test]
    fn the_successor_adopts_the_configuration() {
        let response = run(
            SUCCESSOR,
            Some(Actor::new(VAULT_ALIAS, MULTISIG)),
            true,
            Vec::new(),
            &VaultMessage::Adopt(next_config()),
        );
        assert_eq!(
            response,
            Response::set_state(VaultState::new(Placement::Hidden, next_config()).encode())
        );
    }

    #[test]
    #[should_panic(expected = "only a predecessor, granting this vault's seed")]
    fn adopting_without_the_seed_grant_is_refused() {
        let _response = run(
            SUCCESSOR,
            Some(Actor::new(VAULT_ALIAS, MULTISIG)),
            false,
            Vec::new(),
            &VaultMessage::Adopt(next_config()),
        );
    }

    #[test]
    #[should_panic(expected = "this vault is retired")]
    fn a_retired_vault_refuses_proposals() {
        let mut retired = retiring();
        retired.pending = None;
        let _response = seat_to_vault(
            &retired,
            ToVault::Propose {
                member: alice(),
                action: Action::Transfer {
                    to: DAVE_SEAT,
                    amount: 1,
                },
            },
        );
    }

    #[test]
    fn a_retired_vault_sweeps_a_late_deposit_to_its_successor() {
        let mut retired = retiring();
        retired.pending = None;
        let mut sweeping = retired.clone();
        sweeping.pending = Some(Pending::Sweep);
        assert_eq!(
            seat_to_vault(&retired, ToVault::Sweep { member: carol() }),
            Response::set_state(sweeping.encode()).send_sync(
                Actor::native_balance(VAULT),
                &native_token::Message::ReadState
            )
        );
        assert_eq!(
            balance_reply(&sweeping, 70),
            Response::set_state(retired.encode()).send(native_token::custody_transfer(
                VAULT,
                seed_for(CREATE_KEY, 0),
                SUCCESSOR,
                70
            ))
        );
    }

    #[test]
    #[should_panic(expected = "only a retired vault sweeps")]
    fn an_active_vault_doesnt_sweep() {
        let _response = seat_to_vault(&vault3(), ToVault::Sweep { member: bob() });
    }
}
