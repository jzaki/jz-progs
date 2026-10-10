# LEZ programs: build and run locally

How to build, deploy and call a first LEZ program from the LEZ repo itself, on each current version of the program model. Everything here uses the LEZ repo directly, with Nix and `just`. It needs no Docker or Bedrock: the sequencer's `standalone` feature mocks Bedrock, and the example guests build with the local RISC Zero toolchain instead of Docker.

| Version | Network | Program model | Section |
|---|---|---|---|
| LEZ **v0.2.4** | Local, and the rebooted testnet 0.2 | Programs rewrite whole accounts and claim them | 2 |
| LEZ **v0.3.0** | Local, and testnet 0.3 (experimental) | `plan`/`apply` on per-program shards, with fees | 3 |
| LEZ **v0.4** (unreleased) | None yet | Actors handling messages | 4 |

Sources:

- `logos-blockchain/logos-execution-zone` at tags **v0.2.4** and **v0.3.0**, and branch `artem/transactional-actor-environment` for the v0.4 preview, from the local checkout in `ref-repos/`: `Justfile`, `flake.nix`, `examples/program_deployment/`, `lez/programs/`, `test_programs/`
- `logos-co/logos-docs` [Journey] issues #506–#511 and #514, the LEZ developers' notes for testnet v0.3. Issue numbers like `#514` refer to these, and LEZ PRs are written "PR 875"

The v0.2.4 local flow in section 2 has been run through the hello-world call. Nothing else here has been run. [MODEL_CHANGES.md](MODEL_CHANGES.md) explains the v0.3.0 model in detail, and [GUIDE_v0.2_v0.4.md](GUIDE_v0.2_v0.4.md) covers the move from v0.2 to v0.4.

The debug sequencer config funds two accounts at genesis, and `just wallet-import-test-accounts` imports their public test keys. Never use those keys outside a local chain.

---

## 1. Setup (both versions)

```bash
curl -L https://risczero.com/install | bash && rzup install   # guest toolchain; not in the Nix shell
cd ref-repos/logos-execution-zone
git checkout <tag>                       # v0.2.4 or v0.3.0
nix develop                              # clang, libclang, openssl, pkg-config, pcsclite
```

Run every command below inside `nix develop`, from the repo root unless a step says otherwise. Each tag pins its own Rust version: 1.94.0 for v0.2.4 and 1.98.1 for v0.3.0. If the Nix shell's `cargo` doesn't match, install the pinned version with `rustup` instead.

> **If the RocksDB build fails with `Unable to find libclang`,** point bindgen at the flake's libclang. This happens when a system LLVM is installed, because the dev shell doesn't set `LIBCLANG_PATH`. Run it in each `nix develop` shell before building:
>
> ```bash
> export LIBCLANG_PATH=$(nix eval --raw .#packages.x86_64-linux.wallet.LIBCLANG_PATH)
> ```

Both versions build the example guests the same way:

```bash
cargo build --release -p example_program_deployment_methods
BIN=$PWD/target/riscv-guest/example_program_deployment_methods/example_program_deployment_programs/riscv32im-risc0-zkvm-elf/release
```

If the `.bin` files aren't at `$BIN`, `cargo build` prints where they are.

**Reset either version:** stop the sequencer and run `just clean`. It deletes the chain data and the wallet's storage first, then runs `docker compose down`. Without Docker that last step fails, which is harmless.

---

## 2. LEZ v0.2.4 (local)

This is the model the rebooted testnet 0.2 runs. There are no fees, deploying is free, and a program owns the accounts it claims.

```bash
# Terminal 1: local sequencer on 127.0.0.1:3040. Dev mode skips proving.
RISC0_DEV_MODE=1 just run-sequencer-standalone --listen-address 127.0.0.1

# Terminal 2: wallet. `just run-wallet` keeps its home in lez/wallet/configs/debug.
export LEE_WALLET_HOME_DIR=$PWD/lez/wallet/configs/debug   # the runner below reads the same wallet
just run-wallet check-health             # asks for a password the first time
just wallet-import-test-accounts         # optional here: imports and funds two test accounts
just run-wallet account new public       # the account the program claims
ACCOUNT=<account id printed above>

# Build the guests (section 1), then deploy and call
just run-wallet deploy-program "$BIN/hello_world_with_authorization.bin"
(cd examples/program_deployment && \
  cargo run --release --bin run_hello_world_with_authorization -- \
  "$BIN/hello_world_with_authorization.bin" "$ACCOUNT")

# Check the result
just run-wallet account get --account-id "Public/$ACCOUNT"   # data should hold "Hola mundo!"
```

- **Why the authorization example.** Both hello-world programs claim the account on first use, and v0.2.4 only allows that when the account signed the transaction (`ClaimedUnauthorizedAccount`). `run_hello_world` sends its call unsigned, so it probably fails on a fresh account. `run_hello_world_with_authorization` signs with the account's own key.
- **No payer.** v0.2.4 has no fees, so neither the deploy nor the call needs a funded account.

### Your own program (v0.2.4)

Every file in the guest's `src/bin/` becomes a program, and every file in the runner's `src/bin/` becomes a runner, so neither needs a manifest change.

1. **Copy the program.** Copy `examples/program_deployment/methods/guest/src/bin/hello_world_with_authorization.rs` to `my_program.rs` in the same folder.
2. **Edit it.** The program reads its inputs with `read_lee_inputs`, returns whole-account post-states, and claims new accounts with `Claim`. [MODEL_CHANGES.md](MODEL_CHANGES.md) section 7.2 shows this API next to v0.3.0.
3. **Copy the runner.** Copy `examples/program_deployment/src/bin/run_hello_world_with_authorization.rs` to `run_my_program.rs` in the same folder. Change the instruction it sends.
4. **Build and deploy.** Every rebuild that changes the code gives the program a new ID, so deploy again after each change.

   ```bash
   cargo build --release -p example_program_deployment_methods
   just run-wallet deploy-program "$BIN/my_program.bin"
   ```

5. **Call and check.**

   ```bash
   (cd examples/program_deployment && \
     cargo run --release --bin run_my_program -- "$BIN/my_program.bin" "$ACCOUNT")
   just run-wallet account get --account-id "Public/$ACCOUNT"
   ```

---

## 3. LEZ v0.3.0 (local and testnet)

Every user public transaction in v0.3.0 pays a fee, including deploys, and there is no faucet. The `run_hello_world` runner deploys the program itself through the program loader, so there's no separate deploy step.

### Local

```bash
# Terminal 1: local sequencer on 127.0.0.1:3040. Dev mode skips proving.
RISC0_DEV_MODE=1 just run-sequencer-standalone --listen-address 127.0.0.1

# Terminal 2: wallet. `just run-wallet` keeps its home in lez/wallet/configs/debug.
export LEE_WALLET_HOME_DIR=$PWD/lez/wallet/configs/debug   # the runner below reads the same wallet
just run-wallet check-health             # asks for a password the first time
just wallet-import-test-accounts         # first account listed: Public/CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r
PAYER=CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r
just run-wallet account new public       # the account hello_world writes to
ACCOUNT=<account id printed above>

# Build the guests (section 1), then deploy hello_world (paid by PAYER) and call it
(cd examples/program_deployment && \
  cargo run --release --bin run_hello_world -- "$BIN/hello_world.bin" "$ACCOUNT" "$PAYER")

# Check the result
just run-wallet account get --raw --scope all --account-id "Public/$ACCOUNT"
echo <shard hex> | xxd -r -p             # -> Hola mundo!
```

> **The call probably gets rejected as written.** The runner's deploy declares a fee, but its call is built with `Message::try_new`, which carries no fee. Every user public transaction without a fee fails with `MissingFeeDeclaration` (#514). To fix it, replace the hand-built `Message`, `WitnessSet` and `send_transaction` with `wallet_core.send_pub_tx_paid_by(accounts, instruction_data, program_account_id, Some(payer))` (`lez/wallet/src/lib.rs`). `lez/wallet/src/program_facades/amm.rs` has a call site to copy. The tail-call runners need the same fix.

**Fees (#514):**
- The payer must hold the full reserve for each transaction, about 16,003,000 at the default gas limit and genesis base fees. The unused part is refunded. The genesis account holds far more than that.
- `ACCOUNT` never pays, so it needs no funding.
- A call that reverts is still included and charged, and the wallet still says "included in block". If the shard is empty, suspect the fee or `gas_limit` first (`just run-wallet config get gas_limit`).
- `account get` without `--scope all` shows only the native balance.
- To see the current base fees, query the sequencer:

```bash
curl -s -X POST http://127.0.0.1:3040 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getFeeState","params":[]}'
```

**Your own program:** copy `hello_world.rs` to a new file in `examples/program_deployment/methods/guest/src/bin/`. Change `plan` to choose which accounts to touch, and `apply` to compute the new shard bytes from the old shard and the instruction. `apply` returns `None` to keep the shard, `Some(empty)` to clear it, and `Some(bytes)` to replace it (#509). Rebuild the methods crate, then point a copy of `run_hello_world.rs` at the new `.bin`. [MODEL_CHANGES.md](MODEL_CHANGES.md) section 6.1 covers the rest of the `Plan` API.

```rust
use lee_core::{account::ShardData, program::{Plan, PlanInput, run_program}};

fn main() { run_program(plan, apply) }

fn plan(input: &PlanInput, instruction: Vec<u8>) -> Plan {
    let mut plan = Plan::new(input);
    let [account] = input.accounts.as_slice() else { panic!("expected one account") };
    plan.effect(account, &instruction);
    plan
}

fn apply(greeting: Vec<u8>, pre_data: &ShardData) -> Option<Vec<u8>> {
    let mut bytes = pre_data.clone().into_inner();
    bytes.extend(greeting);
    Some(bytes)
}
```

### Testnet

The public testnet replaces the local sequencer. You still build the guests and the wallet locally.

```bash
just run-wallet change-network testnet   # https://testnet.lez.logos.co
export LEE_WALLET_HOME_DIR=$PWD/lez/wallet/configs/debug
just run-wallet account new public       # the account hello_world writes to
ACCOUNT=<account id printed above>
PAYER=<your funded testnet account id>
(cd examples/program_deployment && \
  cargo run --release --bin run_hello_world -- "$BIN/hello_world.bin" "$ACCOUNT" "$PAYER")
just run-wallet change-network local     # switch back before running locally again
```

- **The payer must be a real funded account.** Funds only enter a zone through an L1 bridge deposit (#510), and there's no first-party command for making one. In practice, ask someone with testnet funds to transfer to you.
- **It needs the full reserve for each transaction,** about 16,003,000 at the default gas limit and genesis base fees (#514). Most of it is refunded.
- **The call fee bug above applies here too.** Fix the runner before calling.
- **Deploying without the runner:** `just run-wallet program-loader deploy --elf <bin> --header <account> --segments <account>... --payer Public/<id>`. Use one fresh public account for the header, plus one per 96 KiB of ELF for the segments. The header's account ID becomes the program's address.

---

## 4. Towards LEZ v0.4.0: the actor model (preview)

> **Not released.** There is no v0.4 tag yet. This preview follows the actor model on LEZ branch `artem/transactional-actor-environment` at `ef1ee95` (2026-10-10), which is unmerged and still changing daily. For example, the entry macro was renamed from `define_actor!` to `define_actor_logic!` between 2026-10-08 and 2026-10-10. There are no build steps here. [GUIDE_v0.2_v0.4.md](GUIDE_v0.2_v0.4.md) covers the model in full.

**The idea.** A program no longer rewrites accounts or plans effects. It is a handler that receives **one message** addressed to **one actor**, and an actor is one program's state on one account. The handler sees only that state, and returns the new state plus any messages it wants to send to other actors.

```
Account  (address + nonce)
 ├─ Actor (account, hello_world)  ─ state: "Hola mundo!"
 ├─ Actor (account, token)        ─ state: a token holding
 └─ Actor (account, native token) ─ state: the balance
```

**Hello world** (`examples/program_deployment/methods/guest/src/bin/hello_world.rs` on that branch):

```rust
use lee_core::program::{ReceiveInput, Response};

lee_core::define_actor_logic!(handle_message);

fn handle_message(input: &ReceiveInput, greeting: Vec<u8>) -> Response {
    let mut bytes = input.pre_state.to_vec();
    bytes.extend(greeting);
    Response::set_state(bytes)
}
```

- **`define_actor_logic!`** generates `main`. It reads the input, decodes the message from borsh into the handler's second argument, and commits the result.
- **`ReceiveInput`** is everything the handler may know:
  - `receiver`: the actor being addressed, as an (account, program) pair.
  - `from`: the actor that sent the message, or `None` when the transaction itself sent it.
  - `is_authorized`: whether the account is authorized, by a signature, the private-account key `ask`, or a PDA seed granted by the caller.
  - `pre_state`: this program's current state on this account. It is empty if the program has never written there.
  - `message`: the raw message bytes.
- **`Response`** says what happens next. `set_state(bytes)` replaces the actor's state and `keep_state()` leaves it unchanged. Either can be followed by `.call(..)`, `.cast(..)` or `.event(..)`.

Hello world never checks `is_authorized`, so anyone can append to its state on any account. The account's balance and other programs' state are out of its reach, so that is harmless.

**How it's used:**

1. **Deploy** the program through the program loader, paid by a funded account. Its address, the `program_account_id`, is the header account the loader writes.
2. **Address an actor.** The transaction names the actor to receive the message, the account plus the program, and lists the public actors it touches. From the example runner:

   ```rust
   let hello = Actor::new(account_id, program_account_id);
   let message = Message::try_new(hello, vec![hello], nonces, greeting)?;
   ```

3. **The engine runs the handler.** It loads that actor's state, runs `handle_message` with the greeting, and stores the returned state.
4. **Read it back.** The wallet shows each program's actor state on the account separately.

At this commit the example runner still builds the call with `Message::try_new` and no fee declaration. That is the same pattern that fails at v0.3.0, so check it before relying on it.

**The same program across versions:**

| | v0.2.4 | v0.3.0 | v0.4 preview |
|---|---|---|---|
| Entry point | `read_lee_inputs`, then `ProgramOutput::write` | `run_program(plan, apply)` | `define_actor_logic!(handle_message)` |
| What it sees | Whole accounts, including other programs' data | `plan`: account handles only. `apply`: one shard | Its own state on one account, plus the sender |
| What it returns | Whole rewritten accounts, plus a `Claim` on new ones | Effects, each applied to one shard | New state, plus messages to other actors |
| Needs the account's signature | Yes, to claim a fresh account | No | No |

**Beyond hello world.** Programs compose by sending messages:

- **`call(actor, &msg)`** runs the other actor in the same transaction, depth-first, and rolls back with it. `simple_tail_call` is just `Response::keep_state().call(callee, &greeting)`.
- **`cast(actor, &msg)`** sends a message for delivery in a later transaction. Token and native-balance credits to private accounts work this way, and the recipient collects them with `wallet pending receive`.
- **Guard internal messages.** A message meant only for your own program should check `input.from_own_program()`.
- **Ask the owner, don't read its data.** To depend on another program's state, call that program's actor with a message that asserts something about its state, or that replies with it.

---

## 5. Example programs to read (v0.3.0)

All paths are in `logos-blockchain/logos-execution-zone` at tag **`v0.3.0`** (`db66590`). The local checkout in `ref-repos/logos-execution-zone` is on `dev`, which is ahead of that tag, so read these with `git show v0.3.0:<path>` or check out the tag. They are all plain `lee_core` guest programs.

### Tutorial examples: `examples/program_deployment/`

These are meant to be read and copied. Guests are in `methods/guest/src/bin/`, and runners are in `src/bin/run_*.rs`.

| Program | What it shows | Runner(s) |
|---|---|---|
| `hello_world.rs` | Smallest program: `plan` selects one account, `apply` appends the instruction bytes to this program's shard. No authorization | `run_hello_world`, `run_hello_world_private` |
| `hello_world_with_authorization.rs` | Same, but panics unless the account is authorized (signature, or `ask` for private accounts) | `run_hello_world_with_authorization` |
| `hello_world_with_move_function.rs` | Two instructions, `write` and `move_data`, moving shard data between a public account and a private one | `run_hello_world_with_move_function` (`write-public`, `write-private`, `move-data-public-to-private`, …) |
| `simple_tail_call.rs` | Program composition: chain-calls `hello_world`. Hard-codes hello_world's program ID | `run_hello_world_through_tail_call`, `…_private` |
| `tail_call_with_pda.rs` | Program-derived account (PDA) authorized by seed through a chained call to `hello_world_with_authorization` | `run_hello_world_with_authorization_through_tail_call_with_pda` |

`examples/program_deployment/README.md` is the tutorial, but **at v0.3.0 it lags the code**. It still says `wallet deploy-program <bin>` and runs the runners with two arguments. The v0.3.0 runners take a third argument, a funded **payer** account, and deploy the program themselves through `program_loader` (`examples/program_deployment/src/lib.rs`). Its account-inspection steps (`wallet account get --raw --scope all`, decoding the shard hex) are still correct. The runners' call transactions also seem to predate fees (see the warning in section 3).

Issue #509 names `hello_world` as "the canonical minimal program". For a realistic program it points to the **token program**:
- `lez/programs/token/src/main.rs` is just `run_program(token_program::plan, token_program::apply)`.
- The routing is in `src/lib.rs`: a pure `plan` over account handles and a nine-variant `Effect` enum.
- A transfer emits exactly one effect per shard, so the recipient's `apply` never sees the sender's data (`src/transfer.rs`).

For emitting events, the model is `lez/programs/bridge` (#507).

### Built-in programs: `lez/programs/`

These are production programs, deployed at genesis. Prebuilt binaries are committed in `artifacts/lez/programs/*.bin`. Each has a `core/` crate (instruction and state types) and a `src/` guest.

| Program | Purpose |
|---|---|
| `token` | Fungible and NFT token definitions and holdings |
| `amm` | Constant-product AMM pools (docs: trade tokens, liquidity pool pages) |
| `associated_token_account` | Deterministic per-owner token holding accounts |
| `fee` | Transaction fees (new in v0.3.0) |
| `clock` | Block ID and timestamp accounts other programs can read |
| `bridge`, `bridge_lock`, `wrapped_token` | Bridging assets to and from Logos Blockchain |
| `cross_zone_inbox`, `cross_zone_outbox`, `cross_zone_marker` | Cross-zone messaging |
| `ping_sender`, `ping_receiver` (+ `ping_core`) | Minimal cross-zone message example. The best starting point for cross-zone work |
| `sequencer_stake` | Sequencer staking, join/exit and slashing |
| `program_loader/core` | Types for the loader that stores deployed programs as header + segment accounts |

### Test programs: `test_programs/guest/src/bin/`

These are small, focused programs used by the integration tests. They're useful as further reading: `cooldown` (clock-gated), `time_locked_transfer`, `authority_proxy` (seeded PDA forwarding), `pda_spend_proxy`, `chain_caller`, `clock_chain_caller`, `data_writer`.
