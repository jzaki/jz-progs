use lee_core::program::{ReceiveInput, Response};

// Hello-account: hello_world with a second message.
//
// - `Hello(bytes)`: appends `bytes` to this program's actor state on the receiving account,
//   as hello_world does.
// - `WriteAccountId`: replaces that actor state with the receiving account's 32-byte id.
//
// prog-tui builds it as the guest `jz_hello_account` (key u, or b for every file here).

#[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
enum Message {
    Hello(Vec<u8>),
    WriteAccountId,
}

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, message: Message) -> Response {
    let account_id = input.receiver.account_id;
    match message {
        Message::Hello(greeting) => {
            let mut bytes = input.pre_state.to_vec();
            bytes.extend(greeting);
            Response::set_state(bytes)
        }
        Message::WriteAccountId => Response::set_state(account_id.value().to_vec()),
    }
}
