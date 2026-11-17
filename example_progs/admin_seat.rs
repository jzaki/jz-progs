use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, Response, SyncMessage, write_once},
};

// Admin seat: only the admin can initialise a PDA's shared state.
//
// A message sent straight to a PDA actor carries nothing to check: `from` is `None` and
// `is_authorized` is false (admission evidence lets the PDA exist, it doesn't authorize it). So the
// admin speaks through a *seat*: this program's actor on the admin's own account.
//
// - `Open { config }`, to the seat, by the admin. The admin signs, so the seat sees
//   `is_authorized`; `from` is `None`, so it was the admin and not another program. The seat sends
//   `Init { config }` to the admin's PDA actor, granting the PDA's seed.
// - `Init { config }`, to the PDA actor, from a seat. The engine fills in `from`, so it can't be
//   forged. The PDA accepts it only from an actor of this same program (code that did the check
//   above) whose account is the one this PDA belongs to. It writes the config's bytes once.
//
// Each admin gets their own PDA: its seed is the admin's account id, so nobody needs to hard-code
// an admin. The PDA is `AccountId::for_public_pda(&program, &PdaSeed::new(admin))`.
//
// **The seed grant.** The seat's message to the PDA carries the PDA's seed
// (`SyncMessage::with_pda_seeds`). A seed this program grants for an address it derives admits that
// address, so the PDA needs no `PublicAccountEvidence` in the transaction, even before it's on
// chain. It also marks the PDA `is_authorized` for that message. A transaction to `Open` only has to
// declare both actors: the seat (the root) and the PDA (in prog-tui, `pda:<admin>` in `actors`).
//
// prog-tui builds it as the guest `jz_admin_seat` (key u, or b for every file here).

#[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
enum Message {
    /// To your seat, signed by you: initialise your PDA with `config`.
    Open { config: String },
    /// Seat to PDA. Refused from anyone else.
    Init { config: String },
}

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, message: Message) -> Response {
    let program = input.receiver.program_account_id;
    match message {
        Message::Open { config } => {
            assert!(
                input.is_authorized && input.from.is_none(),
                "only the account's owner opens, by a transaction they sign"
            );
            let admin = input.receiver.account_id;
            let pda = Actor::new(pda_of(&program, admin), program);
            Response::keep_state().send(
                SyncMessage::new(pda, &Message::Init { config }).with_pda_seeds([seed_of(admin)]),
            )
        }
        Message::Init { config } => {
            let seat = input.from.expect("Init comes from a seat, not a transaction");
            assert!(input.from_own_program(), "Init comes from this program's seat");
            assert_eq!(
                pda_of(&program, seat.account_id),
                input.receiver.account_id,
                "this PDA belongs to another admin"
            );
            Response::set_state(write_once(input.pre_state.as_ref(), config.into_bytes()))
        }
    }
}

/// The seed of `admin`'s PDA: their account id.
fn seed_of(admin: AccountId) -> PdaSeed {
    PdaSeed::new(*admin.value())
}

/// The PDA of `admin`'s instance of this program.
fn pda_of(program: &AccountId, admin: AccountId) -> AccountId {
    AccountId::for_public_pda(program, &seed_of(admin))
}
