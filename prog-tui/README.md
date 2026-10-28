# prog-tui

A terminal UI for deploying and testing the LEZ example programs locally. It runs the steps of [WALKTHROUGH.md](../example_progs/WALKTHROUGH.md) for you, and remembers the account and program ids they print.

## Run it

```bash
cd jz-progs/prog-tui
cargo run --release
```

It expects the LEZ checkout at `../../logos-execution-zone`, which is where it sits next to `jz-progs` now. Press `L` to point it somewhere else; the path is saved.

**Calls need [`lez-call`](../../lez-prog-call/README.md)** (a checkout at `../../lez-prog-call`, or on `PATH`) and the wallet's `program call` subcommand (LEZ branch `jzaki/fix-examples`). **Planning them needs [`lez-dryrun`](../lez-dryrun/README.md)**, in `jz-progs/lez-dryrun`; without it, a call is sent as given, unchecked. `b` builds both. [LEZCALL.md](../LEZCALL.md) is their design. Deploys need only the wallet. Without `lez-call`, `r` still runs the stock `run_*` runners, which deploy again on every call.

Everything else the walkthrough needs must already be installed: the RISC Zero toolchain, `just`, and LEZ's build dependencies. prog-tui doesn't use `nix develop`; start it from inside that shell if you build that way.

**Press `W` on a fresh start.** It runs the whole walkthrough as a checklist:

1. build;
2. reset the chain and start the sequencer;
3. initialise the wallet and import the test accounts;
4. set `PAYER` and create `ACCOUNT`;
5. deploy `hello_world`;
6. call it and check the state reads "Hola mundo!".

## Layout

- **Status bar:** the LEZ checkout (branch, commit, dirty, whether `lez-call` and `lez-dryrun` are built), the sequencer (stopped, starting, or running; started by prog-tui or not; block height), and `PAYER` / `ACCOUNT`.
- **Accounts:** the wallet's accounts. ★ marks `PAYER`, ▶ marks `ACCOUNT`, and each shows its balance, or `off-chain` when it doesn't exist on chain yet. Deploys create a header and some segment accounts in the wallet; `h` shows or hides them.
- **Account:** the selected account's nonce, balance, and every program's actor state on it. Each state is labelled by program name when prog-tui deployed that program, and shown as text when it's printable.
- **Programs:** three lists. *Guests* are the built example guests (`$BIN`). *Deployed* are the programs deployed from here; crossed out means no longer on chain, after a reset. *Built-in* is read-only: the native token and program loader (both run by the engine), and the guests in `lez/programs/` (clock, fee, bridge and so on). Those guests are dimmed when the current chain didn't deploy them, which on the dev chain is all but `clock`, `fee`, `bridge` and `sequencer_stake`. Select one to see its full address and where it comes from. Their names also label the actor states in the Account pane.
- **Log:** each command and its output. The full output of every job is in `.state/logs/`.

## Keys

| Key | Does |
|---|---|
| `W` | Walkthrough. Asks first, because it resets the chain and the wallet |
| `s` | Start the sequencer, or stop it. Asks first when prog-tui didn't start it |
| `R` | Reset: stop, `just clean`, start. Wipes the chain and the wallet |
| `b` | Build the wallet, runners and example guests in LEZ, then `lez-call` and `lez-dryrun` in their checkouts |
| `i` | Initialise the wallet and import the two funded test accounts |
| `n` | New public account, with an optional label |
| `P` / `A` | Make the selected account `PAYER` / `ACCOUNT` |
| `t` | Native token transfer. Prefilled from `PAYER` to the selected account (or `ACCOUNT`) |
| `w` / `x` | Watch an account outside the wallet, such as a PDA, so the Accounts pane shows it (◇) and the middle pane shows its actor states / forget the selected watched account. A call that declares a PDA watches it by itself. A reset forgets them all |
| `v` | Show the messages the selected guest (or deployment) accepts: `lez-call iface`. A built guest doesn't record its message types, so this reads the guest's source in LEZ's guest package (`src/bin/<name>.rs`): the handler named in `actor_entrypoint!`, its message type, and each variant's Borsh index, params, doc comment and hints (below). A deployment whose guest prog-tui doesn't know is found by the image id in its on-chain header (`lez-dryrun source-of`) |
| `d` | Deploy the selected guest (Guests pane): a header account and one segment account per 96 KiB of its code, then `wallet program-loader deploy`, paid by `PAYER` |
| `u` | Build and deploy a guest from `jz-progs/example_progs` (see below) |
| `r` | Run the selected guest's stock `run_*` runner. It deploys again, so the programs it deploys are registered by name, and the TUI waits a block for the runner's last call to land before starting another job |
| `c` | Call the selected deployment (Deployed pane). Pick a message, fill in its params, check the plan, then send. See below |
| `x` | Forget the selected deployment |
| `e` | Print shell commands for the sequencer (watch its log, use it, stop it) and the `source` line for `env.sh` |
| `L` | Change the LEZ checkout |
| `Tab`, `↑↓`/`jk` | Switch pane, select |
| `?` / `q` | Help / quit |

**Calling a deployment.** `c` lists the messages the program accepts, as `v` shows them: one entry per enum variant, or one for a single message type. Each shows its hints: `[signed]` when its handling checks `is_authorized`, `[internal]` when it checks who sent it (another program sends it; a direct call is refused). Choosing one opens a form:
- **`to`:** the account the message goes to, prefilled with `ACCOUNT`. Also a wallet label, or `pda:<SEED>` for the program's PDA of a `PdaSeed` constant in its source (the field's hint lists them).
- **One field per parameter, in Borsh order** (a local struct is split into its fields):

  | Type | What to type |
  |---|---|
  | `Vec<u8>` | text, or `0x…` hex |
  | `String` | text |
  | integers | decimal |
  | `bool` | `true` or `false` |
  | `AccountId` | base58, `0x` hex, or a wallet label (prefilled with `ACCOUNT`) |
  | `Actor` | `account@program` |
  | `Option<T>` | empty for none, or a `T` |
  | `[u8; N]` | hex |
  | anything else | the hex of its Borsh encoding |
- **`sign`:** `auto` (the dry run finds out), `y` or `n`.
- **`actors`, `pda seeds`** (optional, comma-separated): actors to declare yourself (`account@program`, `pda:<seed>`, `pk:<hex>`), and seeds to try for a PDA that isn't on chain yet. Usually not needed.

**Enter plans the call.** `lez-call` encodes the message, and `lez-dryrun` runs the call through the engine locally, against the chain's state, and adds what each run finds missing: an actor a message reaches (declared), a receiver that panics unsigned (signs, if the wallet holds its key), an account that isn't on chain (key or PDA evidence; it tries the source's seed constants and every account id in the call). A pane shows the plan: each actor and why it's there, the states it changes, and the same call as an explicit `wallet program call …` line. A call that would fail shows the guest's panic, and nothing is sent.

**Enter again sends it**, with that `wallet program call …` line, which waits for the transaction. prog-tui then checks each state the plan changes against the chain. The log shows `✓` per state. A call that doesn't match was reverted (it is still included and charged); the log then shows the reason from the sequencer's log, for a sequencer prog-tui started. A PDA the call declares appears in the Accounts pane as `◇ … [PDA of <program>]`.

For example, `jz_admin_seat` (in `example_progs/admin_seat.rs`): choose `0  Open(Vec<u8>)` and type a config. The plan signs for `ACCOUNT` (the seat checks `is_authorized`), declares the seat's PDA (the seat sends to it) with PDA evidence (its seed is `ACCOUNT`'s id), and shows the PDA's state becoming the config.

## Your own programs

Put a guest's source in `jz-progs/example_progs/<name>.rs`, as a single file like LEZ's example guests (`hello_world.rs` there is a copy of LEZ's). Then press `u` and give its path, relative to `example_progs`: prog-tui builds and deploys it as `jz_<name>`.

- **How it builds:** prog-tui symlinks the file into LEZ's example guest package, `examples/program_deployment/methods/guest/src/bin/jz_<name>.rs`, and runs LEZ's normal guest build. So it uses LEZ's toolchain and lockfile, with nothing else to install, and the `.bin` lands in `$BIN` next to the examples.
- **What it can use:** only that package's dependencies: `lee_core`, `borsh` and `risc0-zkvm`.
- **Names:** the stem must be lowercase letters, digits and underscores. The `jz_` prefix keeps your programs apart from LEZ's, and prog-tui never replaces a regular file in LEZ, only its own links.
- **In LEZ's git:** the links show as untracked files (`?? …/jz_hello_world.rs`).
- **Calling it:** `c` picks one of its messages from its source and asks for the params (see "Calling a deployment" above).
- **`b` builds them all:** it links every `.rs` in `example_progs` before building. When you delete a source, `b` removes its link and its built files.

## Native transfers

`t` runs `wallet auth-transfer send --from … --to … --amount …`. The sender signs and pays the fee (about 4,000 on the dev chain).

- **Checked by balances:** the wallet says "included in block" even when a transfer reverts. So prog-tui reads both balances before and after, and gives the credit up to a minute to land, because it's an asynchronous message.
  - When it lands, the log says `✓ moved 1000 … (fee …)`.
  - When nothing is credited, it says the transfer reverted, and shows the reason from the sequencer's log.
- **Refused before sending:** a sender outside the wallet; a sender holding less than the amount plus the fee reserve, which the sequencer checks at submission (the execution gas limit × the base fee, plus the bytes × the storage fee: about 16 million at the wallet's default gas limit, so a freshly funded account needs more than that); and a recipient that isn't on chain and isn't in the wallet. The wallet can only vouch for an unused account it holds the key for; any other unused account needs `wallet auth-transfer send --to-pk <its public key>`, from the shell.

## The sequencer

prog-tui starts the sequencer as `RISC0_DEV_MODE=1 just run-sequencer-standalone`, in its own session. It keeps running after you quit, and prog-tui finds it again next time.

- **Process group:** in `.state/sequencer.pid`.
- **Output:** in `.state/sequencer.log`.
- **Started elsewhere?** A sequencer started any other way shows as "external". prog-tui asks before stopping it.
- **From another terminal:** there's no console to attach to, since the sequencer has no terminal. You can still watch its log, use it through the wallet or RPC, and stop it. Press `e` to print those commands with the real paths and pid filled in.

## Your shell's env vars

prog-tui rewrites `.state/env.sh` on every change. It exports:

- `LEZ_DIR`, `LEE_WALLET_HOME_DIR`, `BIN`, and `LEZ_WALLET` (the wallet `lez-call call` runs);
- `LEE_WALLET_PASSWORD`, the password prog-tui creates the wallet with (`wallet_password` in `.state/state.json`, default `prog-tui`). The wallet encrypts its storage with it, and every wallet command, runner and `lez-call` call reads it from this variable instead of asking;
- `PAYER`, `ACCOUNT`;
- `PROGRAM_<NAME>` for each deployment.

Load it in bash or zsh, and the walkthrough's commands work as written:

```bash
source jz-progs/prog-tui/.state/env.sh
cd "$LEZ_DIR" && just run-wallet account get --raw --scope all --account-id "Public/$ACCOUNT"
```

## Calling without the TUI

With `lez-call` and `lez-dryrun` built (`b`) and on `PATH`:

```bash
source jz-progs/prog-tui/.state/env.sh && cd "$LEZ_DIR"
G=examples/program_deployment/methods/guest/src/bin
lez-call iface $G/hello_world.rs
MSG=$(lez-call encode $G/hello_world.rs 'Hola mundo!')
lez-dryrun plan --payer "$PAYER" --program "$PROGRAM_HELLO_WORLD" --message-hex "$MSG" --to "$ACCOUNT" \
  --signer "$PAYER" --signer "$ACCOUNT"                  # ends with the `wallet program call` line to run
lez-call call $G/hello_world.rs --payer "Public/$PAYER" --program "$PROGRAM_HELLO_WORLD" --to "Public/$ACCOUNT" 'Hola mundo!'
```

The last line encodes and sends in one go, through `$LEZ_WALLET program call`, with the actors you give and no dry run. A program with several messages takes the message's name, then its values as `name=value` or in order: `… Mint to=holder amount=30`. `--json` prints one `LEZCALL {json}` (or `LEZDRYRUN {json}`) line instead of text. `--help` shows the rest.
