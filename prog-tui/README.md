# prog-tui

A terminal UI for deploying and testing the LEZ example programs locally. It runs the steps of [WALKTHROUGH.md](../example_progs/WALKTHROUGH.md) for you, and remembers the account and program ids they print.

## Run it

```bash
cd jz-progs/prog-tui
cargo run --release
```

It expects the LEZ checkout at `../../logos-execution-zone`, which is where it sits next to `jz-progs` now. Press `L` to point it somewhere else; the path is saved.

**Deploy and call need `prog_cli`.** That's `examples/program_deployment/src/bin/prog_cli.rs` in the LEZ checkout (branch `jzaki/fix-examples`). Without it, `r` still runs the stock `run_*` runners, which deploy again on every call.

Everything else the walkthrough needs must already be installed: the RISC Zero toolchain, `just`, and LEZ's build dependencies. prog-tui doesn't use `nix develop`; start it from inside that shell if you build that way.

**Press `W` on a fresh start.** It runs the whole walkthrough as a checklist:

1. build;
2. reset the chain and start the sequencer;
3. initialise the wallet and import the test accounts;
4. set `PAYER` and create `ACCOUNT`;
5. deploy `hello_world`;
6. call it and check the state reads "Hola mundo!".

## Layout

- **Status bar:** the LEZ checkout (branch, commit, dirty, whether `prog_cli` is built), the sequencer (stopped, starting, or running; started by prog-tui or not; block height), and `PAYER` / `ACCOUNT`.
- **Accounts:** the wallet's accounts. ★ marks `PAYER`, ▶ marks `ACCOUNT`, and each shows its balance, or `off-chain` when it doesn't exist on chain yet. Deploys create a header and some segment accounts in the wallet; `h` shows or hides them.
- **Account:** the selected account's nonce, balance, and every program's actor state on it. Each state is labelled by program name when prog-tui deployed that program, and shown as text when it's printable.
- **Programs:** the built guests (`$BIN`) and the programs deployed from here. Crossed out means no longer on chain, after a reset.
- **Log:** each command and its output. The full output of every job is in `.state/logs/`.

## Keys

| Key | Does |
|---|---|
| `W` | Walkthrough. Asks first, because it resets the chain and the wallet |
| `s` | Start the sequencer, or stop it. Asks first when prog-tui didn't start it |
| `R` | Reset: stop, `just clean`, start. Wipes the chain and the wallet |
| `b` | Build the wallet, runners, `prog_cli` and the example guests |
| `i` | Initialise the wallet and import the two funded test accounts |
| `n` | New public account, with an optional label |
| `P` / `A` | Make the selected account `PAYER` / `ACCOUNT` |
| `d` | Deploy the selected guest with `prog_cli` (Guests pane) |
| `r` | Run the selected guest's stock `run_*` runner |
| `c` | Call the selected deployment (Deployed pane). The form is prefilled; Enter sends |
| `m` | `move-data`, for `hello_world_with_move_function` |
| `x` | Forget the selected deployment |
| `e` | Print shell commands for the sequencer (watch its log, use it, stop it) and the `source` line for `env.sh` |
| `L` | Change the LEZ checkout |
| `Tab`, `↑↓`/`jk` | Switch pane, select |
| `?` / `q` | Help / quit |

**What each deployment's call sends:**

| Deployment | `c` sends | Notes |
|---|---|---|
| `hello_world` | `prog_cli hello` | Appends text to the account's state |
| `hello_world_with_authorization` | `hello --sign` | The account signs |
| `hello_world_with_move_function` | `move-write`, or `move-data` with `m` | |
| `simple_tail_call` | `tail` | The callee is prefilled with the latest `hello_world` |
| `tail_call_with_pda` | `pda` | The callee is prefilled with the latest `hello_world_with_authorization` |

**Every call is checked.** `prog_cli` waits for the transaction to be included, then reads the target state back. A call that leaves it unchanged was reverted (it is still included and charged). When that happens, the log shows the reason, taken from the sequencer's log: a guest panic, or an engine error such as `UnadmittedPublicActor`. This only works for a sequencer prog-tui started.

## The sequencer

prog-tui starts the sequencer as `RISC0_DEV_MODE=1 just run-sequencer-standalone`, in its own session. It keeps running after you quit, and prog-tui finds it again next time.

- **Process group:** in `.state/sequencer.pid`.
- **Output:** in `.state/sequencer.log`.
- **Started elsewhere?** A sequencer started any other way shows as "external". prog-tui asks before stopping it.
- **From another terminal:** there's no console to attach to, since the sequencer has no terminal. You can still watch its log, use it through the wallet or RPC, and stop it. Press `e` to print those commands with the real paths and pid filled in.

## Your shell's env vars

prog-tui rewrites `.state/env.sh` on every change. It exports:

- `LEZ_DIR`, `LEE_WALLET_HOME_DIR`, `BIN`;
- `PAYER`, `ACCOUNT`;
- `PROGRAM_<NAME>` for each deployment.

Load it in bash or zsh, and the walkthrough's commands work as written:

```bash
source jz-progs/prog-tui/.state/env.sh
cd "$LEZ_DIR" && just run-wallet account get --raw --scope all --account-id "Public/$ACCOUNT"
```

## Using `prog_cli` without the TUI

```bash
export LEE_WALLET_HOME_DIR=$PWD/lez/wallet/configs/debug SUPPRESS_VERBOSE_PRINTS=1
target/release/prog_cli --payer "$PAYER" deploy "$BIN/hello_world.bin"
target/release/prog_cli --payer "$PAYER" hello --program <id> --account "$ACCOUNT" --text 'Hola mundo!'
```

Run `prog_cli --help` for the other calls. `SUPPRESS_VERBOSE_PRINTS=1` stops the wallet printing every transaction in full.
