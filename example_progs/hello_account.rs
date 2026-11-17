use lee_core::program::{ReceiveInput, Response};

// Hello-account: hello_world with a second message.
//
// - `Hello { greeting }`: appends the greeting to this program's actor state on the receiving
//   account, as hello_world does.
// - `WriteAccountId`: replaces that actor state with the receiving account's 32-byte id.
//
// prog-tui builds it as the guest `jz_hello_account` (key u, or b for every file here).

// SPEL-SPECIFIC: the message is an `enum Message` with named fields, because spel's IDL generator
// (`spel generate-idl --message Message`) describes an enum and turns each field into a flag
// (`--greeting`). LEZ itself doesn't need the names: only the Borsh layout matters to the engine.
#[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
enum Message {
    /// Append `greeting` to this account's state.
    Hello { greeting: String },
    /// Replace this account's state with its own 32-byte id.
    WriteAccountId,
}

// SPEL-SPECIFIC: the state layout, for the IDL. A type named `State` or ending in `State` is what
// prog-tui passes as `spel generate-idl --state`, so `spel inspect` and `spel send` can show the
// actor's state decoded. LEZ itself doesn't read or need it: the engine treats the state as bytes.
// It has to be a struct or enum, and Borsh-encoded, so the state is this struct and not raw bytes.
#[derive(Default, borsh::BorshSerialize, borsh::BorshDeserialize)]
struct HelloAccountState {
    /// Every greeting appended so far, back to back.
    greetings: String,
    /// The account's own id, once `WriteAccountId` has stored it. That message clears `greetings`.
    account_id: Option<[u8; 32]>,
}

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, message: Message) -> Response {
    let state = match message {
        Message::Hello { greeting } => {
            let mut state = if input.pre_state.as_ref().is_empty() {
                HelloAccountState::default()
            } else {
                borsh::from_slice(input.pre_state.as_ref()).expect("the state decodes")
            };
            state.greetings.push_str(&greeting);
            state
        }
        Message::WriteAccountId => HelloAccountState {
            greetings: String::new(),
            account_id: Some(*input.receiver.account_id.value()),
        },
    };
    Response::set_state(borsh::to_vec(&state).expect("the state encodes"))
}
