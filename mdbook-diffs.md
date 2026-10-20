# The two mdbooks: plan/apply vs actor model

schouhy's LEZ manual exists in two copies, first on LEZ branch `schouhy/lez-guide-mdbook` and now with fixes on [`moudy/guide-mdbook-fixes`](https://github.com/logos-blockchain/logos-execution-zone/tree/moudy/guide-mdbook-fixes/docs) (local copy: `moudy_guide-mdbook-fixes_latest/`):

| Book | Model | Written against |
|---|---|---|
| `docs/program-development-book/` | v0.3, `plan`/`apply` | PR 912 ("incremental updates"), merged into `dev` on 2026-09-25 |
| `docs/actor-program-development-book/` | v0.4, transactional actors | PR 935 ("transactional actors with side-effects") at commit `faaa07251` |

The actor book started as a copy of the plan/apply book. It keeps the five-part structure (foundations, privacy, patterns, catalog, reference), but nearly every chapter was rewritten. Only the outline is the same. This file lists what changed so you can tell which book to read for what. For v0.4 work, read the actor book.

Compared at branch tip `8d8ae6e6b` (2026-10-13).

## In one table

| | Plan/apply book | Actor book |
|---|---|---|
| A program is | two entry points: `plan` (no state, emits effects and chained calls) and `apply` (one shard, keep/write/check) | one handler: one message plus one actor's state in, new state plus messages out |
| Unit of state | a **shard**: one program's bytes on one account | an **actor**: `(account_id, program_account_id)`, same bytes, new name |
| Reaching another program | chained calls, and effects/inspects on shards passed in | messages: **Call** (now, all-or-nothing, can grant PDA seeds) and **Cast** (now if possible, otherwise stored) |
| Reading other state | `inspect` a shard handed to you | ask with a message and handle the reply |
| Paying a private account | create a new private account for the recipient inside a proof | Cast a credit; it waits in the recipient's mailbox |
| Private pseudonyms | hashes of secrets kept in public state, per program | protocol-level **blinded addresses**, as sender or destination |
| Who is speaking | caller program id and per-account authorization | `input.from`, the sending actor (account + program), possibly a blinded address |
| Book's comparison boxes | 7 **With actors** boxes noting what changes in v0.4 | **Improvement over plan/apply** and **Trade-off** boxes, in most chapters |

## Structure

The title changed from "LEZ Program Development" to "LEZ Program Development: the Actor Model".

**Part I** went from 9 chapters to 8:

| Plan/apply | Actor |
|---|---|
| Accounts and shards | **Accounts, actors and state** (new file `actors.md`) |
| Programs and deployment | merged into **Programs, deployment and identity** |
| Plan and apply: the execution model | replaced by **Handlers and messages: Call and Cast** (new file `messages.md`) |
| The input echo | rewritten for handlers |
| Authorization and PDAs | rewritten |
| Transactions, public and private | rewritten |
| Private accounts in depth | **Private accounts, addresses and the mailbox** |
| Program identity: disclosed, undisclosed, shadow | merged into Programs, deployment and identity |
| System programs and the block | rewritten |
| Toolchain | rewritten |

**Part II** (privacy) keeps both chapters, rewritten. **Part III** is renamed from "Design patterns for uniform interop" to "Design patterns". **Part IV** (catalog) is reorganised. See below.

## Part I: what is new in the actor book

These topics did not exist in the plan/apply book:

- **The one rule.** A handler reads and writes only its own actor. To change another actor it sends a message. There is no balance field. The native balance is the actor under `NATIVE_TOKEN_PROGRAM_ID`.
- **`ReceiveInput` / `Response`.** The handler sees `receiver`, `from`, `is_authorized`, `pre_state` and `message`. It returns a post-state, Calls, Casts, events and validity windows, built with `Response::set_state(..).call(..).cast(..)`.
- **A Call returns no value.** The book calls this the most common misunderstanding. A read is two messages: `ReadState` and then a `StateReply` Call back. The receiver must check who sent the reply.
- **Cast delivery rules.** A Cast is delivered at once to a declared public actor, or to the submitter's own private actor if the submitter asks. Anything else is stored. Only private addresses can hold stored messages. A Cast to an undeclared public account fails.
- **Depth-first delivery.** A handler can run more than once in one transaction, and the second run sees the state from the first.
- **The mailbox.** Stored messages are *publications*: **clear** if the sender was public, **sealed** if it was private. They are collected one per transaction with `wallet pending list` / `wallet pending receive`.
- **Recovery bindings.** An address must be registered on chain before a public sender can cast to it. Without one, the Cast fails with `UnboundCastDestination`. A private sender's sealed message needs no binding.
- **Blinded addresses.** `account_id.blinded(&factor)`. They hide the account from observers and public senders, but not from a private sender, who needs the recipient's keys to seal a message.
- **`RegularKey::Authorized` vs `Nullifying`.** You can collect a message with or without the spending key. Collecting with it counts as consent to what the message asks.
- **Three runs of one engine** for a private transaction: a dry run, the private part (writes the **boundary**) and the public part (checked against the boundary). A private-to-public message is a *demand*. A public-to-private message is an *assumption*.
- **Proof counts.** One inner proof per message a private actor handles, plus the circuit. Public handlers are never proven, and the native token needs no inner proof. Per PR review counts, a private token transfer went from 3 receipts to 2, and a private trader swapping with a public pool went from 5 to 2.
- **Admission.** The first message to a fresh public account must carry evidence for it, such as its public key. Otherwise the transaction fails and the payer is still charged.
- **Upgrades.** A stored Cast from an old program version may be collected after an upgrade, so the new handler must still accept every message the old version could have sent.
- **Testing.** Build a `ReceiveInput` by hand and call the handler. Test each message three ways: honest sender, wrong sender, no authorization.

### The `is_authorized` footgun

The authorization chapter now ends with a section titled *What `is_authorized` does not mean*, and the book comes back to it in several places. The flag means the owner's credential is present in the transaction. It does not mean the owner sent this message. This bites in two ways:

1. Every program that runs in a transaction you signed sees your accounts as authorized.
2. Anyone can cast `Transfer { to: mallory }` to your private balance. It runs if you collect it with your spending key.

The rule for owner-only operations: check `is_authorized && input.from.is_none()`. Wallets should collect without the spending key by default.

### Dropped from Part I

The actor book drops `plan`/`apply`, effects, guards, `inspect`, selectors, the 10-chained-call limit, family slots and `FamilyBindingConflict`, `Identifier`, init/update witnesses, view tags, and deferred public effects applied at settlement. Shadow programs get one paragraph: the code has them, but the book has not examined them under actors.

## Part II: privacy

- **What leaks.** The published fields are now the **boundary**, the public context, the public root, private actions, sealed casts, clear publications, recovery bindings, windows and program claims. The headline: *anything a private actor says to a public actor is public; only who said it can be hidden.* The worked examples changed from a private swap and a private donation to a private voter and a public salary paid to a private employee.
- **Limits.** Now listed one limit per section. New limits: nobody can make a private actor act; a Call cannot reach a stranger's private actor; a reply from public to private is fixed at proving time, so it goes stale if public state moves; one collected message per transaction; a stored message cannot bounce. A **Trade-off** box lists three things that were easier under plan/apply:
  - **Reading:** a program could inspect another program's state directly.
  - **Staying valid:** a private transaction's public effects were evaluated at settlement.
  - **Receiving:** incoming funds needed no collection step.

## Part III: the pattern language

All 18 old patterns were replaced by 19 new ones, numbered and cited by number throughout the catalog.

- **Old (plan/apply):** proposal and guard, two-sided binding, inspect-don't-write, seed custody, config pinning, caller pinning, self-call for post-conditions, receipt shard, init-only receive, family slots, secret in the instruction, commutative public effects, windows, clock proposal, public bulletin/private actor, private state/single actor, bind by content, deposit key.
- **New (actors):**
  1. tell, do not write
  2. route on the sender first
  3. internal messages
  4. the owner, or a **seat**
  5. reply with an echo
  6. receive without failing
  7. **the proof-carrying message**
  8. pseudonym
  9. cast for strangers
  10. **accept by collecting**
  11. anyone may poke
  12. the public referee
  13. seed-authorized custody
  14. the receipt actor
  15. write once
  16. windows for expiry
  17. clock assertion
  18. group-owned private PDA
  19. the notify hook

Pattern 7 is the important one. A private actor of program P messages a public actor of P, and the public actor trusts it because `from_own_program()` shows its own code produced the message inside a proof. Most of the catalog is built on it. The **seat** (pattern 4) is how a shared actor knows which participant is speaking: each participant gets their own actor of the program, and the shared actor checks `from_own_program()` together with the seat's account. An account id alone identifies nobody.

The anti-patterns list is new for actors. It includes trusting `is_authorized` alone, collecting with the spending key by default, a Call to a stranger's private actor, replying with fast-changing data to a private actor, and identifying a person by account id alone.

`state-placement.md` now has a table of placements (public key-owned, public PDA, private regular, private PDA, group-owned private PDA, the mailbox) with who reads, who runs, who authorizes, and how each is reached. It describes two program shapes: *public hub, private spokes* and *private hub, any spokes*.

## Part IV: the catalog

Each entry now has the same shape: **Shows** line, Goal, Actors, Messages, Handler, What to notice, Public and private, Going further. Entries are kept deliberately small (the token is three messages). The per-entry "interoperability matrix" became a "Public and private" section.

| Plan/apply entry | Actor book |
|---|---|
| Private donations | **Rewritten.** No program at all: a native transfer to a registered blinded address plus collection. The old donation program is gone. |
| (none) | **New: Payment requests.** The request *is* a native `Transfer` cast to the payer, who accepts by collecting it with their spending key. |
| Multisig | **Rewritten.** One handler (`Approve` to a seat, `Approved` to the vault), four placements: public/hidden vault × public/hidden members. A hidden vault is a group-owned private PDA. The group secret lets members operate it but not spend from it. |
| Tokens, NFTs and associated accounts | **Cut down** to "A minimal token": `Transfer`, `Mint`, `Credit`. NFTs and ATAs dropped. |
| Automated market maker | **Removed.** Listed under "what does not work" if the reserves are hidden. |
| Payment streams and vesting (designs A–C) | **One design:** a public PDA stream that anyone can `Release` (poke), with a clock assertion and a custody transfer whose credit is a Cast. |
| Payment channels | **Rewritten.** One-way channel; `Redeem` computes the amount instead of taking it as a parameter. Has a **Trade-off** box: capacity is no longer funded inside `Open`. |
| Private voting (designs A–C) | **One design:** the election casts `Ballot`s to voters, private ballots vote once under fresh blinded addresses (pattern 7). No Merkle tree, membership path or receipt account. |
| Tickets, attestations and registries | **Replaced** by "Credentials and tickets": a badge program whose `Show` Calls a venue with `Attested { issuer }`. The venue only checks the sending program. |
| Rate-limiting nullifiers | **Rewritten** as seat → registry; `Slash` takes 1 argument where the Logos plan/apply program takes 6. |
| Escrow, swaps, auctions and flash loans | **Cut** to "Escrow and sealed bids". HTLC, atomic private swap and flash loans dropped. Sealed bids use a private **paddle** that stores the bid with `write_once` and reveals it later, with no commit/reveal hash. The book flags the multi-paddle loophole. |
| LeZork | **Removed.** |
| Battleship (`battleship.md`) | **Moved** to `hidden-games.md`. The board asks the match for the pending shot in the same transaction (reply pattern). Boards are known by possibly-pseudonymous addresses. |
| Deep Space Radio | **Rewritten.** Radio messages are Casts addressed to the ship, call signs are blinded addresses, and docking is a private-to-public Call. |
| Idea catalog | **Rewritten** with pattern numbers. Adds a proposed clock `Stamp` request/reply, a notification inbox, a name registry and a relayer for private fees. Updated "does not work" list. |

Diagrams were regenerated to match (`diagrams/specs.py`). Diagrams for removed entries are gone. New ones cover requests, credentials, the sealed-bid auction, the four token transfers and the new multisig placements.

## Part V: reference

- **API cheat sheet.** Fully replaced: `define_actor_logic!` (plus a `raw` form), `ReceiveInput`, the `Response` builder, `Actor`, `write_once`, `from_own_program`, native token, clock, and a delivery table for Call/Cast. New engine errors: `UndeclaredActor`, `UnadmittedPublicActor`, `UnboundCastDestination`, `TransitionInputMismatch`, `BoundaryMismatch` / `CrossMessageMismatch`, `LoaderOutsidePublicExecution`, `EmptyValidityWindowIntersection`.
- **Constants.** Cut from about 25 rows to 7. Gone: the 100 KiB shard cap and the 10-chained-call cap, which have no actor-model equivalent ("review found no cap"), plus the fee-market constants. New: one collected message per transaction. A section says the book designs as if state size and message count were capped anyway.
- **Derivations.** Adds the blinded address (`/LEE/v0.3/AccountId/Blinded/`) and a fixed-ids table (native token = zero bytes, loader = `0xFE…`, the three clock accounts).
- **Glossary.** Rewritten: actor, actor state, address, admission, boundary, Call, Cast, collecting, dry run, mailbox, poke, promotion, publication, recovery binding, seat, seed grant, sealed cast, transition. Shard, effect, guard, inspect, plan and apply are gone.
- **Sources.** Now a topic → file table, plus a list of what the book did *not* examine: the AMM and ATA under actors, cross-zone messaging, shadow programs, the wallet's blinded-address handling, and real proving times. The RLN research links were cut to just `logos-co/logos-lez-rln`.

## The actor book vs `dev`

The book is written against PR 935 at `faaa07251`, which is not an ancestor of `dev`. It uses the names that PR had at that commit. As of `dev` `d7ba18f57`:

| Book | `dev` |
|---|---|
| `Call` / `Cast` | `SyncMessage` / `AsyncMessage` |
| `define_actor_logic!` | `actor_entrypoint!` |
| `WholeTransaction` | `WholeExecution` |
| `Scope` | `Mode` (a trait, with `WholeMode`, `PrivateMode`, `PublicMode`) |

`from_own_program`, `write_once`, `blinded`, `RecoveryBinding`, `wallet pending` and `wallet group` are on `dev` under the book's names.

The book's "shipped token" (with its `EnsureHolding` repair message and the `notify` hook) and the AMM and ATA programs are **not in the LEZ repo on `dev`**. They left it in `c16cf3000` ("more removals") and moved to their own repo, `logos-blockchain/lez-programs` (`programs/token`, `programs/ata`, `programs/amm`), from which testnet deploys them. The fixed book points there. `dev` itself ships `wrapped_token`.

## Fixes on `moudy/guide-mdbook-fixes`

That branch corrects both books against the code. The actor book has three changes:

- **Deploying.** `wallet program-loader deploy` takes no positional path: it's `--elf <bin> --header <account> --segments <account>... [--immutable]`. The header and segment accounts must exist first, with empty `program_loader` state, one segment account per 96 KiB of the user ELF (`MAX_SEGMENT_DATA_LEN`). A wrong count fails with `SegmentCountMismatch`. A deploy that fails partway can't be resumed over the same accounts. [INTRO.md](INTRO.md) section 4 already had the first part.
- **Token paths.** `lez/programs/token` becomes `logos-blockchain/lez-programs: programs/token` (see above).
- **Core crates.** The example of a separate `core` crate for message types is now `lez/programs/clock/core`, plus the token's in `lez-programs`.

The plan/apply book's fixes are smaller and matter only for v0.3: the token, ATA and AMM paths moved to `lez-programs` (and `amm_prepared_offers.rs` is gone; the state tests in `lee/state_machine/src/state/tests/privacy_preserving.rs` show the same property), the same deploy CLI, `Message::try_new_with_fees` with a `FeeDeclaration`, `Identifier` is 32 bytes (not `u128`), a view tag is key-derived only on `Init`, `env::log` is always compiled, proof tests aren't feature-gated (`RISC0_DEV_MODE=1` makes them cheap), and `wallet account get --scope` takes `balance`, `all` or a program account id.
