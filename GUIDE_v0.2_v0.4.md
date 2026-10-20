# Porting LEZ programs from v0.2 to the v0.4 actor model

A porting guide for developers who have a Logos Execution Zone (LEZ) program written for **v0.2** and need to move it to **v0.4**. It covers what changed, why, and what to do differently. It does not teach the actor model from scratch. For that, use the actor-model manual (link below). To build and run a program, use [INTRO.md](INTRO.md) section 4.

> **What "v0.4" means here.** There is no `v0.4` tag yet.
> - **v0.2** is LEZ tag **`v0.2.4`**, the last stable release of the account-rewriting model. It is **legacy**.
> - **v0.4** is the **actor model on `dev`** of `logos-blockchain/logos-execution-zone`. It is how programs will be written. It arrived with PR #935 (`artem/transactional-actor-environment`, merge `fa68310f6`). This guide was checked against `dev` at **`e2913677b`** (2026-10-14). `dev` still moves, so recheck names before you rely on them.
> - v0.3.0 (`plan`/`apply` on shards) is the current testnet, but very short-lived, so don't port to it. It is in [MODEL_CHANGES.md](MODEL_CHANGES.md). It appears here only where it explains a v0.4 design. Port straight from v0.2 to v0.4.

## Read this next to the actor-model manual

The manual is `docs/actor-program-development-book/` on LEZ branch **`moudy/guide-mdbook-fixes`**, which fixes schouhy's `schouhy/lez-guide-mdbook` ([browse](https://github.com/logos-blockchain/logos-execution-zone/tree/moudy/guide-mdbook-fixes/docs/actor-program-development-book/src); build it with `mdbook`). This guide links to it by chapter and does not repeat it. The chapters you will use most:

| Topic | Chapter |
|---|---|
| Accounts, actors, state | `foundations/actors.md` |
| Handlers, Call/Cast, ordering, replies, validity windows, events | `foundations/messages.md` |
| `is_authorized`, PDAs, custody, checking the sender | `foundations/authorization.md` |
| Transactions, declared actors, admission, fees | `foundations/transactions.md` |
| Private accounts and the mailbox | `foundations/private-accounts.md` |
| Deployment, identity, upgrades | `foundations/programs.md` |
| Native token, clock, fee, bridge | `foundations/system-programs.md` |
| Guest API cheat sheet, errors | `reference/api.md` |
| Testing a handler | `foundations/toolchain.md` |

> **Names in the manual are one rename behind `dev`.** It was written against an earlier state of PR #935 and says so in its "state of the names" section. Read it with this map:
>
> | Manual | `dev` |
> |---|---|
> | `Call` | `SyncMessage` |
> | `Cast` | `AsyncMessage` |
> | `Response::call(..)`, `.cast(..)` | `Response::send_sync(..)`, `.send_async(..)` |
> | `Response.calls`, `.casts` | `Response.sync_messages`, `.async_messages` |
> | `define_actor_logic!` | `actor_entrypoint!` |
>
> This guide uses the `dev` names. "Sync" is delivered now, in the same transaction. "Async" may be delivered in a later transaction.

---

## 1. The change in one paragraph

In v0.2, a program received **whole accounts** and returned **whole rewritten accounts**, and the engine checked the result against ownership and conservation rules. In v0.4, a program is an **actor**: one handler that receives **one message**, sees **only its own state on one account**, and returns its new state plus **messages to other actors**. Anything else it needs to know or change, it asks the actor that owns it.

## 2. Side by side

| | v0.2.4 | v0.4 (`dev`) |
|---|---|---|
| **Account** | `{ program_owner, balance, data, nonce }`: one data blob, one owner | `{ nonce, data: { actor_states: Map<program, ActorState> } }`: one state slot **per program** |
| **Unit of state** | The account | The **actor**: `Actor { account_id, program_account_id }` |
| **Ownership** | A program *claims* a default account (`Claim::Authorized` / `Claim::Pda`) and owns all its data | No claiming. Every program owns **its own actor state on every account**, and only that |
| **Program entry** | `read_lee_inputs()` gives `pre_states` (whole accounts). Return `ProgramOutput` with `post_states` | `actor_entrypoint!(handle_message)`, with `fn handle_message(&ReceiveInput, Msg) -> Response` |
| **What the handler sees** | Every account passed in, including other programs' data | `receiver`, `from: Option<Actor>`, `is_authorized`, its own `pre_state`, and the message |
| **What it returns** | Full post-state of every account | `Response`: `keep_state()` or `set_state(bytes)`, plus `.send_sync(..)`, `.send_async(..)`, `.event(..)`, validity windows |
| **Correctness checks** | Engine compares pre and post: owner, nonce, balance decrease only by owner, conservation | Engine builds each input and rejects a handler whose echoed input differs (`TransitionInputMismatch`). A handler can't write anything but its own state, so most old rules have nothing left to check |
| **Native balance** | A field any program could change, within the rules | The native token's actor state (program id `[0; 32]`, `NATIVE_TOKEN_PROGRAM_ID`). Only that program writes it |
| **Calling another program** | `ChainedCall { program_id, pre_states, .. }`. The caller **hands over account values** | `SyncMessage::new(Actor, &msg)`. The callee loads its own state and the engine computes its `is_authorized` |
| **Deferred delivery** | None | `AsyncMessage`: delivered now if the transaction can run the receiver, otherwise stored for its owner to collect |
| **Program identity** | `ProgramId`, the RISC Zero image ID | `program_account_id`, the address of a `program_loader` header account |
| **Deploy** | `wallet deploy-program`, free | `wallet program-loader deploy …`, a fee-paying transaction |
| **Message encoding** | risc0 serde | borsh |
| **Fees** | None | Every user public transaction carries a `FeeDeclaration`. Reverts are included and charged |
| **Events** | None | `Response::event(ProgramEvent { selector, data })` |
| **State size** | Account data capped at 100 KiB | No per-actor cap. Size is priced by storage fees |
| **Funding** | Genesis balances and a faucet | No faucet. The local debug chain funds two test accounts at genesis, and real funds enter by bridge deposit |

## 3. Why it changed

- **Isolation by construction, not by rule-checking.** v0.2 let a program produce any post-state and then tried to catch bad ones. A v0.4 handler is given only its own state, so it *cannot* write another program's data or someone's balance. See the manual's `foundations/actors.md` and `foundations/echo.md`.
- **One model for public and private.** The same handler runs on public accounts (on the sequencer) and private ones (in the owner's prover) with no code change. That is what makes mixed transactions, and payments to offline private recipients, possible. See `foundations/messages.md` and `foundations/private-accounts.md`.
- **Message passing replaces planning.** v0.3 split a program into a state-blind `plan` and a one-shard `apply`, which forced every value into the instruction as an unchecked claim. A v0.4 handler sees its state, decides, and then sends further messages. A read, an action and a re-check become a chain of messages.
- **Ask the owner, don't read its data.** Reading another program's bytes ties you to its encoding and lets you act on data you can't verify. The owning program answers for its own state, either by asserting a claim or by replying with its state.

## 4. Porting steps

1. **Rewrite the entry point.** Replace `read_lee_inputs` and `ProgramOutput` with `lee_core::actor_entrypoint!(handle_message)`. Define a borsh `enum Message` with one variant per operation, including the internal messages your program sends to itself.
2. **Make one actor do one account's work.** A handler changes one actor state. An operation that touches several accounts becomes your program sending sync messages to its own actors on those accounts (pattern C).
3. **Guard internal messages.** A variant meant only for your own program, such as `Append` or `Credit`, must check `input.from_own_program()`. Otherwise anyone can send it.
4. **Replace claiming with "is my state empty?".** Remove `Claim::*` and `program_owner`. An uninitialised account is one where `pre_state.is_empty()`. For write-once data, use `lee_core::program::write_once`.
5. **Stop reading other programs' data.** Replace each read with a message to the owning actor: an assertion that panics if the claim is false, or a read request whose reply you handle in a later message (pattern F).
6. **Move native funds through the native token.** Replace direct `balance` edits with `custody_transfer(from, seed, to, amount)` from a PDA your program controls. A user's own funds move only on the user's own authorized `Transfer` (patterns D and E).
7. **Address callees by `Actor`.** The address is `(account_id, program_account_id)`. Take callee program addresses as message fields or configuration, never compile-time constants, because the address is only known after deployment and the same binary can be deployed more than once.
8. **Don't assume credits land in the same transaction.** Native transfers debit now and send the credit as an async message. A credit to a declared public account runs in the same transaction. A credit to a private account runs in the same transaction only if the builder selects it, and is otherwise stored. Don't write logic that depends on the recipient's balance having changed unless you control that.
9. **Switch to borsh** for every message and state type.
10. **Update clients.**
    - Deploy through `wallet program-loader deploy`.
    - Attach a fee and payer to every user call.
    - Name the target `Actor` and every public actor the transaction touches.
    - Treat "included" as different from "succeeded".
    - Collect stored messages with `wallet pending list` and `wallet pending receive --position N`.

### Checklist

- [ ] One borsh `Message` enum. `actor_entrypoint!` entry. Handler returns `Response`.
- [ ] Each operation split into per-actor messages. Internal variants guarded by `from_own_program()`.
- [ ] Claims (`Claim::*`, `program_owner`) replaced with empty-state checks.
- [ ] Every read of foreign data replaced with an assertion or a read/reply flow.
- [ ] Every `input.from` you rely on checked against the exact actor you expect.
- [ ] Native movements via `custody_transfer` from your PDAs, and user transfers via the native token's `Transfer`.
- [ ] No logic that needs an async credit to land in the same transaction.
- [ ] PDA authority passed only by a sync message with `with_pda_seeds`, never assumed across an async one.
- [ ] Callee addresses supplied at runtime.
- [ ] Events added for anything off-chain consumers watch.
- [ ] Clients: `program-loader` deploy, fee payer, target `Actor` and declared public actors, stored-message receipt.

## 5. Pitfalls specific to porting

The manual covers the rest. Read `foundations/authorization.md` ("Knowing who sent a message") and `foundations/messages.md` in full before you ship.

- **Every unauthenticated field is a claim.** A message that says "the source holds X" or "the sender is Y" needs some actor to verify it. Check `input.from` against the actor you expect, and check claims against `pre_state` before acting. v0.2 programs got this check for free from the engine's pre/post rules. In v0.4 you write it.
- **`is_authorized` looks the same and means something different.** In v0.2 it was a bit on a caller-supplied account. In v0.4 the engine computes it for each delivery, from a signature, a private key, or a PDA seed grant. An async message carries no grant. If a callee needs PDA authority, send it as a sync message with `with_pda_seeds`.
- **Reverts cost money.** A failed transaction is included and charged, and only the nonce advances. Fail early, and simulate before you submit.
- **There is no faucet.** Deploys and every user call need a funded, signing payer.
- **Two things in the repo still describe the old flow.** `examples/program_deployment/README.md` says `wallet deploy-program`, which is now `wallet program-loader deploy`. The example runners build calls with `Message::try_new` and no fee, which `dev` rejects as `MissingFeeDeclaration`. [INTRO.md](INTRO.md) section 4 has the working steps.
- **Tooling.** SPEL and scaffold targeted the older models. I found no actor-model support for them on `dev`, and I haven't checked their repos.

## 6. Common patterns, before and after

### A. Write to an account (hello world)

```rust
// v0.2.4: rewrite the whole account, claim it if new
let (ProgramInput { self_program_id, caller_program_id, pre_states, instruction: greeting }, instruction_data)
    = read_lee_inputs::<Vec<u8>>();
let [pre_state] = pre_states.try_into().unwrap();
let mut post = pre_state.account.clone();
let mut bytes = post.data.into_inner();
bytes.extend_from_slice(&greeting);
post.data = bytes.try_into().unwrap();
let post_state = AccountPostState::new_claimed_if_default(post, Claim::Authorized);
ProgramOutput::new(self_program_id, caller_program_id, instruction_data, vec![pre_state], vec![post_state]).write();
```

```rust
// v0.4: change only this program's actor state
use lee_core::program::{ReceiveInput, Response};

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, greeting: Vec<u8>) -> Response {
    let mut bytes = input.pre_state.to_vec();
    bytes.extend(greeting);
    Response::set_state(bytes)
}
```

The v0.2 version needed the account's signature to claim it. The v0.4 version needs none, because it writes only its own slot. The payer still signs for the fee.

### B. Require the account holder's authorization

```rust
assert!(pre_state.is_authorized);   // v0.2.4
assert!(input.is_authorized);       // v0.4
```

The check looks the same. What changed is where the bit comes from: the engine computes it for every delivery, so a caller cannot forge it by passing in an account value.

### C. Move data between two accounts

v0.2 read the source and wrote both accounts in one go. In v0.4 the source actor checks the claimed bytes against its own state, clears itself, and sends a sync message to its sibling actor on the destination. The destination accepts only a message from its own program.

```rust
// v0.4 (examples/program_deployment/methods/guest/src/bin/hello_world_with_move_function.rs)
enum Message { Write(Vec<u8>), MoveData { data: Vec<u8>, to: AccountId }, Append(Vec<u8>) }

fn handle_message(input: &ReceiveInput, message: Message) -> Response {
    match message {
        Message::MoveData { data, to } => {
            assert_eq!(input.pre_state.as_ref(), data.as_slice(), "source does not hold these bytes");
            Response::set_state(Vec::new())
                .send_sync(Actor::new(to, input.receiver.program_account_id), &Message::Append(data))
        }
        Message::Append(data) => {
            assert!(input.from_own_program(), "only a move of this program appends");
            Response::set_state([input.pre_state.as_ref(), &data].concat())
        }
        Message::Write(data) => Response::set_state([input.pre_state.as_ref(), &data].concat()),
    }
}
```

The instruction carries the bytes, and the source verifies them against its own state. This replaces the v0.2 engine reading the source for you.

### D. Transfer a balance

The token program is no longer on `dev`. The native token is the worked example of a transfer. v0.2's transfer read both balances and returned two post-states. In v0.4 the message goes to the **sender's** native-balance actor. It checks authorization, debits, and sends an async `Credit` to the recipient's actor. That actor accepts the credit only from its own program.

```rust
// v0.4 (lee/state_machine/core/src/native_token.rs, trimmed)
Message::Transfer { to, amount } => {
    debit(input, amount)?                       // needs input.is_authorized
        .send_async(Actor::native_balance(to), &Message::Credit(amount))
}
Message::Credit(amount) => {
    if !input.from_own_program() { return Err(TransferError::ForeignCredit { account_id }); }
    /* checked_add into pre_state */
}
```

Because the credit is async, a public recipient is credited in the same transaction, and a private recipient collects it later with `wallet pending receive`. For a custom token, write the same shape in your own program. The manual's `catalog/tokens.md` sketches one, though it hasn't been built.

### E. Move native funds your program holds

```rust
// v0.2.4: edit balances directly; engine checks conservation
holder.balance = holder.balance.checked_sub(amount).unwrap();
escrow.balance = escrow.balance.checked_add(amount).unwrap();
```

```rust
// v0.4: ask the native token, from a PDA this program controls
Response::keep_state().send(custody_transfer(vault_pda, VAULT_SEED, recipient, amount))
```

`custody_transfer` is a sync message to `Actor::native_balance(vault_pda)` that carries the seed. The native token debits the vault, which the seed authorizes, and sends the credit on. The bridge pays out deposits this way (`lez/programs/bridge`).

### F. Depend on another program's state

v0.2 read the other program's data directly. v0.4 has two patterns.

- **Assert.** Send the owner a message that states a condition. Its handler panics if the condition is false, which fails the whole transaction. The clock works this way: `Actor::new(CLOCK_01_PROGRAM_ACCOUNT_ID, clock_core::clock_account_id())` with `clock_core::Message::AssertTimestamp { at_least, at_most }` (`lez/programs/clock`).
- **Read and reply.** Send a read request. The owner sends a sync message back with its state. Check that `input.from` is the actor you asked, decode, and continue. Keep the pending step in your own state. The native token implements `ReadState` → `StateReply`, and the `fee` program uses it (`lez/programs/fee/src/main.rs`).

```rust
// v0.4: read a native balance, then act in the reply handler
Response::set_state(save(Phase::AwaitingBalance { vault, expected }))
    .send_sync(Actor::native_balance(vault), &native_token::Message::ReadState)

// later, when the reply arrives:
assert_eq!(input.from, Some(Actor::native_balance(vault)), "the reply must come from the vault's balance");
let reply: StateReply = borsh::from_slice(&input.message)?;
let balance = native_token::decode_balance(&reply.state)?;
```

`ReadState` only works when another actor sends it. A message submitted by the transaction itself has no sender to reply to. Among the built-ins, only the native token answers `ReadState`. Any other program must define its own read message.

### G. Call another program, with PDA authority

```rust
// v0.2.4: hard-coded image ID, caller passes account values
ChainedCall { program_id: HELLO_WORLD_ID, instruction_data, pre_states, pda_seeds: vec![] }
```

```rust
// v0.4 (examples/.../tail_call_with_pda.rs): callee address comes in the message
const PDA_SEED: PdaSeed = PdaSeed::new([37; 32]);

fn handle_message(input: &ReceiveInput, callee_account_id: AccountId) -> Response {
    Response::keep_state().send(
        SyncMessage::new(Actor::new(input.receiver.account_id, callee_account_id), &greeting)
            .with_pda_seeds(vec![PDA_SEED]),
    )
}
```

PDAs derive from the program's **deployed address** and a seed: `AccountId::for_public_pda(&program_account_id, &seed)`. Private PDAs also bind the controllers' keys. See `foundations/authorization.md`.

### H. Emit an event

There was no v0.2 equivalent. Use `Response::…event(ProgramEvent { selector, data })`. By convention `selector` is the first 8 bytes of `sha256("<program>::<EventName>")`. The runtime records which program emitted it, and the indexer serves events by query and subscription. Only public handlers' events are kept (see `foundations/messages.md`).

## 7. Open questions

1. **Limits.** I found no call-depth or message-count constant in the engine on `dev`. v0.3 capped chained calls at 10. Execution is metered by gas, so a limit may now be gas rather than depth. Confirm with the LEZ team.
2. **Token, AMM and ATA.** They were removed from `dev` (commit `c16cf3000`). Ask whether they return as actor programs, and when. Until then, custom tokens are yours to write.
3. **Tooling.** When will SPEL and scaffold target the actor model?
4. **Names.** The manual and `dev` can still drift. Check `lee/state_machine/core/src/program/mod.rs` for the current names before you copy code from either.
