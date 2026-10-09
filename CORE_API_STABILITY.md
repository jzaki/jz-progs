# Will the LEZ core API change? (v0.3.0 → v0.4)

Researched 2026-10-09. Public sources only.

**Short answer: yes, almost all of it, if PR #935 merges.** The parts of `lee_core` that program authors touch (`program`, the circuit I/O types, `native_token`, `nullifier`, the public `Message`) are rewritten on the branch. The core account/ID types and the PDA derivation are the most stable parts. For the **LEZ Core Module** (the Logos Core wrapper, repo `logos-blockchain/logos-execution-zone-module`), about 6 of its 34 methods need code changes (4 visible to callers: the two `get_account_*` JSON outputs and the two `send_generic_*` calls), plus new methods to receive pending private credits; the rest looks unchanged (section 4).

## What "lez_core" means here

The name is ambiguous, so this file covers both readings. Say which one you meant if it matters.

1. **`lee_core`, the Rust crate** at `lee/state_machine/core` (package version `0.1.0`). This is what guest programs and tooling import (`lee_core::program::...`, `lee_core::account::...`). Sections 1–3 are about this.
2. **The "LEZ Core Module"**: a Logos Core (C++) module, module name `lez_core`, class `LEZCoreModule`, that is "the shared backend for all LEZ apps" (roadmap item [`lez_core_module.md`](https://github.com/logos-co/roadmap/blob/v5/content/blockchain/roadmap/lez_core_module.md)). **Repo: [logos-blockchain/logos-execution-zone-module](https://github.com/logos-blockchain/logos-execution-zone-module)** (API in `src/lez_core_module.h`, 34 methods). It is a thin wrapper over the LEZ `wallet_ffi.h`. Section 4 analyses it.

## 1. The crate itself

- `lee_core` is `0.1.0` and was not bumped across v0.2.x → v0.3.0. There is no semver signal and no CHANGELOG was found, so the version number tells you nothing about stability.
- Since v0.2.4 the crate has already had one full model change (account rewriting → plan/apply + shards, v0.3.0). #935 would be a second one within roughly a month.

## 2. Module-by-module (v0.3.0 vs the #935 branch)

"Churn" is added/deleted lines in the PR for that file. Judgement is mine; "certain" means certain **if #935 merges as is**.

| Module / item | v0.3.0 | #935 branch | Churn | Judgement |
|---|---|---|---|---|
| `program` entry: `run_program(plan, apply)`, `Plan`, `PlanInput`, `AccountMeta`, `ApplyInput/Output` | The v0.3 ABI | Removed. `define_actor!`, `run_actor`, `run_actor_with`, `ReceiveInput`, `Response`, `Transition` | `program/mod.rs` +272/−486 | **Certain to change** |
| `ChainedCall`, `plan.call`, `plan.inspect` | Chained calls with shard selectors; inspect guards | `Call` / `Cast` / `MessageEnvelope`; no inspect | same | **Certain to change** |
| `ProgramShardSelector` | `{account_id, program_account_id}` | Renamed `account::Actor` | `account.rs` +54/−87 | **Certain to change** (rename, same shape) |
| `ShardData` | `account/data.rs` | `ActorState` (`account/actor_state.rs`); `data.rs` deleted | −194 / +61 | **Certain to change** (rename, likely same bytes: "stored program state keeps its layout") |
| `AccountId`, `PdaSeed`, `for_public_pda(&AccountId, &PdaSeed)` | PDA keyed by program account id (signature as recorded in `MODEL_CHANGES.md` §8.1) | Same signature; adds `AccountId::blinded(factor)`. The v0.3.0 source was not diffed, so a changed prefix or hash input is not ruled out | small | **Probably stable, additive** |
| Private PDA / `PrivateAccountKind` | `Regular` / `Pda` | Same two variants. Header length differs between sources: branch code `HEADER_LEN = 65`, PR #932 spec says 97. Not diffed against v0.3.0 | small | Uncertain |
| `ProgramEvent { selector, data }` | Emitted via `plan.event` | Struct has the same two fields; emitted via `Response::event` | not diffed | Struct probably stable, emission call changes |
| `ValidityWindow`, `BlockValidityWindow`, `TimestampValidityWindow` | Plan setters | Types present with the same names; set on `Response` | not diffed | Types probably stable, call site changes |
| `native_token` | Native-token shard, moved by chained `custody_transfer` | "Native balances are native-token actors" | +184/−242 | **Certain to change** |
| `nullifier` | `Identifier`, `IdentifierError` exported | `Identifier`/`IdentifierError` no longer exported; message nullifiers added | +50/−183 | **Certain to change** |
| Crate exports in `lib.rs` | `PublicAction`, `ProvenCall`, `Identifier`, `IdentifierError`, `ViewTag` | Those are gone. Added: `RootCall`, `RegularKey`, `SenderPresentation`, `MessageWitness`, `InvalidMessageEvidence`, `MessageNote`, `Recipient`, `RecoveryBinding`, `RecoveryInput`, `Seal`, `SealedCast`, `InvalidCastSeal`, `seal_casts`, `ProvingInput`, `DummyOutput` | | **Certain to change** |
| `circuit_io` (privacy-circuit input/journal) | | Carries actor boundary, ordered outputs, assumptions, the schedule; circuit image changes | +797/−96 | **Certain to change** |
| `execution_state` | Engine | Actor driver (Live / Derive / Record / Check) | +1247/−417 | **Certain to change** |
| `commitment`, `encryption`, `encoding` | | `Commitment::for_message` added (`commitment.rs` +62/−1); `encryption/mod.rs` +33/−69; `encoding.rs` +5/−7. `ViewTag` is no longer exported | small to medium | Mixed: additive for commitments, some removals |
| Instruction encoding | borsh | borsh (messages are borsh `MessageData`) | | Stable |
| Public `Message` / `FeeDeclaration` | `{program_account_id, shard_selectors, nonces, instruction_data, fee}` | "root actor, message and public actors" | `public_transaction/message.rs` +91/−172 | **Certain to change** (fee types: small diff, likely stable) |
| Program loader (`ProgramHeader`, `ProgramSegment`, `get_program_via`) | `program_loader` header + segments | Same concepts; `PROGRAM_LOADER_ACCOUNT_ID` is a fixed address; `MAX_PROGRAM_SEGMENTS = 20` | `program_loader/core` +86/−192 | Concepts stable, details may move |

## 3. Reading the evidence

- **Stable under both models:** account ids and PDA derivation, the borsh encoding, program-loader concepts, event and validity-window types, private-account kinds. If you build tooling, put your effort here.
- **Rewritten:** how a program receives input and returns output; how programs call each other; how native balances move; the transaction message; the privacy-circuit interface.
- **The roadmap does not announce any of this.** `logos-co/roadmap` has `lez_program_interface.md` (Program Interface Definition, all three boxes ticked) and `lez_unify_execution.md`, and neither mentions an actor model or a v0.4 for LEZ. Do not read "interface documentation complete" as stability.
- **Caveat on #935:** it is a draft with no reviewers, so details (names like `Actor`, `Response`, `define_actor!`, the `Call`/`Cast` split) can still move. The overall direction (programs as message-handling actors) is consistent across the commits and the PR title.

## 4. The LEZ Core Module (`lez_core`)

### 4.1 What it is today

- Repo `logos-blockchain/logos-execution-zone-module`, `main` at `8f374fd` (2026-10-09, PR #65 "breaking-cleanup"). Files: `src/lez_core_module.{h,cpp}` (about 1,340 lines of C++), `metadata.json`, `flake.nix`, tests with a mocked `wallet_ffi`.
- Module metadata version is currently `1.0.0` (`metadata.json`). The history has several version-bump commits ("chore: bump version", "update version to make room for a previous version release", "bump metadata version"), so there is a manual versioning practice; I did not work out how it maps to LEZ versions.
- It builds against LEZ's `wallet_ffi` and pins a LEZ `dev` revision in `flake.nix`: `24789fb` (comment: "latest breaking changes on dev, including removal of amm, ata, token programs and statistics collection removal"). That pin is 23 commits behind `origin/dev` and is **not** the actor branch.
- Method groups: wallet lifecycle (`create_new`, `open`, `save`, `restore_storage`), accounts (`create_account_public/private`, `list_accounts`, `get_balance`, `get_account_public/private`, key getters, base58 helpers), sync, six transfer variants (`transfer_public`, `_shielded`, `_deshielded`, `_private`, `_shielded_owned`, `_private_owned`), `send_generic_public_transaction`, `send_generic_private_transaction`, `send_program_deployment_transaction`, `poll_transaction_status`, `bridge_withdraw`, labels.
- Stability so far: since 2026-07-31 it has had a LEZ bump or breaking adaptation about every one to two weeks (v0.2.2 bump 08-05; wallet fees/payer 09-09; "adapt to wallet-ffi on dev" 09-28; v0.3.0 bumps 09-30 and 10-01; `feat!: breaking update` 10-07; breaking cleanup merge 10-09). Open PRs: a Windows nix build (#58), a host-owned default profile (#47), a macOS doctest fix (#28).
- Other repos that appear to build on it (not read): `logos-execution-zone-wallet-ui`, `logos-amm-module`, `lez-indexer-module`. The roadmap says all LEZ apps share this module as their backend.

### 4.2 Impact of PR #935 on it

Measured by diffing `wallet-ffi/wallet_ffi.h` from the module's pin to the actor branch with a three-dot diff (so only the branch's own changes count): **+147 / −55 lines**. Then mapping each change onto `lez_core_module.cpp`.

| FFI change on the actor branch | Effect on `lez_core` | Kind |
|---|---|---|
| `FfiAccount.shards`/`shards_len`/`FfiShard` → `actor_states`/`actor_states_len`/`FfiActorState` | `ffiAccountToJson` no longer compiles. The JSON key `"shards"` returned by `get_account_public` and `get_account_private` will presumably be renamed, which breaks every UI that parses it | **Breaking** (module code + its output contract) |
| `wallet_ffi_send_generic_public_transaction`: `instruction_data` + `program_account_id` replaced by `root_mention` (index of the actor the message goes to) + `message` | `send_generic_public_transaction(account_ids, signing_requirements, instruction, program_id_hex, payer, shard_program_account_ids_hex)` must change: `program_id_hex` has no direct place in the call, and callers must say which mention is the root | **Breaking** (module's public method signature) |
| `wallet_ffi_send_generic_private_transaction`: `FfiProgramWithDependencies` (with `self_account_id`) → `FfiProgramCatalog` (no `self_account_id`); adds `root_mention` + `message` | The root-program discovery in the module (it identifies the root by `self_account_id` and fakes a never-matching id for shadow dependencies) becomes unnecessary; `program_kind_json` semantics change | **Breaking** (module's public method signature) |
| `FfiIdentifier` removed; `to_identifier` parameter dropped from `wallet_ffi_transfer_shielded` and `_private`; `identifier` removed from `FfiAccountIdentity`, `import_private_account`, `account_id_for_private_pda` | `transfer_shielded` and `transfer_private` stop compiling (they pass a random identifier); `jsonExtractIdentifier`, `randomFfiIdentifier` and the `"identifier"` JSON key become dead code. The module's own method signatures stay the same | Internal break, public API unchanged |
| New: `wallet_ffi_list_pending_messages`, `wallet_ffi_receive_pending_message`, `FfiPendingMessage`, error `MESSAGE_NOT_FOUND` | No module methods wrap these. A private credit "the sender cannot witness stays pending until its owner receives it", so without new methods (for example `list_pending_messages`, `receive_pending_message`) apps could not receive such credits | **Additive, but needed** |
| `FfiAccountIdentityKind` gains `PUBLIC_PDA`, `PUBLIC_FOREIGN`; `FfiAccountIdentity` gains `public_key` | The module only resolves identities through `wallet_ffi_resolve_*`, so source may compile, but the C struct layout changes, so a rebuild is required | ABI change |
| Everything else the module calls: `create_new`, `open`, `save`, `restore`, `get_balance`, key getters, sync, `transfer_public`, `transfer_deshielded`, `transfer_*_owned`, `bridge_withdraw`, `program_loader_deploy`, labels, `poll_transaction_status` | No signature change in the header diff | **Unchanged** |

Bottom line for the module: about 6 of its 34 methods need code changes (`get_account_public`, `get_account_private` through the shared JSON helper, `send_generic_public_transaction`, `send_generic_private_transaction`, `transfer_shielded`, `transfer_private`). Four of those are visible to callers. Most of the rest (lifecycle, balances, sync, simple transfers, labels, deploy) should survive unchanged. Its mock `tests/stubs/wallet_ffi.h` must also be regenerated.

Caveats: this is the header diff only. The Rust side of `wallet-ffi` changed (`generic_transaction.rs`, `types.rs`, `program_deployment.rs`), and the program ELF / message encoding that callers pass in (`instruction` becomes `message` bytes for an actor handler) is a program-level change that the module simply passes through. The module-level API stays small and generic, but **every app that calls `send_generic_*` passes program-specific payloads, so those apps must be ported along with their programs.**

### 4.3 Other layers #935 changes

#935 also changes (from its file list):

- `lez/wallet` (`lib.rs` +1045/−282, `account_manager.rs` +757/−283, `program_facades/*`; new `cli/pending.rs`)
- `lez/wallet-ffi` (`wallet_ffi.h` +147/−55; new `pending.rs`)
- `lez/indexer/ffi` (`indexer_ffi.h` +164/−57) and the indexer JSON protocol (`protocol/src/convert.rs` +496/−149)
- `lez/sequencer/ffi` (`sequencer_ffi.h` +161/−56)

Anything that wraps the wallet or these FFI headers (the LEZ Core Module above, the indexer module `lez-indexer-module`, the explorer UI `lez-explorer-ui`) will have to be rebuilt against the new headers. Only the Core Module's source was read for this note.

## 5. SPEL

SPEL (`logos-co/spel`) generates dispatchers and IDLs over the program ABI. `MODEL_CHANGES.md` already notes that `#[account(init)]` is built on the v0.2-style claim model and "needs checking against a SPEL release that targets LEZ v0.3". Going by the table above, a v0.4 target would be a second rework for SPEL. Not verified: SPEL's own branches or plans for the actor model were not checked.

## 6. How to keep this current

- Watch PR #935 for: reviewers requested, "ready for review", a filled-in description, a milestone or label.
- Re-run the file list: `gh api "repos/logos-blockchain/logos-execution-zone/pulls/935/files?per_page=100" --paginate --jq '.[] | [.additions,.deletions,.filename] | @tsv'`
- Re-diff the `lee_core` surface: `lib.rs`, `program/mod.rs`, `account.rs` between `v0.3.0` (db66590; note the **local** `v0.3.0` tag in the LEZ clone is stale and points at an old commit) and the branch head.
