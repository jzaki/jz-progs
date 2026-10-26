use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, Response, write_once},
};

// Simple token: a fixed-supply token, minted and burned by one admin, transferred by holders.
// Modelled on the native token (lee_core::native_token) and on admin_seat.
//
// Actors, all of this one program:
// - The *seat*: the program's actor on the admin's own account. The admin signs messages to it. Its
//   state is the admin's own token balance, like any holder's.
// - The *treasury*: the PDA seeded by the admin's account id, as in admin_seat. Its state is the
//   fixed `total_supply` and what is still `unissued`. Only the admin's seat can speak to it.
// - *Holders*: the program's actor on any account. State is the balance, as 16 little-endian
//   bytes, empty for zero.
//
// The supply is fixed when the admin opens the token and never changes. Minting moves tokens from
// the treasury's unissued reserve to a holder, burning moves them back, so
// `unissued + sum(balances) == total_supply` always holds.
//
// - `Open { total_supply }`, to your seat, signed by you: seat sends `Init` to the treasury.
// - `Init { total_supply }`, seat to treasury: written once, with everything unissued.
// - `Mint { to, amount }`, to your seat, signed by you: seat sends `Draw` to the treasury, which
//   debits the reserve and sends `Credit` to `to`.
// - `Burn { amount }`, to your seat, signed by you: debits the admin's own balance and sends
//   `Return` to the treasury. (Burning someone else's tokens isn't allowed: they transfer to the
//   admin first.)
// - `Transfer { to, amount }`, to a holder, signed by them: debits them and sends `Credit` to `to`.
// - `Credit(amount)`: only this program can send it, only from a mint or a transfer.
//
// To call `Open` and `Mint`, the transaction must declare the seat and the treasury (and, for
// `Mint`, the recipient), and carry `PublicAccountEvidence::Pda { program, seed }` for the
// treasury while it isn't on chain yet.
//
// prog-tui builds it as the guest `jz_simple_token` (key u, or b for every file here).

#[derive(Clone, Copy, BorshSerialize, BorshDeserialize)]
enum Message {
    /// To your seat, signed by you.
    Open { total_supply: u128 },
    /// Seat to treasury.
    Init { total_supply: u128 },
    /// To your seat, signed by you.
    Mint { to: AccountId, amount: u128 },
    /// Seat to treasury: take `amount` out of the unissued reserve and credit it to `to`.
    Draw { to: AccountId, amount: u128 },
    /// To your seat, signed by you.
    Burn { amount: u128 },
    /// Seat to treasury: put `amount` back into the unissued reserve.
    Return { amount: u128 },
    /// To a holder, signed by them.
    Transfer { to: AccountId, amount: u128 },
    /// Holder or treasury to a holder.
    Credit(u128),
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Treasury {
    total_supply: u128,
    unissued: u128,
}

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, message: Message) -> Response {
    let program = input.receiver.program_account_id;
    match message {
        Message::Open { total_supply } => {
            assert_admin_call(input);
            let treasury = Actor::new(treasury_of(&program, input.receiver.account_id), program);
            Response::keep_state().send_sync(treasury, &Message::Init { total_supply })
        }
        Message::Init { total_supply } => {
            assert_from_own_seat(input);
            let state = Treasury {
                total_supply,
                unissued: total_supply,
            };
            Response::set_state(write_once(
                input.pre_state.as_ref(),
                borsh::to_vec(&state).unwrap(),
            ))
        }
        Message::Mint { to, amount } => {
            assert_admin_call(input);
            let treasury = Actor::new(treasury_of(&program, input.receiver.account_id), program);
            Response::keep_state().send_sync(treasury, &Message::Draw { to, amount })
        }
        Message::Draw { to, amount } => {
            assert_from_own_seat(input);
            let mut treasury = decode_treasury(input);
            treasury.unissued = treasury
                .unissued
                .checked_sub(amount)
                .expect("mint exceeds the unissued supply");
            Response::set_state(borsh::to_vec(&treasury).unwrap())
                .send_async(Actor::new(to, program), &Message::Credit(amount))
        }
        Message::Burn { amount } => {
            assert_admin_call(input);
            let treasury = Actor::new(treasury_of(&program, input.receiver.account_id), program);
            let post = decode_balance(&input.pre_state)
                .checked_sub(amount)
                .expect("burn exceeds the admin's balance");
            Response::set_state(encode_balance(post))
                .send_sync(treasury, &Message::Return { amount })
        }
        Message::Return { amount } => {
            assert_from_own_seat(input);
            let mut treasury = decode_treasury(input);
            treasury.unissued = treasury
                .unissued
                .checked_add(amount)
                .expect("unissued supply overflows");
            assert!(
                treasury.unissued <= treasury.total_supply,
                "returned more than was issued"
            );
            Response::set_state(borsh::to_vec(&treasury).unwrap())
        }
        Message::Transfer { to, amount } => {
            assert!(input.is_authorized, "only the holder transfers");
            let post = decode_balance(&input.pre_state)
                .checked_sub(amount)
                .expect("transfer exceeds the balance");
            Response::set_state(encode_balance(post))
                .send_async(Actor::new(to, program), &Message::Credit(amount))
        }
        Message::Credit(amount) => {
            assert!(
                input.from_own_program(),
                "a credit comes from this program's mint or transfer"
            );
            let post = decode_balance(&input.pre_state)
                .checked_add(amount)
                .expect("balance overflows");
            Response::set_state(encode_balance(post))
        }
    }
}

/// The admin acting through their seat: signed, and not relayed by another program.
fn assert_admin_call(input: &ReceiveInput) {
    assert!(
        input.is_authorized && input.from.is_none(),
        "only the account's owner calls, by a transaction they sign"
    );
}

/// The caller is the seat of the admin this treasury belongs to.
fn assert_from_own_seat(input: &ReceiveInput) {
    let program = input.receiver.program_account_id;
    let seat = input.from.expect("this comes from a seat, not a transaction");
    assert!(input.from_own_program(), "this comes from this program's seat");
    assert_eq!(
        treasury_of(&program, seat.account_id),
        input.receiver.account_id,
        "this treasury belongs to another admin"
    );
}

fn decode_treasury(input: &ReceiveInput) -> Treasury {
    borsh::from_slice(input.pre_state.as_ref()).expect("the token isn't opened")
}

fn encode_balance(balance: u128) -> Vec<u8> {
    if balance == 0 {
        Vec::new()
    } else {
        balance.to_le_bytes().to_vec()
    }
}

fn decode_balance(data: &[u8]) -> u128 {
    if data.is_empty() {
        return 0;
    }
    u128::from_le_bytes(data.try_into().expect("a balance is 16 bytes"))
}

/// The treasury PDA of `admin`'s token.
fn treasury_of(program: &AccountId, admin: AccountId) -> AccountId {
    AccountId::for_public_pda(program, &PdaSeed::new(*admin.value()))
}

#[cfg(test)]
mod tests {
    use lee_core::program::{AsyncMessage, SyncMessage};

    use super::*;

    const PROGRAM: AccountId = AccountId::new([7; 32]);
    const ADMIN: AccountId = AccountId::new([1; 32]);
    const ALICE: AccountId = AccountId::new([2; 32]);

    fn actor(account_id: AccountId) -> Actor {
        Actor::new(account_id, PROGRAM)
    }

    fn treasury() -> Actor {
        actor(treasury_of(&PROGRAM, ADMIN))
    }

    fn treasury_state(total_supply: u128, unissued: u128) -> Vec<u8> {
        borsh::to_vec(&Treasury {
            total_supply,
            unissued,
        })
        .unwrap()
    }

    fn input(
        receiver: Actor,
        from: Option<Actor>,
        is_authorized: bool,
        pre_state: Vec<u8>,
    ) -> ReceiveInput {
        ReceiveInput {
            receiver,
            from,
            is_authorized,
            pre_state: pre_state.into(),
            message: Vec::new(),
        }
    }

    /// A transaction signed by the admin, to their seat holding `balance`.
    fn signed_to_seat(balance: u128) -> ReceiveInput {
        input(actor(ADMIN), None, true, encode_balance(balance))
    }

    /// From the admin's seat to the treasury.
    fn seat_to_treasury(pre_state: Vec<u8>) -> ReceiveInput {
        input(treasury(), Some(actor(ADMIN)), false, pre_state)
    }

    #[test]
    fn opening_initialises_the_treasury_with_everything_unissued() {
        let open = handle_message(&signed_to_seat(0), Message::Open { total_supply: 100 });
        assert_eq!(open.post_state, None);
        assert_eq!(
            open.sync_messages,
            [SyncMessage::new(treasury(), &Message::Init { total_supply: 100 })]
        );

        let init = handle_message(
            &seat_to_treasury(Vec::new()),
            Message::Init { total_supply: 100 },
        );
        assert_eq!(init.post_state, Some(treasury_state(100, 100).into()));
    }

    #[test]
    #[should_panic(expected = "only the account's owner calls")]
    fn an_unsigned_open_is_refused() {
        let _response = handle_message(
            &input(actor(ADMIN), None, false, Vec::new()),
            Message::Open { total_supply: 100 },
        );
    }

    #[test]
    #[should_panic(expected = "this treasury belongs to another admin")]
    fn another_seat_cannot_init_this_treasury() {
        let _response = handle_message(
            &input(treasury(), Some(actor(ALICE)), false, Vec::new()),
            Message::Init { total_supply: 100 },
        );
    }

    #[test]
    #[should_panic(expected = "actor state already holds different data")]
    fn the_supply_is_fixed_once_opened() {
        let _response = handle_message(
            &seat_to_treasury(treasury_state(100, 100)),
            Message::Init { total_supply: 1_000 },
        );
    }

    #[test]
    fn minting_draws_on_the_unissued_supply() {
        let mint = handle_message(&signed_to_seat(0), Message::Mint { to: ALICE, amount: 30 });
        assert_eq!(
            mint.sync_messages,
            [SyncMessage::new(treasury(), &Message::Draw { to: ALICE, amount: 30 })]
        );

        let draw = handle_message(
            &seat_to_treasury(treasury_state(100, 100)),
            Message::Draw { to: ALICE, amount: 30 },
        );
        assert_eq!(draw.post_state, Some(treasury_state(100, 70).into()));
        assert_eq!(
            draw.async_messages,
            [AsyncMessage::new(actor(ALICE), &Message::Credit(30))]
        );
    }

    #[test]
    #[should_panic(expected = "mint exceeds the unissued supply")]
    fn minting_beyond_the_supply_is_refused() {
        let _response = handle_message(
            &seat_to_treasury(treasury_state(100, 20)),
            Message::Draw { to: ALICE, amount: 30 },
        );
    }

    #[test]
    #[should_panic(expected = "only the account's owner calls")]
    fn a_holder_cannot_mint_through_another_program() {
        let _response = handle_message(
            &input(actor(ADMIN), Some(actor(ALICE)), true, Vec::new()),
            Message::Mint { to: ALICE, amount: 30 },
        );
    }

    #[test]
    fn burning_returns_the_admins_tokens_to_the_unissued_supply() {
        let burn = handle_message(&signed_to_seat(50), Message::Burn { amount: 20 });
        assert_eq!(burn.post_state, Some(encode_balance(30).into()));
        assert_eq!(
            burn.sync_messages,
            [SyncMessage::new(treasury(), &Message::Return { amount: 20 })]
        );

        let ret = handle_message(
            &seat_to_treasury(treasury_state(100, 50)),
            Message::Return { amount: 20 },
        );
        assert_eq!(ret.post_state, Some(treasury_state(100, 70).into()));
    }

    #[test]
    #[should_panic(expected = "burn exceeds the admin's balance")]
    fn burning_more_than_the_admin_holds_is_refused() {
        let _response = handle_message(&signed_to_seat(10), Message::Burn { amount: 20 });
    }

    #[test]
    fn a_transfer_debits_the_holder_and_credits_the_recipient() {
        let transfer = handle_message(
            &input(actor(ALICE), None, true, encode_balance(30)),
            Message::Transfer { to: ADMIN, amount: 10 },
        );
        assert_eq!(transfer.post_state, Some(encode_balance(20).into()));
        assert_eq!(
            transfer.async_messages,
            [AsyncMessage::new(actor(ADMIN), &Message::Credit(10))]
        );

        let credit = handle_message(
            &input(actor(ADMIN), Some(actor(ALICE)), false, encode_balance(5)),
            Message::Credit(10),
        );
        assert_eq!(credit.post_state, Some(encode_balance(15).into()));
    }

    #[test]
    fn transferring_the_whole_balance_leaves_the_state_empty() {
        let transfer = handle_message(
            &input(actor(ALICE), None, true, encode_balance(30)),
            Message::Transfer { to: ADMIN, amount: 30 },
        );
        assert_eq!(transfer.post_state, Some(Vec::new().into()));
    }

    #[test]
    #[should_panic(expected = "only the holder transfers")]
    fn an_unsigned_transfer_is_refused() {
        let _response = handle_message(
            &input(actor(ALICE), None, false, encode_balance(30)),
            Message::Transfer { to: ADMIN, amount: 10 },
        );
    }

    #[test]
    #[should_panic(expected = "transfer exceeds the balance")]
    fn a_transfer_beyond_the_balance_is_refused() {
        let _response = handle_message(
            &input(actor(ALICE), None, true, encode_balance(5)),
            Message::Transfer { to: ADMIN, amount: 10 },
        );
    }

    #[test]
    #[should_panic(expected = "a credit comes from this program's mint or transfer")]
    fn a_credit_from_a_transaction_is_refused() {
        let _response = handle_message(
            &input(actor(ALICE), None, true, Vec::new()),
            Message::Credit(1_000),
        );
    }

    #[test]
    #[should_panic(expected = "a credit comes from this program's mint or transfer")]
    fn a_credit_from_another_program_is_refused() {
        let _response = handle_message(
            &input(
                actor(ALICE),
                Some(Actor::new(ADMIN, AccountId::new([8; 32]))),
                false,
                Vec::new(),
            ),
            Message::Credit(1_000),
        );
    }
}
