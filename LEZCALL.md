# lezcall: call any LEZ program from what it is

A design for a CLI that works out a program's interface and the shape of a call to it, then sends the call. It replaces `prog_cli`, whose subcommands (`hello`, `move-write`, `tail`, `pda`, …) each hard-code one example program. prog-tui would use it to list a program's messages, make calls, and show the command that does the same call by hand.

Status: design only, nothing built. It targets LEZ `dev` as merged into `jzaki/fix-examples` (`850886286`), public accounts only. `lezcall` is a placeholder name.

## 1. What a public call needs

A public transaction is `Message::new(to, message, actors, nonces, fee, evidence)` plus signatures ([public_transaction/message.rs](../logos-execution-zone/lee/state_machine/src/public_transaction/message.rs)). Each part has a different source:

| Part | What the engine does with it | Where lezcall gets it |
|---|---|---|
| `to`, the root actor | The first delivery. It must be declared | **The caller.** Which account you mean (your seat, a holder, a PDA) is intent, not something to infer |
| `message` | The handler's Borsh-decoded second parameter | **Source:** the handler's message type (§2) |
| `actors` | Every actor that receives a sync message must be declared, or the run fails with `UndeclaredActor` ([execution_state.rs:1146](../logos-execution-zone/lee/state_machine/core/src/execution_state.rs#L1146)). An async message to a declared actor is delivered in the same transaction. To an undeclared one it goes to the mailbox as pending ([execution_state.rs:1262](../logos-execution-zone/lee/state_machine/core/src/execution_state.rs#L1262)) | **Dry run** (§3) |
| `nonces`, the signers | A signer's account is `is_authorized` for every program's actor on it ([execution_state.rs:1321](../logos-execution-zone/lee/state_machine/core/src/execution_state.rs#L1321)) | **Dry run.** Source hints let it guess first |
| `evidence` | An account that is not on chain, does not sign, and is not reached by a matching seed grant must come with `Key` or `Pda` evidence ([execution_state.rs:1089](../logos-execution-zone/lee/state_machine/core/src/execution_state.rs#L1089), [public_backend.rs:104](../logos-execution-zone/lee/state_machine/src/validated_state_diff/public_backend.rs#L104)) | **Dry run**, plus seed candidates for a `Pda` (§3.3) |
| fee payer | Signs and pays | **The caller** (`PAYER`) |

So inference has two layers:
- The **interface** is static and comes from the source: what messages exist and how to encode them.
- The **plan** is dynamic and comes from running the call: which actors, signers and evidence one particular call needs.

The only thing the caller supplies is which account to send to.

## 2. The interface, from source

This is what prog-tui's [msgtypes.rs](prog-tui/src/msgtypes.rs) does today, moved into lezcall and extended. It parses the guest file with `syn`, finds `actor_entrypoint!(handler)`, takes the type of the handler's second parameter, and lists that enum's variants (or the single type) in Borsh order with their doc comments.

**Finding the source for a deployed program, with no registry.** A program account's loader state is a `ProgramHeader` with the bytecode's `image_id` ([program/mod.rs:516](../logos-execution-zone/lee/state_machine/core/src/program/mod.rs#L516)). lezcall reads the header over RPC and computes the image id of each locally built guest `.bin`, the same way `Program::new` does. The match names the guest, so the source is `methods/guest/src/bin/<name>.rs`. `--source <file>` overrides this. If nothing matches, the interface is "raw bytes", and the plan (§3) still works, because a dry run only needs the ELF and that is on chain.

**Additions over msgtypes.rs:**
- **Local structs and enums inside a parameter** are expanded to their fields in Borsh order. Today they fall back to hex.
- **`Option<T>`, `Vec<T>` and tuples** are encoded generically.
- **`PdaSeed` constants** in the file are collected, e.g. `const PDA_SEED: PdaSeed = PdaSeed::new([37; 32])`. They become seed candidates (§3.3) and named targets (`--to pda:PDA_SEED`).
- **Per-variant hints** come from scanning each match arm, following calls into local functions one level deep:
  - `input.is_authorized` in an assert → *signed*.
  - `input.from.expect(…)`, `from_own_program()` or `from.is_some()` → *internal*, meaning a program sends it, not a transaction.

  The hints only label and order things: prog-tui dims internal variants, and the dry run tries signing first for signed ones. Whether a call works is decided by the dry run, never by a hint.

`lezcall iface --json` for `jz_simple_token`, trimmed:

```json
{
  "program": "4Fk…", "guest": "jz_simple_token", "source": "…/guest/src/bin/jz_simple_token.rs",
  "image_id": "9c41…", "handler": "handle_message", "message": "enum Message",
  "methods": [
    { "index": 0, "name": "Open", "params": [{ "name": "total_supply", "ty": "u128" }],
      "doc": "To your seat, signed by you.", "hints": ["signed"] },
    { "index": 1, "name": "Init", "params": [{ "name": "total_supply", "ty": "u128" }],
      "doc": "Seat to treasury.", "hints": ["internal"] },
    { "index": 2, "name": "Mint",
      "params": [{ "name": "to", "ty": "AccountId" }, { "name": "amount", "ty": "u128" }],
      "doc": "To your seat, signed by you.", "hints": ["signed"] }
  ],
  "seeds": []
}
```

This JSON is the "intermediate file". It is generated whenever it's needed, and prog-tui can cache it keyed by image id. Nobody writes or maintains it, and no program has to change to produce it.

## 3. The plan, from a dry run

### 3.1 The engine, run locally

The sequencer executes a public transaction as `WholeExecution::new(public_context, TransactionEntry::Sync(root), &[]).execute(&mut backend)` ([validated_state_diff/mod.rs:263](../logos-execution-zone/lee/state_machine/src/validated_state_diff/mod.rs#L263)). `WholeExecution`, `PublicExecutionContext` and the `ExecutionEnvironment` trait are all public. lezcall runs the same call with its own environment, `DryRun`:

| `ExecutionEnvironment` method | `DryRun` |
|---|---|
| `handle_message(input, view)` | Records `input` (`receiver`, `from`, `is_authorized`) before running it. The program loader isn't reachable from a call. The native token runs `native_token::handle_message`. A guest is loaded with `load_program` from loader states read over RPC (or a local `.bin`), and executed with `Program::write_receive_input` and risc0's `default_executor`. The journal is decoded as `transition_journal` does, with `lee_core::from_frame` and Borsh: `transition_journal` is `pub(crate)`, so lezcall copies its 5 lines. Without the hint, `Mint` would take one more run: unsigned, the seat panics "only the account's owner calls…", so the root signs |
| `public_actor_state(actor)` | `getAccountView` over RPC, cached per run |
| `admits(account)` | On chain, a signer, or already planned evidence → `true`. Otherwise it **records the account and returns `true`**, so a single run collects every admission need without stopping |
| `promote(…, body)` | Records the async message, returns `false` |

The RPC client is async and the trait is sync, so the run goes in `spawn_blocking` with a blocking client, or prefetches as it goes. No signatures, nonces or fees are involved: the dry run starts after the point where the sequencer checks those.

### 3.2 The fixpoint

```
declared  = { root }          signers = { root's account, if hinted "signed" and the wallet holds its key }
evidence  = {}                loop (at most 8 runs):
  run WholeExecution(context(declared, signers), root, message) with DryRun
  UndeclaredActor { actor }        → declared += actor; again
  guest panicked at receiver R, with is_authorized = false, and the wallet holds R's key, and R isn't a signer yet
                                   → signers += R; again   (if it still panics there, report the panic)
  other guest panic, out of gas    → stop: report the panic message. Nothing is sent, nothing is charged
  ok:
    each recorded unadmitted account A → evidence: Key (wallet key), Pda (seed candidate, §3.3), or stop and ask for --pda-seed
    each recorded async message to public P → declare P and deliver it now, if P can be admitted (or --leave-pending)
    if anything was added → again, else done
```

The result is the plan: root, declared actors, signers, evidence, async messages left pending, the predicted post-states, and the cycles used. It maps one-to-one onto the wallet's own call path, `WalletCore::send_pub_tx_paid_by(Vec<AccountMention>, root, message, Some(payer))` ([lib.rs:1232](../logos-execution-zone/lez/wallet/src/lib.rs#L1232)):
- a signer → `AccountIdentity::Public`
- a declared non-signer → `PublicNoSign`
- `Pda` evidence → `PublicPda { program, seed }`
- `Key` evidence for a key the wallet holds → `PublicNoSign`. The wallet adds the key evidence itself ([account_manager.rs:879](../logos-execution-zone/lez/wallet/src/account_manager.rs#L879))

So lezcall needs no transaction building of its own. `prog_cli`'s `send()` goes away.

### 3.3 PDA seeds

`UnadmittedPublicActor` gives the account but not the seed. For each program in the transaction, lezcall checks `AccountId::for_public_pda(program, seed) == account` against these candidates:
1. the file's `PdaSeed` constants;
2. every account id in the call: root, signers, declared actors, and `AccountId`/`Actor`/`[u8; 32]` parameters;
3. `--pda-seed`.

That covers every example here (§5). A seed the guest hashes from parameters isn't found, and the plan stops and asks for `--pda-seed`.

### 3.4 What a dry run can't know

- **Parameters that must mirror state.** For example, `MoveData.data` must equal the source's current state. That's the caller's job. The dry run catches a mismatch and shows the guest's own message, before anything is spent.
- **State can change between the dry run and inclusion.** The result check (§4, `call`) still runs.
- **Private accounts.** `AccountMention` already covers them, and a dry run would pass witnesses to `WholeExecution`. Left out until the private-recipient wallet fix (LEZ PR #985) lands.

## 4. The CLI

Accounts use the wallet's mention syntax: `Public/<id>`, a label, or `m/…` for a keycard. An actor is `<mention>[@<program>]`, where the program defaults to `--program`. A program can be named by id, or by guest name where prog-tui has a record.

```
lezcall iface  <program-id | guest | file.rs> [--json]
lezcall plan   --payer <m> --program <p> --to <m | pda:<SEED_CONST|seed>> <Method> [name=value]… [--json]
lezcall call   (same as plan)  [--leave-pending]   # plan, send, wait, check the root/target changed
lezcall send   --payer <m> --program <p> --message-hex <hex> --to <actor>[:sign] \
               [--actor <actor>[:sign]]… [--actor pda:<program>/<seed>]…   # explicit: no inference
lezcall encode --program <p> <Method> [name=value]…                         # message hex only
lezcall deploy --payer <m> <guest.bin>
```

`plan`'s output, for `jz_simple_token` `Mint` sent to the admin's seat:

```
jz_simple_token · 2 Mint { to: AccountId, amount: u128 }   message 02 b3…9e 1e00…00 (49 B)

run 1  Ad1… (signs: hint) → Tr7…   undeclared                → declare Tr7…
run 2  ok; async Credit → Bo2…                                → declare Bo2…
run 3  ok   3 deliveries, 412k cycles

root    Ad1…@token   signs
actor   Tr7…@token   treasury = PDA(token, Ad1…)   on chain, no evidence
actor   Bo2…@token   Credit(30) delivered in this tx
payer   9xP…
after   Tr7… {total 100, unissued 70}    Bo2… 30

same call by hand:
  lezcall send --payer Public/9xP… --program 4Fk… --message-hex 02b3…00 \
    --to Public/Ad1…:sign --actor Public/Tr7… --actor Public/Bo2…
```

`--json` gives the same plan as data, for prog-tui.

### The "equivalent wallet command"

The `wallet` CLI has no generic "send this message to this program" subcommand. Its only program subcommands are `auth-transfer`, `bridge` and `program-loader` ([cli/mod.rs:49](../logos-execution-zone/lez/wallet/src/cli/mod.rs#L49)). So:
- **Native token:** lezcall prints the real `wallet auth-transfer send …`.
- **Any other program:** it prints the `lezcall send …` line above. That line has every inferred piece written out explicitly, so it gives the same transaction with no inference. Its flags mirror `AccountMention` one-to-one. If a `wallet program call` subcommand is ever added upstream, these flags can be its interface, and the printed line changes only its first word. That would be a wallet change, which is outside the scope you set, so it isn't part of this design.

## 5. The examples, as they are

No example program needs a change for lezcall to call it.

| Program · message | Caller gives | Dry run finds |
|---|---|---|
| `hello_world`, `jz_hello_world` · `Vec<u8>` | `--to` | nothing more |
| `hello_world_with_authorization` | `--to` | root signs (hint, confirmed by the panic) |
| `hello_world_with_move_function` · `Write` | `--to` | nothing more |
| ″ · `MoveData { data, to }` | `--to` (the source), `data` = its current state | declare `to@program` (undeclared). A wrong `data` stops with the guest's message |
| ″ · `Append` | | hint *internal*. A direct call panics on `from_own_program` |
| `simple_tail_call` · `Actor` | `--to`, the callee actor | declare the callee (undeclared) |
| `tail_call_with_pda` · `AccountId` | `--to pda:PDA_SEED`, the callee program | root `Pda{caller, [37;32]}` evidence (seed constant). Declare `(pda, callee)`. The callee is authorized by the seed grant |
| `jz_hello_account` · `Hello`, `WriteAccountId` | `--to` | nothing more |
| `jz_admin_seat` · `Open(Vec<u8>)` | `--to` your account | you sign. Declare your PDA. Its first time: `Pda{program, your id}` (seed = the root's id) |
| ″ · `Init` | | hint *internal* |
| `jz_simple_token` · `Open`, `Burn` | `--to` your account | you sign. Declare the treasury. `Open`: `Pda` evidence, seed = your id |
| ″ · `Mint { to, amount }` | `--to` your account | as above, plus declare `to` for the async `Credit` (or `--leave-pending`) |
| ″ · `Transfer { to, amount }` | `--to` your account | you sign. Declare `to` for `Credit` |
| ″ · `Init`, `Draw`, `Return`, `Credit` | | hint *internal* |

**One optional program change** (not needed): `jz_admin_seat` and `jz_simple_token` could grant the PDA seed when the seat messages its PDA. That is, `Response::keep_state().send(SyncMessage::new(pda, &Init(…)).with_pda_seeds([PdaSeed::new(*admin.value())]))`. The engine then admits the PDA through the grant, with no evidence, and marks it `is_authorized`. The transaction would only need to declare the PDA: no seed search, and nothing different the first time. `Init`'s check could become `input.is_authorized && input.from_own_program()`. This would be an edit to `jz-progs/example_progs/{admin_seat,simple_token}.rs`, which prog-tui copies to the `jz_*` guests. It changes the examples' semantics a little (the PDA becomes authorized), so it's your call.

## 6. prog-tui on lezcall

- **`v`** runs `lezcall iface --json` and drops [msgtypes.rs](prog-tui/src/msgtypes.rs). It works for a deployment too, through the image id, and dims *internal* variants.
- **`c`:** pick a message, fill the params, then Enter runs `lezcall plan --json`. A confirm pane shows:
  - the plan: actors, who signs, evidence, pending async messages, predicted states;
  - or the guest's panic;
  - and the "same call by hand" line.

  A second Enter runs `lezcall call`. The `also`, `pda seed` and `sign` fields go: they become overrides behind a toggle, passed to `plan` as `--actor`, `--pda-seed` and `--sign`/`--no-sign`. `account` becomes `--to`, which also offers `pda:<CONST>` for each seed constant.
- **Log and `e`:** each call logs its "by hand" line, and `e` can print the last one.
- **The walkthrough's fixed forms** (`hello`, `move-*`, `tail`, `pda`) become ordinary interface calls with prefilled values, and `prog_cli` is deleted.

## 7. Where it lives

**Recommended:** a binary in `examples/program_deployment` (`src/bin/lezcall.rs` plus a module or two), replacing `prog_cli`.
- It already depends on `wallet`, `lee`, `lee_core` and risc0 through `lee`, so the only new dependencies are `syn`, `quote` and `serde_json`.
- prog-tui's `b` already builds it.
- The cost is that it lives on your LEZ branch, as `prog_cli` does now.

**The alternative:** a crate in `jz-progs` with path dependencies on the LEZ checkout. It's yours alone, but it compiles LEZ's dependency tree a second time, and has to be kept on the same LEZ commit as the checkout.

## 8. Open questions

1. The name, and where it lives (§7).
2. Should a `wallet program call` upstream proposal be part of this, or stay out of scope (§4)?
3. Should the seed-grant change to `jz_admin_seat` and `jz_simple_token` go in (§5)?
4. Should async messages to public accounts be delivered in the same transaction by default? That's what the wallet's `async_destination` does. The alternative is to leave them pending unless asked.
5. How this relates to SPEL v0.4: this answers [SPEL.md](SPEL.md) §7 open question 2 ("which actor it targets, so the CLI can build the transaction") without an IDL. A v0.4 IDL could still emit the §2 JSON, so that lezcall reads it instead of parsing the source.
