
## 6. What the v0.3 "Journey" issues add for program authors (#506–#514)

`logos-co/logos-docs` issues #506–#511 and #514 are open **[Journey]** issues, written by the LEZ developers between 2026-09-28 and 2026-09-30, each targeting **testnet v0.3**. Each one describes a user journey that needs a docs page, and **none of them has a page in this repo yet**. #512 and #513 in that range are merged Storage PRs, not LEZ.

The issues link code at `f7fda38a4` and `079e4fb6b` on `dev`, and both commits are in the `v0.3.0` tag. #511 uses tag `v0.3.0-rc1`, and #514 uses `v0.3.0`.

| Issue | Journey | Relevance to writing a program | Runnable check from the issue |
|---|---|---|---|
| #509 | Incremental updates: the `plan`/`apply` program ABI | **Core.** This is the ABI every v0.3 program is written against | `RISC0_DEV_MODE=1 cargo test --release -p lee_core execution_state` |
| #514 | Fees for public transactions | **Core.** Every user public tx must declare a fee, including deploys and calls | Local walkthrough with exact balances ([INTRO.md](INTRO.md) section 3 uses it) |
| #507 | Event exposure | High. Programs emit events, and clients query them from the indexer | `RISC0_DEV_MODE=1 cargo test --release -p indexer_core accept_block_captures_emitted_events` |
| #508 | Private authorization key (`ask`) | High for any program that checks `is_authorized` on private accounts | `RISC0_DEV_MODE=1 cargo test --release -p lee ask` |
| #506 | Cross-zone messaging | Medium. Writing emitter and target programs between zones | `just demo-cross-zone-bridge` (needs Docker) |
| #510 | L1 bridge: deposits and funding | Medium. Where all zone funds come from. Explains the missing faucet | `RISC0_DEV_MODE=1 cargo test --release -p bridge_program -p programs bridge` |
| #511 | Run a decentralised sequencer | Low. Operator journey. Useful for the testnet RPC URL and the committee model | Join and stake steps (needs Bedrock) |

### 6.1 Program ABI: `plan` and `apply` (#509)

The program no longer receives account state or returns a post-state. Programs written against the old `ProgramOutput`/`AccountPostState` ABI **no longer compile**, and that includes the v0.2.4 hello world in [INTRO.md](INTRO.md) section 2.

- **`plan(input: &PlanInput, instruction) -> Plan`.** It can't see any account data. `PlanInput` has `self_account_id`, `caller_account_id`, `accounts: Vec<AccountMeta>` (id, `is_authorized`, `program_account_id`; no data) and `instruction_data`. The `Plan` builder collects the program's intents:
  - `plan.effect(account, &effect)`: change this program's shard on a named account handle
  - `plan.inspect(account, owner, &guard)`: a read-only check, run through the owning program's own `apply`
  - `plan.call(..)`: a chained call (`ChainedCall { program_account_id, shard_selectors, instruction_data, pda_seeds }`, at most 10 deep)
  - `plan.event(..)`: emit an event (6.3)
  - validity-window setters, which narrow the block range in which the transaction is accepted
- **`apply(effect, pre_data: &ShardData) -> Option<…>`** sees exactly one shard. It returns `None` to keep the shard, `Some(empty)` to clear it, or `Some(data)` to replace it.
- **Sharded accounts.** `Account` = nonce + `shards: BTreeMap<AccountId, ShardData>`, keyed by the owning program. `ProgramShardSelector { account_id, program_account_id }` names one pair. Accounts can no longer be claimed, and **a program can write only its own shard**.
- **Native balance** lives in the native-token shard (program id all zeros). Only protocol code moves it: no guest program can write it.
- **What the engine enforces.** The engine builds every input itself and rejects outputs that don't echo them (`PlanInputMismatch`, `ApplyInputMismatch`). It also rejects effects outside the declared accounts (`EffectOutsideInputs`) and writes to another program's shard (`ForeignShardWrite`). Public and private execution share one engine, so a program behaves the same on both.
- **The tests read as the spec:** `lee/state_machine/core/src/execution_state/tests.rs`, `lee/state_machine/core/src/program/tests.rs`, and `lee/state_machine/src/state/tests/public_program_rules.rs`.

### 6.2 Fees (#514)

- Every user public transaction carries a `FeeDeclaration { payer, gas_limit, tip, max_fee }` inside the signed message, and **the payer must be a signer**. A transaction without one is rejected; it is never run for free. Program deployment is charged too (storage gas only).
- `reserve = gas_limit × base_fee_exec + tx_bytes × base_fee_stor + tip`. `actual = min(cycles, gas_limit) × base_fee_exec + tx_bytes × base_fee_stor + tip`. The difference is refunded.
- **Reverts are included and charged.** A panic or running out of gas is billed the full `gas_limit`. A clean non-zero exit is billed for the cycles it used. The nonce still advances. The wallet doesn't report reverts, and the fee is reported nowhere: work it out from the payer's balance change.
- The wallet builds the declaration from `gas_limit` (`wallet config get/set gas_limit`, default 2,000,000, cap 10,000,000). It sets `max_fee = (gas_limit + 100000) × 64` and `tip = 0`, and chooses the first funded signer as payer.
- The current base fees come from the sequencer RPC `getFeeState`. The genesis base fee is 8 for both execution and storage, and it moves at most 12.5% per block (`lez/programs/fee/core/src/market.rs`).
- **Exempt:** private transactions (for now; [LEZ #941](https://github.com/logos-blockchain/logos-execution-zone/issues/941)), system injections (bridge deposits, cross-zone dispatches), cross-zone locks, and stake operations.
- **Not in v0.3.0 yet:** gas estimation or dry runs, setting `tip`/`max_fee` from the CLI, and fee receipts in the indexer or explorer.
- **Common errors:** `Incorrect fee` means the payer can't cover the reserve, `gas_limit` is over the cap, or `max_fee` is below the reserve. An account showing only `Balance` and `nonce` means you need `--scope all`.

### 6.3 Events (#507)

- **Emitting.** In `plan`, call `plan.event(ProgramEvent { selector, data })`, with `selector: [u8; 8]`. The runtime records the emitting program's account id from the call frame, so a program can't emit events as another program.
- **Selector convention.** Take the first 8 bytes of `sha256("<program>::<EventName>")`. Store it as a constant next to a `SELECTOR_NAME` string, with a unit test that checks the derivation. Nothing enforces this at runtime. The model is `lez/programs/bridge/core/src/event.rs` (`"bridge::Deposit"`), emitted in `lez/programs/bridge/src/main.rs`.
- **Events aren't stored in blocks.** The indexer re-derives them by re-executing each block (`TxEvents { tx_index, tx_hash, events }`).
- **Reading.** Use indexer JSON-RPC `getEvents` (filter by `tx_hash`, or `from_block`/`to_block`, plus optional `program_account_id` and hex `selector`) and `subscribeToEvents`. FFI equivalents: `query_events` on the indexer and `sequencer_ffi_query_events` on the sequencer.
- **Common problem.** The indexer's `event_filter` config controls what is kept. Shipped configs use `"archival"`. **With no filter configured, nothing is kept**, and every query fails as `UncoveredEventQuery`. Check this first when events "don't work". Events also need an indexer, which the local setup in [INTRO.md](INTRO.md) section 3 doesn't run (`just run-indexer`).

### 6.4 Private authorization key, `ask` (#508)

- **Key hierarchy.** Keys derive in this order: `ssk` → `ask` → `nsk` → `npk`. The viewing key `vsk` derives from `ssk` separately. Holding `nsk`/`vsk` lets someone track, decrypt and update a private account, but only `ask` makes the update **authorized**.
- **For program authors.** A private account's `is_authorized` is true only when a valid `ask` was supplied. **Without `ask` the account can still be spent and updated**: only programs that check `is_authorized` on their accounts refuse it. A transaction signer entry can't stand in for a missing `ask`. Any program that guards a private account's shard must check authorization itself, as `hello_world_with_authorization` does.
- Private PDAs use a different scheme: (authority program, seed) grants limited to the called program and the programs it calls in turn. `tail_call_with_pda` in [INTRO.md](INTRO.md) section 5 shows the public version.

### 6.5 Cross-zone messaging (#506)

- **Sending.** A program chains into `cross_zone_outbox::Emit`. That writes a write-once `OutboxRecord` into a PDA keyed by (emitter, target zone, ordinal), with the emitter taken from the caller.
- **Receiving.** The destination sequencer's watcher injects a dispatch. `cross_zone_inbox` deduplicates it, then chains into the target with a **source marker account at position 0**. That marker's address encodes (source zone, source program).
- **The target program decides which sources to accept.** Its config PDA lists them (`allowed_routes`, optionally with a lifetime `mint_cap`). The destination indexer independently re-derives every dispatch from the source chain, so a dishonest sequencer can only stop the chain, not forge messages.
- **Programs to read:** `ping_sender`/`ping_receiver` (the minimal pair) and `bridge_lock` → `wrapped_token` (value transfer with escrow).
- **Setup.** Cross-zone is off by default. A `cross_zone` section must be present in both the sequencer and indexer configs, and it can't be changed on an existing chain. The demos need Docker and Bedrock: `just demo-cross-zone-ping`, `just demo-cross-zone-bridge`, and `just cross-zone-chat` (web UI on :8088).

### 6.6 L1 bridge and funding (#510)

- The **only way** funds enter a zone is a Bedrock `DepositOp` on the zone's channel. The sequencer turns it into a mint through the `bridge` program, with a receipt PDA so each deposit is minted exactly once and a `bridge::Deposit` event. The bridge escrow starts at `u128::MAX` (the unissued supply) and can only increase.
- **Genesis funding uses the same path.** Each `supply_account` entry becomes a synthetic deposit (`/LEZ/v0.3/GenesisDeposit`). That's why [INTRO.md](INTRO.md) section 3 imports genesis keys rather than using a faucet.
- **Withdrawals are disabled in v0.3**: `Withdraw` panics. There is no first-party CLI for making a deposit.
- **For program authors:** user transactions can't call `bridge::Deposit` (they are dropped). Programs can't touch the escrow except to increase it.

### 6.7 Decentralised sequencer (#511)

This is an operator journey rather than a program one, but two facts matter for program authors:
- The public testnet's sequencer RPC is `https://testnet.lez.logos.co`.
- Committee members share mempool transactions over gossip. A transaction sent to a member no one can reach is included only on that member's turn.

It also shows another funding pattern, `wallet auth-transfer send` from a genesis account. No prebuilt binaries or Docker images are published for v0.3.

### What this means for the docs

- **#509 and #514 are prerequisites** for a "write and deploy an LEZ program" page at v0.3. The docs site's scaffold page and the LEZ example README ([INTRO.md](INTRO.md) section 5) both predate them.
- **A v0.3 programming guide needs these pages:**
  - the `plan`/`apply` model (#509)
  - fees and payers (#514)
  - authorization, covering both `is_authorized` for public signers and `ask` for private accounts (#508)
  - events (#507)
  - a hello world with a fee-paying call ([INTRO.md](INTRO.md) section 3, plus the runner fix)
- **Cross-zone (#506) and the bridge (#510)** fit as separate concept or tutorial pages.
- **#511** belongs under "run a node".

---

## 7. The LEZ model change, v0.2.x → v0.3.0, for program authors

**Before** means LEZ **v0.2.4**, the last stable release before Testnet v0.3. **After** means **v0.3.0**. The code below is from those tags in `ref-repos/logos-execution-zone`, trimmed (comments and lint attributes removed). LEZ v0.1.2 uses the same "before" model; only the names differ (`read_nssa_inputs`, `nssa_core`).

> **"Actor" in v0.3.0 refers to something else.** The release also rebuilt the sequencer internally as actors: the RpcServer, Executor and Storage actors (PRs 691, 736, 798). That is invisible to programs and clients, which still see the same RPC on `:3040`. Everything below is the program and account model, which *is* visible to programs.

### 7.1 What changed underneath

| | v0.2.4: account-rewriting model | v0.3.0: plan/apply on shards |
|---|---|---|
| **Account** | `{ program_owner, balance, data, nonce }`: one data blob, one owner | `{ nonce, shards: BTreeMap<AccountId, ShardData> }`: one data slot (shard) **per program** |
| **Ownership** | A program *claims* a default account (`Claim::Authorized`, `Claim::Pda(seed)`), then owns all of its data | There is no claiming. Every program owns **its own shard on every account**, and only that shard |
| **Program entry** | `read_lee_inputs()` → full `pre_states` (whole accounts) → program returns full `post_states` | `run_program(plan, apply)`. `plan` sees account **handles without data**. `apply` sees **one shard** |
| **Who checks correctness** | The engine compares pre and post against rules: nonce unchanged, owner unchanged, balance decreases only by the owner, data changes only by the owner or on a default account, total balance conserved | The engine builds every input itself and rejects mismatched echoes, effects outside declared accounts, and writes to another program's shard. Most of those rules can no longer be broken, so they don't need checking |
| **Native balance** | A field every program could change, within the conservation and ownership rules | The native-token shard (program id all zeros). Only protocol code writes it. Programs move it with a chained `custody_transfer` |
| **Calling another program** | `ChainedCall { program_id, instruction_data, pre_states, pda_seeds }`: the caller **passes account values** | `ChainedCall { program_account_id, shard_selectors, instruction_data, pda_seeds }`: the caller names (account, shard) pairs, and the engine supplies the live values and `is_authorized` |
| **Program identity** | `ProgramId` = RISC Zero image ID | `program_account_id`, the address of a `program_loader` header account. Builtins use name-derived addresses |
| **Deploy** | `wallet deploy-program <bin>`, free | `program_loader` header + segment accounts, an ordinary transaction that is **charged** with a payer |
| **Instruction encoding** | risc0 serde (`risc0_zkvm::serde::to_vec`) | borsh (PR 770) |
| **Transactions** | No fees | Every user public tx carries a `FeeDeclaration`. Reverts are included and charged (#514) |
| **Outputs besides state** | None | `plan.event(..)` (#507), plus block and timestamp validity windows |
| **Funding** | Genesis balances plus a faucet program (`pinata`) | No faucet. Bridge deposits only, and genesis is synthetic deposits (#510) |

**The rule to remember:** in v0.3 a program **cannot read state to decide what to do**. `plan` decides using only the instruction and account metadata, so anything it would have read now has to arrive as a **claim in the instruction**. `apply`, which does see the data, then **asserts** the claim against the real shard. If the claim is false, `apply` panics and the whole transaction reverts. Most of the scenarios below are this pattern.

### 7.2 Scenarios that changed

#### A. Write to an account (hello world)

The v0.2.4 version clones the whole account, edits `data`, and claims the account if it's new. The v0.3.0 version declares one effect, and `apply` changes only hello_world's shard. The account's balance and other programs' data are out of reach.

```rust
// v0.2.4
let (ProgramInput { self_program_id, caller_program_id, pre_states, instruction: greeting }, instruction_data)
    = read_lee_inputs::<Vec<u8>>();
let [pre_state] = pre_states.try_into().unwrap();
let mut post_account = pre_state.account.clone();
let mut bytes = post_account.data.into_inner();
bytes.extend_from_slice(&greeting);
post_account.data = bytes.try_into().unwrap();
let post_state = AccountPostState::new_claimed_if_default(post_account, Claim::Authorized);
ProgramOutput::new(self_program_id, caller_program_id, instruction_data, vec![pre_state], vec![post_state]).write();
```

```rust
// v0.3.0
fn main() { run_program(plan, apply) }
fn plan(input: &PlanInput, instruction: Vec<u8>) -> Plan {
    let mut plan = Plan::new(input);
    let [account] = input.accounts.as_slice() else { panic!() };
    plan.effect(account, &instruction);
    plan
}
fn apply(greeting: Vec<u8>, pre_data: &ShardData) -> Option<Vec<u8>> {
    let mut bytes = pre_data.clone().into_inner();
    bytes.extend(greeting);
    Some(bytes)
}
```

Effect on the runner: the v0.2.4 hello_world **claimed** the account, so the first call needed the account's signature. The v0.3.0 hello_world writes its own shard with no authorization at all, so its runner sends no signature. Since v0.3.0 needs a fee, though, a payer must still sign ([INTRO.md](INTRO.md) section 3).

#### B. Require the account holder's authorization

This barely changed: `is_authorized` moved from `pre_state` to the `AccountMeta` handle, and the check now runs in `plan`.

```rust
// v0.2.4                                         // v0.3.0 (in plan)
assert!(pre_state.is_authorized, "…");            assert!(account.is_authorized, "…");
```

What changed is *where the bit comes from*. In v0.2.4 a caller passed `pre_states` into a chained call. In v0.3.0 the engine computes `is_authorized` itself for every callee, so a caller can't pass an authorization along unless it holds the PDA seed. For private accounts, v0.3.0 sets it only when the `ask` was supplied (#508).

#### C. Read one account and act on another (move data)

The v0.2.4 version **reads** the source data and writes it to the destination. The v0.3.0 `plan` can't read it, so **the instruction carries the bytes**. One effect empties the source, and its `apply` asserts the bytes were really there. The other effect appends them to the destination.

```rust
// v0.2.4
fn move_data(from_pre: AccountWithMetadata, to_pre: AccountWithMetadata) -> Vec<AccountPostState> {
    let from_data: Vec<u8> = from_pre.account.data.clone().into();   // read
    let from_post = { let mut a = from_pre.account; a.data = Data::default(); /* claim */ };
    let to_post   = { let mut a = to_pre.account;  /* append from_data */ };
    vec![from_post, to_post]
}
```

```rust
// v0.3.0
type Instruction = (u8, Vec<u8>);                  // (function id, the bytes being moved)
enum Effect { Append(Vec<u8>), MoveOut(Vec<u8>) }
fn plan(input: &PlanInput, (function_id, data): Instruction) -> Plan {
    let mut plan = Plan::new(input);
    match (input.accounts.as_slice(), function_id) {
        ([from, to], MOVE_DATA_FUNCTION_ID) => {
            plan.effect(from, &Effect::MoveOut(data.clone()));
            plan.effect(to, &Effect::Append(data));
        }
        /* … */
    }
    plan
}
fn apply(effect: Effect, pre_data: &[u8]) -> Option<Vec<u8>> {
    Some(match effect {
        Effect::Append(data) => [pre_data, &data].concat(),
        Effect::MoveOut(data) => { assert_eq!(pre_data, data, "source does not hold these bytes"); Vec::new() }
    })
}
```

So the **client** must now read the source off-chain and include it in the instruction. That makes transactions bigger, and storage gas is charged per byte (#514).

#### D. Token transfer between two holdings

The v0.2.4 `transfer` reads both holdings, checks that their definition IDs match, and returns two post-states. The v0.3.0 version declares two effects, `Withdraw` and `Deposit`. Each `apply` sees only its own holding. `plan` can't see the definition, so the instruction carries a `TokenDescriptor { definition_id, kind }`, and each `apply` asserts its holding matches it.

```rust
// v0.2.4 (lez/programs/token/src/transfer.rs)
pub fn transfer(sender: AccountWithMetadata, recipient: AccountWithMetadata, amount: u128) -> Vec<AccountPostState> {
    assert!(sender.is_authorized);
    let mut s = TokenHolding::try_from(&sender.account.data).unwrap();
    let mut r = if recipient.account == Account::default() { TokenHolding::zeroized_clone_from(&s) }
                else { TokenHolding::try_from(&recipient.account.data).unwrap() };
    assert_eq!(s.definition_id(), r.definition_id());
    /* s.balance -= amount; r.balance += amount; */
}
```

```rust
// v0.3.0
pub fn transfer(plan: &mut Plan, sender: &AccountMeta, recipient: &AccountMeta,
                descriptor: TokenDescriptor, amount: u128) {
    assert!(sender.is_authorized, "Sender authorization is missing");
    plan.effect(sender,    &Effect::Withdraw { descriptor, amount });
    plan.effect(recipient, &Effect::Deposit  { descriptor, amount });
}
pub fn deposit(pre_data: &ShardData, descriptor: &TokenDescriptor, amount: u128) -> ShardData {
    let mut h = if pre_data.is_empty() { descriptor.zeroized() } else { TokenHolding::try_from(pre_data).unwrap() };
    assert_kind(&h, descriptor);                    // the claim is checked here
    /* h.balance += amount */
    ShardData::from(&h)
}
```

Two further effects: the sender's data never reaches the recipient's `apply`, and a "new recipient" is now just an **empty shard**, not a default account.

#### E. Move native balance

The v0.2.4 program changed `balance` directly. The engine accepted that only if the program owned the debited account and the total was conserved. In v0.3.0 no program can write the native-balance shard. Instead the program **chains a call** to the native-token program, holding the source account under a PDA seed.

```rust
// v0.2.4 (lez/programs/bridge_lock)
let mut holder_account = holder.account.clone();
holder_account.balance = holder.account.balance.checked_sub(amount).expect("insufficient balance");
let mut escrow_account = escrow.account.clone();
escrow_account.balance = escrow.account.balance.checked_add(amount).unwrap();
// …returned as post-states; conservation checked by the engine
```

```rust
// v0.3.0 (lez/programs/bridge_lock)
plan.call(custody_transfer(           // lee_core::native_token
    holding.account_id,
    holding_seed(&holder.account_id.into_value()),   // this program's PDA seed for the source
    escrow.account_id,
    amount,
));
```

As a result, a program can move native funds only out of **its own PDAs** (custody). Moving a user's funds takes the user's signature on the native-token transfer itself.

#### F. Create or initialise an account

| v0.2.4 | v0.3.0 |
|---|---|
| A new account is `Account::default()`. The program claims it with `AccountPostState::new_claimed(…, Claim::Authorized)` (needs the signature) or `Claim::Pda(seed)`, and then **owns** it | No claim step. "Uninitialised" means **this program's shard is empty** (`pre_data.is_empty()`). Writing it is just an effect, and other programs can still use the same account for their own shards |
| `init` checks compared the whole account against default | Check the shard: return `Some(data)` only if it's empty, otherwise panic. This is how the bridge receipt gives replay protection: "Deposit was already processed" (#510) |

SPEL's `#[account(init)]` is built on the v0.2-style claim. Its v0.3 behaviour needs checking against a SPEL release that targets LEZ v0.3.

#### G. Call another program

The v0.2.4 caller passes **whole account values** and addresses the callee by a hard-coded image ID. The v0.3.0 caller passes (account, shard) **selectors**, and the callee is a `program_account_id`. The example now receives it in the instruction instead of hard-coding it.

```rust
// v0.2.4 (simple_tail_call)
const HELLO_WORLD_PROGRAM_ID_HEX: &str = "e9dfc5a5…";
let chained_call = ChainedCall {
    program_id: hello_world_program_id(),
    instruction_data: risc0_zkvm::serde::to_vec(&greeting).unwrap(),
    pre_states,                                     // the caller hands over account values
    pda_seeds: vec![],
};
ProgramOutput::new(/* … */ vec![pre_state], vec![post_state]).with_chained_calls(vec![chained_call]).write();
```

```rust
// v0.3.0 (simple_tail_call); a plan-only program, it has no apply
let ProgramCall::Plan(input, callee_account_id) = read_program_call::<AccountId>() else { panic!() };
let [account] = <[_; 1]>::try_from(input.accounts.clone()).unwrap();
let mut plan = Plan::new(&input);
plan.call(ChainedCall::new(
    callee_account_id,
    vec![ProgramShardSelector::new(account.account_id, callee_account_id)],   // the callee's shard on that account
    &greeting,
));
plan.write()
```

The **v0.2.4 tutorial warning** ("if it fails, check the hard-coded Hello World ID matches your build") no longer applies, because the callee's address now arrives in the instruction. Image IDs also differ between Docker and local builds (`lez/programs/README.md`). Call depth is capped at 10, and the callee runs after the caller's `plan` (depth-first).

#### H. Depend on state owned by another program

In v0.2.4 a program could read any account it was given, including data owned by another program, and branch on it. In v0.3.0 it can't read that data, so it **makes a claim** about it and asks the **owning** program to verify it with `plan.inspect(account, owner_program, &guard)`. The owner's `apply` checks the guard against its own shard.

```rust
// v0.3.0 (lez/programs/associated_token_account/src/create.rs)
// The caller says what the ATA holds (`contents`); the token program verifies it.
plan.inspect(ata_account, token_program_id,
    &Effect::AtaContents { descriptor: TokenDescriptor { definition_id, kind }, contents });
match contents {
    AtaContents::Empty     => { /* chain InitializeAccount */ }
    AtaContents::Intended  => plan.inspect(token_definition, token_program_id, &Effect::DefinitionKind { kind }),
    AtaContents::Squatted  => assert!(owner.is_authorized),
}
```

The comment in that file explains why: taking `contents` "on the caller's word is a hole, not a shortcut". **Every claim that reaches `plan` through the instruction must be checked by some `apply` or `inspect`.** This is the most important new rule for reviewers.

#### I. Emit something for off-chain consumers

There's no v0.2.4 equivalent: observers compared account state. In v0.3.0, `plan.event(ProgramEvent { selector, data })` emits an event. The runtime records the emitting program, and the indexer re-derives and serves events (#507).

#### J. Deploy and submit

| | v0.2.4 | v0.3.0 |
|---|---|---|
| Deploy | `wallet deploy-program hello_world.bin` | `program_loader` writes header + segment accounts. In code: `deploy_program(wallet, bytecode, payer)` (`examples/program_deployment/src/lib.rs`). On the CLI: `wallet program-loader …` |
| Address used to call it | Image ID (`ProgramId`) | The header's `AccountId` (`program_account_id`) |
| Cost | Free | Storage gas, paid by a funded signing payer |
| A call | `Message::try_new(program_id, account_ids, nonces, instr)`, free | Must carry a `FeeDeclaration`. Use `Message::try_new_with_fees` or the wallet's `send_pub_tx_paid_by`. `Message::try_new` is now for system transactions only |
| A failed call | Rejected; nothing lands | Included and **charged**; only the nonce advances |
| A first-time payer | Fund it from the faucet | Transfer to it from a funded account; it needs the full reserve |

### 7.3 Porting checklist (v0.2 program → v0.3)

1. Divide the program into `plan`, which picks accounts and effects using only the instruction, and `apply`, which turns one shard plus one effect into the new shard. Define an `Effect` enum for each kind of shard change.
2. For every value the old code **read** before deciding: put it in the instruction as a claim, and check it in the relevant `apply`, or in the owning program with `plan.inspect`.
3. Replace "claim a default account" with "is my shard empty?". Remove `Claim::*` and `program_owner` handling.
4. Replace direct `balance` changes with a chained `custody_transfer` from a PDA this program controls. Change the design if it debited users directly.
5. Replace chained calls that pass `pre_states` with `ProgramShardSelector`s. Address callees by `program_account_id`, not image ID.
6. Move authorization checks into `plan`. For private accounts, assume `is_authorized` is true only when the `ask` was supplied.
7. Switch instruction (de)serialisation to borsh.
8. Clients: read state off-chain to build those claims. Attach a fee and payer. Treat "included" as different from "succeeded".
9. Tests: the engine suites named in 6.1 serve as the spec. A program that reverts in a test may be failing a claim check, not running out of gas.

---

## 8. Logos LIPs on the program model

The source is [`logos-co/logos-lips`](https://github.com/logos-co/logos-lips) at `fddc2c1` (2026-10-06), shallow-cloned into `ref-repos/logos-lips`. All the LIPs below have **raw** status. One LIP specifies the LEZ program model (LIP 237). A few others relate to it.

| LIP | Title (path under `docs/`) | What it covers | Relevance |
|---|---|---|---|
| **237** | [LEE v0.3 Specifications](https://github.com/logos-co/logos-lips/blob/master/docs/blockchain/raw/lez/lee-v0.3-specifications.md) (`blockchain/raw/lez/lee-v0.3-specifications.md`) | The whole LEE state machine: account schema, account IDs and PDAs, commitments and nullifiers, private-account encryption, **program interface and the rules checked on program outputs**, claiming, validity windows, public and private transaction formats, the privacy circuit, acceptance criteria, state transitions | **The** program-model spec, but **stale**; see 8.1 |
| 237 (appendix) | [Built-in Programs](https://github.com/logos-co/logos-lips/blob/master/docs/blockchain/raw/lez/lee-v0.3-specifications/appendices/builtin-programs.md) | Authenticated transfer, Piñata, Token, AMM, ATA, Clock, Vault, Faucet | Stale. Compare with the v0.3.0 list in [INTRO.md](INTRO.md) section 5 |
| 98 | [Mantle](https://github.com/logos-co/logos-lips/blob/master/docs/blockchain/raw/bedrock-v1.1-mantle-specification.md): `CHANNEL_INSCRIBE`, `CHANNEL_DEPOSIT`, `CHANNEL_WITHDRAW` | The Bedrock (L1) operations a zone sequencer uses: posting blocks, and deposits with a `metadata` field | The L1 half of the bridge (#510): a zone deposit is a `ChannelDeposit` whose `metadata` carries the LEZ recipient |
| 201 | [Execution Market](https://github.com/logos-co/logos-lips/blob/master/docs/blockchain/raw/execution-market.md) (+ 190 analysis, 191 gas-cost analysis) | **Bedrock** L1 fee market: an EIP-1559-style base fee with EMA updates and a reward pool | Background only. It never mentions zones. The LEZ fee market (#514) follows the same target-based idea, but is specified only in code (`lez/programs/fee/core`) |
| 226 (deprecated) | [Template: cross-channel messaging](https://github.com/logos-co/logos-lips/blob/master/docs/blockchain/deprecated/v1.1.0-template-cross-channel-messaging.md) | Earlier designs for channels and zones messaging each other | Historical background for cross-zone (#506). The shipped design (outbox → watcher → inbox, verified by the indexer) has no LIP |
| 244 | [LON Oracle Zone](https://github.com/logos-co/logos-lips/blob/master/docs/anoncomms/raw/logos-oracle-zone.md) (`anoncomms/raw/logos-oracle-zone.md`) | A price oracle whose membership tree and accepted prices live in a "LEZ contract" | The one LIP that **designs an application as an LEZ program**. See 8.3 |

### 8.1 LIP 237 describes the v0.2-style model, not v0.3.0

LIP 237 says it was taken from LEZ branch `schouhy/add-specs`, `docs/specs.md`. That branch was last committed on **2026-06-01**. The LIP was moved into `logos-lips` on 2026-06-08 (PR 353), and its only change since is a link fix on 2026-06-12. LEZ **v0.3.0 shipped on 2026-09-29**. The "v0.3" in the title matches the `/LEE/v0.3/…` hash prefixes, which v0.3.0 still uses, but **the program model it specifies is the one section 7 calls "before"**:

| Topic | LIP 237 says | LEZ v0.3.0 code does | Where it changed |
|---|---|---|---|
| Account | `{ program_owner, balance, data, nonce }` | `{ nonce, shards }`; balance is the native-token shard | PRs 886, 723; #509 |
| Program interface | `fn(ProgramId, Option<ProgramId>, List<AccountWithMetadata>, InstructionData) -> ProgramOutput` with whole-account `pre_states`/`post_states` | `run_program(plan, apply)`, `PlanInput`/`Plan`, `ApplyInput`/`ApplyOutput` | PR 912; #509 |
| Validation | 8 rules on pre/post states (nonce, owner, balance decrease, data change, conservation, …) | Engine-computed inputs that outputs must echo; `EffectOutsideInputs`, `ForeignShardWrite` | #509; §7.1 |
| Ownership | "Account claiming mechanism": `Claim::Authorized` / `Claim::Pda(seed)` sets `program_owner` | No claims; a program owns its shard on every account | §7.2 F |
| Chained call | `ChainedCall { program_id, pre_states, instruction_data, pda_seeds }` | `{ program_account_id, shard_selectors, instruction_data, pda_seeds }` | PRs 790, 735 |
| Program identity / PDAs | `ProgramId = [u32; 8]` (image ID); public PDA = f(`program_id`, seed) | Programs addressed by `AccountId`; `for_public_pda(account_id: &AccountId, seed)` | PRs 722, 720 |
| Instruction data | `List<u32>` words (risc0 serde) | borsh bytes | PR 770 |
| Public tx `Message` | `{ program_id, account_ids, nonces, instruction_data }` | `{ program_account_id, shard_selectors, nonces, instruction_data, fee: Option<FeeDeclaration> }` | PRs 801, 751; #514 |
| Private authorization | "knowledge proofs of the corresponding nullifier secret key" | The separate `ask` key; without it the account can still be updated, but is unauthorized | PR 669; #508 |
| Deployment | `ProgramDeploymentTransaction { bytecode }`; ID from image | `program_loader` header + segments in an ordinary charged public tx | `program_loader` |
| Not covered at all | — | Fees, events, `plan.inspect`, cross-zone, bridge deposits, sequencer staking | #506, #507, #510, #511, #514 |
| Built-ins (appendix) | Authenticated transfer, **Piñata**, Token, AMM, ATA, Clock, **Vault**, **Faucet** | Piñata removed (PR 817), Faucet removed (PR 875), Vaults deleted (#510). Added: `fee`, `bridge`, `bridge_lock`, `wrapped_token`, `cross_zone_*`, `ping_*`, `sequencer_stake`, `program_loader` | [INTRO.md](INTRO.md) §5 |

**Probably still useful, but check against the code first:** the parts that don't depend on the program interface. These are account-ID derivation, commitments and nullifiers, private-account kinds and encryption, view tags and discovery, and the domain-separator table. Even so, PDA derivation already differs (it now uses the program's `AccountId`), and the key hierarchy gained `ask` (#508). Treat LIP 237 as a **history** of the v0.2-style model, not a spec for writing v0.3 programs.

### 8.2 What this means for the docs

- No LIP describes the model that v0.3 programs are written against. The best sources are, in order: the v0.3.0 code (`lee/state_machine/core/src/program/mod.rs` and the engine tests named in 6.1), the [Journey] issues #509 and #514, and section 7 of this file.
- A docs page should **not link LIP 237 as "the LEZ program spec"** without a note saying it describes the pre-v0.3 interface. It would fit better as background on accounts, privacy and nullifiers.
- **Worth raising upstream:** LIP 237 needs a revision for the plan/apply, shard, fee, `ask` and `program_loader` changes, or a new LIP. The built-ins appendix needs the v0.3.0 list. Raise this with the LIP's editors and the LEZ team (the authors of #506–#514).

### 8.3 Worked example: LIP 244's "LEZ contract" under v0.3

LIP 244 ("Write path, checks in order on LEZ Contract") describes a contract that reads its own state, checks it, and then updates it:
1. Roll the cycle over if `round` has passed `cycleStartRound + AOS_cycle`.
2. Check the sender's membership proof against `root_cycle`.
3. Require `round > lastAcceptedRound`.
4. Check the sender is one of the three proposers for this round.
5. Store `AttestedPrice` and set `lastAcceptedRound`.

That reads naturally as a v0.2-style program. Under v0.3 it maps as follows:
- **Keep all the oracle state in this program's own shard on one state account** (for example a PDA). Then every check that needs that state runs in a single `apply`, which sees exactly that shard. `plan` stays trivial: one `plan.effect(state_account, &Submit { round, price, observations, proofs })`.
- **Checks that need no state** (proof format, observation count) can fail early in `plan`.
- **If the state were split across accounts**, `plan` couldn't read `lastAcceptedRound` to decide anything. The design would need the claim-and-verify pattern (§7.2 C/H): the submitter states the values, and each `apply` checks them.
- **"Fail and revert" now costs the submitter.** A reverted submission is included and charged (#514). The LIP's incentive analysis (§"Incentivization") should account for proposers paying fees for submissions that lose a race or arrive late.
- **Posting an accepted price can also emit an event** (`plan.event`, #507), so indexers and consumers can follow prices without diffing the shard.
