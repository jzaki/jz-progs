# SPEL as it is

SPEL ("Smart Program Execution Layer", `logos-co/spel`) is a framework for writing Logos Execution Zone (LEZ) guest programs, modelled on Anchor for Solana. You write instruction functions with attributes on their account parameters. SPEL generates the guest entry point, the dispatcher, the account checks and an IDL. From the IDL it gives you a generic CLI, typed clients and a project scaffold.

This file describes SPEL **as it is**, as groundwork for redesigning it for the v0.4 actor model. It covers what SPEL solves, how it is built, what using it looks like next to a hand-written guest, how its extension mechanism works (with `spel-admin-authority` as the worked case), and what it costs. Section 7 maps each part onto v0.4 without proposing a design.

**Every claim is tagged with its basis:**
- *specified*: SPEL's own docs or comments say so.
- *implied*: the code does it, though no doc says so.
- *inferred*: my conclusion from the code, not stated anywhere and not run.

**Sources:**

| Repo | Ref | Used for |
|---|---|---|
| `logos-co/spel` | `main` at `c6f73ce` (2026-10-13, PR #290), also tagged `v0.7.0-rc.3` | Framework, CLI, client generator, docs. `../spel` |
| `logos-blockchain/lez-programs` | `main` at `071375c` (2026-10-21). Pins spel at the same `c6f73ce` | Real usage. `../lez-programs` |
| `mmlado/spel-admin-authority` | `main` at `027197f` (2026-10-16). Pins spel tag `v0.7.0` (LEZ v0.2.4) | The first extension (section 5). Read on GitHub |
| `mmlado/spel-freeze-authority` | `main` at `959c0cf` (2026-10-14). Same pins | The second extension, stacked on the first. Shared primitive: `mmlado/spel-authority` `2902dc7` |
| `logos-blockchain/logos-execution-zone` | tag `v0.2.5-rc2` (`d8596eb73`, 2026-09-09) | The LEZ API SPEL compiles against |
| `logos-blockchain/logos-execution-zone` | `jzaki/fix-examples` (`928493c00`) | v0.4 contrast, using `dev` names |

---

## 0. Which LEZ SPEL targets

**SPEL `main` targets LEZ `v0.2.5-rc2`.** That is not the v0.2.4 these notes call "v0.2", and it is not v0.3 or v0.4. The `v0.2.5-rc*` tags are pre-releases on `dev` on the way to v0.3.0: each one is an ancestor of `v0.3.0`, and `v0.2.4` is not an ancestor of any of them. There is no `v0.2.5` final tag. The rc tags come before PR 912 (plan/apply), so v0.2.5 is still the account-rewriting model, with most of the v0.3 infrastructure already in.

| SPEL | Date | LEZ | Notes (`CHANGELOG.md`) |
|---|---|---|---|
| v0.5.0 | 2026-06-01 | v0.1.2 (retag of `v0.2.0-rc3`) | |
| v0.6.0 | 2026-07-15 | v0.2.0 | `nssa`/`nssa_core` became `lee`/`lee_core` |
| v0.7.0 | 2026-09-16 | v0.2.4 | Extensions, witness exchange, private-PDA `vpk` |
| `main` (`c6f73ce`, tag `v0.7.0-rc.3`) | 2026-10-13 | v0.2.5-rc2 | The port, PR #290. The crates still say 0.7.0, and the CHANGELOG has no entry yet |

SPEL still imports LEZ under the old names: `nssa_core = { package = "lee_core", tag = "v0.2.5-rc2" }` (`spel-framework/Cargo.toml`). The prelude re-exports what a program needs, and SPEL's docs say "do not import from `nssa_core` directly to avoid version conflicts" (README).

### What v0.2.5-rc2 changed from v0.2.4

| | v0.2.4 | v0.2.5-rc2 |
|---|---|---|
| Program identity | `ProgramId`, the image id | The `AccountId` of a `program_loader` header account. The same binary can be deployed several times |
| Deploy | `wallet deploy-program`, free | `program_loader`: segment accounts plus a header. `spel deploy` does the bookkeeping |
| Entry | `read_lee_inputs()` | `read_lee_call()` → `ProgramCall::Execute(..)`, or `respond_unsupported_call(..)` |
| Instruction encoding | risc0 serde (`Vec<u32>`) | borsh (`Vec<u8>`) |
| Output | `post_states`: whole accounts, with `Claim` | `state_diffs`: `AccountStateDiff { pre_state, post_balance_diff: BalanceDiff, post_data: Option<Data> }` |
| Claiming | `Claim::Authorized` / `Claim::Pda(seed)` | None. The first program to write data to a default-owned account becomes its owner (`acquire_ownership_on_data_write`) |
| Chained call | `ChainedCall { program_id, pre_states, .. }`: the caller passes account values | `ChainedCall { program_account_id, pre_state_ids, instruction_data, pda_seeds }`: the engine resolves the values and `is_authorized` |
| Accounts in the output | Unowned, untouched accounts could be dropped | Every declared account must be returned (`DeclaredAccountMissingFromOutput`) |
| Pre-states | | Each returned `pre_state` must equal the chain's value (`InconsistentAccountPreState`) |
| Fees | None | `FeeDeclaration`, and the payer must sign. A failed program call is included in the block and charged (below) |
| Events | None | `ProgramOutput::with_events`. SPEL doesn't expose it (section 3.4) |

What stays the same as v0.2.4:
- The program receives the **whole account** (balance, data, `program_owner`, nonce) for every account the transaction names.
- The engine checks its output against ownership and conservation rules (`validate_execution` in `lee/state_machine/core/src/program/mod.rs`).

**Failed calls are included and charged.** In `from_public_transaction_metered` (`lee/state_machine/src/validated_state_diff/mod.rs`), a "chargeable" failure keeps no effects but advances the signers' nonces, and the fee stays paid. A guest panic, out of gas, or invalid program behaviour all count as chargeable. Only `InvalidInput` and an unknown top-level program are not chargeable (`LeeError::is_chargeable`, `lee/state_machine/src/error.rs:105`). So "the transaction was included" doesn't mean "the instruction succeeded". Section 3.5 shows why that matters for SPEL.

---

## 1. Why SPEL exists

> "Write your program logic with proc macros. Get IDL generation, a full CLI with TX submission, and project scaffolding for free." (README, *specified*)

### What a v0.2.5 guest must do by hand

1. Read the call. Match `ProgramCall::Execute`, and pass any other call kind to `respond_unsupported_call`.
2. Decode an instruction type of its own and dispatch on it.
3. Destructure `pre_states` by position, with a count check, for each instruction.
4. Check each account's role:
   - a signer is `is_authorized`;
   - a new account is `Account::default()`;
   - an existing account is owned by the expected program;
   - a PDA's id equals `AccountId::for_public_pda(self, seed)`.
5. Decode and encode its state bytes.
6. Build an `AccountStateDiff` for **every** declared account, including untouched ones, add any chained calls, and `write()` a `ProgramOutput`.

Off-chain there is more to do:
- Every client hand-codes the instruction layout and the account order.
- Every client derives the PDAs.
- Every client signs with the right keys and fetches their nonces.
- Every client attaches a fee.
- Account bytes have to be decoded to be read.

### What SPEL provides

| Job | SPEL | Basis |
|---|---|---|
| Entry point, dispatch, `ProgramOutput` | `#[lez_program]` generates `main` | specified |
| Instruction encoding | A generated borsh `Instruction` enum, or `#[lez_program(instruction = "core::Instruction")]` | specified |
| Account destructuring | One typed parameter per account, plus `Vec<AccountWithMetadata>` for the rest | specified |
| Role checks | `#[account(signer, init, owner = .., pda = ..)]` become generated `__validate_*` checks, run before the handler | specified |
| Trusted context | A `ProgramContext` parameter (`self_account_id`, `caller_account_id`) | specified |
| Interface description | An IDL: instructions, accounts and their roles, PDA seeds, arg types, account data types | specified |
| Calling a program | The `spel` CLI: subcommands and typed args from the IDL, PDA derivation, signing, nonces, fee payer, `--dry-run` | specified |
| Multi-party signing | `--export` / `--co-signer`, then `spel sign`, then `spel submit` | specified |
| Reading state | `spel inspect <account> --type T` decodes account data with the IDL | specified |
| Clients | `spel-client-gen`: a typed Rust client, C FFI with JSON in and out, a C header, and a Qt/QML Logos Basecamp module | specified |
| Deploy | `spel deploy` creates the segment and header accounts and prints the program address | specified |
| Project layout | `spel init` scaffolds the guest, the core crate, the IDL and CLI binaries, and a Makefile | specified |
| Reusable security modules | Extensions: a dependency plus a marker attribute contribute instructions, inject accounts and wrap handlers | specified |

---

## 2. Architecture

| Crate | Lines (src) | Role |
|---|---|---|
| `spel-framework` | ~30 | Umbrella. Re-exports the macros, core and `prelude`. Re-exports `borsh` so a program uses LEZ's version |
| `spel-framework-core` | ~10k | `SpelOutput`, `SpelError`, `ProgramContext`, PDA helpers, IDL types. Behind the `idl-gen` feature: source-parsing IDL generation, dependency walking, extension discovery |
| `spel-framework-macros` | ~3k | `#[lez_program]`, `#[instruction]`, `#[account_type]`, `generate_idl!` |
| `spel-cli` (bin `spel`) | ~8k | The IDL-driven CLI, `init`, `deploy`, `pda`, `inspect`, `sign` / `submit` |
| `spel-client-gen` | ~7k | Rust client, C FFI, header, Basecamp module generator |

Two pipelines leave the same source file:

```text
methods/guest/src/bin/prog.rs
   │
   ├─ #[lez_program] ──► guest ELF (.bin) ──spel deploy──► program address (AccountId)
   │       │
   │       ├─ PROGRAM_IDL_JSON   const string, compiled into the guest
   │       └─ __program_idl()    structured IDL fn, host tooling
   │
   ├─ generate_idl!("…/prog.rs")  in examples/src/bin/generate_idl.rs: a host binary that prints the IDL
   └─ spel generate-idl prog.rs   the CLI parses the source itself
                │
                ▼
          prog-idl.json ──► spel CLI: calls, pda, inspect, dry-run, export/sign/submit
                        └─► spel-client-gen: Rust client, C FFI + .h, Qt/QML Basecamp module
```

The IDL has four producers. Shared helpers in `spel-framework-core` (injection, ordering, type collection) keep instructions, accounts and types in step. They still differ in two ways:
- whether empty `types`/`errors` keys are emitted, which the docs record (`docs/reference/extensions.md`, *specified*);
- which per-instruction extras appear (section 3.8).

`spel init my-program` writes (README, *specified*):

```text
my-program/
├── Cargo.toml                 # workspace
├── Makefile                   # build, idl, cli, deploy, inspect, setup
├── my_program_core/src/lib.rs # shared types (guest + host)
├── methods/guest/             # RISC Zero guest, with its own Cargo.lock
│   └── src/bin/my_program.rs
└── examples/src/bin/
    ├── generate_idl.rs        # spel_framework::generate_idl!("../methods/guest/src/bin/my_program.rs");
    └── my_program_cli.rs      # #[tokio::main] async fn main() { spel::run().await; }
```

The guest is built with risc0's Docker build, with the project directory as the context (`target/riscv32im-risc0-zkvm-elf/docker/…`). A `spel.toml` lets `spel` find the IDL and binary without flags (`docs/reference/cli.md`).

---

## 3. Design choices and implementation details

### 3.1 The model SPEL assumes

An instruction is a function from **(accounts in a fixed order, args)** to **(per-account diffs, chained calls, validity windows)**. Everything else is built on that shape: the account attributes, the IDL's per-instruction account lists, the CLI's account resolution, and extension injection. It is Anchor's model. It fits LEZ v0.2.x because a v0.2 program sees the accounts the transaction names, whole. *Inferred:* this is the choice that doesn't carry over to v0.4 (section 7).

### 3.2 What `#[lez_program]` generates

From `spel-framework-macros/src/lib.rs:229-642` (*implied*).
- An `Instruction` enum, unless an external one is named. Each variant is the PascalCase name of an `#[instruction]` fn, and its fields are that fn's non-account parameters. It derives borsh only, through `#[borsh(crate = "spel_framework::borsh")]`. Variants follow source order, and the code says to treat them as append-only, because borsh encodes the variant index as a leading tag byte (`:347-360`, *specified* in a comment).
- `pub const PROGRAM_IDL_JSON: &str` and `pub fn __program_idl()`.
- `pub mod <name>`, which holds the module's other items, each handler with its attributes stripped, and a `pub fn __validate_<ix>(..)` for each instruction that has any check. The module is `pub` so host tests can call handlers and validators directly.
- `pub fn main()`, under `#[cfg(not(test))]` so `cargo test` works on the host. The program still writes `#![no_main]` and `risc0_zkvm::guest::entry!(main);` itself.

Below is the expansion for a two-account `initialize`, written out by hand from the generator code. It is not `cargo expand` output, and the names are the generated ones:

```rust
pub mod my_counter {
    pub fn initialize(counter: AccountWithMetadata, owner: AccountWithMetadata) -> SpelResult { /* as written */ }

    pub fn __validate_initialize(
        accounts: &[AccountWithMetadata],
        self_account_id: &AccountId,
        _instruction_data: &InstructionData,
    ) -> Result<(), SpelError> {
        // order: owner checks, signer checks, init checks, PDA checks
        if !accounts[1].is_authorized {
            return Err(SpelError::Unauthorized { message: /* "Account 'owner' (index 1) must be a signer" */ });
        }
        if accounts[0].account != Account::default() {
            return Err(SpelError::AccountAlreadyInitialized { account_index: 0 });
        }
        {
            let __seed_0 = spel_framework::pda::seed_from_str("counter");
            let __expected_id = spel_framework::pda::compute_pda(self_account_id, &[&__seed_0]);
            if accounts[0].account_id != __expected_id {
                return Err(SpelError::PdaMismatch { /* .. */ });
            }
        }
        Ok(())
    }
}

#[cfg(not(test))]
pub fn main() {
    let __call = read_lee_call::<Instruction>();
    let ProgramCall::Execute(ProgramInput { self_account_id, caller_account_id, pre_states, instruction }, instruction_data)
        = __call else { respond_unsupported_call(__call) };

    let result = match instruction {
        Instruction::Initialize => {
            let [counter, owner] = <[_; 2]>::try_from(pre_states)
                .unwrap_or_else(|v: Vec<_>| panic!("Account count mismatch: expected {}, got {}", 2, v.len()));
            my_counter::__validate_initialize(&[counter.clone(), owner.clone()], &self_account_id, &instruction_data)
                .expect("account validation failed");
            my_counter::initialize(counter, owner).map(|output| output.into_parts())
        }
        // … one arm per instruction
    };
    let parts = match result {
        Ok(output) => output,
        Err(e) => panic!("Program error [{}]: {}", e.error_code(), e),
    };
    ProgramOutput::new(self_account_id, caller_account_id, instruction_data, parts.state_diffs)
        .with_chained_calls(parts.chained_calls)
        .with_block_validity_window(parts.block_validity_window)
        .with_timestamp_validity_window(parts.timestamp_validity_window)
        .write();
}
```

### 3.3 Account parameters and constraints

A parameter whose type's last path segment is `AccountWithMetadata` is an account. The match is by name: the macro sees only syntax, never types. `Vec<AccountWithMetadata>` takes the rest of the accounts. A type named `ProgramContext` is the context. Any other parameter is an instruction arg (`lib.rs:740-773`, *implied*).

| Attribute | Generated runtime check | In the IDL | Notes |
|---|---|---|---|
| `signer` | `accounts[i].is_authorized`, else `Unauthorized` | `signer: true` | The CLI signs with this account and fetches its nonce |
| `init` | `accounts[i].account == Account::default()`, else `AccountAlreadyInitialized`. Implies `mut` | `init: true` | It doesn't claim the account. Ownership comes only when the handler writes non-empty data (v0.2.5). An `init` handler that writes nothing leaves the account default, so a second `init` call passes too (*inferred* from `acquire_ownership_on_data_write` and `tests/e2e/fixture_program`) |
| `mut` | **None** | `writable: true` | IDL only. LEZ's ownership rules are the real check (*implied*) |
| `owner = <expr>` | `accounts[i].account.program_owner == *<expr>`, else `AccountOwnerMismatch`. The expression is evaluated inside `__validate_*`, so `owner = self_account_id` works | In `__program_idl()` only, not in `PROGRAM_IDL_JSON` | Runs first |
| `pda = const("s")` (alias `literal`) | `account_id == compute_pda(self_account_id, seeds)`, else `PdaMismatch` | `pda.seeds[{kind:"const"}]` | The string is zero-padded to 32 bytes, and more than 32 bytes panics (`seed_from_str`) |
| `pda = account("name")` | Uses that account's id as the seed | `{kind:"account", path}` | `"name.id"` is accepted, and the suffix is dropped |
| `pda = arg("name")` | `ToSeed::to_seed(&arg)` | `{kind:"arg", path}` | `ToSeed` exists for `[u8; 32]` (raw), `u32`/`u64` (LE, zero-padded) and `String`/`&str` (zero-padded). Any other arg type doesn't compile (*inferred*) |
| `pda = [s1, s2, ..]` | Several seeds become **`SHA256(s1 ‖ s2 ‖ …)`**, used as the one 32-byte `PdaSeed` | The seed list | A **SPEL convention**. LEZ takes one seed, so every non-SPEL client has to copy the hash (`pda.rs`, `docs/multi-seed-pda.md`) |
| `private_pda, npk = arg(..), vpk = arg(..)` | `compute_private_pda(.., npk, vpk, DEFAULT_PRIVATE_PDA_IDENTIFIER)` | `pda.private: true`, `visibility: ["private"]` | The identifier is always `0`. A non-zero identifier needs hand-written validation (`lib.rs:1415-1420`, *specified* in a comment) |
| `Vec<AccountWithMetadata>` | Count: at least the number of fixed accounts | `rest: true` | The CLI takes a comma-separated list. Validation indexes by **parameter** position, so a `signer` on a rest list would check only its first element (*inferred* from `generate_validation`) |
| The same ident twice | Merged into one transaction account, with the union of `mut`/`signer`, then cloned into every position at the call | Listed once | Exists for embedded extension roles (section 5) |
| `ctx: ProgramContext` | Must be the first parameter, and at most one | Not in the IDL or ABI | `caller_account_id` reads as all zeros for a top-level call |

### 3.4 Handler output

`SpelOutput { state_diffs, chained_calls, block_validity_window, timestamp_validity_window }` is `#[non_exhaustive]`, with builder methods (`spel-framework-core/src/types.rs`).
- `SpelOutput::execute(diffs, calls)` accepts anything that implements `IntoStateDiff`.
- `AccountStateDiff` passes through unchanged.
- **`AccountWithMetadata` becomes `AccountStateDiff::unchanged(acc)`**, meaning "returned untouched" (`spel_output.rs:34-39`).
- Every declared account must be in the output, or LEZ rejects it with `DeclaredAccountMissingFromOutput` (*specified*, doc comment on `execute`).
- The v0.2.4 claim machinery (`AutoClaim`, `execute_with_claims`, and the macro that rewrote `execute(..)` calls in handler bodies) was removed in the port. "The call the author wrote is the call that runs" (`lib.rs:1221-1227`, *specified*).
- **No events.** v0.2.5 `ProgramOutput` has `with_events`, but `SpelOutput` has no events field and the generated `main` never calls it (*implied*).

### 3.5 Errors

`SpelError` has fixed variants with numeric codes: `AccountCountMismatch` 1000, then `InvalidAccountOwner`, `AccountAlreadyInitialized`, `AccountNotInitialized`, `InsufficientBalance`, `DeserializationError`, `SerializationError`, `Overflow`, `Unauthorized`, `PdaMismatch`, and `AccountOwnerMismatch` 1010. `Custom { code }` is reported as `6000 + code` (`error.rs`). All of them end the same way:

- A handler `Err(e)` becomes `panic!("Program error [{code}]: {e}")`.
- A validation failure becomes `.expect("account validation failed")`.
- A wrong account count is a `panic!`.

So nothing structured leaves the guest. On v0.2.5 a guest panic is a chargeable failure: the transaction is **included**, the fee is charged, the nonces advance, and nothing else changes (section 0). `spel` waits only for inclusion and then prints `✅ Transaction confirmed — included in a block.` (`spel-cli/src/tx.rs:765-771`). The error code reaches only the sequencer log, and SPEL's gotchas tell you to read that log (`skills/spel/references/gotchas.md`). Taken together: **a SPEL client can't tell a failed instruction from a successful one** unless it reads the state back. lez-programs found this in practice: "spel reports `Transaction confirmed` even when the program rejected the instruction" (`lez-programs/README.md:199`).

### 3.6 Wire format and instruction identity

- The wire format is borsh: the tag byte is the variant index, followed by the fields. The CLI and client-gen encode from the IDL, using the IDL position as the tag (`lib.rs:1642-1645`, *specified*).
- **The IDL's `discriminator`** is `SHA256("global:<name>")[..8]`, for lssa-lang compatibility (`docs/reference/idl.md`). It never goes on the wire, and dispatch matches only the borsh tag (*implied*). Clients that read it as Anchor's 8-byte selector would get it wrong (*inferred*).
- **External instruction enum.** `#[lez_program(instruction = "core::Instruction")]` expands to `use core::Instruction as Instruction;`. The match arms assume variant and field names that match the fns, so a mismatch fails to compile. The IDL lists instructions in **that enum's** variant order, which was PR #290's fix (`adb46c6`, `d58984a`). A shared core crate lets a caller build `callee_core::Instruction::X { .. }` for a chained call. Across projects that crate has to be a **git** dependency, because the Docker build can't see a `..` path that leaves the project (`docs/reference/macros.md`, *specified*).

### 3.7 The proc-macros do file and process I/O

`#[lez_program]` reads more than its input tokens (*implied*):
- It reads `CARGO_MANIFEST_DIR`, then searches the candidate source paths (`src/bin/*.rs`, `src/*.rs`, `[[bin]]` paths) for the file that defines the module, so it can collect `#[account_type]` items (`lib.rs:465-588`).
- It resolves the crate's dependencies. Path dependencies come from a manifest walk, and git and registry ones from a `cargo metadata --offline` subprocess. It keeps both the transitive set (for IDL types) and the direct set (for extensions) (`dep_walk.rs`).
- It parses the **source files of dependencies**, to find `#[account_type]` items and extension `#[instruction]` fns. A file the consumer's edition can't re-lex is skipped with a warning (`idl_gen/unlexable.rs`).
- `generate_idl!("path")` reads a file relative to the manifest.

Consequences (*inferred*):
- The expansion depends on files that cargo doesn't track for this crate, so the IDL and extension surface can go stale until something forces a rebuild.
- Expansion needs `cargo` and a resolvable lockfile.
- Discovery is by attribute and type **names** in source text, not by resolved types.

The code knows about this: it refuses to compile when embedded markers are present but the module's source file can't be found (`lib.rs:577-585`).

### 3.8 IDL

The top level holds `version`, `name`, `instructions`, `accounts`, `types`, `errors` (always empty), the lssa-lang fields `spec` and `metadata`, and `instruction_type` (`docs/reference/idl.md`).
- **An instruction** has `name` and `accounts[]`. Each account has `name`, `writable`, `signer`, `init`, and optionally `pda.seeds[]` (`kind` is `const`, `account` or `arg`), `pda.private`, `rest` and `visibility`. The instruction also has `args[]` with structured types, and `discriminator`, `execution` and `variant` in `__program_idl()`.
- **Top-level `accounts`** holds the field schemas of `#[account_type]` types, and `types` holds the helper types they reference, collected transitively.
- `spel inspect` decodes account data with these schemas, and so do the generated FFI's `decode_account` and the generated UIs.
- **The producers fill different fields.** `__program_idl()` sets `discriminator`, `execution` and `variant`. The source-parsing generator behind `spel generate-idl` and lez-programs' `tools/idl-gen` sets all three to `None` (`spel-framework-core/src/idl_gen.rs:345-351`). So lez-programs' committed IDLs have no discriminators. They have no PDA seeds either, because no instruction uses `pda =` (*implied*).

### 3.9 The CLI

The CLI builds a public transaction from the IDL (`spel-cli/src/tx.rs`, *implied* unless marked):
- **Args** are parsed per IDL type. The type format table is in the README.
- **Account ids** are taken from flags. PDA accounts are derived and not asked for. The derivation and its seeds are printed.
- **Signers:**
  - every `signer` account;
  - plus each `--co-signer`;
  - plus the fee payer. The payer must sign (`is_fee_authorized`) and defaults to the first signer.

  The CLI fetches nonces for all of them and signs with keys from the wallet (`LEE_WALLET_HOME_DIR`).
- **Fee and gas.** The gas limit is the zkVM cycle budget, not an accounting figure. "Overrun is not an error the caller sees — the guest bails, the full budget is charged" (`tx.rs:614-622`, *specified*). `max_fee = max(DEFAULT_MAX_FEE, (gas_limit + 100_000) × 64)`.
- **Program address.** `-p` accepts a binary (only for the privacy path, since v0.2.5) or an address. `--bin name=path` no longer fills program-id args, because an address isn't in the binary (`tx.rs:92-104`, *specified*).
- **`--dry-run[=json]`** resolves everything and prints it without submitting. It is a global flag and must come before `--`. lez-programs reports that after `--` "it is silently swallowed — the transaction is **submitted for real**" (`docs/token/README.md:482-485`). `--export` and `--co-signer` are explicitly rejected in that position (README).
- **`--export file`** writes a partially signed transaction. `spel sign file` verifies the existing witnesses against the same bytes and adds local signatures, offline. `spel submit file` broadcasts it. A transaction file goes stale when any signer's nonce moves (README, *specified*).
- **`spel deploy bin --fee-payer ..`** creates `ceil(len / MAX_SEGMENT_DATA_LEN)` segment accounts and a header, then prints the header address on stdout, so `ADDR=$(spel deploy …)` works (`deploy.rs`).
- **`spel pda`** derives a PDA from the IDL, or from raw seeds, including private PDAs (`--npk`, `--vpk`, `--identifier`).

### 3.10 Client generation

`spel-client-gen --idl x.json --out-dir gen/` writes three things:
- a typed Rust client (`<p>_client.rs`): one async method per instruction, with the accounts in the right order and PDA helpers;
- a C FFI (`<p>_ffi.rs`): `extern "C"` functions that take and return JSON strings, with the IDL embedded so `decode_account` works at runtime;
- a header (`<p>.h`).

A Qt/QML **Logos Basecamp module** generator (`logos_module_codegen.rs`) writes a backend, a plugin, `Main.qml`, `module.yaml` and `manifest.json`, with a form per instruction. Extension instructions get entry points and forms like native ones (`docs/reference/extensions.md`, *specified*).

### 3.11 Keeping up with LEZ

Two Monday workflows build SPEL against LEZ's latest `main` and open a bump PR (`.github/workflows/lez-compat.yml`, `lez-compat-improved.yml`), and a third publishes a weekly release-candidate tag (`weekly-rc.yml`). The bump rewrites `rev = "<sha>"` pins, but the crates now pin LEZ with `tag = "v0.2.5-rc2"`, so it may no longer change anything (*inferred*). Each LEZ change has been a breaking SPEL release: the `lee` rename, then `vpk`, then the v0.2.5 diffs. The v0.2.5 port touched the macro (claims removed), core (`IntoStateDiff`), the CLI (fees, deploy, borsh) and client-gen.

---

## 4. With and without SPEL

The **without** side is a hand-written v0.2.5-rc2 guest. Where it helps, a **v0.4** version follows, to show which parts of SPEL's work disappear in the actor model. Code marked *written for this doc* has not been compiled.

### 4a. A counter: initialise and increment

**With SPEL.** This is SPEL's tutorial program (`docs/tutorial.md`, Step 2), with one change: the writes return `AccountStateDiff::new(..)`. The tutorial, the README and the `spel init` template write `acc.account.data = …` and then return `acc`. That idiom predates v0.2.5 and no longer works (section 6.1).

```rust
#![no_main]
use spel_framework::prelude::*;
risc0_zkvm::guest::entry!(main);

#[account_type]
#[derive(Debug, Clone, Default, BorshSerialize, BorshDeserialize)]
pub struct CounterState { pub count: u64, pub owner: [u8; 32] }

#[lez_program]
mod my_counter {
    #[allow(unused_imports)]
    use super::*;

    #[instruction]
    pub fn initialize(
        #[account(init, pda = literal("counter"))] counter: AccountWithMetadata,
        #[account(signer)] owner: AccountWithMetadata,
    ) -> SpelResult {
        let state = CounterState { count: 0, owner: *owner.account_id.value() };
        let data = encode(&state)?;
        Ok(SpelOutput::execute(
            vec![AccountStateDiff::new(counter, BalanceDiff::Add(0), data), AccountStateDiff::unchanged(owner)],
            vec![],
        ))
    }

    #[instruction]
    pub fn increment(
        #[account(mut, pda = literal("counter"))] counter: AccountWithMetadata,
        #[account(signer)] owner: AccountWithMetadata,
        amount: u64,
    ) -> SpelResult {
        let mut state: CounterState = borsh::from_slice(counter.account.data.as_ref())
            .map_err(|e| SpelError::DeserializationError { account_index: 0, message: e.to_string() })?;
        if *owner.account_id.value() != state.owner {
            return Err(SpelError::Unauthorized { message: "only the owner can increment".into() });
        }
        state.count = state.count.checked_add(amount)
            .ok_or(SpelError::Overflow { operation: "increment".into() })?;
        let data = encode(&state)?;
        Ok(SpelOutput::execute(
            vec![AccountStateDiff::new(counter, BalanceDiff::Add(0), data), AccountStateDiff::unchanged(owner)],
            vec![],
        ))
    }

    fn encode(state: &CounterState) -> Result<Data, SpelError> {
        let bytes = borsh::to_vec(state).map_err(|e| SpelError::SerializationError { message: e.to_string() })?;
        Data::try_from(bytes).map_err(|_| SpelError::custom(1, "state too large"))
    }
}
```

What you get without writing more: `spel increment --owner <id> --amount 5`, with the counter PDA derived, the owner signed for, the fee payer added; `spel inspect <pda> --type CounterState`; and an IDL with both instructions.

**Without SPEL, v0.2.5-rc2.** *Written for this doc*, following LEZ's own `examples/program_deployment/methods/guest/src/bin/hello_world_with_authorization.rs` at `v0.2.5-rc2`:

```rust
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{Account, AccountId, AccountWithMetadata, BalanceDiff, Data},
    program::{read_lee_call, respond_unsupported_call, AccountStateDiff, PdaSeed, ProgramCall, ProgramInput, ProgramOutput},
};

#[derive(BorshSerialize, BorshDeserialize)]
enum Instruction { Initialize, Increment { amount: u64 } }   // append-only: the tag is the index

#[derive(BorshSerialize, BorshDeserialize)]
struct CounterState { count: u64, owner: [u8; 32] }

fn counter_seed() -> PdaSeed {
    let mut s = [0u8; 32];
    s[..7].copy_from_slice(b"counter");
    PdaSeed::new(s)
}

fn main() {
    let call = read_lee_call::<Instruction>();
    let ProgramCall::Execute(ProgramInput { self_account_id, caller_account_id, pre_states, instruction }, instruction_data)
        = call else { respond_unsupported_call(call) };

    let [counter, owner]: [AccountWithMetadata; 2] =
        pre_states.try_into().unwrap_or_else(|_| panic!("expected [counter, owner]"));
    assert_eq!(counter.account_id, AccountId::for_public_pda(&self_account_id, &counter_seed()), "counter PDA");
    assert!(owner.is_authorized, "owner must sign");

    let state = match instruction {
        Instruction::Initialize => {
            assert!(counter.account == Account::default(), "already initialised");
            CounterState { count: 0, owner: *owner.account_id.value() }
        }
        Instruction::Increment { amount } => {
            let mut s: CounterState = borsh::from_slice(counter.account.data.as_ref()).expect("counter state");
            assert_eq!(s.owner, *owner.account_id.value(), "only the owner can increment");
            s.count = s.count.checked_add(amount).expect("overflow");
            s
        }
    };
    let data: Data = borsh::to_vec(&state).unwrap().try_into().expect("fits");

    ProgramOutput::new(self_account_id, caller_account_id, instruction_data, vec![
        AccountStateDiff::new(counter, BalanceDiff::Add(0), data),
        AccountStateDiff::unchanged(owner),
    ])
    .write();
}
```

The guest is about the same length. The difference is off-chain. Without SPEL there is no IDL, no CLI and no decoder, so a client has to rebuild the account order, the borsh layout, the PDA seed padding, the signer list, nonces and the fee by hand.

**v0.4 (`dev`).** *Written for this doc.* Nothing in this program sees accounts. The counter is this program's actor state on a PDA. A message sent straight to a PDA actor has no sender and isn't authorized, so the owner speaks through a **seat**: this program's actor on the owner's own account. It is the pattern of [`example_progs/admin_seat.rs`](example_progs/admin_seat.rs).

```rust
use lee_core::{account::{AccountId, Actor}, program::{PdaSeed, ReceiveInput, Response}};

#[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
enum Message {
    Initialize,                          // to your seat, signed by you
    Increment(u64),                      // to your seat, signed by you
    Apply { owner: AccountId, op: Op },  // seat → counter PDA only
}
#[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
enum Op { Init, Add(u64) }

#[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
struct CounterState { count: u64, owner: AccountId }

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, message: Message) -> Response {
    match message {
        Message::Initialize => relay(input, Op::Init),
        Message::Increment(n) => relay(input, Op::Add(n)),
        Message::Apply { owner, op } => apply(input, owner, op),
    }
}

/// On the user's seat: the signer check, then a sync message to the counter PDA.
fn relay(input: &ReceiveInput, op: Op) -> Response {
    // "signer": the receiver's own key signed, and no program relayed it
    assert!(input.is_authorized && input.from.is_none(), "sign for your own seat");
    let owner = input.receiver.account_id;
    Response::keep_state().send_sync(counter(input), &Message::Apply { owner, op })
}

/// On the counter PDA: accept only a seat of this program, then do the work.
fn apply(input: &ReceiveInput, owner: AccountId, op: Op) -> Response {
    let program = input.receiver.program_account_id;
    // the engine fills `from`, so this cannot be forged
    assert_eq!(input.from, Some(Actor::new(owner, program)), "only a seat of this program applies");
    assert_eq!(input.receiver, counter(input), "only the counter PDA holds state");
    let next = match op {
        Op::Init => {
            assert!(input.pre_state.is_empty(), "already initialised"); // "init"
            CounterState { count: 0, owner }
        }
        Op::Add(n) => {
            let mut s: CounterState = borsh::from_slice(&input.pre_state).expect("counter state");
            assert_eq!(s.owner, owner, "only the owner can increment");
            s.count = s.count.checked_add(n).expect("overflow");
            s
        }
    };
    Response::set_state(borsh::to_vec(&next).unwrap())
}

/// SPEL's `pda = const("counter")`, derived from this program's deployed address.
fn counter(input: &ReceiveInput) -> Actor {
    let program = input.receiver.program_account_id;
    let mut seed = [0u8; 32];
    seed[..7].copy_from_slice(b"counter");
    Actor::new(AccountId::for_public_pda(&program, &PdaSeed::new(seed)), program)
}
```

What carried over from SPEL's job list:
- **`signer`** became `is_authorized && from.is_none()` on the seat.
- **`init`** became `pre_state.is_empty()`.
- **The PDA check** became deriving the PDA from `receiver.program_account_id`.
- **The owner check** became checking `from` against the actor you expect.

What vanished: account lists and their order, returning every account, ownership rules and the `owner` constraint. What became more work: one user operation is now **two messages**. The client also has to declare both actors in the transaction and attach `PublicAccountEvidence::Pda { program, seed }` while the PDA is new.

A simpler v0.4 design is often better. Drop the PDA and keep each user's counter as this program's actor state on the user's own account. There `input.is_authorized` is the whole access check, because every program owns its own slot on every account.

### 4b. lez-programs: token `transfer`

lez-programs splits each program into three crates:
- **`*_core`** holds the borsh `Instruction` enum, the state types and the PDA helpers.
- **`programs/<p>/src`** holds plain handler fns. They depend only on `lee_core` and the cores.
- **`methods/guest`** is the SPEL guest. Each `#[instruction]` there is a thin wrapper that calls one handler.

**With SPEL.** The guest wrapper (`programs/token/methods/guest/src/bin/token.rs:22-34`):

```rust
#[lez_program(instruction = "token_core::Instruction")]
mod token {
    // …
    /// Transfer tokens from sender to recipient.
    /// Fresh public recipients must be explicitly authorized in the same transaction.
    #[instruction]
    pub fn transfer(
        #[account(mut, signer)] sender: AccountWithMetadata,
        #[account(mut)] recipient: AccountWithMetadata,
        amount_to_transfer: u128,
    ) -> SpelResult {
        Ok(spel_framework::SpelOutput::execute(
            token_program::transfer::transfer(sender, recipient, amount_to_transfer),
            vec![],
        ))
    }
}
```

The handler doesn't use SPEL (`programs/token/src/transfer.rs`, trimmed). It repeats the signer check that SPEL already generated, because it is the unit under test and must stand alone:

```rust
pub fn transfer(sender: AccountWithMetadata, recipient: AccountWithMetadata, balance_to_move: u128)
    -> Vec<AccountStateDiff>
{
    assert!(sender.is_authorized, "Sender authorization is missing");
    let mut sender_holding = TokenHolding::try_from(&sender.account.data).expect("Invalid sender data");
    let mut recipient_holding = if recipient.account == Account::default() {
        TokenHolding::zeroized_clone_from(&sender_holding)
    } else {
        TokenHolding::try_from(&recipient.account.data).expect("Invalid recipient data")
    };
    // … definition-id check, checked_sub / checked_add per holding kind …
    vec![
        AccountStateDiff::new(sender, BalanceDiff::Add(0), Data::from(&sender_holding)),
        AccountStateDiff::new(recipient, BalanceDiff::Add(0), Data::from(&recipient_holding)),
    ]
}
```

**Without SPEL, v0.2.5-rc2.** *Written for this doc.* The handler and the core crate stay as they are, and only the guest changes:

```rust
use lee_core::program::{read_lee_call, respond_unsupported_call, ProgramCall, ProgramInput, ProgramOutput};
use token_core::Instruction;

fn main() {
    let call = read_lee_call::<Instruction>();
    let ProgramCall::Execute(ProgramInput { self_account_id, caller_account_id, pre_states, instruction }, instruction_data)
        = call else { respond_unsupported_call(call) };

    let (diffs, calls) = match instruction {
        Instruction::Transfer { amount_to_transfer } => {
            let [sender, recipient] = pre_states.try_into().expect("Transfer takes [sender, recipient]");
            (token_program::transfer::transfer(sender, recipient, amount_to_transfer), vec![])
        }
        // … one arm per variant (10 for token)
    };
    ProgramOutput::new(self_account_id, caller_account_id, instruction_data, diffs)
        .with_chained_calls(calls)
        .write();
}
```

In this codebase SPEL saves the dispatch arms and gives you the IDL (`artifacts/token-idl.json`) that the `spel` CLI and `spel inspect` run on. The account checks it generates are duplicated by the handlers.

### 4c. lez-programs: ATA `transfer`, a chained call under PDA authority

**With SPEL.** The guest wrapper (`programs/ata/methods/guest/src/bin/ata.rs:48-70`) takes `ProgramContext` for the program's own address:

```rust
#[instruction]
pub fn transfer(
    ctx: ProgramContext,
    #[account(signer)] owner: AccountWithMetadata,
    #[account(mut)] sender_ata: AccountWithMetadata,
    #[account(mut)] recipient: AccountWithMetadata,
    token_program_id: AccountId,
    amount: u128,
) -> SpelResult {
    let (post_states, chained_calls) = ata_program::transfer::transfer_from_associated_token_account(
        owner, sender_ata, recipient, ctx.self_account_id, token_program_id, amount,
    );
    Ok(spel_framework::SpelOutput::execute(post_states, chained_calls))
}
```

The handler (`programs/ata/src/transfer.rs:7-76`, trimmed) works in four steps:
1. It checks authorization and the sender ATA's owner.
2. It **derives and checks the PDA by hand**, using its own seed scheme, `SHA-256(token_program ‖ owner ‖ definition)` (`ata_core::compute_ata_seed`).
3. It reports every account unchanged.
4. It hands the balance change to the token program, granting it the ATA's seed:

```rust
assert!(owner.is_authorized, "Owner authorization is missing");
assert_eq!(sender_ata.account.program_owner, token_program_id, "Sender ATA must be owned by expected token program");
let sender_definition_id = TokenHolding::try_from(&sender_ata.account.data)
    .expect("Sender ATA must hold a valid token").definition_id();
let sender_seed = ata_core::verify_ata_and_get_seed(
    &sender_ata, &owner, token_program_id, sender_definition_id, ata_program_id);
// … recipient checks: initialised, owned by the token program, same definition …

let state_diffs = vec![
    AccountStateDiff::unchanged(owner),
    AccountStateDiff::unchanged(sender_ata),
    AccountStateDiff::unchanged(recipient),
];
// `pda_seeds` is what authorizes the callee over the sender ATA — the call carries
// account ids, not pre-states, so there is no flag to set.
let chained_call = ChainedCall::new(
    token_program_id,
    vec![sender_ata_id, recipient_id],
    &token_core::Instruction::Transfer { amount_to_transfer: amount },
)
.with_pda_seeds(vec![sender_seed]);
(state_diffs, vec![chained_call])
```

The caller builds the callee's instruction from **`token_core::Instruction`**. That is why the enum lives in a core crate and the guest names it with `instruction = "..."`.

**Without SPEL, v0.2.5-rc2.** Same shape as 4b: a `match` arm that destructures `[owner, sender_ata, recipient]` and calls the same handler. LEZ's own minimal form of this pattern is `examples/program_deployment/methods/guest/src/bin/tail_call_with_pda.rs` at `v0.2.5-rc2`. It returns the account unchanged, adds `ChainedCall { program_account_id, instruction_data, pre_state_ids: vec![id], pda_seeds: vec![PDA_SEED] }`, then calls `ProgramOutput::new(..).with_chained_calls(..).write()`.

**v0.4 (`dev`).** *Inferred, not built.* The token, AMM and ATA programs were removed from `dev` (commit `c16cf3000`), so there is no v0.4 code to compare with. The call becomes a sync message to the token program's actor on the sender ATA, granting the seed: `SyncMessage::new(Actor::new(sender_ata_id, token_program_id), &token::Message::Transfer { .. }).with_pda_seeds([sender_seed])` (pattern G in [GUIDE_v0.2_v0.4.md](GUIDE_v0.2_v0.4.md)). The ATA program also stops reading `sender_ata.account.program_owner` and the token program's bytes, because a v0.4 handler can't see them. It would have to ask the token program, or trust the message flow. And in v0.4 every program already owns a slot on every account, so a token program could keep each user's holding as its own actor state on the user's account. An ATA is then no longer needed just to give a holding an address.

### 4d. What lez-programs uses, and what it doesn't

From lez-programs `main` at `071375c`, across six programs (token, amm, ata, stablecoin, twap_oracle, token_mint_authority) with 50 instructions.

| SPEL feature | Used? | Detail |
|---|---|---|
| `#[lez_program(instruction = "<p>_core::Instruction")]` | All six guests | Core enums are borsh-only and append-only. The comment on each one is a history lesson: generated enums used to derive serde as well, which "silently rotted every instruction-encoding site in `modules/*/ffi`" during the v0.2.5 port (`programs/token/core/src/lib.rs:8-25`). SPEL `main` now derives only borsh |
| `#[account(mut / signer / init)]` | Yes | 75 `mut`, 30 `signer`, 17 `init`, 17 `mut, signer`, 7 `init, signer`. The handlers repeat the signer and init checks |
| `ProgramContext` | 45 of 50 | Only `ctx.self_account_id` is read |
| `SpelOutput::execute` | 50 | `.with_timestamp_validity_window(..deadline)` on 5 AMM handlers |
| `#[account_type]` | 5 of 6 core crates | The only reason the cores depend on `spel-framework-macros`. It feeds `spel inspect --type`. The IDL generator also picks up types from dependencies, so the ATA IDL includes the token types |
| Writes as `AccountStateDiff::new(pre, BalanceDiff::Add(0), data)` | Every write | No handler mutates `acc.account.data` and returns the account, so they avoid the section 6.1 pitfall |
| `pda = ..` | **Never** | "Program PDAs use a **SHA-256 seed scheme** (in each `*_core`), which is **not** what `spel pda` computes (that pads raw bytes) — so `spel pda` will give wrong addresses" (`docs/testnet-runbook.md:270-272`). The schemes hash single literals, append unpadded tags, sort a token pair, or use a 36-byte tag. SPEL's seed grammar can't express any of that. Each program ships an `examples/*_pdas` helper instead |
| `owner = ..`, `private_pda`, rest accounts, `SpelError` | Never | Handlers fail with `assert!`/`expect` |
| Extensions | Never | Stablecoin keeps `admin_account_id` and `freeze_authority_account_id` inline in `ProtocolParameters` and checks them by hand (`programs/stablecoin/src/checks.rs:100-109`). The spec calls this a deliberate choice that "adapts trivially to RFP-001 / RFP-002 when they land" (`docs/stablecoin/README.md:83`) |
| `spel generate-idl` / `idl_gen` | Yes, through `tools/idl-gen` | `make idl` writes `artifacts/<p>-idl.json`, and CI regenerates and diffs it. The tool sorts `types` itself, because the order isn't deterministic |
| `spel` CLI | Yes | Calls, `--fee-payer`, `inspect`. The deploy and testnet scripts poll `spel inspect` after each call, because "confirmed" can be wrong in **both** directions: confirmed when the instruction reverted (README:199), and "NOT confirmed" when it landed after spel gave up waiting (`scripts/deploy-stablecoin.sh:156-159`) |
| `spel deploy`, `spel-client-gen`, `generate_idl!`, test utils | Never | Deploys use `wallet program-loader deploy`. The FFI (`modules/*/ffi`) is hand-written, because the IDL couldn't describe some instructions (`modules/token/README.md`) |

Limits the repo documents:
- **No `defined` types.** The CLI's serializer has no encoding for IDL `defined` types, so "`spel` cannot currently create NFT definitions" (`docs/token/README.md:372-381`).
- **No optional accounts or args.** That is why authority operations come in pairs, such as `mint` / `mint_with_authority` (`docs/token/README.md:155-158, 475-478`).
- **A misplaced `--dry-run` submits for real.** After `--` it "is silently swallowed — the transaction is **submitted for real**" (`docs/token/README.md:482-485`).
- **No clock on `ProgramContext`.** Programs take the clock as an account and check its id by hand (`docs/stablecoin/README.md:415-444`).
- **Old crate names.** The macro emits `nssa_core::` paths, so every guest declares `nssa_core = { …, package = "lee_core" }`.
- **Lint suppressions for macro output.** For example `clippy::cloned_ref_to_slice_refs`, "SPEL macro emits cloned validation slices for one-account instructions" (`token.rs`, `amm.rs`).

What lez-programs does with SPEL, then:
- **It uses:** the dispatcher, the external-enum binding, the IDL (types and account roles only), the generic CLI and `inspect`.
- **It works around or avoids:** PDAs, errors, client generation and deployment.
- **Its handlers** are framework-free fns that return `AccountStateDiff`s, so they can be unit-tested without SPEL.

---

## 5. Extensions, with spel-admin-authority as the worked case

An extension is an ordinary crate that a program depends on and switches on with a marker attribute. Once active it can:
- add whole instructions to the program's dispatcher and IDL;
- inject account parameters into instructions that carry its gate attribute;
- wrap every instruction with a check;
- keep its state inside one of the program's own accounts ("embedded mode").

SPEL's docs: `README.md` ("Extension Libraries"), `docs/reference/extensions.md`, `docs/extension-trust-model.md`. The implementation is in `spel-framework-core/src/extension.rs`, `extension/{metadata,inject,marker}.rs`, `dep_walk.rs` and `spel-framework-macros/src/slot_offsets.rs`.

**Status: the extensions are a release behind SPEL `main`.** `spel-admin-authority` and `spel-freeze-authority` pin `spel-framework` **tag `v0.7.0`** (`4107dcb`, LEZ **v0.2.4**). Their instructions return `(Account, AutoClaim)` tuples (`AutoClaim::Claimed(Claim::Pda(..))`), and their tests import `nssa_core::program::{AccountPostState, validate_execution}`. All of these were removed by the v0.2.5 port on `main`. So they describe the mechanism as designed for v0.2.4, and wouldn't build against `c6f73ce` until ported (*inferred*: the types they use no longer exist there). lez-programs uses neither extension. Commits read: admin-authority `027197f` (2026-10-16, one commit past tag v0.2.0), freeze-authority `959c0cf` (2026-10-14), and their shared primitive `mmlado/spel-authority` `2902dc7` (2026-10-14).

### 5.1 How the framework runs an extension

Activation takes two actions, both by the consumer (`docs/extension-trust-model.md`, *specified*):
1. The extension is in the consumer's **own** `[dependencies]`. Discovery reads direct dependencies only, never transitive ones. Path dependencies are found by walking manifests, and git and registry dependencies by `cargo metadata --offline`.
2. Its marker attribute is on the `#[lez_program]` module, **below** `#[lez_program]`. Above it, the marker is consumed first and never seen, which the framework detects and reports.

A marker with no matching dependency is a compile error. A dependency with no marker does nothing. Generated call paths use the dependency's `[package].name`, never its directory name.

Everything an extension does is declared in its `Cargo.toml`:

| Metadata | Effect | Basis |
|---|---|---|
| `[package.metadata.spel] extension_attr = "x"` | Names the marker `#[x]`. The framework parses the crate's `src/lib.rs` **source** for `#[instruction]` fns and adds them as instructions. The dispatcher calls `::crate_name::fn(..)` (`lib.rs:302-325`) | specified |
| `[[inject]] wrapper = "gate"` with `[[inject.account]] name, seed, signer` | On any instruction that carries the bare `#[gate]`, synthesise those account parameters unless the fn already declares them. They go after a leading `ProgramContext`, in block order. When several extensions inject, the order of their markers on the module decides. The injected accounts are part of the ABI and appear in the IDL | specified |
| `[wrap_instructions] wrapper, skip, self_exempt_marker, exempt` | Put `wrapper` on **every** dispatched instruction, the consumer's own and other extensions' alike. A fn carrying `self_exempt_marker` is skipped, and so is any instruction named in `exempt`, by its qualified name. The `skip` word on the marker (`#[x(manual)]`) turns wrapping off | specified |
| `[embedded] skip, state_type` | Embedded mode. The consumer writes `#[x(role = account_ident, offset = N)]`. Each role's inject entry is re-pointed at the consumer's account, with the constraint copied from that account's `init` declaration, minus `init`/`mut`. The instructions listed in `skip` (the initialiser) are not emitted | specified |
| `[[bound_args]] arg, from, default` | A **trailing** fn parameter (for example `offset: usize`) is removed from the ABI and IDL, and the dispatcher fills it with a compile-time literal: the marker's kwarg, a peer marker's kwarg (`from = "peer::offset"`), or the default. "A caller-supplied offset would be a caller-controlled write location" | specified |
| `#[account_type]` struct field `#[<name>_slot]` | The struct gains a `<NAME>_OFFSET` const, computed as a sum of `FixedBorshSize::SIZE`, plus a generated layout test. `#[lez_program]` asserts at compile time that each marker's `offset` equals that const | specified |
| `FixedBorshSize for state_type` | `#[lez_program]` emits a const assert for every pair of embeds in one account, so overlapping windows don't compile. Windows that only touch are fine | specified |

**When does extension code run?** `#[lez_program]` expands first, because it is the outer attribute. It rewrites the module, merges and injects, and emits each handler **with the gate attribute still on it**. Then rustc expands each gate, a proc-macro from the extension's macros crate, on the emitted fn. By that point the injected parameters exist, so the gate can refer to them by name. Gates rewrite the body, and markers expand to nothing: "The framework strips nothing" (README, *specified*). That forces an extension to be **two crates**: a `proc-macro = true` crate for the marker and gates, and a library that re-exports them along with `pub use spel_framework::instruction;`. It also needs `extern crate self as <name>;`, so that `::name::Type` resolves inside the library too.

**The failure modes are deliberate** (*specified*):
- Malformed metadata, two instructions with the same name, a marker on the wrong side of `#[lez_program]`, a marker with no matching dependency, overlapping windows, and a marker offset that disagrees with `<NAME>_OFFSET` are all compile errors.
- Environmental problems are warnings: an unreadable manifest, or an extension that contributes nothing.
- `generate_idl!` throws those warnings away (`docs/reference/extensions.md`, "Gotchas").

### 5.2 spel-admin-authority

**Crates.**
- `admin-authority`: the library, `src/{lib,config,errors,genesis}.rs`.
- `admin-authority-macros`: the proc-macros.
- Three samples: dedicated, manual and embedded, with trybuild compile-fail tests.
- 13 ADRs under `docs/adr/`.

The slot logic lives in `spel-authority` (`AuthoritySlot`), which freeze-authority also uses.

**State.** The admin config is one 64-byte slot:

```rust
// spel-authority/src/lib.rs
#[account_type]
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct AuthoritySlot {
    holder: AccountId,   // bytes 0..32
    pending: AccountId,  // bytes 32..64, a nominee for the two-step handover
}

// admin-authority/src/config.rs
#[account_type]
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct AdminConfig { slot: AuthoritySlot }

impl spel_framework::FixedBorshSize for AdminConfig { const SIZE: usize = 64; }
```

`AuthoritySlot::read_at(data, offset)` and `write_at` slice `account.data[offset..offset + 64]`. `assert(signer)` checks three things, in order:
1. The slot isn't renounced (the holder isn't all zeros).
2. `signer.is_authorized`.
3. `signer.account_id == holder`.

Dedicated mode stores the config at the PDA `(program, "admin_config")`.

**Metadata** (`admin-authority/Cargo.toml`, verbatim):

```toml
[package.metadata.spel]
extension_attr = "admin_authority"

[[package.metadata.spel.inject]]
  wrapper = "admin_initialize"

[[package.metadata.spel.inject.account]]
  name = "admin_config"
  seed = [{ const = "admin_config" }]

[[package.metadata.spel.inject.account]]
  name = "caller"
  signer = true

[[package.metadata.spel.inject]]
wrapper = "require_admin"

  [[package.metadata.spel.inject.account]]
  name = "admin_config"
  seed = { const = "admin_config" }

  [[package.metadata.spel.inject.account]]
  name = "caller"
  signer = true

[package.metadata.spel.embedded]
skip = ["admin_initialize"]
state_type = "admin_authority::AdminConfig"

[[package.metadata.spel.bound_args]]
arg = "offset"
from = "offset"
default = 0
```

**Contributed instructions.** There are five: `admin_initialize`, `admin_transfer`, `admin_nominate`, `admin_accept` and `admin_renounce`. A test keeps `pub const INSTRUCTIONS` in step with them. Two are quoted here:

```rust
// admin-authority/src/lib.rs (SPEL v0.7.0 / LEZ v0.2.4 claim API)
#[instruction]
pub fn admin_initialize(
    #[account(init, pda = literal("admin_config"))] mut config: AccountWithMetadata,
    #[account(signer)] caller: AccountWithMetadata,
) -> SpelResult {
    require_genesis(&caller)?;
    AdminConfig::bootstrap(&mut config, AdminCandidate::Signer, &caller)?;
    Ok(SpelOutput::execute(
        vec![
            (config.account, AutoClaim::Claimed(Claim::Pda(PdaSeed::new(seed_from_str("admin_config"))))),
            (caller.account, AutoClaim::None),
        ],
        vec![],
    ))
}

#[instruction]
pub fn admin_transfer(
    #[account(mut, pda = literal("admin_config"))] mut admin_config: AccountWithMetadata,
    #[account(signer)] caller: AccountWithMetadata,
    new_account: AccountWithMetadata,
    candidate: ::admin_authority::AdminCandidate,
    offset: usize,                                   // bound arg: not in the IDL, filled by the dispatcher
) -> SpelResult {
    AdminConfig::perform_transfer_at(&mut admin_config, offset, &caller, candidate, &new_account)?;
    Ok(SpelOutput::execute(
        vec![admin_config.account, caller.account, new_account.account],
        vec![],
    ))
}
```

- **Who becomes the first admin.** `admin_initialize` installs whoever calls it first. A LEZ deployment carries no instructions and no deployer identity, so that is a front-running race. The fix is an optional compile-time key: `SPEL_ADMIN_GENESIS` (`genesis.rs`, `option_env!`). It has to be set in `.cargo/config.toml` `[env]`, because the Docker build ignores the shell environment.
- **Transfer needs consent.** `candidate: AdminCandidate` is either `Signer` or `Pda { program_id, seed }`. `new_account` is the evidence for it:
  - A `Signer` candidate must be `is_authorized` and owned by some program (`UnownedSigner` otherwise).
  - A `Pda` candidate must sit at the derived address and already be deployed.

  `new_account` has no `#[account(signer)]`. The co-signature is checked at runtime, in `AuthorityCandidate::validate`. The new admin's signature comes from SPEL's `--co-signer` / `--export` / `spel sign` / `spel submit` flow:

  ```bash
  spel --idl program-idl.json --program <program-id> \
    --co-signer <new-admin> --export handover.json -- \
    admin-transfer --caller <current-admin> --new-account <new-admin> --candidate Signer
  spel sign handover.json     # new admin, offline
  spel submit handover.json
  ```

- **Two-step handover.** The `--export` file goes stale as soon as any signer's nonce moves, so ADR-0011 adds a second path: `admin_nominate(new_admin)`, signed by the admin alone, then `admin_accept()`, signed by the nominee. A PDA nominee accepts through a chained call from its owning program.
- **Renounce is final.** "Any recovery path would also be an exploit path."

**The gate.** `#[require_admin]` takes the kwargs `admin_config=`, `caller=` and `offset=`, and prepends one block to the handler (`admin-authority-macros/src/lib.rs:209-213`):

```rust
let prologue: syn::Stmt = parse_quote! {{
    let __admin_cfg = ::admin_authority::AdminConfig::from_account_at(&#config_ident, #offset)?;
    __admin_cfg.assert_admin(&#caller_ident)?;
}};
func.block.stmts.insert(0, prologue);
```

The errors, as `SpelError::custom(code)`, reported as `6000 + code`, are:
- `NotInitialized` (101)
- `Renounced` (102)
- `NotAdmin` (103)
- `MissingSignature` (104)
- `SlotOutOfBounds` (111)

**The gate checks neither the account's owner nor its address.** It trusts that `admin_config` is the right account, because the injected `pda` constraint runs first in `__validate_*`. admin-authority's README warns that a module with no marker, or a fn whose config account was declared without `pda = ..`, "a caller can pass an account that names them admin, and the gate passes". Open issue admin #27 ("Gate refuses to expand unless the framework checked it", which needs logos-co/spel#294) is about closing that.

**Embedded mode.** In embedded mode the consumer keeps the admin slot inside its own config account. Its own `init` instruction carries `#[admin_initialize]`. In embedded mode that attribute splices `AdminConfig::bootstrap_at(&mut config, OFFSET, Signer, &caller)?;` before the fn's tail expression. It refuses to expand without a framework-stamped offset, because dedicated mode has the contributed `admin_initialize` instead.

### 5.3 spel-freeze-authority, stacked on admin

Freeze adds a program-wide freeze flag and per-account freezes. It is the case that exercises auto-wrap, peer bound args, and two embeds in one account.

```toml
# freeze-authority/Cargo.toml (excerpt)
[package.metadata.spel.wrap_instructions]
wrapper = "freeze_authority::require_not_frozen"
skip = "manual"
self_exempt_marker = "freeze_exempt"
exempt = [
  "admin_authority::admin_initialize", "admin_authority::admin_transfer",
  "admin_authority::admin_nominate", "admin_authority::admin_accept", "admin_authority::admin_renounce",
]

[[package.metadata.spel.inject]]
wrapper = "require_not_frozen"
  [[package.metadata.spel.inject.account]]
  name = "freeze_config"
  seed = { const = "freeze_config" }
  [[package.metadata.spel.inject.account]]
  name = "freeze_account"
  seed = [{ const = "frozen" }, { account = "caller" }]
  [[package.metadata.spel.inject.account]]
  name = "caller"
  signer = true

# Block order is load-bearing: offset first, admin_offset second.
[[package.metadata.spel.bound_args]]
arg = "offset"
from = "offset"
default = 0
[[package.metadata.spel.bound_args]]
arg = "admin_offset"
from = "admin_authority::offset"
default = 0
```

- **The state.** `FreezeConfig { slot: AuthoritySlot, is_frozen: bool }` is 65 bytes. Per-account freezes are a `FrozenAccountState { is_frozen }` at the PDA `(program, "frozen", target)`.
- **The gate.** `#[require_not_frozen]` prepends two checks. A strict decode of `freeze_config` fails on `is_frozen`. A lenient decode of the caller's `frozen` PDA treats empty data as unfrozen.
- **Wrapping.** Every dispatched instruction gets the gate, except freeze's own instructions (`#[freeze_exempt]`, a no-op marker that the framework reads) and admin's five, which are listed by **name** in `exempt`. So freeze hard-codes another crate's instruction names, and open issue freeze #13 asks for a test that the two lists match.
- **Reading admin's state.** `freeze_authority_transfer` uses `admin_offset`, which is bound from **admin's** marker, so it can read admin's state without depending on where admin embedded it.

Both extensions embedded in one account (`freeze-authority-sample-embedded/src/main.rs`):

```rust
#[account_type]
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default)]
pub struct ProgramConfig {
    pub value: u64,
    pub padding: [u8; 24],
    #[admin_slot]  pub admin: AdminConfig,    // ADMIN_SLOT_OFFSET = 32
    #[freeze_slot] pub freeze: FreezeConfig,  // FREEZE_SLOT_OFFSET = 96
}

#[lez_program]
#[admin_authority(admin_config = config, offset = 32)]
#[freeze_authority(freeze_config = config, offset = 96)]
mod freeze_authority_sample_embedded {
    use admin_authority::require_admin;

    #[admin_initialize]       // splices the admin bootstrap; `caller` is injected
    #[instruction]
    pub fn initialize(
        #[account(init, pda = literal("program_config"))] mut config: AccountWithMetadata,
    ) -> SpelResult { /* write ProgramConfig defaults */ Ok(SpelOutput::execute(vec![config], vec![])) }

    #[require_admin]          // auto-wrapped with require_not_frozen as well
    #[instruction]
    pub fn update_value(
        #[account(mut, pda = literal("program_config"))] mut config: AccountWithMetadata,
        new_value: u64,
    ) -> SpelResult { /* … */ }
}
```

The handler for `update_value` declares one account. Its IDL entry has three: `caller` (signer) and `freeze_account` (PDA `[frozen, caller]`) are injected, and `config` is merged, because both roles resolve to it (from the samples' dry-runs).

### 5.4 What the extensions rely on in v0.2

Extension code is compiled into the consumer, so every account it writes belongs to the consumer program: the config PDAs, the frozen PDAs and the embedding account. That part fits "a program writes only its own state". What doesn't fit:

1. **Reading other programs' accounts.**
   - `AuthorityCandidate::validate` reads a candidate's `program_owner` and compares its whole `Account` with the default. The candidate is a wallet account or another program's PDA.
   - `AuthoritySlot::accept` checks that the signer's `program_owner` isn't the default.
2. **Claiming.**
   - `AutoClaim::Claimed(Claim::Pda(..))` on initialise.
   - `AutoClaim::pda_from_seeds` the first time a frozen PDA is touched.
   - ADR-0008: nothing can be closed, because `Claim::Pda` needs a default pre-state and ownership never goes back to the default.
3. **One account list per instruction.**
   - Every input is passed through as a whole post-state, paired by position, including accounts the program doesn't own (`caller`, `new_account`).
   - `post_state_pair` folds an account that is both read and written into one entry, for LEZ's duplicate-id rule.
4. **Byte windows in `account.data`.**
   - Embedded mode works because the gate can see the embedding account's raw bytes.
   - The slot's offset is compiled into the program.
5. **Authorization from the witness set.**
   - `is_authorized` on a passed-in account, meaning "this key signed".
   - A PDA authority signs through a chained call with seeds.

Section 7 covers what these become under v0.4.

---

## 6. Other things worth knowing, and trade-offs

### 6.1 Docs lag the v0.2.5 port

The port changed the code. These docs and skills still describe v0.2.4 behaviour (*implied*, from grepping `AutoClaim|execute_with_claims`):
- `README.md`
- `docs/tutorial.md`
- `docs/reference/{types,macros,extensions}.md`
- `skills/spel/SKILL.md` and `skills/spel/references/{quickstart,gotchas,extensions}.md`

| Docs say | Code at `c6f73ce` |
|---|---|
| `execute(vec![acct, …])`, and "the macro derives the claim" | No claims exist. `execute` turns a bare account into `AccountStateDiff::unchanged` |
| Extensions write `execute_with_claims(&[..], &[AutoClaim::..])` | Removed |
| `ChainedCall::new(program_id, pre_states, &ix)` with risc0 serde `Vec<u32>` | `ChainedCall::new(program_account_id, pre_state_ids, &ix)`, borsh |
| "Rule 7" (non-default account left default-owned) and `ClaimedIfDefault` for signers | Gone in v0.2.5 |
| The trust model: "path dependencies only" | `dep_walk.rs` also discovers git and registry dependencies, through `cargo metadata --offline` |
| `#[account_type]` "must live at file top level" | The macro collects it from inside the module too (`lib.rs:541-553`) |

**The write pitfall that comes from this lag** (*inferred* from code, not run):
1. A handler mutates `acc.account.data` and returns `acc`.
2. `IntoStateDiff` turns that into `AccountStateDiff::unchanged(acc)`.
3. That diff carries the **mutated** account as its `pre_state`.
4. v0.2.5 compares every output `pre_state` with chain state, and fails the call with `InconsistentAccountPreState`. LEZ's own example says so: "It will only succeed if the pre states coincide with the previous values of the accounts."
5. The failure is chargeable, so the transaction is included and `spel` reports it as confirmed.

The template `spel init` writes does exactly this (`spel-cli/src/init.rs:794-795`). `scripts/init-e2e-test.sh` sends `initialize` and `do_something` and checks only the CLI's exit status, never the resulting state, so CI wouldn't notice.

The fix is to return `AccountStateDiff::new(pre, BalanceDiff::Add(0), new_data)` and leave the `AccountWithMetadata` unmodified. lez-programs does this for every write (section 4d), and its README warns that "spel reports `Transaction confirmed` even when the program rejected the instruction" (`README.md:199`). That warning confirms the reporting half of this pitfall.

### 6.2 What the abstractions don't check

- **`mut` is never enforced.** It only reaches the IDL.
- **`init` doesn't claim** (section 3.3).
- **Errors can't be told apart from success off-chain** (section 3.5).
- **The private-PDA identifier is fixed at 0.**
- **A rest list is checked only by position**, so a constraint on it applies to its first element.
- **`owner =` takes any expression.** It is written into the IDL as text by `__program_idl()`, not as structured data.

### 6.3 Conventions you are locked into

- **Multi-seed PDAs** are `SHA256(seed1 ‖ …)`, and seed strings are zero-padded to 32 bytes. Anything that derives a SPEL program's PDAs (another program, a wallet, an indexer) has to copy this. The flip side: a program with its own scheme gets no PDA support at all. lez-programs is in that position, so it checks every PDA by hand and ships its own derivation helpers (section 4d).
- **Instruction tags** are the borsh variant index, which comes from source order or the external enum. Reordering or inserting a variant breaks every client and every caller program silently. Only appending is safe.
- **The IDL dialect** is a superset of lssa-lang. Its `discriminator` looks like Anchor's selector but isn't used.

### 6.4 Complexity

- The core of the framework is small. The code that writes the dispatcher and the checks is about 1.3k lines of `spel-framework-macros/src/lib.rs`.
- Around it are about 9k lines, tests included, of extension discovery, dependency walking, injection, embedded slots and source-parsing IDL generation. That code runs inside proc-macros that do file and process I/O (section 3.7).
- The mechanism is well guarded. Malformed metadata is a hard error, and the code checks duplicate names, collisions and agreement with the source. But it is a second, implicit build system. The extension guarantees hold only if every producer (the macro, `generate_idl!`, the CLI) resolves the same source tree the same way (*inferred*).

### 6.5 What SPEL does well, and should keep

- **One source of truth.** Instruction signatures produce the guest dispatch, the IDL, the CLI and the clients, so they can't drift apart, apart from the ordering issue PR #290 fixed.
- **Generated checks run before the handler**, so a handler can assume its accounts have their declared roles. lez-programs repeats those checks in its handlers anyway, so they still hold when the handlers are unit-tested without the dispatcher.
- **The CLI handles the hard client work:**
  - PDA derivation;
  - signer and nonce collection;
  - the fee payer;
  - multi-party signing, with stale-nonce detection and witnesses verified against the exact bytes;
  - a dry run;
  - decoding account data.
- **Deploying is one command**, which hides `program_loader`'s segment bookkeeping.
- **The extension trust model is explicit**, with two actions to opt in, and it fails closed.

---

## 7. What this means for a v0.4 redesign

These are observations, not a design. In v0.4 a program is one handler: it gets one message, `ReceiveInput { receiver, from, is_authorized, pre_state, message }`, and returns a `Response` (new state, sync and async messages, events, validity windows). See [GUIDE_v0.2_v0.4.md](GUIDE_v0.2_v0.4.md).

| SPEL piece | In v0.4 | Why |
|---|---|---|
| Account parameters, ordering, `Vec<AccountWithMetadata>` | **Gone** | A handler sees one actor's state, never a list of accounts |
| Returning every account, `IntoStateDiff`, diffs | **Gone** | `Response::keep_state()` / `set_state(bytes)` |
| `owner = ..` | **Gone** | Every program owns exactly its own slot on every account |
| `mut` | **Gone** | The state slot is the only writable thing |
| `signer` | **Changed** | `input.is_authorized`, and usually `from.is_none()` (signed directly) or `from == expected actor`. A PDA actor that receives a message directly is not authorized |
| `init` | **Changed** | `pre_state.is_empty()`, or `lee_core::program::write_once` |
| `pda = ..` | **Changed** | PDAs still derive from `(program_account_id, seed)`. The check becomes "the receiver is the PDA I expect", or the caller's address is the seed. A brand-new PDA needs `PublicAccountEvidence::Pda` in the transaction, which is client work |
| `ProgramContext` | **Changed** | `receiver.program_account_id` and `from` replace `self_account_id` and `caller_account_id` |
| Chained calls | **Changed** | `SyncMessage` (now, all-or-nothing, can carry `with_pda_seeds`) and `AsyncMessage` (may land later, carries no PDA grant) |
| Cross-account operations | **Changed, and harder** | One user call becomes a chain of messages, with internal variants guarded by `from_own_program()` or an exact `from` check. A framework could generate these guards |
| Reading another program's state | **Changed** | An assert message, or `ReadState` → `StateReply` with the pending step kept in your own state |
| Instruction enum and dispatch | **Still useful** | A borsh `Message` enum and `actor_entrypoint!`. The internal variants need separate marking from user-callable ones |
| Errors | **Still an issue** | Handlers return `Response`, not `Result`, and fail by panicking. Reverts are charged. Structured errors are still missing |
| Events | **New** | `Response::event(ProgramEvent { selector, data })`. SPEL has nothing for this today |
| IDL | **Still useful, different content** | Messages and their senders (user, own program, a named program), which actor a message goes to (the user's own account, a PDA from given seeds, any account), the state type per actor, events |
| CLI and client-gen | **Still useful, more needed** | It has to work out the target actor, the declared public actors, PDA evidence, the fee payer, and stored-message collection (`wallet pending`) |
| `spel deploy` | **Still useful** | `program_loader` remains |
| Framework-free handler fns (the lez-programs layout) | **Still useful** | A v0.4 `handle_message` is already a plain fn of `(&ReceiveInput, Message) -> Response`. So framework-free handlers that can be unit-tested come for free, if a framework stays out of the way |
| Extensions | **Needs rethinking** | See below |

**Extensions under v0.4** (*inferred*). An extension's instructions are compiled into the consumer's binary, so they run as the **consumer program**. That keeps something:
- The admin config is still this program's own state on some account.
- Embedded mode becomes a byte window in the consumer's own actor state, with no foreign read involved.

What changes is how a gate works. A gate like `#[require_admin]` can read `admin_config` inline only when the gated message is delivered to the actor that holds it. Otherwise the gate becomes a sync assert message ("is `from` the admin?") to the config actor, or the seat pattern. A gate can no longer be a simple body rewrite over injected account parameters. It changes the message flow.

### Open questions

1. Should SPEL generate the seat/PDA pattern, where a user-signed message to the user's own actor is relayed to a program-held actor, or only describe it in the IDL?
2. How should an IDL say who may send each message (a transaction signer, own program, a named program) and which actor it targets, so the CLI can build the transaction?
3. Errors: a convention for panic messages, or events, so clients can tell a reverted call from a successful one?
4. Is the extension mechanism worth keeping, given v0.4's isolation? Or is a library of handler combinators (plain Rust, no source scanning) enough?
5. Can IDL generation become hermetic, from macro output or a build-time artifact, instead of parsing source and running `cargo metadata` inside proc-macros?
6. lez-programs, the largest SPEL user, uses only the dispatcher, the external-enum binding, the IDL and the CLI. It writes its own PDAs, admin and freeze checks, and FFI. Should a v0.4 SPEL start from that subset, and add PDA support only for schemes it can describe, such as a hash over typed seed parts?
