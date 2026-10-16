use lee_core::program::{ReceiveInput, Response};

// Hello-world, copied from LEZ's examples/program_deployment/methods/guest/src/bin/hello_world.rs.
//
// This program reads an arbitrary sequence of bytes as its message
// and appends those bytes to this program's own actor state on the receiving account.
//
// prog-tui builds it as the guest `jz_hello_world` (key u, or b for every file here).

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, greeting: Vec<u8>) -> Response {
    let mut bytes = input.pre_state.to_vec();
    bytes.extend(greeting);
    Response::set_state(bytes)
}
