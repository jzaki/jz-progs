# Calling any LEZ program from what it is

Three pieces work out a program's interface and the shape of a call to it, then send the call:
- **`lez-call`** ([../lez-call](../lez-call/README.md)) is a small standalone CLI. It reads a guest's source for its interface and encodes messages. It links no LEZ crate.
- **`lez-dryrun`** ([lez-dryrun/](lez-dryrun/README.md)) runs the call through LEZ's engine locally, against chain state, to work out its actors, signers and evidence.
- **`wallet program call`** sends exactly the transaction given. It's the one change in LEZ.

prog-tui uses all three to list a program's messages, plan calls, send them, and show the command that does the same call by hand.

These replace `prog_cli`, whose subcommands (`hello`, `move-write`, `tail`, `pda`, …) each hard-coded one example program. They also replace `lezcall`, which did all of this inside LEZ's `examples/program_deployment` and so pulled in the wallet crate, the engine and risc0.

Status: built, on LEZ `jzaki/fix-examples` (LEZ `dev` merged at `850886286`). Public accounts only.

## 1. What a public call needs

A public transaction is `Message::new(to, message, actors, nonces, fee, evidence)` plus signatures ([public_transaction/message.rs](../logos-execution-zone/lee/state_machine/src/public_transaction/message.rs)). Each part has a different source:

| Part | What the engine does with it | Where it comes from |
|---|---|---|
| `to`, the root actor | The first delivery. It must be declared | **The caller.** Which account you mean (your seat, a holder, a PDA) is intent, not something to infer |
| `message` | The handler's Borsh-decoded second parameter | **Source**, by `lez-call` (§2) |
| `actors` | Every actor that receives a sync message must be declared, or the run fails with `UndeclaredActor` ([execution_state.rs:1146](../logos-execution-zone/lee/state_machine/core/src/execution_state.rs#L1146)). An async message to a declared actor is delivered in the same transaction ([execution_state.rs:1262](../logos-execution-zone/lee/state_machine/core/src/execution_state.rs#L1262)). One to an undeclared actor goes to the mailbox, which needs a recovery binding for its address, or the transaction fails with `UnboundAsyncDestination` ([validated_state_diff/mod.rs:414](../logos-execution-zone/lee/state_machine/src/validated_state_diff/mod.rs#L414)). So in practice a public destination has to be declared | **Dry run**, by `lez-dryrun` (§3) |
| `nonces`, the signers | A signer's account is `is_authorized` for every program's actor on it ([execution_state.rs:1321](../logos-execution-zone/lee/state_machine/core/src/execution_state.rs#L1321)) | **Dry run.** Source hints let it guess first |
| `evidence` | An account that is not on chain, does not sign, and is not reached by a matching seed grant must come with `Key` or `Pda` evidence ([execution_state.rs:1089](../logos-execution-zone/lee/state_machine/core/src/execution_state.rs#L1089), [public_backend.rs:104](../logos-execution-zone/lee/state_machine/src/validated_state_diff/public_backend.rs#L104)) | **Dry run**, plus seed candidates for a `Pda` (§3.3), and `pk:<hex>` for a key the wallet doesn't hold |
| fee payer | Signs and pays | **The caller** (`PAYER`) |

So inference has two layers:
- The **interface** is static and comes from the source: what messages exist and how to encode them. That's `lez-call`.
- The **plan** is dynamic and comes from running the call: which actors, signers and evidence one particular call needs. That's `lez-dryrun`.

The caller supplies only the account to send to, plus a public key for any account the wallet can't vouch for. The wallet does the rest: nonces, fee, signatures.

## 2. The interface, from source (`lez-call`)

`lez-call` parses the guest file with `syn`. It finds `actor_entrypoint!(handler)`, takes the type of the handler's second parameter, and lists that enum's variants (or the single type) in Borsh order with their doc comments.

**Beyond the variants:**
- **Local structs inside a parameter** are expanded into their fields in Borsh order, named `param.field`.
- **A local enum whose variants are all units** takes a variant name (or index).
- **`Option<T>`** takes empty or `none`, or a `T`. **`Vec<T>`** (other than `Vec<u8>`) takes comma-separated values. An `AccountId` also takes a wallet label, from `wallet account list` or `--label`.
- **`PdaSeed` constants** in the file are collected, e.g. `const PDA_SEED: PdaSeed = PdaSeed::new([37; 32])`. They become seed candidates (§3.3) and named targets (`--to pda:PDA_SEED`).
- **Per-variant hints** come from scanning each match arm, following calls into local functions one level deep:
  - `is_authorized` → *signed*.
  - `from_own_program`, `from.expect`, `from.unwrap` or `from.is_some()` → *internal*, meaning a program sends it, not a transaction.

  The hints only label and order things: prog-tui marks internal variants, and the dry run signs first for signed ones. Whether a call works is decided by the dry run, never by a hint.

`lez-call --json iface jz_simple_token.rs`, trimmed:

```json
{
  "source": "…/guest/src/bin/jz_simple_token.rs",
  "handler": "handle_message", "message": "enum Message",
  "methods": [
    { "index": 0, "name": "Open", "fields": " { total_supply: u128 }",
      "params": [{ "name": "total_supply", "ty": "u128", "hint": "decimal" }],
      "doc": "To your seat, signed by you.", "hints": ["signed"] },
    { "index": 1, "name": "Init", "fields": " { total_supply: u128 }",
      "params": [{ "name": "total_supply", "ty": "u128", "hint": "decimal" }],
      "doc": "Seat to treasury.", "hints": ["internal"] },
    { "index": 2, "name": "Mint", "fields": " { to: AccountId, amount: u128 }",
      "params": [{ "name": "to", "ty": "AccountId", "hint": "account: base58, Public/…, or a wallet label" },
                 { "name": "amount", "ty": "u128", "hint": "decimal" }],
      "doc": "To your seat, signed by you.", "hints": ["signed"] }
  ],
  "seeds": []
}
```

This JSON is the "intermediate file". It is generated whenever it's needed, from a source file in milliseconds. Nobody writes or maintains it, and no program has to change to produce it.

**`lez-call` takes a source file, not a program id.**
- **Why:** finding the source of a deployed program means matching the image id in its on-chain `ProgramHeader` ([program/mod.rs:516](../logos-execution-zone/lee/state_machine/core/src/program/mod.rs#L516)) against each local `.bin`'s. Computing an image id takes risc0, about 135 crates.
- **Who does it instead:** `lez-dryrun source-of <program-id> --bin-dir <dir>`, which has risc0 anyway. Image ids are cached in `lez-dryrun-image-ids.json` next to the builds, keyed by size and modification time.
- **In prog-tui:** a deployment usually doesn't need it, because prog-tui knows which guest it deployed.

## 3. The plan, from a dry run (`lez-dryrun`)

### 3.1 The engine, run locally

The sequencer executes a public transaction as `WholeExecution::new(public_context, TransactionEntry::Sync(root), &[]).execute(&mut backend)` ([validated_state_diff/mod.rs:263](../logos-execution-zone/lee/state_machine/src/validated_state_diff/mod.rs#L263)). `WholeExecution`, `PublicExecutionContext` and the `ExecutionEnvironment` trait are all public. `lez-dryrun` runs the same call with its own environment, `DryRun` (`dry_run.rs`):

| `ExecutionEnvironment` method | `DryRun` |
|---|---|
| `handle_message(input, view)` | Records the receiver and `is_authorized`, then runs the receiver:<br>• the program loader isn't run;<br>• the native token runs `native_token::handle_message`;<br>• a guest is loaded with `load_program` from loader states read over RPC, and executed with `Program::write_receive_input` and risc0's `default_executor`, with the guest's stderr captured for its panic message.<br>The journal is decoded with `lee_core::from_frame` and Borsh, as `transition_journal` (`pub(crate)`) does |
| `public_actor_state(actor)` | `getAccountView` over RPC, cached for the whole plan |
| `admits(account)` | On chain, a signer, the payer, or planned evidence → `true`. Otherwise it **records the account and returns `true`**, so a single run collects every admission need without stopping |
| `promote(…)` | `false`: the run's outcome lists the async messages left for the mailbox |

**Chain reads:**
- The trait is synchronous, so each chain read blocks on a small runtime of `lez-dryrun`'s own, over the sequencer's RPC client (`chain.rs`).
- It holds no keys. The caller says which accounts the wallet can sign for (`--signer`): prog-tui passes the wallet's public accounts.
- No signatures, nonces or fees are involved: the dry run starts after the point where the sequencer checks those.

A plan of a few runs takes well under a second for the examples.

### 3.2 The fixpoint

```
declared  = { root } + --actor     signers = { root's account, if --sign, or hinted "signed" and it's a --signer }
evidence  = pda:/pk: given         loop (at most 12 runs):
  run WholeExecution(context(declared, signers + payer), root, message) with DryRun
  UndeclaredActor { actor }        → declared += actor; again
  guest panicked at receiver R, with is_authorized = false, and R is a --signer, and R isn't signing yet
                                   → signers += R; again   (if it panics the same way signed, R is dropped as a
                                                            signer and the panic is reported)
  other guest panic, out of gas    → stop: report the panic message. Nothing is sent, nothing is charged
  ok:
    each recorded unadmitted account A → evidence: Key (a --signer's, or pk: given), Pda (seed candidate, §3.3),
                                         or stop and ask for --seed
    each async message to an undeclared P → declare P (delivered now), if P can be admitted
    if anything was declared → again
    any async message left without a recovery binding at its address → stop: P can't be declared (ask for
                                         --actor pk:<hex>) or --leave-pending was given
    else done
```

Without the source hint, `Mint` takes one more run: unsigned, the seat panics "only the account's owner calls…", so the root signs.

The result is the plan: root, declared actors (each with why it's there and how it's admitted), signers, async messages left pending, events, the predicted state changes, and the cycles used. It maps one-to-one onto `wallet program call`'s actors:
- a signer, or the payer when it's declared → `<account>:sign`
- `Pda` evidence → `pda:<program>/<seed>`
- a key given as `pk:` → `pk:<key>`
- any other declared actor → `<account>`. For a new account whose key it holds, the wallet adds the key evidence itself ([account_manager.rs:879](../logos-execution-zone/lez/wallet/src/account_manager.rs#L879))

So no one but the wallet builds the transaction.

### 3.3 PDA seeds

`UnadmittedPublicActor` gives the account but not the seed. For each program in the transaction, `lez-dryrun` checks `AccountId::for_public_pda(program, seed) == account` against these candidates:
1. every `--seed`: the source's `PdaSeed` constants (as `NAME=0x…`, which also lets `pda:NAME` name them), and any the caller adds;
2. every account id in the call: root, signers, payer, declared actors, programs, and the ids the message's values name (`--mention`, from `lez-call encode`).

That covers every example here (§5). A seed the guest hashes from parameters isn't found, and the plan stops and asks for `--seed`.

### 3.4 What a dry run can't know

- **Parameters that must mirror state.** For example, `MoveData.data` must equal the source's current state. That's the caller's job. The dry run catches a mismatch and shows the guest's own message, before anything is spent.
- **State can change between the dry run and inclusion.** So prog-tui checks each state the plan changes against the chain afterwards. A mismatch means the call reverted.
- **Private accounts.** A dry run would pass witnesses to `WholeExecution`, and `wallet program call` would need private actors. Left out until the private-recipient wallet fix (LEZ PR #985) lands.

## 4. The commands

**Formats:**
- **Account:** base58, `Public/<base58>`, or a wallet label.
- **Actor:** `<account>[@<program>][:sign]`, `pda:[<owner>/]<seed>[@<program>]` or `pk:<hex>[@<program>]`. The program, and a PDA's owner, default to `--program`.
- **Seed:** 0x hex or base58. To `lez-call` and `lez-dryrun`, also a `PdaSeed` constant's name.
- **Message:** its variant's name (or index) and its values, as `name=value` or in order. A program with one message type takes just its values.

```
lez-call iface  <guest.rs>
lez-call encode <guest.rs> [<Method>] [values]…                                # the message's hex
lez-call call   <guest.rs> --program <id> --to <actor> [--actor <actor>]… [--payer <account>]
                [--print] [<Method>] [values]…         # encode, then run (or print) `wallet program call`

lez-dryrun [--rpc <url>] plan --payer <account> --program <id> --message-hex <hex> --to <actor>
           [--actor <actor>]… [--signer <account>]… [--seed [NAME=]<seed>]… [--mention <account>]…
           [--label <name>=<id>]… [--sign | --no-sign] [--hinted-signed] [--leave-pending] [--budget <n>]
lez-dryrun [--rpc <url>] source-of <program-id> --bin-dir <dir>

wallet program call --program <id> --message-hex <hex> --to <actor> [--actor <actor>]… [--payer <account>]
```

**JSON output:**
- `lez-call --json` prints one `LEZCALL {json}` line: the interface, or `{method, index, hints, message_hex, mentioned}`, plus `{command, wallet_args}` for `call --print`.
- `lez-dryrun --json` prints one `LEZDRYRUN {json}` line: `{plan, lines}`. The plan's `command` and `wallet_args` are the `wallet program call` that sends it.

`lez-dryrun plan`'s output, for `jz_simple_token` `Mint` to the admin's seat, after `Open`. The `sent`/`✓` lines are prog-tui's, after it runs the `wallet program call` line:

```
message 0x0215145aee…1e000000000000000000000000000000 (49 bytes)   paid by 2RHZhw…

run 1  CVip3f…@program sends to 5yXhhk…@program: declare it
run 2  ok; async messages to 2RHZhw…@program: declare them
run 3  ok: 3 deliveries, 30727 cycles

to    CVip3f…@jz_simple_token  signs (the source checks is_authorized): the message goes to it
actor 5yXhhk…@jz_simple_token  on chain: CVip3f…@program sends to it
actor 2RHZhw…@jz_simple_token  signs (it pays the fee): async from 5yXhhk…@program, delivered in this transaction
after 2RHZhw…@jz_simple_token  (empty) → 0x1e000000000000000000000000000000
after 5yXhhk…@jz_simple_token  0x64…64… → 0x64…46…
30727 cycles

same call by hand:
  wallet program call --payer Public/2RHZhw… --program FsPuuX… --message-hex 0x0215145aee… --to Public/CVip3f…:sign --actor Public/5yXhhk… --actor Public/2RHZhw…:sign

✓ 2RHZhw…@FsPuuX…: now 0x1e000000000000000000000000000000 (as the dry run said)
✓ 5yXhhk…@FsPuuX…: now 0x64…46… (as the dry run said)
```

### `wallet program call`

`wallet program call` is in [cli/programs/program.rs](../logos-execution-zone/lez/wallet/src/cli/programs/program.rs), on the LEZ branch `jzaki/fix-examples`. It's the only change to LEZ that this needs.

- **What it takes:** the message already Borsh-encoded, and every actor named.
- **How it sends:** through the wallet's own `WalletCore::send_pub_tx_paid_by`.
- **Accounts:** a wallet key path (`m/…`) also works as an account there.
- **Seeds:** 0x hex or base58. The wallet has no source, so it doesn't know seed constants' names; `lez-call call` puts their bytes in.

For the native token, `lez-dryrun` also prints the real `wallet auth-transfer send …`.

## 5. The examples, as they are

No example program needed a change to be called. Each row was called on a local sequencer, and the chain matched the dry run.

| Program · message | Caller gives | Dry run finds |
|---|---|---|
| `hello_world`, `jz_hello_world` · `Vec<u8>` | `--to` | nothing more |
| `hello_world_with_authorization` | `--to` | root signs (hint; without it, found from the panic) |
| `hello_world_with_move_function` · `Write` | `--to` | nothing more |
| ″ · `MoveData { data, to }` | `--to` (the source), `data` = its current state | declare `to@program` (undeclared). A wrong `data` stops with the guest's message |
| ″ · `Append` | | hint *internal*. A direct call panics on `from_own_program` |
| `simple_tail_call` · `Actor` | `--to`, the callee actor | declare the callee (undeclared) |
| `tail_call_with_pda` · `AccountId` | `--to pda:PDA_SEED`, the callee program | root `Pda{caller, [37;32]}` evidence (seed constant). Declare `(pda, callee)`. The callee is authorized by the seed grant |
| `jz_hello_account` · `Hello`, `WriteAccountId` | `--to` | nothing more |
| `jz_admin_seat` · `Open(Vec<u8>)` | `--to` your account | you sign. Declare your PDA. Its first time: `Pda{program, your id}` (seed = the root's id) |
| ″ · `Init` | | hint *internal*. A direct call panics "Init comes from a seat" |
| `jz_simple_token` · `Open`, `Burn` | `--to` your account | you sign. Declare the treasury. `Open`: `Pda` evidence, seed = your id |
| ″ · `Mint { to, amount }` | `--to` your account | as above, plus declare `to` for the async `Credit`: key evidence when it's new and in the wallet. A new recipient the wallet has no key for needs `--actor pk:<hex>` |
| ″ · `Transfer { to, amount }` | `--to` the holder | the holder signs. Declare `to` for `Credit` |
| ″ · `Init`, `Draw`, `Return`, `Credit` | | hint *internal* |
| native token · `Transfer` | `--to` the sender, `--message-hex` (it has no guest source) | declare the recipient for `Credit`; prints `wallet auth-transfer send` too |

**One optional program change** (not made): `jz_admin_seat` and `jz_simple_token` could grant the PDA seed when the seat messages its PDA.
- **The change:** `Response::keep_state().send(SyncMessage::new(pda, &Init(…)).with_pda_seeds([PdaSeed::new(*admin.value())]))`. `Init`'s check could then become `input.is_authorized && input.from_own_program()`.
- **What it buys:** the engine admits the PDA through the grant, with no evidence, and marks it `is_authorized`. The transaction would only need to declare the PDA: no seed search, and nothing different the first time.
- **Where:** an edit to `jz-progs/example_progs/{admin_seat,simple_token}.rs`, which prog-tui copies to the `jz_*` guests.
- **The catch:** it changes the examples' semantics a little (the PDA becomes authorized), so it's your call.

## 6. prog-tui on these

- **`v`** shows `lez-call iface` for the guest's source. A deployment's source is its guest's, by name. When prog-tui doesn't know the guest, `lez-dryrun source-of` finds it by image id.
- **`c`** lists `lez-call --json iface`'s messages, with their hints and doc comments.
  - **The form:** `to` (prefilled with `ACCOUNT`; its hint lists `pda:<CONST>` for each seed constant), one field per param, then `sign` (`auto`, `y`, `n`), `actors` and `pda seeds`.
  - **Enter:** encodes with `lez-call` and plans with `lez-dryrun`, as a job. A pane shows the plan, or the guest's panic, and the `wallet program call` line.
  - **Enter again:** runs that line through the wallet, then checks each state the plan changes against the chain. The log shows a `✓`/`✗` per state, and the revert reason from the sequencer's log when there is one.
- **Without `lez-dryrun`:** the pane shows the encoded message and a `wallet program call` with just `to`. `to` signs when asked, or when the source hints it. That call is sent unchecked.
- **PDAs** the call declares are watched in the Accounts pane, as `PDA of <program>`.
- **Deploying** (`d`, `u`, the walkthrough) is the wallet's alone:
  1. a fresh header account, and one fresh segment account per 96 KiB of the guest's user ELF (read from the `.bin` header);
  2. then `wallet program-loader deploy`, paid by `PAYER`.
- **`b`** builds the wallet, the runners and the guests in LEZ, then `lez-call` and `lez-dryrun` in their own checkouts.

## 7. Where it lives

| Piece | Where | Depends on |
|---|---|---|
| `lez-call` | its own repo, `../lez-call` | `syn`, `quote`, `serde`, `serde_json`, `bs58`, `anyhow` (14 crates in all) |
| `lez-dryrun` | `jz-progs/lez-dryrun`, its own Cargo workspace | LEZ's `lee`, `lee_core`, `program_loader_core` and `sequencer_service_rpc`, by path, plus risc0 (LEZ's `Cargo.lock` and toolchain, copied) |
| `wallet program call` | LEZ, `lez/wallet/src/cli/programs/program.rs` | the wallet |
| prog-tui | `jz-progs/prog-tui` | no LEZ crate; runs `wallet`, `lez-call` and `lez-dryrun` |

**`lez-dryrun` files:**

| File | What it holds |
|---|---|
| `main.rs` | The CLI, and account, seed and actor syntax |
| `chain.rs` | Cached, blocking chain reads over RPC |
| `dry_run.rs` | The `ExecutionEnvironment` (§3.1) |
| `plan.rs` | The fixpoint (§3.2), the plan, and its text |
| `guests.rs` | The image-id lookup for `source-of` |

**Leftovers on the branch:**
- `prog_cli` is deleted.
- `deploy_program` (`examples/program_deployment/src/lib.rs`) still prints its `PROG_CLI {"program_id":…}` line, which prog-tui reads from the stock runners' output.

## 8. Open questions

1. `wallet program call` needs proposing upstream.
2. Should the seed-grant change to `jz_admin_seat` and `jz_simple_token` go in (§5)?
3. Private accounts: once LEZ PR #985 lands, the dry run can take private witnesses, and `wallet program call` can take private actors. That's also when `--leave-pending` becomes useful.
4. How this relates to SPEL v0.4: this answers [SPEL.md](SPEL.md) §7 open question 2 ("which actor it targets, so the CLI can build the transaction") without an IDL. A v0.4 IDL could still emit the §2 JSON, so that `lez-call` reads it instead of parsing the source.
