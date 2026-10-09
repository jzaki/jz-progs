# Was the "one private tx per block" limit lifted in v0.3?

Researched 2026-10-09. Public sources only. Code read at tag `v0.3.0` = commit `db66590` (2026-09-29) and `v0.2.4` = `47eba25`.

> **Tag warning.** In a stale local clone of the LEZ repo the `v0.3.0` tag pointed at an unrelated old commit (6c5ac1b, Jan 2025). The real tag is `db66590`, confirmed with `gh api repos/logos-blockchain/logos-execution-zone/git/ref/tags/v0.3.0`. Use the GitHub refs, not local tags.

## Answer: partially

- **Lifted in v0.3.0:** the race between a privacy proof and the *public* pre-state it was proven against.
- **Not lifted:** the serialisation per *private account*. Two updates of the same private account cannot both land from the same account version.
- **There is no explicit rule.** No code at any version limits transactions per block, per program, per account or per nullifier. The limit people describe is a consequence of how the proof and the nullifier work. Nothing in code or tests names a per-block limit, so the "per block" wording is an inference.

## 1. No block-level rule

`build_block_from_mempool` (`lez/sequencer/core/src/lib.rs`, ~1700–1856) pops the mempool in order and applies each tx to a working state via `apply_mempool_transaction` (~1397). A tx that fails is dropped: not re-queued, re-executed or merged. The only block caps are `max_block_size`, `max_num_tx_in_block` and the declared-gas budget. A search for "contention" or "one tx per block" in `lez/`, `lee/` and `docs/` at `db66590` found nothing.

## 2. Before (v0.2.4): public pre-state race

`validated_state_diff/mod.rs` (~401–413) built `public_pre_states` from live state and called `check_privacy_preserving_circuit_proof_is_valid(&proof, &public_pre_states, message)`. The proof journal committed the full public pre- and post-state (`PublicAction { pre, post }`). If any other tx changed one of those public accounts between proving and inclusion the result was `InvalidPrivacyPreservingProof`. Two private txs touching the same public account conflicted.

## 3. After (v0.3.0): public side settles on live state

- `validated_state_diff/mod.rs:384`: "The journal carries no public state, only the effects settlement must fold."
- The journal holds `PublicAction { account_id, is_authorized, effects: Vec<DeferredPublicEffect> }` (`lee/state_machine/core/src/circuit_io.rs`).
- Settlement runs `apply_public_effects` against **live** state. The guest's `apply` runs on the current shard; a failure rejects the tx atomically and spends no nullifier.
- Tests (`lee/state_machine/src/state/tests/privacy_preserving.rs`): `two_deshielded_transfers_to_one_recipient_compose_at_settlement` (two private senders credit the same public account; both settle in one block, balance correct), `a_guest_evaluated_public_effect_settles_against_live_state`, `a_failing_deferred_effect_leaves_the_state_untouched`.
- PR [#912](https://github.com/logos-blockchain/logos-execution-zone/pull/912) "incremental updates with restricted program surface" (merged 2026-09-26, in v0.3.0): "A prepared transaction can remain valid after public data change modulo program-bound constraints." Earlier attempts: #767 (closed; "the race condition where an unrelated public transaction landing between proving and validation would invalidate an otherwise-valid proof"), #902 (closed; "shifts privacy transactions race condition from protocol level to program level"). #835 (sharded program state) also helps: programs write only their own shard (`ForeignShardWrite`), so different programs' shards on one account do not collide.
- Residue at program level: `MODEL_CHANGES.md` §7.2 H. A program cannot read state in `plan`, so reads become claims in the instruction and `apply` asserts them. If the live value no longer matches the claim, `apply` panics and the tx reverts. That is the "program-bound constraint" in #912.

## 4. Private side: still one update per private account version

- A private update proves membership of the account's current commitment under a historical root and emits `Nullifier::for_account_update(commitment, nsk)`.
- `state/mod.rs` ~419–435 (`check_nullifiers_are_valid`) rejects "Nullifier already seen" and "Unrecognized commitment set digest". A second tx built on the same account version has the same nullifier and fails. A tx built on the next version needs that new commitment in the tree first, so the first update must already be included.
- Tests: `circuit.rs::private_accounts_can_only_be_initialized_once` (~line 1018), and a similar init-nullifier test (~1160).
- Not a conflict: other txs growing the commitment tree. Old roots stay valid via `root_history`.
- Other conflicts that remain: "Commitment already seen" (duplicate commitments), "Nonce mismatch" for public signers, and a public-effect `apply` that fails because the live balance or claim changed.
- Private txs are fee-exempt for now (#941).

Practical reading: you can have many *different* private accounts updating in one block, and many private txs crediting the *same public* account, but you cannot chain two dependent updates of the *same private account* before the first is included. Whether a wallet can submit dependent private txs within one block depends on its proving flow; this was not tested at `db66590`.

## 5. Does #935 (actors) change this?

Only an inference from tests; the branch has no text about contention.

- Sending becomes non-conflicting at protocol level: many senders can `Cast` to the same actor without touching its state. `identical_casts_to_a_private_receiver_are_spent_independently`: each message has its own position and `Nullifier::for_message`.
- Receiving is still per account: a private receipt consumes one message nullifier, initialises the receiver on first receipt, then updates it with a normal account nullifier. A repeated receipt gives "Nullifier already seen"; re-initialising gives "Commitment already seen". (`pending_messages.rs`, `sealed_messages.rs`; in the test the receipts land at block 2 and 3, which is illustrative, not a rule.)
- `TransactionEntry<R>` (`execution_state.rs:17`) has a single root `Call` or `Cast`, consistent with "a proof claims at most one consumed message".
- Net: the model lets writers enqueue instead of updating shared state, which is the point of asynchronous messaging. The private receiver's own account update remains serialised.

## 6. Related claims and roadmap items

- A "one transaction per block (per program)" limit is sometimes claimed; no public source states it. It is unclear whether it refers to the public-state race (now fixed) or the private-account nullifier (still there); the evidence above is consistent with the latter.
- `logos-co/roadmap` `content/blockchain/roadmap/lez_state_contention.md`: "State contention is a fundamental challenge in the private execution model" and "Solution may introduce centralized actors to sequence access to private state". Checklist: **study complete [ ]**, **approach proposed [ ]**, both unchecked, no date. Note this "actors" is a sequencing-service idea, not the program model.
