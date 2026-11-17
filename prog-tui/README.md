# prog-tui

A terminal UI for deploying and testing LEZ programs locally. It builds and deploys:
- [lez-programs](https://github.com/logos-blockchain/lez-programs)' programs (token and nft today);
- your own single-file guests in [`example_progs/`](../example_progs).

It runs the steps of [WALKTHROUGH.md](../example_progs/WALKTHROUGH.md) for you, and remembers the account and program ids they print.

## Run it

```bash
cd jz-progs/prog-tui
cargo run --release              # opens on the network selector
cargo run --release -- local     # or skip it: a network's key, as the selector's choices have them
```

**It opens on a network selector** (Enter or 1–9 to choose, `q` to quit), starting on the network used last:
- **Local standalone:** LEZ's standalone sequencer on `127.0.0.1:3040`, which prog-tui starts, stops and resets, with LEZ's debug wallet. Everything below applies.
- **A network from [`networks.json`](networks.json)**, one choice per sequencer listed there (a devnet, say). prog-tui only talks to that sequencer's RPC.

`networks.json` names each network, its sequencers' addresses, and optionally its channel id:

```json
{
  "networks": [
    {
      "name": "local-devnet",
      "channel_id": "0101010101010101010101010101010101010101010101010101010101010101",
      "sequencers": ["127.0.0.1:3040", "10.0.0.7", "http://seq.example:3041"]
    }
  ]
}
```

- **Addresses:** an IP or host, with an optional port (default 3040, the sequencer's RPC port) and an optional `http://` or `https://`.
- **The channel id isn't needed** to use the network: the wallet only talks to the sequencer's RPC. When it's given, prog-tui compares it with the sequencer's `getChannelId`, and the status bar shows `channel ✓`, or the channel it found in red. A wrong address shows up at once, rather than as accounts that aren't there.
- **The file as checked in** lists LEZ's four-sequencer docker devnet (`docker-compose.yml` in LEZ publishes their RPCs on ports 3040–3043, on channel `0101…`). Edit it for your devnet's addresses. LEZ's local standalone sequencer also listens on 3040, so with it running, the first devnet entry reaches it.

**On a remote network:**
- **Its own wallet and state:** a wallet home at `.state/networks/<key>/wallet`, made from LEZ's debug `wallet_config.json` with `sequencer_addr` set to the chosen sequencer (later edits to that file are kept), and its own `PAYER`, `ACCOUNT`, deployments, `env.sh` and job logs in `.state/networks/<key>/`. The checkouts and the wallet password are shared.
- **No sequencer control:** `s` (start/stop) and `R` (reset) are refused, and `e` points at the network's `env.sh` instead.
- **`i`** imports the two test accounts into the network's wallet. LEZ's docker devnet funds them at genesis, as the local chain does; another devnet may not.
- **`W`** builds, checks the sequencer is healthy and on the configured channel, then runs the rest of the walkthrough there. Nothing is reset, and the accounts it creates get no labels, so it can run again.
- **Revert reasons** are in that sequencer's own log, which prog-tui can't read.

**`N`** goes back to the selector, between jobs.

It uses three checkouts next to `jz-progs`. Press `L` to point it at others; the paths are saved.

| Checkout | Default | What prog-tui uses it for |
|---|---|---|
| **LEZ** | `../../logos-execution-zone` | The sequencer, the wallet (`target/release/wallet`) and its debug home, `just clean`, `just wallet-import-test-accounts` |
| **lez-programs** | `../../lez-programs` | The programs it builds and deploys, their spel IDLs (`artifacts/<program>-idl.json`), and the `jz/` package your guests build in |
| **spel** | `../../spel`, or `spel` on `PATH` | Calls (`spel send`) to lez-programs' programs and to your guests, your guests' IDLs (`spel generate-idl`), and reading state (`spel inspect`) |

Keep LEZ at the tag lez-programs pins (`v0.4.0-rc3` today), and spel on its `refactor/lez-v0.4.x` branch, which pins the same tag. A mismatch shows up as a sequencer that refuses transactions it can't decode.

**Your own guests are called with spel too**, when their message is an `enum Message`: `b` generates a spel IDL for each. Every guest in `example_progs` has one.

**A guest spel can't describe falls back to [`lez-call`](https://github.com/jzaki/lez-prog-call)** (a checkout at `../../lez-prog-call`, or on `PATH`), planned by [`lez-dryrun`](../lez-dryrun/README.md) (in `jz-progs/lez-dryrun`). Sending those needs a wallet with the `program call` subcommand. That subcommand arrived with LEZ PR #989, after v0.4.0-rc3, so at that tag such a call can be planned but not sent. [LEZCALL.md](../LEZCALL.md) is their design.

`b` builds all of these. Everything else must already be installed: the RISC Zero toolchain, `just`, `make`, and LEZ's build dependencies. Guests build with the local RISC Zero toolchain through their methods crates, with no Docker. prog-tui doesn't use `nix develop`; start it from inside that shell if you build that way.

**Press `W` on a fresh start.** It runs the whole walkthrough as a checklist:

1. build;
2. reset the chain and start the sequencer;
3. initialise the wallet and import the test accounts;
4. set `PAYER` and create `ACCOUNT`;
5. deploy `token`;
6. create a token, JZ, with its whole supply of 1000 on `ACCOUNT`, and check `ACCOUNT`'s holding with `spel inspect`.

## Layout

- **Status bar:**
  - **LEZ:** the checkout's branch, commit and whether it's dirty.
  - **PROGS:** the same for lez-programs, then whether `spel`, `lez-call` and `lez-dryrun` are built.
  - **SEQ:** the local sequencer (stopped, starting, or running; started by prog-tui or not; block height), and `PAYER` / `ACCOUNT`. On a remote network, **NET** instead: the network's name, whether its sequencer is reachable, its block height and URL, and `channel ✓` when its channel matches `networks.json`.
- **Accounts:** the wallet's accounts. ★ marks `PAYER`, ▶ marks `ACCOUNT`, and each shows its balance, or `off-chain` when it doesn't exist on chain yet. Deploys create a header and some segment accounts in the wallet; `h` shows or hides them.
- **Account:** the selected account's nonce, balance, and every program's actor state on it. Each state is labelled by program name when prog-tui deployed that program, and shown as text when it's printable. For a program with a spel IDL (lez-programs' programs and your guests), the selected account's state is decoded with `spel inspect` and shown as JSON, marked `(spel)`: between jobs, and again only when the state's bytes change. A state spel can't decode stays as text or hex.
- **Programs:** three lists.
  - *Guests:* lez-programs' programs (`(lez-programs)`; `not built` until `b`), then your built guests (`(example_progs)`).
  - *Deployed:* the programs deployed from here. Crossed out means no longer on chain, after a reset.
  - *Built-in:* read-only. The native token and program loader (both run by the engine), and the guests in LEZ's `lez/programs/` (clock, fee, bridge and so on). Those guests are dimmed when the current chain didn't deploy them, which on the dev chain is all but `clock`, `fee`, `bridge` and `sequencer_stake`. Select one to see its full address and where it comes from. Their names also label the actor states in the Account pane.
- **Log:** each command and its output. The full output of every job is in `.state/logs/`.

## Keys

| Key | Does |
|---|---|
| `W` | Walkthrough. Asks first, because it resets the chain and the wallet |
| `s` | Start the sequencer, or stop it. Asks first when prog-tui didn't start it. Local only |
| `R` | Reset: stop, `just clean`, start. Wipes the chain and the wallet. Local only |
| `b` | Build: LEZ's wallet; lez-programs' programs and their IDLs (`make idl`); your guests; `spel`, `lez-call` and `lez-dryrun` in their checkouts; then your guests' IDLs (`spel generate-idl`) |
| `i` | Initialise the wallet and import the two funded test accounts |
| `n` | New public account, with an optional label |
| `P` / `A` | Make the selected account `PAYER` / `ACCOUNT` |
| `t` | Native token transfer. Prefilled from `PAYER` to the selected account (or `ACCOUNT`) |
| `w` / `x` | Watch an account outside the wallet, such as a PDA, so the Accounts pane shows it (◇) and the middle pane shows its actor states / forget the selected watched account. A call that declares a PDA watches it by itself. A reset forgets them all |
| `v` | Show the messages the selected guest (or deployment) accepts, from its spel IDL. For a guest without one, `lez-call iface` reads its source: the handler named in `actor_entrypoint!`, its message type, and each variant's Borsh index, params, doc comment and hints (below). A deployment prog-tui doesn't know is matched to one of your built guests by the image id in its on-chain header (`lez-dryrun source-of`) |
| `d` | Deploy the selected guest (Guests pane): a header account and one segment account per 96 KiB of its code, then `wallet program-loader deploy`, paid by `PAYER` |
| `u` | Build and deploy a guest from `jz-progs/example_progs` (see below) |
| `c` | Call the selected deployment (Deployed pane). Pick a message, fill in its fields, check the plan, then send. See below |
| `x` | Forget the selected deployment |
| `e` | Print shell commands for the sequencer (watch its log, use it, stop it) and the `source` line for `env.sh` |
| `L` | Change the LEZ and lez-programs checkouts |
| `N` | Back to the network selector |
| `Tab`, `↑↓`/`jk` | Switch pane, select |
| `?` / `q` | Help / quit |

## Calling with spel

Every guest with a spel IDL is called this way: lez-programs' programs, from their `artifacts/<program>-idl.json` (lez-programs' `make idl`), and your guests, from `lez-programs/jz/idl/jz_<name>.json` (`spel generate-idl`). Both are regenerated by `b`. `c` lists one entry per message: its tag, name, fields and the first line of its doc comment. Choosing one opens a form:
- **`to`:** the actor the message goes to, prefilled with `ACCOUNT`. An account (the actor is that account under this program), or `account@program`. Accounts may be wallet labels. `pda:<account>` is this program's PDA seeded by that account's id.
- **One field per message field.** Type values as spel parses them:

  | Type | What to type |
  |---|---|
  | `account_id` | a wallet label, base58, or `0x` hex (prefilled with `ACCOUNT`) |
  | integers | decimal |
  | `string` | text |
  | `bool` | `true` or `false` |
  | `Option<T>` | empty or `none` for none, or a `T` |
  | `Vec<T>` | comma-separated |
  | a defined type | a variant name, or a JSON object |
- **`signers`:** comma-separated accounts that authorize. Each signs for all its actors in the call. spel never works this out, so it starts as `ACCOUNT`; the program's doc comments say who must sign.
- **`actors`** (optional, comma-separated): actors spel can't see from the fields. spel declares the root, these, one actor on the program per account field, and one per signer. Besides `account@program`, prog-tui takes two shorthands:
  - `pda:<account>`: this program's PDA seeded by that account's id (`AccountId::for_public_pda`), as admin_seat's PDA and simple_token's treasury are;
  - `clock`: the clock's actor, which a program that reads the clock sends to (clock_gate).

  In the token program, `burn` also reaches the holding's definition: add `<definition>@token`.

**Enter plans the call** with `spel send --dry-run`. A pane shows each actor and why it's declared, the signers, the encoded message, and the same call as a `spel …` line to paste into a shell that sourced `env.sh`.

**Enter again sends it** with `spel send`.
- **Simulated first:** spel doesn't send a transaction the program or LEZ would refuse. The log then shows the reason, and nothing is charged.
- **Once included:** spel prints each declared actor that changed, before and after.
- **Included with nothing changed** (a no-op, or a refusal the simulation didn't foresee): the job fails, and the log shows the sequencer's reason, for a sequencer prog-tui started.

For example, to create a token as the walkthrough does:
1. Make a new account for the definition (`n`, label `usdc`).
2. Call `NewDefinition` with:
   - `to` = `usdc`;
   - `name`, `total_supply` and `mint_authority` (empty for none);
   - `holding` = `ACCOUNT`;
   - `signers` = `usdc, ACCOUNT`.

lez-programs' [token runbook](https://github.com/logos-blockchain/lez-programs/blob/main/docs/token/README.md) has the rest of the token program's messages. Your guests, each checked on a local chain at v0.4.0-rc3:

| Guest · message | `to` | `signers` | `actors` |
|---|---|---|---|
| `jz_hello_world` · `Hello { greeting }` | any account | any (it checks nothing) | |
| `jz_hello_account` · `Hello`, `WriteAccountId` | any account | any | |
| `jz_admin_seat` · `Open { config }` | your account | your account | `pda:<your account>` |
| `jz_simple_token` · `Open`, `Mint`, `Burn` | your account (the admin) | your account | `pda:<your account>` (the treasury) |
| `jz_simple_token` · `Transfer { to, amount }` | the holder | the holder | |
| `jz_clock_gate` · `Set` | any account | any | `clock` |

The PDAs need no admission evidence even before they're on chain: the seat grants the PDA's seed when it messages it. `Init`, `Draw`, `Return` and `Credit` are internal (one of the program's own actors sends them), so a direct call is refused in simulation.

## Calling with lez-call

For a guest with no spel IDL: one whose message isn't an `enum Message` (a bare `Vec<u8>`, say), or a deployment prog-tui doesn't know. `c` lists the messages the program accepts, as `v` shows them: one entry per enum variant, or one for a single message type. Each shows its hints: `[signed]` when its handling checks `is_authorized`, `[internal]` when it checks who sent it (another program sends it; a direct call is refused). Choosing one opens a form:
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

**Enter plans the call.** `lez-call` encodes the message, and `lez-dryrun` runs the call through the engine locally, against the chain's state, and adds what each run finds missing: an actor a message reaches (declared), a receiver that panics unsigned (signs, if the wallet holds its key), an account that isn't on chain (key or PDA evidence; it tries the source's seed constants and every account id in the call). A pane shows the plan: each actor and why it's there, the states it changes, and the same call as an explicit `wallet program call …` line. The pane ends with the wallet's own `--dry-run` of that line: the fee at the head of the chain and whether the signed transaction would apply or revert (a revert is flagged, and still charges the fee). A call that would fail shows the guest's panic, and nothing is sent.

**Without `program call`** (LEZ v0.4.0-rc3), the plan still shows, and the pane says the call can't be sent with this wallet.

**Enter again sends it**, with that `wallet program call …` line, which waits for the transaction. prog-tui then checks each state the plan changes against the chain. The log shows `✓` per state. A call that doesn't match was reverted (it is still included and charged); the log then shows the reason from the sequencer's log, for a sequencer prog-tui started. A PDA the call declares appears in the Accounts pane as `◇ … [PDA of <program>]`.

For example, a guest like admin_seat without its seed grant: the plan signs for `ACCOUNT` (the seat checks `is_authorized`), declares the seat's PDA (the seat sends to it) with PDA evidence (its seed is `ACCOUNT`'s id), and shows the PDA's state becoming the config.

## Your own programs

Put a guest's source in `jz-progs/example_progs/<name>.rs`, as a single file. Then press `u` and give its path, relative to `example_progs`: prog-tui builds and deploys it as `jz_<name>`.

- **How it builds:** prog-tui keeps a small guest package in the lez-programs checkout, `lez-programs/jz/`, written from [`templates/jz/`](templates/jz). It symlinks each file into `jz/methods/guest/src/bin/jz_<name>.rs` and builds the package with `cargo build --release --manifest-path jz/methods/Cargo.toml --target-dir target`, run in lez-programs. So it uses lez-programs' toolchain and build cache, and the `.bin` lands in `$JZ_BIN`.
- **What it can use:** the package's dependencies: `lee_core` and `clock_core` at LEZ v0.4.0-rc3, `borsh` and `risc0-zkvm`. To add one, edit the template; prog-tui rewrites the package's files from it on the next build.
- **All or nothing:** the package builds every guest at once, so one that doesn't compile stops the others. `b` logs that and carries on with the rest of the build (so `W` still runs); `u` fails.
- **For spel to call it:** its message type must be an `enum Message` (spel's IDLs describe an enum; a struct or a bare type gets no IDL, and falls back to `lez-call`). Name the variants' fields (`Hello { greeting: String }`, not `Hello(String)`), since they become spel's flags (`--greeting`), and use `String` for text: spel types a `Vec<u8>` as a list of decimal bytes. The entrypoint can still be `raw`, as clock_gate's is: spel only reads the enum.
- **Its state, decoded:** when the source declares exactly one type named `State` or ending in `State` (a struct, an enum, or an alias such as clock_gate's `type GateState = Timestamps;`), the IDL describes it, so spel shows the state decoded rather than as a byte count.
- **Names:** the stem must be lowercase letters, digits and underscores. The `jz_` prefix keeps your programs apart from others, and prog-tui never replaces a regular file there, only its own links.
- **In lez-programs' git:** `jz/` is an untracked directory. lez-programs' own builds (`make build-programs`, `programs/*/methods/guest`) never see it.
- **Calling it:** `c` picks one of its messages from its spel IDL, and asks for its fields (see "Calling with spel" above).
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

- `LEZ_DIR`, `LEE_WALLET_HOME_DIR`, and `LEZ_WALLET` (the wallet `lez-call call` runs);
- `PROGRAMS_DIR` (lez-programs), `JZ_BIN` (your built guests), and `SPEL` (the spel binary, once built);
- `LEE_WALLET_PASSWORD`, the password prog-tui creates the wallet with (`wallet_password` in `.state/state.json`, default `prog-tui`). The wallet encrypts its storage with it, and every wallet command, `spel` and `lez-call` call reads it from this variable instead of asking;
- `PAYER`, `ACCOUNT`;
- `PROGRAM_<NAME>` for each deployment.

Load it in bash or zsh, and the walkthrough's commands work as written:

```bash
source jz-progs/prog-tui/.state/env.sh
cd "$LEZ_DIR" && just run-wallet account get --raw --scope all --account-id "Public/$ACCOUNT"
"$SPEL" --idl "$PROGRAMS_DIR/artifacts/token-idl.json" --program "$PROGRAM_TOKEN" inspect "$ACCOUNT"
```

## Calling without the TUI

lez-programs' programs, with `spel`:

```bash
source jz-progs/prog-tui/.state/env.sh
IDL=$PROGRAMS_DIR/artifacts/token-idl.json
"$SPEL" --idl "$IDL" --program "$PROGRAM_TOKEN" --fee-payer "$PAYER" \
  send --root "$ACCOUNT" --signer "$ACCOUNT" --dry-run -- transfer --to <RECIPIENT> --amount 10
```

Drop `--dry-run` to send. `"$SPEL" --idl "$IDL" --program "$PROGRAM_TOKEN" -- <message> --help` lists a message's fields.

Your own guests the same way, with the IDLs `b` generates:

```bash
source jz-progs/prog-tui/.state/env.sh
"$SPEL" --idl "$PROGRAMS_DIR/jz/idl/jz_hello_world.json" --program "$PROGRAM_JZ_HELLO_WORLD" --fee-payer "$PAYER" \
  send --root "$ACCOUNT" --signer "$ACCOUNT" -- hello --greeting 'Hola mundo!'
"$SPEL" --idl "$PROGRAMS_DIR/jz/idl/jz_clock_gate.json" --program "$PROGRAM_JZ_CLOCK_GATE" --fee-payer "$PAYER" \
  send --root "$ACCOUNT" --actor 4BdcjoXkq786TMWcBGGHqcxeLYMZmn17rL4eM9ZyRWNT@2hwaRW8sMnyKLLRbDRw8k68tiv9bnfq98oxcdtcp157Z -- set
```

The `--actor` there is the clock's actor, which prog-tui's `clock` shorthand stands for. Outside prog-tui there's no `pda:` shorthand: give the PDA's id, as the plan pane's `spel …` line does.

With `lez-call` and `lez-dryrun` built (`b`) and on `PATH`, and a wallet with `program call`, the same guests go through `lez-call` (except clock_gate, whose raw entrypoint `lez-call` can't see past):

```bash
source jz-progs/prog-tui/.state/env.sh
G=$PROGRAMS_DIR/jz/methods/guest/src/bin
lez-call iface $G/jz_hello_world.rs
MSG=$(lez-call encode $G/jz_hello_world.rs Hello 'Hola mundo!')
lez-dryrun plan --payer "$PAYER" --program "$PROGRAM_JZ_HELLO_WORLD" --message-hex "$MSG" --to "$ACCOUNT" \
  --signer "$PAYER" --signer "$ACCOUNT"                  # ends with the `wallet program call` line to run
lez-call call $G/jz_hello_world.rs --payer "Public/$PAYER" --program "$PROGRAM_JZ_HELLO_WORLD" --to "Public/$ACCOUNT" Hello 'Hola mundo!'
```

The last line encodes and sends in one go, through `$LEZ_WALLET program call`, with the actors you give and no dry run. A program with several messages takes the message's name, then its values as `name=value` or in order: `… Mint to=holder amount=30`. `--json` prints one `LEZCALL {json}` (or `LEZDRYRUN {json}`) line instead of text. `--help` shows the rest.
