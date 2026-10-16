# LEZ programs: build and run locally

How to build, deploy and call a first LEZ program from the LEZ repo itself, on each program model. **Write new programs for v0.4, the actor model, which is how programs will be written: use section 4 and the `dev` branch.** Sections 2 and 3 cover the older tags: v0.2.4 is legacy, and v0.3.0 is the current testnet but very short-lived, so don't start on it. Use section 2 to maintain an existing program or to try a deployment on a public (legacy) testnet. Everything here uses the LEZ repo directly, with Nix and `just`. It needs no Docker or Bedrock: the sequencer's `standalone` feature mocks Bedrock, and the example guests build with the local RISC Zero toolchain instead of Docker.

| Version | Status | Network | Program model | Section |
|---|---|---|---|---|
| LEZ **v0.4** (unreleased, on `dev`) | **Recommended.** How programs will be written | None yet (listed as TBD) | Actors handling messages | **4** |
| LEZ **v0.3.0** | Current testnet, but very short-lived. Don't start on it | Local, and testnet 0.3 (experimental) | `plan`/`apply` on per-program shards, with fees | 3 |
| LEZ **v0.2.4** | Legacy | Local, and the legacy testnet | Programs rewrite whole accounts and claim them | 2 |

Sources:

- `logos-blockchain/logos-execution-zone` on `dev` (checked at `e2913677b`, 2026-10-14) for v0.4, run from branch `jzaki/fix-examples` (`be940d8f8`, which adds runner fixes on top of `e2913677b`), and tags **v0.2.4** and **v0.3.0** for the older sections. `../logos-execution-zone` is the local checkout. Files used: `Justfile`, `flake.nix`, `rust-toolchain.toml`, `examples/program_deployment/`, `lez/wallet/`, `lez/programs/`.
- `logos-co/logos-docs` [Journey] issues #506–#511 and #514, the LEZ developers' notes for testnet v0.3. Issue numbers like `#514` refer to these, and LEZ PRs are written "PR 875".
- `logos-co/logos-docs` branch `docs/zone-endpoint-ref`, file `docs/get-started/testnet-endpoints.md` (commit `de03a2b0`, unmerged at the time of writing, so it may move): the RPC, explorer, release and channel for each testnet. Section 1 copies the rows used here. If they differ, that page wins.
- The actor-model manual on LEZ branch `schouhy/lez-guide-mdbook` (`docs/actor-program-development-book/`). Section 4 links to it instead of repeating it.

**What is referenced.** The v0.4 steps are written from the code and its `--help` text. [MODEL_CHANGES.md](MODEL_CHANGES.md) explains the v0.3.0 model in detail, and [GUIDE_v0.2_v0.4.md](GUIDE_v0.2_v0.4.md) covers porting from v0.2 to v0.4.

The debug sequencer config funds two accounts at genesis, and `just wallet-import-test-accounts` imports their public test keys. Never use those keys outside a local chain.

---

## 1. Setup (all versions)

```bash
curl -L https://risczero.com/install | bash && rzup install   # guest toolchain; not in the Nix shell
cd ../logos-execution-zone               # your LEZ checkout
git checkout <ref>                       # dev for v0.4, or the tag v0.2.4 / v0.3.0
nix develop                              # clang, libclang, openssl, pkg-config, pcsclite
```

Run every command below inside `nix develop`, from the repo root unless a step says otherwise. Each ref pins its own Rust version: 1.94.0 for v0.2.4 and 1.98.1 for v0.3.0 and `dev`. If the Nix shell's `cargo` doesn't match, install the pinned version with `rustup` instead.

> **If the RocksDB build fails with `Unable to find libclang`,** point bindgen at the flake's libclang. This happens when a system LLVM is installed, because the dev shell doesn't set `LIBCLANG_PATH`. Run it in each `nix develop` shell before building:
>
> ```bash
> export LIBCLANG_PATH=$(nix eval --raw .#packages.x86_64-linux.wallet.LIBCLANG_PATH)
> ```

### Testnets

Each LEZ version has its own public testnet. Build the wallet and the guests from the **ref that matches the testnet**, never from `dev`, because a wallet and a chain must speak the same transaction format.

| Testnet | LEZ RPC | LEZ explorer | Build from |
|---|---|---|---|
| 0.4 (actor model) | TBD | TBD | TBD |
| 0.3 (current) | `https://testnet.lez.logos.co` | `https://explorer.testnet.lez.logos.co/` | tag `v0.3.0` |
| 0.2.4 (legacy) | `https://testnet-legacy.lez.logos.co/` | `https://explorer.testnet-legacy.lez.logos.co/` | tag `v0.2.4-lez-legacy` |

Both also appear on the Logos Blockchain explorer, `https://testnet.blockchain.logos.co/web/explorer/`. The channel ids are on the endpoints page (`logos-docs`, branch `docs/zone-endpoint-ref`, `docs/get-started/testnet-endpoints.md`).

- **`change-network testnet` always means the 0.3 RPC.** The alias is hard-coded in `lez/wallet/src/cli/network.rs`, and it is the same at `v0.2.4`, `v0.3.0` and `dev`. For the legacy testnet, pass the URL: `wallet change-network https://testnet-legacy.lez.logos.co/`. `change-network` accepts `testnet`, `local` or any sequencer URL.
- **`v0.2.4-lez-legacy` is not `v0.2.4`.** It is three commits ahead: built against logos-blockchain 0.3.1, plus a sequencer fix for stale pending inscriptions. Use the legacy tag for the legacy testnet.
- **Keep one wallet home per network.** The wallet keeps its config and `storage.json` in `LEE_WALLET_HOME_DIR`. Reusing one home across networks mixes accounts and nonces from different chains. Copy `lez/wallet/configs/debug` to a new directory for each testnet, and export that path as `LEE_WALLET_HOME_DIR` in every terminal (the `run_*` runners read the same wallet). `just run-wallet` always sets the variable to `configs/debug` itself, so for a testnet either call `cargo run --release -p wallet --` directly, or accept that `configs/debug` is your home and reset it with `just clean` afterwards.
- **zsh users: `$WALLET` doesn't split.** Sections 2 and 3 set `WALLET="cargo run --release -p wallet --"` and then run `$WALLET ...`. That works in bash, and inside `nix develop`, which starts bash. zsh doesn't word-split an unquoted variable, so it fails with `command not found: cargo run --release -p wallet --`. In zsh, define a function instead, which works in bash too, and call `wallet ...` wherever the steps say `$WALLET ...`:

  ```bash
  wallet() { cargo run --release -p wallet -- "$@"; }
  ```

All versions build the example guests the same way:

```bash
cargo build --release -p example_program_deployment_methods
BIN=$PWD/target/riscv-guest/example_program_deployment_methods/example_program_deployment_programs/riscv32im-risc0-zkvm-elf/release
```

If the `.bin` files aren't at `$BIN`, `cargo build` prints where they are.

**Reset any version:** stop the sequencer and run `just clean`. It deletes the chain data and the wallet's storage first, then runs `docker compose down`. Without Docker that last step fails, which is harmless.

---

## 2. LEZ v0.2.4 (local and legacy testnet)

> **Legacy.** Use this only to maintain an existing v0.2 program. New programs should target v0.4 (section 4), and [GUIDE_v0.2_v0.4.md](GUIDE_v0.2_v0.4.md) covers porting.

This is the model the legacy testnet (the rebooted testnet 0.2) runs. There are no fees, deploying is free, and a program owns the accounts it claims.

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

### Testnet (legacy, v0.2.4)

The legacy testnet runs the 0.2.4 model: no fees, free deploys, and a program owns the accounts it claims. Run this from a checkout of tag `v0.2.4-lez-legacy`. The wallet and guest build steps are section 1's.

```bash
export LEE_WALLET_HOME_DIR=$HOME/lez-wallet-legacy      # a copy of lez/wallet/configs/debug (see section 1, Testnets)
WALLET="cargo run --release -p wallet --"
$WALLET change-network https://testnet-legacy.lez.logos.co/   # not the `testnet` alias, which is the 0.3 chain
$WALLET check-health                      # asks for a password the first time
$WALLET account new public                # the account the program claims
ACCOUNT=<account id printed above>

# Build the guests (section 1), then deploy and call
$WALLET deploy-program "$BIN/hello_world_with_authorization.bin"
(cd examples/program_deployment && \
  cargo run --release --bin run_hello_world_with_authorization -- \
  "$BIN/hello_world_with_authorization.bin" "$ACCOUNT")

# Check the result
$WALLET account get --account-id "Public/$ACCOUNT"   # data should hold "Hola mundo!"
```

- **Check it landed.** Look the account up in the legacy explorer, `https://explorer.testnet-legacy.lez.logos.co/`. Public testnets have other users, so the block may take a few seconds to appear.
- **No payer, and no genesis test keys.** v0.2.4 has no fees, so neither the deploy nor the call needs a funded account. Do not use `just wallet-import-test-accounts`: its keys are public, and they belong to the local chain.
- **Funding.** The v0.2.4 wallet has `wallet pinata claim --to <account>`, which I haven't tried on the legacy testnet. If you need a balance for a program that moves native tokens, check the testnet's faucet instructions in the Logos docs first.
- **Switch back.** Point the wallet home elsewhere or run `change-network local` before running a local sequencer again.

---

## 3. LEZ v0.3.0 (local and testnet)

> **Current testnet, but very short-lived.** Don't start new work on `plan`/`apply` (don't start on 0.3): the actor model (section 4) replaces it. It is here for programs already on `plan`/`apply`. To try a public deployment, use the legacy testnet in section 2.

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

The 0.3 testnet replaces the local sequencer: RPC `https://testnet.lez.logos.co`, explorer `https://explorer.testnet.lez.logos.co/`. You still build the guests and the wallet locally, from tag `v0.3.0` (section 1, Testnets).

```bash
export LEE_WALLET_HOME_DIR=$HOME/lez-wallet-v03          # a copy of lez/wallet/configs/debug
WALLET="cargo run --release -p wallet --"
$WALLET change-network testnet           # https://testnet.lez.logos.co
$WALLET check-health                     # asks for a password the first time
$WALLET account new public               # the account hello_world writes to
ACCOUNT=<account id printed above>
PAYER=<your funded testnet account id>   # must already be in this wallet
(cd examples/program_deployment && \
  cargo run --release --bin run_hello_world -- "$BIN/hello_world.bin" "$ACCOUNT" "$PAYER")
$WALLET account get --raw --scope all --account-id "Public/$ACCOUNT"   # shard hex; echo <hex> | xxd -r -p -> Hola mundo!
```

- **The payer must be a real funded account that this wallet holds the key for.** Funds only enter a zone through an L1 bridge deposit (#510), and there's no first-party command for making one. In practice, ask someone with testnet funds to transfer to you, then import or receive it into the wallet.
- **It needs the full reserve for each transaction,** about 16,003,000 at the default gas limit and genesis base fees (#514). Most of it is refunded.
- **The call fee bug above applies here too.** Fix the runner before calling, or the call is rejected and the shard stays empty.
- **Check it landed.** Search for the account or the transaction in `https://explorer.testnet.lez.logos.co/`. A call that reverts is still included and charged, so an included transaction with an empty shard means a failure, not a missing block.
- **Deploying without the runner:** `$WALLET program-loader deploy --elf <bin> --header <account> --segments <account>... --payer Public/<id>`. Use one fresh public account for the header, plus one per 96 KiB of ELF for the segments. The header's account ID becomes the program's address.
- **Switch back.** Run `change-network local` (or use a different wallet home) before running a local sequencer again.

---

## 4. LEZ v0.4: the actor model on `dev` (local)

> **Not released.** There is no v0.4 tag or testnet yet. The endpoints page lists 0.4 as TBD, so local is the only option for now. This is the actor model that PR #935 merged into LEZ `dev`, checked at `e2913677b`. Build against `dev` and expect names to move. The steps below follow the code. The local run was tried on 2026-10-16 on branch `jzaki/fix-examples` (`be940d8f8`), which fixes the example runners. The deploy and the hello-world call both work there.

**The idea.** A program no longer rewrites accounts or plans effects. It is a handler that receives **one message** addressed to **one actor**, and an actor is one program's state on one account. The handler sees only that state, and returns the new state plus any messages it wants to send to other actors.

```
Account  (address + nonce)
 ├─ Actor (account, hello_world)  ─ state: "Hola mundo!"
 ├─ Actor (account, your program) ─ state: whatever it keeps
 └─ Actor (account, native token) ─ state: the balance
```

**Hello world** (`examples/program_deployment/methods/guest/src/bin/hello_world.rs`):

```rust
use lee_core::program::{ReceiveInput, Response};

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, greeting: Vec<u8>) -> Response {
    let mut bytes = input.pre_state.to_vec();
    bytes.extend(greeting);
    Response::set_state(bytes)
}
```

Hello world never checks `is_authorized`, so anyone can append to its state on any account. That is harmless: the program cannot touch the balance or any other program's state. `hello_world_with_authorization.rs` adds the check.

For what `ReceiveInput` and `Response` contain, and how sync and async messages differ, read the manual (`docs/actor-program-development-book/` on branch `schouhy/lez-guide-mdbook`): `foundations/messages.md` and `reference/api.md`. Its names are one rename behind `dev`: where it says `Call` and `Cast`, `dev` has `SyncMessage` and `AsyncMessage` ([the name map](GUIDE_v0.2_v0.4.md#read-this-next-to-the-actor-model-manual)). Its `foundations/toolchain.md` also covers testing a handler without a chain.

### Run it locally

Do section 1 first, with `git checkout jzaki/fix-examples` for the tag. That branch is `dev` at `e2913677b` plus fixes to the example runners (see the note below the runner). The pinned Rust is 1.98.1.

```bash
# Terminal 1: local sequencer, listening on 127.0.0.1:3040 by default. Dev mode skips proving.
RISC0_DEV_MODE=1 just run-sequencer-standalone

# Terminal 2: wallet. `just run-wallet` keeps its home in lez/wallet/configs/debug.
export LEE_WALLET_HOME_DIR=$PWD/lez/wallet/configs/debug   # the runner below reads the same wallet
just run-wallet check-health             # asks for a password the first time
just wallet-import-test-accounts         # imports and funds two test accounts; prints the list
PAYER=<first account id listed above>
just run-wallet account new public       # the account hello_world writes to
ACCOUNT=<account id printed above>
```

Then build the guests (section 1) and run the hello-world runner. It deploys the program through the program loader, so there is no separate deploy step:

```bash
(cd examples/program_deployment && \
  cargo run --release --bin run_hello_world -- "$BIN/hello_world.bin" "$ACCOUNT" "$PAYER")
```

**Expect a lot of output.** The runner, like `wallet program-loader deploy`, prints every transaction in full, including the program bytes in each segment upload. That comes to about 1 MB of numbers in the terminal, and it's normal. The lines to look for are `Transaction hash is …` and `Transaction is included in block N`. For hello world there are four pairs, one per deploy transaction: three segments and the header. The call itself prints nothing. To see only those, pipe the output through `grep -E 'Transaction (hash|is included)|rror|panicked'`.

> **On `dev` (`e2913677b`) this runner fails. Use branch `jzaki/fix-examples`, which fixes it.** On `dev`, the deploy miscounts segments (`SegmentCountMismatch`), the call declares no fee (`MissingFeeDeclaration`), and a fresh `ACCOUNT` isn't admitted, so the call reverts with `UnadmittedPublicActor`. The branch's commits fix all three. Their messages and diffs explain each change.
>
> - **What the branch covers.** The fee fix is in every `run_*` runner. The admission fix is only in `run_hello_world`. The other runners haven't been tried, so they may still revert on a fresh account that doesn't sign the call.
> - **Check the call yourself.** The runner doesn't check the call's result, even on the branch. A failed call still exits cleanly, so check the account (below).

Check the result. `--scope all` asks the sequencer for the whole account (`getAccount`), with every program's actor state listed separately:

```bash
just run-wallet account get --scope all --account-id "Public/$ACCOUNT"        # decoded
just run-wallet account get --raw --scope all --account-id "Public/$ACCOUNT"  # hex, under actor_states
echo <hex> | xxd -r -p                                                        # -> Hola mundo!
```

**"Account is Uninitialized" means the call reverted.** The wallet prints it when `getAccount` returns nothing, which means the account isn't on chain at all. Hello world never touches the balance, so a successful call is the only thing that creates `ACCOUNT`. A reverted call is still included and charged: the payer's nonce goes up, but nothing is written, and the wallet still says "included in block". With a runner from `dev` instead of the branch, the usual cause is that `ACCOUNT` wasn't admitted (`UnadmittedPublicActor`), then a missing fee. To confirm, query the sequencer directly:

```bash
curl -s -X POST http://127.0.0.1:3040 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getAccount","params":["'$ACCOUNT'"]}'   # null: never written
```

The revert reason appears only in the sequencer's log (Terminal 1).

**Deploying without the runner:**

```bash
just run-wallet account new public --label hdr      # the header; its id becomes the program's address
just run-wallet account new public --label seg0     # one segment account per 96 KiB chunk of the user ELF
just run-wallet account new public --label seg1
just run-wallet account new public --label seg2
just run-wallet program-loader deploy --elf "$BIN/hello_world.bin" \
  --header Public/<hdr id> \
  --segments Public/<seg0 id> Public/<seg1 id> Public/<seg2 id> \
  --payer Public/$PAYER
```

- **hello_world needs three segments.** The segment count is the size of the `.bin`'s `user_elf` divided by 96 KiB (`MAX_SEGMENT_DATA_LEN`), rounded up. It is not based on the whole `.bin`. `hello_world.bin` (305,616 bytes, tried on 2026-10-16) needs 3.
- **The count must match exactly.** If it doesn't, the deploy fails before sending anything, with `Program bytecode splits into N segment(s) but M were supplied`. Create N segment accounts and run the deploy again, passing them in order.
- **Done looks like** `Program deployed. Header account id: <hdr id>`.

The `--immutable` flag marks the program as immutable (not enforced by the protocol). The runner's `deploy_program` passes it, so a runner-deployed program is immutable. Run `wallet program-loader deploy --help` for the exact argument form. `examples/program_deployment/README.md` still says `wallet deploy-program`, which no longer exists.

**Reset:** stop the sequencer and run `just clean`.

### Your own program

Every file in the guest's `src/bin/` becomes a program, and every file in the runner's `src/bin/` becomes a runner, so neither needs a manifest change.

1. **Copy the program.** Copy `hello_world.rs` (or `hello_world_with_move_function.rs`, for several message types) to `my_program.rs` in the same folder.
2. **Define the message.** Use a borsh `enum Message` with one variant per operation. Guard any variant meant only for your own program with `input.from_own_program()`.
3. **Copy a runner.** On `jzaki/fix-examples`, copy `run_hello_world.rs` to `run_my_program.rs`, then change it to send the message you defined. The branch's version already declares the fee and admits `ACCOUNT`.
4. **Rebuild and deploy.** A code change gives the program a new image ID. Redeploy to a new header, or update the existing one with `wallet program-loader update`.
5. **Test the handler without a chain.** Build a `ReceiveInput` by hand and call the handler. The manual's `foundations/toolchain.md` shows how, and `lez/programs/clock/src/main.rs` has a worked example.

What to design around, such as authorization, who sent a message, stored messages and replies, is in the manual's Part I. If you are porting a v0.2 program, use [GUIDE_v0.2_v0.4.md](GUIDE_v0.2_v0.4.md).

### Where programs interact

- **Sync messages** run the other actor in the same transaction, depth-first, and roll back with it. `simple_tail_call.rs` is just `Response::keep_state().send_sync(callee, &greeting)`. The callee address comes in the message.
- **Async messages** are delivered later if the receiver can't be run in this transaction. Native credits to private accounts work this way, and the recipient collects them with `wallet pending list` and `wallet pending receive --position N`.
- **PDAs.** `tail_call_with_pda.rs` shows a program authorizing an account it controls with `SyncMessage::new(..).with_pda_seeds(..)`.

---

## 5. Example programs to read

### Tutorial examples: `examples/program_deployment/` (on `dev`: v0.4)

These are meant to be read and copied. Guests are in `methods/guest/src/bin/`, and runners are in `src/bin/run_*.rs`. All of them use `actor_entrypoint!`.

| Program | What it shows | Runner(s) |
|---|---|---|
| `hello_world.rs` | Smallest program: appends the message bytes to this program's own actor state. No authorization | `run_hello_world`, `run_hello_world_private` |
| `hello_world_with_authorization.rs` | Same, but panics unless the account is authorized (signature, or `ask` for private accounts) | `run_hello_world_with_authorization` |
| `hello_world_with_move_function.rs` | `Write`, `MoveData` and `Append` messages. A sync message moves data between a public account and a private one, and `Append` is guarded by `from_own_program()` | `run_hello_world_with_move_function` (`write-public`, `write-private`, `move-data-public-to-private`, …) |
| `simple_tail_call.rs` | Composition: sends a sync message to `hello_world`. The callee is passed in the message | `run_hello_world_through_tail_call`, `…_private` |
| `tail_call_with_pda.rs` | A program-derived account authorized by seed through a sync message to `hello_world_with_authorization` | `run_hello_world_with_authorization_through_tail_call_with_pda` |

`examples/program_deployment/README.md` is a tutorial, but it **lags the code**. It still says `wallet deploy-program <bin>`, and on `dev` the runners it describes lack the fee fix, which is on branch `jzaki/fix-examples`. Its account-inspection steps (`wallet account get --raw --scope all`, decoding the hex) are still correct.

### The native token: `lee/state_machine/core/src/native_token.rs`

The simplest complete actor program, and the model for a transfer, a custody transfer from a PDA, and a read/reply. It is not a guest binary. The engine and the privacy circuit run it directly. The manual covers it in `foundations/system-programs.md`.

### Built-in programs: `lez/programs/`

Production programs, deployed at genesis. Prebuilt binaries are committed in `artifacts/lez/programs/*.bin`. Each has a `core/` crate for its message and state types and a `src/` guest.

| Program | Purpose |
|---|---|
| `clock` | Block ID and timestamp accounts. Other programs *assert* about them with `AssertTimestamp`. The best example of handler unit tests |
| `fee` | Fee market state and distribution. Uses the read/reply pattern against the native token |
| `bridge`, `bridge_lock`, `wrapped_token` | Bridging assets to and from Logos Blockchain. `bridge` is the model for custody from a PDA and for exactly-once receipts |
| `cross_zone_inbox`, `cross_zone_outbox`, `cross_zone_marker` | Cross-zone messaging |
| `ping_sender`, `ping_receiver` (+ `ping_core`) | Minimal cross-zone message example. The best starting point for cross-zone work |
| `sequencer_stake` | Sequencer staking, join/exit and slashing |
| `program_loader/core` | Types for the loader that stores deployed programs as header + segment accounts |

The token, AMM and associated-token-account programs that shipped with v0.3.0 were removed from `dev` (commit `c16cf3000`). To read them, check out the `v0.3.0` tag.

### Test programs

- `test_programs/guest/src/bin/`: `cooldown` (clock-gated), `time_locked_transfer`, `listener`, `live_payout`, `scripted`.
- `lee/state_machine/test_methods/guest/src/bin/`: `flash_swap_initiator` and `flash_swap_callback` (read a balance, lend, run a callback, re-check, revert if not repaid), `forges_echo`, `malformed_journal` and others that test the engine's checks.

---

## 6. Example programs to read (v0.3.0 tag)

All paths are in `logos-blockchain/logos-execution-zone` at tag **`v0.3.0`** (`db66590`). The local checkout is on `dev`, which has moved on, so read these with `git show v0.3.0:<path>` or check out the tag. They are all plain `lee_core` guest programs that use `plan`/`apply`.

### Tutorial examples (v0.3.0)

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

### Programs removed from `dev`

At v0.3.0, `lez/programs/` also held `token`, `amm` and `associated_token_account` (docs: trade tokens, liquidity pool pages). They are not on `dev`. The other built-ins and the test programs are listed in section 5.
