# The LEZ "transactional actors" model (planned for v0.4)

Researched 2026-10-09. **Public sources only** (GitHub).

Status in one line: the work is real, in progress, and large, but it is a **draft PR** with an empty description and no merge date. Treat every detail below as subject to change until it merges.

## 1. Where it lives

| Item | Detail |
|---|---|
| PR | [logos-blockchain/logos-execution-zone#935](https://github.com/logos-blockchain/logos-execution-zone/pull/935), "feat!: transactional actors with side-effects" |
| Branch | `artem/transactional-actor-environment`, base `dev` |
| State | **Draft**, open, review required, no reviewers requested, no labels or milestone. Opened 2026-09-30, last updated 2026-10-08 |
| Size | 360 files, +31,860 / −24,675 lines |
| PR body | Unfilled template ("TO COMPLETE"). The intent has to be read from the commits |
| Follow-up issues | [#956](https://github.com/logos-blockchain/logos-execution-zone/issues/956) (encode private `from` address decodings for shielded async replies), [#957](https://github.com/logos-blockchain/logos-execution-zone/issues/957) (optimise a second cipher for messages whose recipient is in the same tx; motivated by an AMM swap where the user casts to itself) |
| Spec PR | [#932](https://github.com/logos-blockchain/logos-execution-zone/pull/932), "doc(lee): updating LEE/LEZ specs" (see section 2a) |
| Not this | #691, #736, #798, #833, #885 and the `arjentix/actors-phase-1`, `erhant/gossip-as-actor`, `schouhy/slasher-as-actor` branches are the **sequencer/node** actor refactor that shipped in v0.3.0. It is invisible to program authors |

Related branches by the same author, which show the direction of travel: `artem/continuations-interface`, `artem/top-call-semantics`, `artem/predicate-effects`, `artem/bundle-actions`, `artem/lee-environment-unification`.

## 2. What the model is (from the code on the branch)

v0.3.0 is plan/apply over per-program shards. The branch replaces it with **programs as actors that exchange messages**.

| v0.3.0 | #935 branch |
|---|---|
| `ProgramShardSelector { account_id, program_account_id }` | `account::Actor` (renamed, commit 83f8d9b90): the `(account, program)` pair, "the unit of state one handler owns" |
| `ShardData` (`account/data.rs`) | `ActorState` (`account/actor_state.rs`); `data.rs` is deleted |
| `run_program(plan, apply)`; `plan` sees account handles, `apply` sees one shard | `define_actor!(handler)` → `run_actor`; one handler per actor: `handle_message(&ReceiveInput, M) -> Response` |
| `Plan` builder: `effect`, `inspect`, `call`, `event`, window setters | `Response { post_state, calls, casts, events, block_validity_window, timestamp_validity_window }` with `Response::keep_state()`, `set_state()`, `.call()`, `.cast()`, `.event()`, `.block_window()`, `.timestamp_window()` |
| `ChainedCall { program_account_id, shard_selectors, instruction_data, pda_seeds }`, depth cap 10 | `Call { to: Actor, message, pda_seeds }`, delivered depth-first in the same transaction |
| (nothing) | `Cast { to: Actor, message }`: **asynchronous**, "publishes a message for delivery in a later transaction" |
| Instruction to a set of accounts | `MessageEnvelope { from, to, message }` between single actors |
| `plan.inspect(account, owner, &guard)` to check another program's state | Gone from `program/mod.rs`. A handler "reads only its own shard"; no replacement seen in `core`. A `StateReply { state }` type exists; its intended use is not documented |
| Handler input: `PlanInput` metadata, then one shard | `ReceiveInput { receiver, from: Option<Actor>, is_authorized, pre_state, message }`. Handlers see the sending program as their **origin** |
| Native balance in the native-token shard, moved by chained `custody_transfer` | "native balances are native-token actors" (commit 396a2adce) |
| Public tx names a program and accounts | Public tx "names a root actor, a message and their public actors"; `ProgramWithDependencies` becomes `ProgramCatalog` |

Execution: one driver, four modes, per commit 396a2adce: **Live** (public transactions), **Derive** and **Record** (prover), **Check** (settlement).

Privacy additions in `lee_core`: `sealing.rs` (`Seal`, `SealedCast`, `seal_casts`), `recovery.rs` (`MessageNote`, `Recipient`, `RecoveryBinding`), `AccountId::blinded(factor)` (an address that hides which account it names), `Publication { Clear | Sealed }`, `Nullifier::for_message`. A pending message is consumed by a transaction entry, and the commit log says a proof claims at most one consumed message.

Compatibility statements in the commits: "stored program state keeps its layout"; program `.bin` artifacts change; genesis, devnet and stored sequencer/indexer state **must be reset**.

## 2a. The v0.3 spec PR (#932): read in full, no actor content

Read in full by an agent on 2026-10-09 (spec files at head `fd1635a`).

- **State:** draft, open, 0 reviews; review requested from three LEZ developers; branch is 7 ahead and 54 behind `dev`; CI green except the `artifacts` check, which fails because the circuit ELF changes and needs `just build-artifacts` (per the PR's own commit message). Two of the seven commits are co-authored by an AI model, so treat the text as LLM-assisted (the commits say it was revised "against the code").
- **Contents (all new files, no older in-repo version to diff against):** `lee-specs.md` (1555 lines: types, accounts and shards, key/ID derivations, plan/apply programs, native token, validity windows, deployment/loader, tx formats, privacy circuit, fees, acceptance pseudocode), `lez-specs.md` (1110 lines: components, encodings, blocks, fee market, genesis, sequencer/committee/slashing, cross-zone, indexer, events, RPC) and `docs/builtin_programs.md` (224 lines: clock, fee, bridge, sequencer_stake, cross_zone_*, bridge_lock, wrapped_token, ping_*). It also touches 22 Rust files (small refactors: `Commitment::new` and `Nullifier::for_account_initialization` take `AccountId` by value; one added loader test).
- **Actors:** no hit for "actor", Call/Cast, mailbox, pending message, `MessageEnvelope`, `ActorState`, or "v0.4" in any of the three files. The only mention is in the PR body: "Specs likely to be require updates to include future works for actors, privacy transaction fees, etc."
- **Accuracy vs v0.3.0 code (spot-checked against tag `db66590`):** matches `AccountMeta`, `PlanInput/Output`, `ApplyInput/Output`, `ChainedCall` with `shard_selectors`, the plan/apply validation rules, `MAX_NUMBER_CHAINED_CALLS = 10`, `MAX_PROGRAM_SEGMENTS = 20`, `native_token` operations, `circuit_io` witness types, `FeeDeclaration` and the fee-market constants. Divergences: it shows by-value `Commitment::new(AccountId)` where the tag takes `&AccountId` (the by-value form is this PR's own refactor); `lee-specs.md` line 784 has a garbled sentence about invalid encodings; `EphemeralPublicKey` is shown as `[u8; 1088]` where code uses a `Vec<u8>`; the key-protocol (BIP-32 derivation) is referenced, not specified; the built-ins list omits amm, token and associated_token_account (not checked why).
- **Usefulness:** the only complete written description of the shipped v0.3 model, and largely accurate, but an unreviewed draft. Best sections for v0.3 program authors: "Programs" (plan/apply, validation, traversal, authorization, PDA binding), native token, deployment/loader, ID and PDA derivations, circuit and tx formats, `builtin_programs.md`.
- **If #935 merges, invalidated or heavily rewritten:** the Accounts/Shards sections and all of "Programs"; the `shard_selectors` field in the public `Message`; the private-tx deferred-effect settlement; circuit input/output (`ProvenCall`, `RootCall`); shard-map encodings in commitments and ciphertexts; every built-in program's account and instruction list; the RPC shapes that carry shards. **Likely still valid:** key and ID derivations, nullifiers, ML-KEM/ChaCha20 encryption, commitment tree, validity windows, fee-market math, block structure, sequencer/indexer/cross-zone flows, domain separators.
- **Versus LIP 237** (`logos-lips` `docs/blockchain/raw/lez/lee-v0.3-specifications.md`, last touched 2026-06-12; header and first 140 lines read, the rest not): its source is the LEZ branch `schouhy/add-specs`, and it describes the v0.2-style model. #932 supersedes it for v0.3. Visible differences:

  | | LIP 237 | #932 (v0.3.0 code) |
  |---|---|---|
  | Account | `{program_owner, balance, data, nonce}` | `{nonce, shards}` |
  | Instruction data | `List<u32>` | Borsh bytes |
  | Private identifier | `Identifier = u128` | `[u8; 32]` (and removed again on the #935 branch) |
  | Note encryption | secp256k1 ECDH, 33-byte ephemeral/viewing keys | ML-KEM-768 (1088-byte ciphertext, 1184-byte viewing key) |
  | Keys | no `ask` | `ask` → `nsk` → `Npk` |

  So LIP 237 is two models behind the #935 branch (v0.2-style → v0.3 → actors), and #932 is one behind.

## 3. What breaks for program authors (if #935 merges as is)

- Every guest program must be rewritten from `plan`/`apply` to a `define_actor!` message handler. The branch ports `token`, `amm`, `associated_token_account`, `clock`, `fee`, `sequencer_stake`, `bridge`, `bridge_lock`, `wrapped_token`, `cross_zone_*`, `ping_*` and the `examples/program_deployment` guests, which is itself a useful porting reference.
- Chained calls (caller-chosen accounts) are replaced by messages to actors. Reading another program's state through `plan.inspect` has no equivalent in `core`.
- Existing deployed program binaries and chain state do not carry over: reset required.
- Clients: wallet send signatures change, the indexer JSON and both C FFI headers (`indexer_ffi.h`, `sequencer_ffi.h`, `wallet_ffi.h`) change (`FfiActor`, `FfiOrigin`, `FfiBoundary` and others). Wallet gains a `pending` command and `pending.rs` FFI.
- Large new test files show the intended semantics: `lee/state_machine/src/state/tests/pending_messages.rs`, `sealed_messages.rs`, `publications.rs`, `recovery_bindings.rs`, `integration_tests/tests/amm_exact_input_casts.rs`, `token_sealed_casts.rs`.

See `CORE_API_STABILITY.md` for the per-module view of `lee_core`.

## 4. Stated benefits (from commits and issues)

The PR has no description, so the only stated reasons are in commit "Before/After" text and the two issues:

- Async `Cast` lets a transfer's credit be delivered later, so the recipient is left out of the sending transaction (f4a129b08).
- Fixed-offer swaps can be "prepared at any price" via a promised payout (3925f713b).
- Messages to actors replace chained calls that "could reach any program with caller-chosen accounts", with origin-based authentication (786e1c0b1).
- Stake slashing works without the owner's witness (d9596262d).
- A private receipt is spent as an ordinary private action (a5455108b).
- Issue #957: AMM use by a shielded user.

**Not stated anywhere public:** performance, throughput, fees, proving cost, or a migration guide. The effect on the private-transaction contention limit is analysed in `PRIVATE_TX_CONTENTION.md` (an inference from tests, not a stated claim).

## 5. Open items to track

- Does #935 merge as one PR or get split? It has no reviewers yet.
- PR description, a design doc or an ADR: none found. The PR's changed files contain no doc other than `examples/program_deployment/README.md`, and no CHANGELOG or v0.4 milestone turned up. The repo's `docs/` directory was not read exhaustively.
- Whether `plan.inspect`-style cross-state guards come back in another form.
- Whether `identifier`s on private addresses are removed (the `Identifier` and `IdentifierError` exports are already gone from `lee_core/src/lib.rs` on the branch).
- Issues #956 and #957 (private reply addressing, cipher optimisation).
- Whether the SPEL framework targets the actor model (not checked; see `CORE_API_STABILITY.md`).

## 6. Sources

- PR #935 and its file list: `gh api repos/logos-blockchain/logos-execution-zone/pulls/935/files`
- `lee/state_machine/core/src/lib.rs`, `program/mod.rs`, `account.rs` on `artem/transactional-actor-environment`, compared with `v0.3.0`
- Commits 83f8d9b90, 396a2adce, 94ad28fb1, d95675479, 26e671fe5, 786e1c0b1, 31acd695a, edd2be9b1, 3925f713b, 39b0c48f8, 6246aac0d, f4a129b08, d9596262d, a5455108b
- Issues #956, #957; PR #932
- `logos-co/roadmap` (branch `v5`): `content/blockchain/roadmap/lez_state_contention.md`, `lez_unify_execution.md`. No "actor model" or "v0.4" item exists for LEZ there (the v0.4 headings in that repo belong to logoscore, messaging and anoncomms)
