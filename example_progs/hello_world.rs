use lee_core::program::{ReceiveInput, Response};

// Hello-world, adapted from LEZ's examples/program_deployment/methods/guest/src/bin/hello_world.rs.
//
// `Hello { greeting }` appends the greeting's bytes to this program's own actor state on the
// receiving account. No authorization: anyone can append to it on any account, which is harmless,
// since the program can't touch the balance or any other program's state.
//
// LEZ's original takes the bytes themselves as its message (`Vec<u8>`). Here they are wrapped in a
// one-variant enum, and the text is a `String`, so `spel` can call it: spel's IDLs describe a
// message enum, and spel types a `String` as text (a `Vec<u8>` is a list of decimal bytes). The
// state is a Borsh `HelloState` holding the greetings, back to back.
//
// prog-tui builds it as the guest `jz_hello_world` (key u, or b for every file here).

// SPEL-SPECIFIC: the message is an `enum Message`, not LEZ's bare `Vec<u8>`, because spel's IDL
// generator (`spel generate-idl --message Message`) describes an enum and names each variant's
// fields as flags. LEZ itself doesn't need this wrapper: `handle_message` could take the greeting
// directly, as in LEZ's original.
#[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
enum Message {
    /// Append `greeting` to this account's hello-world state.
    Hello { greeting: String },
}

// SPEL-SPECIFIC: the state layout, for the IDL. A type named `State` or ending in `State` is what
// prog-tui passes as `spel generate-idl --state`, so `spel inspect` and `spel send` can show the
// actor's state decoded. LEZ itself doesn't read or need it: the engine treats the state as bytes.
// It has to be a struct or enum (spel's `--state` can't name a bare `String`), and Borsh-encoded,
// which is why this isn't LEZ's raw bytes: a Borsh decoder can't read those without a length prefix.
#[derive(Default, borsh::BorshSerialize, borsh::BorshDeserialize)]
struct HelloState {
    /// Every greeting appended so far, back to back.
    greetings: String,
}

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, message: Message) -> Response {
    let Message::Hello { greeting } = message;
    let mut state = if input.pre_state.as_ref().is_empty() {
        HelloState::default()
    } else {
        borsh::from_slice(input.pre_state.as_ref()).expect("the state decodes")
    };
    state.greetings.push_str(&greeting);
    Response::set_state(borsh::to_vec(&state).expect("the state encodes"))
}
