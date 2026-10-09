# LEZ programs: from the v0.2 model to the v0.4 actor model

A guide for developers writing or porting Logos Execution Zone (LEZ) programs. It covers what changed, why it changed, and what you need to do differently.

> **Status of "v0.4".** There is no `v0.4` tag yet. In this guide:
> - **v0.2** means LEZ tag **`v0.2.4`**, the last stable release of the account-rewriting model.
> - **v0.4** means the **actor model** on `logos-blockchain/logos-execution-zone` branch **`artem/transactional-actor-environment`**, at **`b17727de3` (2026-10-08)**. That branch is unmerged and still changing daily, with many `BREAKING!` commits. Names and details below may change before release.
>
> The intermediate v0.3.0 model (`plan`/`apply` on shards) is mentioned only where it explains a v0.4 design. See [MODEL_CHANGES.md](MODEL_CHANGES.md) for it.

Sources: the two trees above, the branch's commit messages (which record each design decision as "Before / After / Mitigation"), its `examples/program_deployment`, and its built-in programs under `lez/programs/`. Nothing here has been run.

---

## 1. The change in one paragraph

In v0.2, a program received **whole accounts** and returned **whole rewritten accounts**. The engine then checked the result against a set of ownership and conservation rules. In v0.4, a program is an **actor**: one handler that receives **one message**, sees **only its own state on one account**, and returns its new state plus **messages to other actors**. Anything else it needs to know or change, it must **ask the actor that owns it**. Messages are either a **Call**, which runs now in the same transaction, or a **Cast**, which may be delivered in a later transaction.

## 2. Side by side

| | v0.2.4 | v0.4 (actor branch) |
|---|---|---|
| **Account** | `{ program_owner, balance, data, nonce }`: one data blob, one owner | `{ nonce, data: { actor_states: Map<program, ActorState> } }`: one state slot **per program** |
| **Unit of state** | The account | The **actor**: `Actor { account_id, program_account_id }`, one program's state on one account |
| **Ownership** | A program *claims* a default account (`Claim::Authorized` / `Claim::Pda`) and owns all its data | No claiming. Every program owns **its own actor state on every account**, and only that |
| **Program entry** | `read_lee_inputs()` gives `pre_states` (whole accounts). Return `ProgramOutput` with `post_states` | `define_actor!(handle_message)`, with `fn handle_message(&ReceiveInput, Msg) -> Response` |
| **What the handler sees** | Every account passed in, including other programs' data | `receiver`, `from: Option<Actor>`, `is_authorized`, its own `pre_state`, and the message |
| **What it returns** | Full post-state of every account | `Response`: `keep_state()` or `set_state(bytes)`, plus `.call(..)`, `.cast(..)`, `.event(..)`, validity windows |
| **Correctness checks** | Engine compares pre and post: owner, nonce, balance decrease only by owner, conservation, data changed only by owner | Engine builds each input and rejects a forged echo (`TransitionInputMismatch`). A handler *can't* write anything but its own state, so most old rules have nothing to check |
| **Native balance** | A field any program could change, within the rules | The native-token program's actor state (program id all zeros). Only that program writes it |
| **Calling another program** | `ChainedCall { program_id, pre_states, .. }`. The caller **hands over account values** | `Call::new(Actor, &msg)`. The callee loads its own state and the engine computes its `is_authorized` |
| **Asynchronous delivery** | None | `Cast`: a durable message, published as pending and received by a later transaction |
| **Program identity** | `ProgramId`, the RISC Zero image ID | `program_account_id`, the address of a `program_loader` header account |
| **Deploy** | `wallet deploy-program`, free | `program_loader` header and segment accounts, in a fee-paying transaction (`wallet program-loader …`) |
| **Message encoding** | risc0 serde | borsh |
| **Fees** | None | Every user public transaction carries a `FeeDeclaration`. Reverts are included and charged |
| **Events** | None | `Response::event(ProgramEvent { selector, data })` |
| **State size** | Account data capped at 100 KiB | No per-actor cap. Size is priced by storage fees |
| **Funding** | Genesis balances and a faucet | Bridge deposits only. Genesis is synthetic deposits |

## 3. Key design decisions and why

1. **Isolation by construction, not by rule-checking.** v0.2 let a program produce any post-state, then tried to catch bad ones with pre/post rules. v0.4 gives a handler only its own state, so it *cannot* write another program's data or someone's balance. The engine's job shrinks to rebuilding each input and checking the program echoed it honestly. This is simpler to audit and harder to get wrong.

2. **One model for public and private execution.** The same driver runs a handler in four modes: live public execution, private derivation, proof recording, and settlement checking. A program cannot tell whether an account is public or private and behaves identically. That is what makes mixed public/private transactions, and private calls into public programs, possible.

3. **Ask the owner, don't read its data** (commit `396a2adce`: "account-specific facts from another actor travel as fields its program fills from its own receiver"). Reading another program's bytes ties you to its encoding and lets you act on data you can't verify. In v0.4 the owning program answers for its own state. It either asserts a claim (the token program's `AssertKind`) or replies with its state (the native token's `ReadState` → `StateReply`).

4. **Message passing replaces planning.** v0.3 split a program into a data-blind `plan` and a one-shard `apply`. That forced every value a program needed into the instruction as an unchecked claim. v0.4 keeps the per-actor isolation but lets a handler see its own state, decide, and then send further messages. Multi-step logic, such as a read, then an action, then a re-check, becomes a chain of messages. The handler does not have to predict everything up front.

5. **Call versus Cast.** A Call runs depth-first inside the transaction and rolls back with it. A Cast is a durable message for a later transaction. Casts exist because a sender often cannot run code on the recipient's account. A private recipient's state can only be updated by someone holding its keys. So credits to other people are Cast, and the recipient (or their wallet) receives them (commit `895e009be`).

6. **Authenticate by sender, not by copied data** (`2c40b5ef8`, `4f104e132`). A receiver gets `from: Option<Actor>`, the full sending actor as set by the engine. Notifications and state replies are trusted because of *who sent them*, not because they carry an account id. A private sender may appear **blinded** (`0e3e8e649`), so its real address is not revealed.

7. **Least authority across Casts** (`3ba67dde9`). A Call can pass PDA authority on with `with_pda_seeds`, and authorization the caller inherited carries down the Call tree. A Cast, even one run immediately, carries **no** inherited authorization and no seed grants. Only the transaction's own signatures still apply. A deferred message cannot carry delegated power into another transaction.

8. **Programs are addressed by deployment, not by bytecode.** The same ELF can be deployed at several addresses, each a separate instance with its own PDAs and state. Callees are passed in messages, never hard-coded, because the address is only known after deploy.

## 4. What it means for program design

### 4.1 Required changes

1. **Rewrite the entry point.** Replace `read_lee_inputs` and `ProgramOutput` with `define_actor!(handle_message)`. Define a borsh `enum Message` with one variant per operation, including the internal messages your program sends to itself.
2. **Make one actor do one account's work.** A handler changes one actor state. An operation that touches several accounts becomes your program sending messages to its own actors on those accounts, as in `move_data` below.
3. **Guard internal messages.** A variant meant only for your own program, such as `Append` or `Credit`, must check `input.from_own_program()`. Otherwise anyone can send it directly.
4. **Replace claiming with "is my state empty?".** Remove `Claim::*` and `program_owner`. An uninitialised account is one where `pre_state.is_empty()`. For write-once data, use `lee_core::program::write_once`.
5. **Stop reading other programs' data.** Replace each read with a Call to the owning actor: an assertion message that panics if the claim is false, or a read request whose reply you handle in a later message.
6. **Move native funds through the native-token program.** Replace direct `balance` edits with `custody_transfer(from, seed, to, amount)` from a PDA your program controls. A user's own funds move only on the user's own authorized transfer.
7. **Address callees by `Actor`.** The address is `(account_id, program_account_id)`. Take callee program addresses as message fields or configuration, never compile-time constants.
8. **Expect credits to arrive later.** Native and token transfers debit the sender now and Cast the credit. A credit to a declared public account runs in the same transaction. A credit to a private account runs in the same transaction only if the transaction builder selects it, and is otherwise published as pending. Don't write logic that assumes a recipient's balance has changed in the same transaction unless you control that.
9. **Switch to borsh** for every message and state type.
10. **Clients:** deploy through `program_loader`, attach a fee and payer to every call, and name the target `Actor` plus the declared public actors. Treat "included" as different from "succeeded", and use `wallet pending list` / `wallet pending receive --position N` to collect cast credits.

### 4.2 Improvements you get

- **Smaller, local handlers.** A handler is a function of `(its state, message, sender, authorized?)`. That is easy to unit test without building whole accounts.
- **No more trusting caller-supplied account values.** v0.2 callees received `pre_states` from their caller. In v0.4 the engine loads the callee's own state and computes `is_authorized` itself.
- **Composability with real control flow.** You can read, act, and re-check within one atomic transaction. The `flash_swap_initiator` test program reads a vault balance, lends, runs an arbitrary callback, then reads the balance again and reverts if it wasn't repaid. Nested swaps work because delivery is depth-first.
- **Shared accounts without conflicts.** Many programs can keep state on the same account. A token holding, an ATA marker and your own data can all live on one user account.
- **Privacy for free.** The same program runs on public and private accounts with no code change. Senders can be presented blinded.
- **Events** give indexers a stable feed without diffing state. **Validity windows** let a handler limit when the transaction is valid.
- **No per-account size cap.** Size is limited by what you pay in storage fees.

### 4.3 Considerations and pitfalls

- **Every unauthenticated field is a claim.** If a message says "the source holds X" or "the sender is Y", some actor must verify it. Check `input.from` against the actor you expect, and check claims against `pre_state` before acting. This is the most important review rule.
- **`is_authorized` is per delivery.** It comes from a transaction signature, the private account's `ask` key, or a PDA seed granted by the calling program. A Cast, even one promoted to run now, inherits none of the caller's grants. If a callee needs PDA authority, deliver it with a Call and `with_pda_seeds`.
- **Ordering.** Calls run depth-first, in the order a handler lists them, each subtree before the next sibling. A handler's Casts are collected before those of its children. Receipt of pending Casts is **unordered**, so never rely on two Casts arriving in sequence.
- **Pending messages are received only at the transaction entry.** A program cannot pull a pending message mid-transaction (`26e671fe5`). A Cast to a private address needs that address's recovery binding first, or the transaction fails and is still charged (`f4d66fa87`).
- **Multi-step flows need state.** A request and its reply are separate handler runs. Keep "what I'm waiting for" in your own actor state, as a stack if calls can nest, and clear it when done.
- **Reverts cost money.** A failed transaction is included and charged, and only the nonce advances. Fail early, before expensive sends, and let clients simulate.
- **Fee-paying everything.** Deploys and every user call need a funded, signing payer. There is no faucet.
- **The tutorial lags the code.** The branch's `examples/program_deployment/README.md` still says `wallet deploy-program` and mentions a hard-coded hello-world ID. The CLI has `wallet program-loader` instead, and callee ids are now passed in the message.
- **Specs are stale.** LIP 237 describes the v0.2 model. The best references are `lee/state_machine/core/src/program/mod.rs`, the engine tests in `lee/state_machine/core/src/execution_state/tests.rs`, and the built-in programs.
- **SPEL and scaffold** target the older models. `#[account(init)]` and the account-list style do not map to actors yet.

## 5. Common patterns, before and after

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
lee_core::define_actor!(handle_message);

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

v0.2 read the source and wrote both accounts in one go. In v0.4 the source actor checks the claimed bytes against its own state, clears itself, and Calls its sibling actor on the destination. The destination accepts only a message from its own program.

```rust
// v0.4 (examples/.../hello_world_with_move_function.rs)
enum Message { Write(Vec<u8>), MoveData { data: Vec<u8>, to: AccountId }, Append(Vec<u8>) }

fn handle_message(input: &ReceiveInput, message: Message) -> Response {
    match message {
        Message::MoveData { data, to } => {
            assert_eq!(input.pre_state.as_ref(), data.as_slice(), "source does not hold these bytes");
            Response::set_state(Vec::new())
                .call(Actor::new(to, input.receiver.program_account_id), &Message::Append(data))
        }
        Message::Append(data) => {
            assert!(input.from_own_program(), "only a move of this program appends");
            Response::set_state([input.pre_state.as_ref(), &data].concat())
        }
        Message::Write(data) => Response::set_state([input.pre_state.as_ref(), &data].concat()),
    }
}
```

### D. Token transfer

v0.2's `transfer` read both holdings and returned two post-states. In v0.4 the message goes to the **sender's** token actor. It checks authorization, debits, and Casts a `Credit` to the recipient's token actor. That actor accepts the credit only from the token program, then creates or updates the holding.

```rust
// v0.4 (lez/programs/token/src/lib.rs, simplified)
Message::Transfer { descriptor, amount, .. } => {
    assert!(input.is_authorized, "Sender authorization is missing");
    Some(transfer::withdraw(&input.pre_state, &descriptor, amount))   // + Cast Credit to `to`
}
Message::Credit { descriptor, amount, .. } => {
    assert!(input.from_own_program(), "A credit must come from the token program");
    Some(transfer::deposit(&input.pre_state, &descriptor, amount))   // empty state = new holding
}
```

To react to an incoming payment, attach `notify: Some(Notify { to, payload })` to the transfer. The credit then Calls your actor with a `Notification`. Your actor must check that `input.from` is the holding it expects under the token program it expects, as the AMM does.

### E. Move native balance

```rust
// v0.2.4: edit balances directly; engine checks conservation
holder.balance = holder.balance.checked_sub(amount).unwrap();
escrow.balance = escrow.balance.checked_add(amount).unwrap();
```

```rust
// v0.4: ask the native-token program, from a PDA this program controls
Response::keep_state().send(custody_transfer(vault_pda, VAULT_SEED, recipient, amount))
```

The native-token actor debits the vault, which is authorized by the seed, and Casts the credit to the recipient.

### F. Depend on another program's state

v0.2 read the other program's data directly. v0.4 offers two patterns.

- **Assert.** Call the owner with a check, and it panics if the check fails. The ATA program does this before creating a holding: `.call(Actor::new(definition_id, token_program), &token::Message::AssertKind { kind })`.
- **Read and reply.** Call the owner with a read request. It Calls you back with `StateReply { state }`. Check `input.from` is the actor you asked, decode, and continue. Keep the pending step in your own state. Currently the native-token program implements `ReadState`, and the `fee` program uses it.

```rust
// v0.4: read a native balance, then act in the reply handler
Response::set_state(save(Phase::AwaitingBalance { vault, expected }))
    .call(Actor::native_balance(vault), &native_token::Message::ReadState)

// later, in the handler for the reply:
assert_eq!(input.from, Some(Actor::native_balance(vault)), "the reply must read the vault");
let balance = decode_balance(&reply.state)?;
```

### G. Call another program, with PDA authority

```rust
// v0.2.4: hard-coded image ID, caller passes account values
ChainedCall { program_id: HELLO_WORLD_ID, instruction_data, pre_states, pda_seeds: vec![] }
```

```rust
// v0.4: callee address from the message; grant the PDA seed if it needs authorization
fn handle_message(input: &ReceiveInput, callee: AccountId) -> Response {
    Response::keep_state().send(
        Call::new(Actor::new(input.receiver.account_id, callee), &greeting)
            .with_pda_seeds(vec![PDA_SEED]),
    )
}
```

PDAs derive from the program's **deployed address** and a seed: `AccountId::for_public_pda(&program_account_id, &seed)`. Private PDAs also bind the controllers' keys.

### H. Emit an event

There was no v0.2 equivalent. In v0.4 use `Response::...event(ProgramEvent { selector, data })`. The convention is that `selector` is the first 8 bytes of `sha256("<program>::<EventName>")`. The runtime records which program emitted it, and the indexer serves events by query and subscription.

## 6. Porting checklist

- [ ] One borsh `Message` enum. `define_actor!` entry. Handler returns `Response`.
- [ ] Each operation split into per-actor messages. Internal variants guarded by `from_own_program()`.
- [ ] Claims (`Claim::*`, `program_owner`) replaced with empty-state checks.
- [ ] Every read of foreign data replaced with an assert-Call or a read/reply flow.
- [ ] Every `input.from` you rely on checked against the exact actor you expect.
- [ ] Native movements via `custody_transfer` from your PDAs. Token movements via the token program.
- [ ] No logic that needs a Cast credit to land in the same transaction.
- [ ] PDA authority passed only by Call, never assumed across a Cast.
- [ ] Callee addresses supplied at runtime.
- [ ] Events added for anything off-chain consumers watch.
- [ ] Clients: `program_loader` deploy, fee payer, target `Actor` and public actors, pending-message receipt.

## 7. Open questions

1. **Is the actor branch what ships as v0.4?** Nothing upstream labels it v0.4, and it is not merged into `dev`. Confirm with the LEZ team before publishing this guide.
2. Is there a call-depth or message-count limit in v0.4? v0.3 capped chained calls at 10. I found no equivalent constant on the branch.
3. Will other built-ins, such as the token program, answer `ReadState`, or is read/reply left to each program?
4. When will SPEL and scaffold target the actor model?
