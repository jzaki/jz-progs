# LEZ program review: scaffold, SPEL, and a first program

Sources, read on 2026-10-08:

- `logos-co/scaffold` at **v0.4.0** (latest release): `README.md`, `docs/commands.md`, `src/constants.rs`, `templates/`
- `logos-co/spel` at **v0.7.0** (latest release; `v0.7.0-rc.2` is newer but pre-release): `README.md`
- `logos-blockchain/logos-execution-zone` at **v0.1.2** (what scaffold pins) and **v0.3.0** (Testnet v0.3.0), from the local checkout in `ref-repos/`: `examples/`, `lez/programs/`, `test_programs/`, `lez/testnet_initial_state/`, `README.md`
- This repo: `docs/lez/programs/write-and-deploy-lez-program-with-scaffold.md` (on `main`, and as rewritten by open PR #442)
- `logos-co/logos-lips` at `fddc2c1` (2026-10-06), cloned into `ref-repos/logos-lips`: LIP 237 (LEE v0.3 Specifications) and related LIPs (section 8)
- `logos-co/logos-docs` [Journey] issues #506–#511 and #514, the LEZ developers' notes for testnet v0.3 (section 6). Issue numbers like `#514` in this file refer to these. LEZ PRs are written "PR 875"

Nothing below has been run. The commands come from the upstream READMEs and templates at the tags above.

> **Version gap.** scaffold v0.4.0 pins **LEZ v0.1.2** and **SPEL v0.5.0** (`DEFAULT_LEZ` and `DEFAULT_SPEL` in `src/constants.rs`). scaffold `master` (`d4b15ff`, 2026-10-05) pins the same versions. The docs site targets **Testnet v0.3.0** (LEZ tag `v0.3.0`, 2026-09-29), and the latest SPEL is **v0.7.0**. LEZ v0.3.0 changed the program model: accounts now hold per-program **shards**, programs are written as `plan`/`apply` functions through `run_program`, and deploys go through `program_loader` and need a fee payer. So code built with scaffold's defaults does not match the testnet. Sections 4 and 5 use LEZ v0.3.0 directly.

---

## 1. What is scaffold

`logos-scaffold` (binary alias `lgs`) is the project CLI for LEZ programs. It works like `forge init`/`anvil` or `anchor init`/`solana-test-validator`. It creates a project, pins LEZ and SPEL, builds a sequencer and wallet for that project, and runs the build → deploy loop against a local sequencer. It supports only the standalone-sequencer flow and does not depend on `logos-blockchain`. It runs on Unix only.

### Components

| Component | What it is |
|---|---|
| `scaffold.toml` | Project config: LEZ/SPEL pins (`[repos.*]`), `[circuits]`, `[localnet]`, framework choice |
| `.scaffold/` | Project-local state: wallet home (`.scaffold/wallet`), circuits download, default-wallet record, test-node dirs |
| Templates | `default`: the LEZ `program_deployment` example (raw `nssa_core` hello_world programs plus Rust runner binaries). `lez-framework`: a SPEL-based `lez_counter` program with IDL and client generator |
| `methods/guest/src/bin/*.rs` | Guest programs. Each file becomes one deployable program, with a RISC Zero image ID as its `program_id` |
| `src/bin/run_*.rs` | Host-side runners that build, sign and submit transactions |
| `lgs localnet` | One long-lived, daemonised sequencer per project on `127.0.0.1:3040` |
| `lgs test-node` | Disposable sequencers for integration tests, each with its own port, DB and state |
| `lgs wallet` / `lgs spel` | Passthroughs to the project's own `wallet` and `spel` binaries, at the project's pinned versions |
| `logos_scaffold::api` | The same features as a typed Rust library, for tests and tooling |

### Prerequisites

- `git`, Rust ≥ 1.81 with `cargo`, `lsof`, `ps`, `kill`, `curl`
- RISC Zero via `rzup`: `rzup install rust` and `rzup install cargo-risczero 3.0.5`. `r0vm` must be exactly 3.0.5: without it `localnet start` still reports ready, but `deploy` fails (PR #442)
- Docker/Podman is optional for guest builds. Nix is needed only for `lgs basecamp`

### Key commands

```bash
cargo install logos-scaffold            # installs logos-scaffold + lgs
lgs new my-app                          # default template; add --template lez-framework for SPEL
lgs run                                 # setup → build → IDL → localnet → topup → deploy, in one step
lgs run --watch                         # re-run on every file change
lgs doctor                              # what's missing / misconfigured (--json for CI)
lgs localnet status | logs --tail 200 | stop
lgs wallet list
lgs wallet -- <any wallet subcommand>   # e.g. account new public
lgs spel -- <any spel subcommand>       # e.g. inspect, generate-idl
```

If you need to debug one phase, run the stages one at a time: `lgs setup`, `lgs localnet start`, `lgs build`, `lgs deploy`, `lgs wallet topup`.

### Notes

- Port `3040` is hard-wired. Changing `[localnet].port` does not move the sequencer (scaffold 0.3.0 and 0.4.0).
- The first `setup` compiles LEZ and `spel` from source. It takes several minutes and about 5 GB of disk.
- The docs page on `main` still says to `git clone --branch v0.4.0` and lists Nix as a prerequisite. PR #442 changes it to `cargo install` and drops Nix. That PR currently conflicts with `main`.

---

## 2. What is SPEL

SPEL (`logos-co/spel`, crate `spel-framework`) is an Anchor-style framework for LEZ programs. You write instruction handlers in a `#[lez_program]` module. SPEL then generates the instruction dispatcher, account validation, a JSON IDL, and a complete CLI that submits transactions to those instructions. It is optional: a program can also be raw `nssa_core` code, like the scaffold `default` template.

### Components

| Component | What it is |
|---|---|
| `spel-framework` (proc macros) | `#[lez_program]`, `#[instruction]`, `#[account(...)]`, `#[account_type]`, `SpelResult`/`SpelOutput`/`SpelError` |
| Account attributes | `mut`, `init`, `signer`, and PDA seeds `pda = literal("x")` / `account("a")` / `arg("k")`. `signer` and `init` get automatic runtime checks |
| IDL generator | `spel_framework::generate_idl!("path/to/guest.rs")` or `spel generate-idl`. It produces self-contained JSON that is a superset of the lssa-lang IDL |
| `spel` CLI (`spel-cli`) | Turns IDL instructions into subcommands, derives PDAs, serialises arguments, signs and submits through the wallet. Supports `--dry-run[=json]` and `inspect` |
| `spel init` project | Workspace with `<name>_core` (shared types), `methods/guest` (on-chain program), `examples/` (IDL generator and a 3-line CLI wrapper), and a `Makefile` |
| Extension libraries | Third-party crates whose `#[instruction]`s merge into your program, enabled by a marker attribute. Examples: `admin-authority`, `freeze-authority` (see `docs/lez/extensions/`) |

### Shape of a program

```rust
#[lez_program]
mod my_program {
    use super::*;

    #[instruction]
    pub fn initialize(
        #[account(init, pda = literal("state"))] state: AccountWithMetadata,
        #[account(signer)] owner: AccountWithMetadata,
    ) -> SpelResult {
        Ok(SpelOutput::states_only(vec![
            AccountPostState::new_claimed(state.account.clone(), Claim::Authorized),
            AccountPostState::new(owner.account.clone()),
        ]))
    }
}
```

### Key commands

```bash
cargo install --path spel-cli           # from a spel clone; installs `spel` (needs rustc ≥ 1.94.0)
spel init my-program && cd my-program
make build && make idl && make deploy
make cli ARGS="--help"                  # auto-generated subcommands

spel inspect program.bin                # program ID from a binary
spel generate-idl methods/guest/src/bin/my_program.rs > my_program-idl.json
spel --idl my_program-idl.json --dry-run -p program.bin -- initialize --owner-account <BASE58>
spel --idl my_program-idl.json --program <64-char-hex> -- <instruction> [args]
spel inspect <account-id> --idl my_program-idl.json --type VaultState
```

In a scaffold project you don't install `spel` yourself: use `lgs spel -- …`, which runs the version scaffold pinned (v0.5.0 for scaffold v0.4.0).

### Notes

- `--program` accepts only a 64-char hex ID, a name from `spel.toml`, or a path. A base58 image ID is read as a file path and fails (`create-and-transfer-custom-tokens…md`).
- The extension pages in this repo pin SPEL at an unreleased commit (`8183b01`). That commit predates v0.7.0 and should now be replaceable with a release tag.

---

## 3. First Logos program: hello world

> **This section targets LEZ v0.1.2, not Testnet v0.3.0.** The program below uses the pre-shard API (`read_nssa_inputs`, `AccountPostState`, account claiming). For Testnet v0.3.0, use section 5.

This uses scaffold's `default` template. Its `hello_world` guest program appends the instruction bytes to an account's `data`, and claims the account on first use. The runner sends `"Hola mundo!"`.

**The program** (`methods/guest/src/bin/hello_world.rs`, from LEZ v0.1.2, shortened):

```rust
use nssa_core::program::{AccountPostState, Claim, ProgramInput, ProgramOutput, read_nssa_inputs};

fn main() {
    let (ProgramInput { self_program_id, caller_program_id, pre_states, instruction: greeting }, instruction_data)
        = read_nssa_inputs::<Vec<u8>>();
    let [pre_state] = pre_states.try_into().unwrap_or_else(|_| panic!("expected one account"));

    let mut account = pre_state.account.clone();
    let mut bytes = account.data.into_inner();
    bytes.extend_from_slice(&greeting);
    account.data = bytes.try_into().expect("data too large");

    let post_state = AccountPostState::new_claimed_if_default(account, Claim::Authorized);
    ProgramOutput::new(self_program_id, caller_program_id, instruction_data, vec![pre_state], vec![post_state])
        .write(); // nothing happens without .write()
}
```

**Create, deploy, call, check** (requires the section 1 prerequisites):

```bash
cargo install logos-scaffold
lgs new hello && cd hello
RISC0_DEV_MODE=1 lgs run                       # build + local sequencer + deploy; dev mode skips proving

lgs wallet -- account new public               # a fresh account for hello_world to claim
export NSSA_WALLET_HOME_DIR="$PWD/.scaffold/wallet" LEE_WALLET_HOME_DIR="$PWD/.scaffold/wallet"
RISC0_DEV_MODE=1 cargo run --bin run_hello_world -- <PUBLIC_ACCOUNT_ID>

lgs wallet -- account get --account-id Public/<PUBLIC_ACCOUNT_ID>   # data should hold "Hola mundo!"
```

Why each step is needed:

- **A fresh account.** `hello_world` succeeds only on an uninitialised account or one it already owns. The funded default wallet account is owned by another program, so the call is rejected.
- **Both wallet env vars.** LEZ v0.1.2 reads `NSSA_WALLET_HOME_DIR`, and v0.2.0+ reads `LEE_WALLET_HOME_DIR` (PR #442). Exporting both works with either pin.
- **The runner signs the transaction.** Claiming the account needs `Claim::Authorized`. An unsigned transaction is rejected with `InvalidProgramBehaviour`.
- **No `program_id` to copy.** The runner embeds `HELLO_WORLD_ELF` and derives the program ID from it.

To edit it, change `hello_world.rs` (or the `greeting` bytes in `src/bin/run_hello_world.rs`) and run `lgs run` again. scaffold redeploys only what changed.

**SPEL version of the same loop:** `lgs new hello --template lez-framework && cd hello && lgs run` deploys `lez_counter` (`initialize`, then `increment <amount>`). Its README describes the calls through `src/bin/run_lez_counter.rs` or the generated IDL at `idl/lez_counter.json`.

### Open questions to check by running it

1. Does `lgs run` apply `RISC0_DEV_MODE=1` to the guest build and the localnet it starts, or does it need another flag?
2. Does `lgs wallet -- account get --account-id Public/<id>` decode `data`, or print only raw bytes?
3. ~~Does the hello world work against Testnet v0.3.0 with these pins?~~ Almost certainly not: LEZ v0.3.0 changed the program API and the deploy flow (see the version gap note above).

---

## 4. Example Logos programs for Testnet v0.3.0

All paths are in `logos-blockchain/logos-execution-zone` at tag **`v0.3.0`** (`db66590`). The local checkout in `ref-repos/logos-execution-zone` is on `dev`, which is ahead of that tag, so read these with `git show v0.3.0:<path>` or check out the tag. None of them use SPEL. They are raw `lee_core` guest programs.

### Tutorial examples: `examples/program_deployment/`

These are meant to be read and copied. Guests are in `methods/guest/src/bin/`, and runners are in `src/bin/run_*.rs`.

| Program | What it shows | Runner(s) |
|---|---|---|
| `hello_world.rs` | Smallest program: `plan` selects one account, `apply` appends the instruction bytes to this program's shard. No authorization | `run_hello_world`, `run_hello_world_private` |
| `hello_world_with_authorization.rs` | Same, but panics unless the account is authorized (signature, or `ask` for private accounts) | `run_hello_world_with_authorization` |
| `hello_world_with_move_function.rs` | Two instructions, `write` and `move_data`, moving shard data between a public account and a private one | `run_hello_world_with_move_function` (`write-public`, `write-private`, `move-data-public-to-private`, …) |
| `simple_tail_call.rs` | Program composition: chain-calls `hello_world`. Hard-codes hello_world's program ID | `run_hello_world_through_tail_call`, `…_private` |
| `tail_call_with_pda.rs` | Program-derived account (PDA) authorized by seed through a chained call to `hello_world_with_authorization` | `run_hello_world_with_authorization_through_tail_call_with_pda` |

`examples/program_deployment/README.md` is the tutorial, but **at v0.3.0 it lags the code**. It still says `wallet deploy-program <bin>` and runs the runners with two arguments. The v0.3.0 runners take a third argument, a funded **payer** account, and deploy the program themselves through `program_loader` (`examples/program_deployment/src/lib.rs`). Its account-inspection steps (`wallet account get --raw --scope all`, decoding the shard hex) are still correct. The runners' call transactions also seem to predate fees (see the warning in section 5).

Issue #509 names `hello_world` as "the canonical minimal program". For a realistic program it points to the **token program**:
- `lez/programs/token/src/main.rs` is just `run_program(token_program::plan, token_program::apply)`.
- The routing is in `src/lib.rs`: a pure `plan` over account handles and a nine-variant `Effect` enum.
- A transfer emits exactly one effect per shard, so the recipient's `apply` never sees the sender's data (`src/transfer.rs`).

For emitting events, the model is `lez/programs/bridge` (#507).

### Built-in programs: `lez/programs/`

These are production programs, deployed at genesis. Prebuilt binaries are committed in `artifacts/lez/programs/*.bin`. Each has a `core/` crate (instruction and state types) and a `src/` guest.

| Program | Purpose |
|---|---|
| `token` | Fungible and NFT token definitions and holdings (the docs' custom-token page drives it with `spel`) |
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

---

## 5. Build and run locally with the fewest dependencies (no scaffold, no SPEL)

This path builds **two binaries** (sequencer and wallet) and **one guest crate**, all from the LEZ repo at `v0.3.0`. It does not need Docker, Bedrock/Logos Blockchain, the indexer, the explorer, Nix, `just`, scaffold or SPEL.

How each component is avoided:

- **Bedrock.** The sequencer's `standalone` feature replaces it with a mock (`lez/sequencer/service/Cargo.toml`: `standalone = ["sequencer_bedrock_actor/mock"]`).
- **Docker.** Only `cargo risczero build` uses it. The example's `methods` crate uses `risc0_build::embed_methods()`, which builds guests with the local `rzup` toolchain. The trade-off is that the build isn't reproducible, so program IDs can differ from a Docker build (`lez/programs/README.md`).
- **A faucet.** There is none in v0.3.0: PR 875 removed it (#510). Instead, the debug sequencer config's `supply_account` genesis entries fund `CbgR6tj5…` and `2RHZhw9h…` as synthetic bridge deposits, and their signing keys are public test keys in the `wallet-import-test-accounts` recipe of the `Justfile` (#511, #514).

`just` wraps every step below (`run-sequencer-standalone`, `run-wallet`, `wallet-import-test-accounts`, `clean`). The commands here are those recipes written out in full, so `just` itself is optional. Issue #514 runs the same setup through `just` and shows the expected output.

### Prerequisites

```bash
sudo apt install build-essential clang libclang-dev libssl-dev pkg-config   # Fedora/macOS: see the LEZ README
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh              # rust-toolchain.toml pins 1.98.1
curl -L https://risczero.com/install | bash && rzup install                 # restart your shell first
```

### Steps

```bash
cd ref-repos/logos-execution-zone
git checkout v0.3.0                      # the checkout is on `dev`; match the testnet

# Terminal 1: standalone sequencer, RPC on :3040, one block every 15 s (= `just run-sequencer-standalone`).
# Run it from lez/sequencer/service: the config's relative `home` resolves there.
# RISC0_DEV_MODE=1 skips proving and disables proof verification (local only).
# The RPC binds 0.0.0.0 by default and has no auth, hence --listen-address (#514).
(cd lez/sequencer/service && RISC0_DEV_MODE=1 RUST_LOG=info,kameo=warn \
  cargo run --release --features standalone -p sequencer_service -- \
  configs/debug/sequencer_config.json --listen-address 127.0.0.1)
# -> Publishing block 2 on pin <msg id>

# Terminal 2: wallet, in its own home so it doesn't write into the repo
mkdir -p ~/lez-wallet && cp lez/wallet/configs/debug/wallet_config.json ~/lez-wallet/
export LEE_WALLET_HOME_DIR=~/lez-wallet
cargo install --locked --path lez/wallet
wallet check-health                      # asks for a password the first time

# A funded payer: import the genesis-funded test account (= first line of
# `just wallet-import-test-accounts`)
wallet account import public \
  --private-key 7f273098f25b71e6c005a9519f2678da8d1c7f01f6a27778e2d9948abdf901fb
wallet account list                      # -> Public/CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r (#514)
PAYER=CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r
wallet account new public                # the account hello_world writes to
ACCOUNT=<account id printed above>

# Optional: the fee market the deploy and call are priced against (#514)
curl -s -X POST http://127.0.0.1:3040 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getFeeState","params":[]}'
# -> ..."base_fee_exec":8,"base_fee_stor":8,...

# Build the example guests with the local risc0 toolchain (no Docker)
cargo build --release -p example_program_deployment_methods
BIN=$PWD/target/riscv-guest/example_program_deployment_methods/example_program_deployment_programs/riscv32im-risc0-zkvm-elf/release

# Deploy hello_world (paid by PAYER) and call it, in one runner
cd examples/program_deployment
cargo run --release --bin run_hello_world -- "$BIN/hello_world.bin" "$ACCOUNT" "$PAYER"

# Check the result
wallet account get --raw --scope all --account-id "Public/$ACCOUNT"
echo <shard hex> | xxd -r -p             # → Hola mundo!
```

The imported key is a public test key from the debug config. Never use it outside a local chain.

> **The hello_world call probably gets rejected at v0.3.0 (found by reading the code, not by running it).** `run_hello_world` deploys through `deploy_program(…, payer)`, which declares a fee, so the deploy should land. It then builds the **call** with `Message::try_new`, which the v0.3.0 docs describe as "a fee-exempt message (`fee: None`). Correct for system transactions". It signs with an empty witness set. `lez/chain_state/src/classify.rs` exempts only bridge deposits, cross-zone dispatches, cross-zone locks, staking and genesis. Every other user public transaction without a fee fails with `MissingFeeDeclaration` (#514: "If one omits the fee declaration, that transaction is rejected"). `dev` hasn't changed this runner.
>
> **Fix:** replace the hand-built `Message`, `WitnessSet` and `send_transaction` with the wallet helper `wallet_core.send_pub_tx_paid_by(accounts, instruction_data, program_account_id, Some(payer))` (`lez/wallet/src/lib.rs`, around line 883). It adds the `FeeDeclaration` (from the wallet's `gas_limit`), and `PAYER` co-signs without becoming one of the program's accounts. For a call site to copy, see `lez/wallet/src/program_facades/amm.rs`. The same fix applies to `run_hello_world_through_tail_call`. The `_with_authorization` runner signs, but still uses `try_new`, so it needs checking too.

**Fees, as they apply here (#514):**
- The payer, `CbgR6tj5…` with 10,000,000,000,000 at genesis, pays both the deploy and the call. It must hold the **reserve**, not just the final fee: `gas_limit × base_fee_exec + tx_bytes × base_fee_stor + tip`, about 16,003,000 at the defaults. The unused part is refunded.
- `ACCOUNT` never pays, so it doesn't need funding. hello_world writes to a shard on it and checks no authorization.
- The deploy pays storage gas only. The call pays execution gas plus storage gas.
- A call that **reverts is still included and charged**. The wallet still prints "included in block", and the shard is unchanged. If the shard is empty, suspect `gas_limit` first (`wallet config get gas_limit`; the default is 2,000,000).
- `account get` without `--scope all` shows only the native balance, so the hello_world shard looks missing.

**Your own program:** copy `hello_world.rs` to a new file in `methods/guest/src/bin/`. Change `plan` (which accounts and shards to touch) and `apply` (new shard bytes from the old shard plus the instruction). Rebuild the methods crate, then point a copy of `run_hello_world.rs` at the new `.bin`. `apply` returns `None` to keep the shard, `Some(empty)` to clear it, and `Some(bytes)` to replace it (#509). Section 6 covers the rest of the `Plan` API. The program is just:

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

**Reset:** stop the sequencer, then delete `lez/sequencer/service/sequencer_home`, `lez/sequencer/service/rocksdb*`, `lez/sequencer/service/bedrock_signing_key` and `~/lez-wallet`. These are the non-Docker parts of `just clean`.

**Even less building:** the public testnet removes the sequencer build, so you only need the wallet and the guest. Point the wallet at the testnet: the RPC is `https://testnet.lez.logos.co` (#511), and setup is in `docs/lez/get-started/run-lez-wallet-via-cli.md`. The payer then has to be a real account. Funds can only enter through an L1 bridge deposit (#510), and the payer must hold the full reserve, about 16,003,000 at the default gas limit and genesis base fees (#514).

### Open questions for section 5 (not run)

1. ~~Does `sequencer_service` build without `logos-blockchain-circuits`?~~ Probably yes. #514 builds and runs the standalone sequencer at v0.3.0 with only the README's build dependencies, and lists no circuits step.
2. ~~Does the imported key derive to `CbgR6tj5…`?~~ Resolved: the key is now the Justfile's, and #511 and #514 both show it importing as `Public/CbgR6tj5…`. The `testnet_initial_state` key I used before was wrong for this config.
3. Is the `riscv-guest/…/release` output path above right for this workspace? `cargo build` prints the `.bin` locations if it isn't.
4. Is the default `gas_limit` (2,000,000) enough to deploy and call hello_world? The token-send example in #514 used 34,583 cycles, so hello_world should fit easily.

