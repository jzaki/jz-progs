# Scaffold as it is

`logos-scaffold` (`lgs`, repo `logos-co/scaffold`) is the command-line tool for building, running and deploying LEZ programs against a local sequencer. `lgs new` creates a project from a template. `lgs run` chains setup, build, IDL generation, a local sequencer, wallet funding and deploy into one command. Scaffold also runs throwaway sequencers for integration tests (`lgs test-node`), and builds and launches Logos Basecamp desktop modules (`lgs basecamp`). Everything the CLI does is also a Rust API, `logos_scaffold::api`. Its README compares it to Foundry and Anchor: `lgs new` is `forge init` / `anchor init`, and `lgs localnet start` is `anvil` / `solana-test-validator`.

This file describes scaffold **as it is**. It sits next to [SPEL.md](SPEL.md) as groundwork for moving LEZ tooling to the v0.4 actor model. It covers:
- what scaffold solves and how it is built;
- what the workflow and the client code look like with and without it;
- how it integrates LEZ, SPEL and Basecamp;
- what it costs.

Section 7 maps each part onto v0.4 without proposing a design.

**Every claim is tagged with its basis**, as in SPEL.md:
- *specified*: scaffold's README, `docs/`, `ADR.md`, `FURPS.md` or code comments say so.
- *implied*: the code does it, though no doc says so.
- *inferred*: my conclusion from the code, not stated anywhere and not run.

**Sources:**

| Repo | Ref | Used for |
|---|---|---|
| `logos-co/scaffold` | `master` at `fbdcb1f` (2026-10-21). Crate 0.4.0; last tag `v0.4.0` (2026-09-25) | The tool as released. `../scaffold` |
| `logos-co/scaffold` PR #273, "Migrate to LEZ 0.3 (v0.3.1); drop older LEZ support" | branch `feat/lez-0.3` at `347e6b7` (2026-10-12), open | The LEZ 0.3 port |
| `logos-co/scaffold` PR #199, "rename lez-framework to spel; delegate scaffolding to spel init" | branch `feat/spel-template` at `efb7620` (2026-10-12), open since 2026-06-02 | The SPEL v0.7 / LEZ v0.2.4 port |
| `logos-co/spel` | tag `v0.5.0` (`73fc462`) | The SPEL that scaffold pins. SPEL.md covers SPEL `main` |
| `logos-blockchain/logos-execution-zone` | tags `v0.1.2`, `v0.2.4`, `v0.3.1` | What scaffold and its PRs build against |
| `logos-blockchain/logos-execution-zone` | `jzaki/fix-examples` (`928493c00`) | v0.4 contrast, using `dev` names |

I read the two PRs from their branches. Their numbers, titles and authors come from the repo's GitHub pull-request list, read on 2026-10-22. Nothing here was run: every claim comes from reading code and docs.

---

## 0. Which LEZ scaffold targets

**Every scaffold release since v0.2.0 pins LEZ `v0.1.2` and SPEL `v0.5.0`, and so does `master`** (`src/constants.rs:27-34`). v0.1.2 is older than the v0.2.4 these notes call "v0.2". SPEL's CHANGELOG calls it a retag of `v0.2.0-rc3`. At v0.1.2, LEZ still has:
- the `nssa`/`nssa_core` crate names, `read_nssa_inputs`, post-states and `Claim`;
- a free `wallet deploy-program`, where the program id is the risc0 image id;
- the Piñata faucet.

Two open PRs move scaffold on, and they treat SPEL in opposite ways. Both branch from `d4b15ff` (2026-10-05). Neither targets v0.2.5-rc2, which is what SPEL `main` uses (SPEL.md §0), or `dev`.

| | LEZ | SPEL | Status |
|---|---|---|---|
| Releases v0.2.0 (2026-06-11) to v0.4.0 (2026-09-25), and `master` | v0.1.2 | v0.5.0 | Released. PR #276 prepares a 0.4.1 patch release |
| PR #199 (`feat/spel-template`) | v0.2.4 | v0.7.0 | Open. Renames the `lez-framework` template to `spel` and generates it with `spel init`. Ports the default template to the v0.2.4 API |
| PR #273 (`feat/lez-0.3`) | v0.3.1 | none | Open. Removes the SPEL template and every SPEL command "rather than left pointing at an incompatible toolchain" |
| none | v0.2.5-rc2 (SPEL `main`) | | No branch |
| none | `dev` (v0.4) | | No branch. No open PR mentions the actor model |

**Scaffold supports one release of each dependency at a time.** On `master` the rule is written down for Basecamp, in ADR "Scaffold Supports One Basecamp Release". PR #273 adds an ADR, "Scaffold Supports One LEZ Release (0.3)", which deletes the compatibility code for older LEZ pins. It also says what the next move will cost (*specified*):

> Moving to a later release line is a migration like this one: change `DEFAULT_LEZ`, re-derive the wire facts in `testnode/`, re-check the wallet CLI surface `wallet_support`/`deploy` rely on, and bump the schema if old projects cannot keep working.

PR #273 changes 86 files: 4,718 lines added and 7,846 removed.

### What changes under scaffold at each LEZ line

| What scaffold drives | v0.1.2 (`master`) | v0.3.1 (PR #273) | `dev` (v0.4) |
|---|---|---|---|
| Guest API | `read_nssa_inputs`, post-states, `Claim` | `lee_core::program::{Plan, run_program}`, account shards | `actor_entrypoint!`, `ReceiveInput`, `Response` |
| Deploy | `wallet deploy-program <bin>`, free | `wallet program-loader deploy`: segment accounts plus a header | Same `program-loader deploy --elf --header --segments --payer` ([WALKTHROUGH.md](example_progs/WALKTHROUGH.md)) |
| Program id | The risc0 image id, computed from the ELF | The header account id, new on every deploy. Scaffold records it in `.scaffold/state/deployments.json` | The header account id |
| Funding | `wallet pinata claim` | `auth-transfer send` from the account LEZ's debug genesis funds. Its key is published in LEZ's `Justfile` | The same key is in `dev`'s `Justfile`. `send` adds `--to-pk` for a public account that hasn't been used yet |
| Fees | None | The payer must hold the fee reserve, about 16M units at the default gas limit | Yes. Without a fee the call fails with `MissingFeeDeclaration` |
| Admission | None | Not checked | An account that isn't on chain yet needs `PublicAccountEvidence`. Without it: `UnadmittedPublicActor` |
| Wallet home env var | `NSSA_WALLET_HOME_DIR`. Scaffold also sets `LEE_WALLET_HOME_DIR` | `LEE_WALLET_HOME_DIR` only | `LEE_WALLET_HOME_DIR` |
| Sequencer start | `sequencer_service <config.json>`. Scaffold patches `port` and `max_block_size` into the config | `sequencer_service <config> --port --listen-address --home` | `sequencer_service setup <home> --config .. --funding-public-key ..`, then `start <home> --rpc-listen-address ..` |
| Account read | `balance`, `nonce`, `program_owner`, `data` | Per-program shards | `nonce` plus one actor state per program. `getAccountView(actor)` reads one |
| `getTransaction` | The transaction, or `null` | `[transaction, block_id]` | `(LeeTransaction, BlockId)` |
| A call that fails in the guest | Not included (*inferred*: v0.1.2 has no `is_chargeable`) | Included, charged and reverted (`LeeError::is_chargeable`) | Included, charged and reverted |
| Circuits | Scaffold downloads release 0.4.1 | LEZ's build fetches its own, and PR #273 deletes scaffold's code for it | Not checked |

The v0.4 column comes from LEZ on `jzaki/fix-examples`:
- `lez/sequencer/service/src/main.rs` and `lez/sequencer/service/rpc/src/lib.rs`;
- `lez/wallet/src/cli/`;
- `lee/state_machine/src/error.rs:105`;
- [WALKTHROUGH.md](example_progs/WALKTHROUGH.md).

**Most of the v0.3.1 column carries over to `dev`**: deploy, program id, funding and the wallet env var. The sequencer's CLI, the account shape and the wire format changed again (*inferred*).

---

## 1. Why scaffold exists

> "`lgs run` builds your project, starts a local sequencer, funds a wallet, and deploys your programs. That is the whole inner loop in one command." (README, *specified*)

> "Developers need one bootstrap target that is immediately runnable and easy to modify." "Local development should work without requiring manually managed external node setup." (ADR "Project Structure" and "Local Runtime", *specified*)

### What a developer does by hand

jz-progs's [WALKTHROUGH.md](example_progs/WALKTHROUGH.md) is the by-hand path on `dev`. In outline:
1. Clone LEZ at the right ref and enter its toolchain (`nix develop`).
2. Reset the chain and start a standalone sequencer in its own terminal.
3. Point the wallet at a home with `LEE_WALLET_HOME_DIR`. Initialise the wallet, import the funded test accounts, and create the accounts the program uses.
4. Build the guests, and find the `.bin` path under `target/riscv-guest/...`.
5. Create a header and N segment accounts, deploy through the program loader, and note the header id.
6. Build the call with a fee and admission evidence, sign it, submit it, then read the state back to find out whether it reverted.
7. Repeat steps 4 to 6 on every change, and steps 2 and 3 after every reset.

Off the inner loop, there is more to do:
- keep the LEZ checkout, the sequencer, the wallet and any framework CLI on matching versions;
- build guests whose image id is the same on every machine;
- for integration tests, run a sequencer per test and tell "committed" from "rejected" from "still waiting";
- for Basecamp modules, run isolated desktop profiles for two users on one machine.

### What scaffold provides

| Need | Scaffold | Where |
|---|---|---|
| A project to start from | `lgs new [--template default\|lez-framework]`. `lgs init` adopts an existing project | `commands/new.rs`, `commands/init.rs` |
| Matching versions | Every dependency pinned by commit in `scaffold.toml`, cloned into a cache and built locally. Nothing goes on `PATH`. `doctor` reports drift | `repo.rs`, `commands/setup.rs`, `commands/doctor.rs` |
| The inner loop | `lgs run`: build, IDL, localnet, top-up, deploy, then post-deploy hooks. `--watch` re-runs it on every change | `commands/run.rs` |
| A local chain | `lgs localnet start\|stop\|status\|logs\|reset`, one daemonised sequencer per project | `commands/localnet.rs` |
| A funded wallet | A project-local wallet home, a default address and `lgs wallet topup` | `commands/wallet.rs` |
| Deploy | `lgs deploy` finds every guest, deploys it and prints its program id. `--json` | `commands/deploy.rs` |
| Reproducible guests | Guests built in a pinned Docker image by default | `commands/build.rs` |
| Framework support | IDL and client generation for the SPEL template, and `lgs spel --` | `commands/idl.rs`, `commands/client.rs` |
| Integration tests | `lgs test-node`: isolated sequencers, typed transaction outcomes, block-consistent reads, state seeding | `src/testnode/` |
| Desktop modules | `lgs basecamp`: build, install and launch `.lgx` modules in `alice` and `bob` profiles | `commands/basecamp.rs` |
| Diagnostics | `lgs doctor [--json]`, and `lgs report`, a redacted bundle for bug reports | `commands/doctor.rs`, `commands/report.rs` |
| AI agents | Four skills copied into each project, for Claude Code, Cursor and `AGENTS.md` | `template/skills.rs` |
| Embedding | `logos_scaffold::api::Project` mirrors the CLI | `src/api/` |

**What scaffold doesn't do:**
- **It doesn't touch program code.** It has no program framework, no account model and no instruction encoding of its own. The program model reaches it in only three places: the templates, the example runners and the IDL harness (*implied*).
- **It runs a local chain only.** It supports "the standalone sequencer flow only" (README:249). FURPS and the ADR plan DevNet targeting through env files (FURPS Functionality 1, ADR "Network Configuration"). But the generated `.env.local` holds only `RUST_LOG` and `RISC0_DEV_MODE` (*implied*).

Contributions have a triage bar: "Generic cleanups, cosmetic refactors, and 'this would be nice' PRs from contributors with no connection to [real users] are the lowest triage priority and are likely to be closed" (CONTRIBUTING.md, *specified*).

---

## 2. Architecture

One crate builds two identical binaries (`logos-scaffold` and `lgs`) and a library. Line counts include inline unit tests:

| Area | Files | Lines |
|---|---|---|
| The LEZ loop: new, init, setup, build, deploy, localnet, wallet, run, IDL, client, repo sync, migration | `src/commands/{new,init,setup,build,deploy,localnet,wallet,wallet_support,run,run_state,idl,client}.rs`, `src/{repo,migrate,sequencer_config,circuits,lez_layout}.rs` | ~11.7k |
| Basecamp | `src/commands/basecamp.rs` | ~7.6k |
| Test nodes | `src/testnode/`, `src/commands/testnode.rs`, `src/api/testnode.rs` | ~6.8k |
| Diagnostics | `src/commands/{doctor,report}.rs`, `src/doctor_checks.rs` | ~3.4k |
| Config | `src/config.rs`, `src/model.rs` | ~3.0k |
| **All of `src/`** | | **~38.7k** |
| Integration tests | `tests/` | ~5.8k |

The two commands most projects use, as scaffold runs them at the v0.1.2 pin (*implied*, from the code):

```
lgs new <name>
  clone LEZ at the pin into the cache
  copy LEZ's examples/program_deployment into <name>/
  render templates/<variant>/ over it (Cargo.toml, runners, README, the SPEL counter)
  write scaffold.toml, .scaffold/ and the AI skills

lgs run
  [1/5] build     setup: sync and build sequencer_service, wallet and spel; seed the wallet
                  cargo build --workspace; build the guests in Docker or on the host
  [2/5] IDL       lez-framework only: cargo test __lssa_idl_print → idl/*.json
  [3/5] localnet  reuse the project's sequencer, or start one
  [4/5] topup     wallet account get → auth-transfer init → pinata claim
  [5/5] deploy    per guest: wallet deploy-program, one per block; spel program-id → program id
                  skipped when the binaries, IDL, config and sequencer pid are unchanged
  post_deploy     sh -c hooks, with SEQUENCER_URL, SCAFFOLD_PROGRAM_ID and the rest
```

### A generated project

```
my-app/
  Cargo.toml              workspace: ".", "methods", "methods/guest" (and "crates/lez-client-gen")
  methods/guest/src/bin/  the guest programs: LEZ's examples, or the SPEL counter
  src/bin/run_*.rs        example clients, from scaffold's overlay
  src/lib.rs              runner helpers, plus a host copy of the program for lez-framework
  idl/                    lez-framework only: the generated IDL
  scaffold.toml
  .scaffold/              state/, logs/, wallet/, circuits/ (added to .gitignore)
  .claude/skills/, .cursor/rules/, AGENTS.md
```

A minimal `scaffold.toml`, from the parser's test fixture (`src/config.rs:1254-1287`) with the default pins filled in:

```toml
[scaffold]
version = "0.2.0"

[repos.lez]
source = "https://github.com/logos-blockchain/logos-execution-zone.git"
pin = "cf3639d8252040d13b3d4e933feb19b42c76e14a"   # v0.1.2

[repos.spel]
source = "https://github.com/logos-co/spel.git"
pin = "73fc462eb8f0a4d00f1a846437c627ec2e523f83"   # v0.5.0

[wallet]
home_dir = ".scaffold/wallet"

[framework]
kind = "default"            # or "lez-framework"
version = "0.1.0"

[framework.idl]
spec = "lssa-idl/0.1.0"
path = "idl"

[localnet]
port = 3040
risc0_dev_mode = true
```

`lgs new` also writes `[repos.basecamp]` and `[repos.lgpm]`. Projects can add `[build]`, `[run]`, `[modules.*]` and `[basecamp]`. **The schema has three namespaces** (ADR "scaffold.toml Schema: Three Namespaces", *specified*):
- `[repos.<name>]`: pinned dependencies;
- `[modules.<name>]`: the project's Logos modules;
- `[<feature>]`: runtime config per feature.

The file stores no checkout path. Scaffold derives it at runtime as `<cache_root>/repos/<name>/<pin>`, so the file is portable across machines (*specified*).

---

## 3. Design choices and implementation details

### 3.1 Pin everything, build it locally

- **One entry per dependency.** Each is `[repos.<name>]` with a `source` and a `pin`, cloned to `<cache_root>/repos/<name>/<pin>`. When the source isn't scaffold's default, the directory gets a `-src-<hash>` suffix (docs/commands.md). `--vendor-deps` puts the clones in `.scaffold/repos/` instead.
- **`setup` builds three binaries in their own checkouts, all `--release`:**
  - `sequencer_service --features standalone`. The feature swaps the indexer and Bedrock clients for mocks. Without it, the sequencer exits on any host without a Bedrock stack (`setup.rs:19-35`, *specified*);
  - `wallet`;
  - `spel`.
- **Binaries stay project-local.** "Project-local binaries are never installed to PATH. Reach them through `lgs wallet -- …` and `lgs spel -- …`" (AGENTS.md, *specified*).
- **The first run compiles LEZ from source**, and the README warns it takes a while. `--prebuilt` downloads only `sequencer_service`, from scaffold's own releases, and checks its SHA-256.
- **A cross-pin invariant.** "DEFAULT_SPEL must point at a spel commit whose `spel-cli/Cargo.toml` vendors LEZ at the same ref as DEFAULT_LEZ" (`constants.rs:21-26`, *specified*). `doctor` checks it by searching that file's text for the LEZ tag or sha (`doctor.rs:417-445`, *implied*).
- **Some pins move as a set.** Basecamp, `lgpm`, the companion modules and the list of modules Basecamp bundles must all change together (ADR "Basecamp Pin Bumps Move as a Set"). Scaffold keeps tables of the default pins it used to write, so `basecamp setup` can move old projects off them (`RETIRED_BASECAMP_PIN_SETS`).
- **Old configs are migrated by `init`.** Every other command refuses an old `scaffold.toml` shape with one line pointing at `init`. `init` rewrites the file with `toml_edit` and keeps its comments (ADR "Schema Migration via init"). Default-filling was rejected because its warning would corrupt `--json` output (*specified*).

### 3.2 Templates are LEZ's example plus an overlay

- **`lgs new` copies LEZ's own example.** It clones LEZ at the pin and copies `examples/program_deployment` into the project (`new.rs:199-204`). Then it renders `templates/<variant>/` over the copy, replacing `{{crate_name}}`, `{{lez_pin}}`, `{{lez_tag}}` and `{{spel_pin}}`. It fails on any token left unresolved (`template/project.rs:107-115`).
- **The guests come from LEZ, the clients from scaffold.** The default overlay brings its own `Cargo.toml`, `src/lib.rs` and seven `src/bin/run_*.rs` runners, and LEZ's guests stay as copied (*implied*). So the guest code follows the LEZ pin and the runners follow scaffold. Section 4b shows why that matters.
- **A program id patched into source.** For the default template, `patch_simple_tail_call_program_id` writes scaffold's `DEFAULT_HELLO_WORLD_IMAGE_ID_HEX` into the `HELLO_WORLD_PROGRAM_ID_HEX` constant in `simple_tail_call.rs` (`template/copy.rs:10-46`, `constants.rs:57`). It does nothing when the constant is missing. It is missing at v0.3.1 and on `dev`, where the callee's address travels in the message (*implied*).
- **`lez-framework` starts from the same copy.** It deletes the hello-world guests and runners (`new.rs:239-264`), then overlays a SPEL counter: the guest, a runner, a client-generator crate and a committed IDL. Section 5.3 covers it.
- **Both templates use LEZ crates as git dependencies at the pin.** `lez-framework` uses the tag form "so Cargo resolves a single nssa_core instead of treating equivalent tag and rev sources as distinct" (`templates/lez-framework/Cargo.toml.template`, *specified*).

### 3.3 Builds

- **Host crates, then guests.** `build` runs `setup`, then `cargo build --workspace`. If `methods/Cargo.toml` exists, it then builds the guests by manifest path, because real projects often keep the guest crate out of the parent workspace (ADR "Guest Build Discovery", *specified*).
- **Deterministic builds are the default.** `[build].guest = "docker"` runs `cargo risczero build` inside `risczero/risc0-guest-builder:r0.1.97.0` (`constants.rs:118`), so the same source gives the same ELF and image id on any machine. `local` uses the host toolchain instead. The ADR's reason: "a default whose failure mode is a silently non-portable `program_id` is a footgun" (*specified*).
- **"The last `lgs build` decides what `lgs deploy` ships"** (ADR). The two modes write to separate trees, and each build deletes the other mode's output. A CI workflow builds the guests twice from a clean tree and compares the digests.
- **The rationale is the image id.** PR #273 notes that from LEZ 0.3 the program id no longer comes from the ELF. It keeps deterministic builds for the guest bytes, and for the image id that private executions prove against (*specified*).

### 3.4 Localnet

- **One long-lived sequencer per project**, on `127.0.0.1:3040`. It starts under `setsid`, so it outlives the terminal (docs/commands.md).
- **`start` prepares the run.** It copies LEZ's debug config into `.scaffold/state/` and patches `port` and `max_block_size = "8 MiB"` into it (`sequencer_config.rs:13-21`). It sets `RISC0_DEV_MODE`. When `RISC0_SERVER_PATH` is unset, it looks for the `r0vm` matching the risc0 version in LEZ's `Cargo.lock` (`localnet.rs:345-370`).
- **`start` waits for readiness**: the process must be alive and the port open. There is no fixed sleep.
- **`status` tells three cases apart**: a managed process, stale state, and a foreign process holding the port.
- **`reset`** wipes chain state, and `--reset-wallet` also wipes the wallet.

### 3.5 Wallet

- **Both env var names, always.** The wallet home is `.scaffold/wallet`, and every wallet subprocess gets both `NSSA_WALLET_HOME_DIR` and `LEE_WALLET_HOME_DIR`. The reason: "a v0.2.0 wallet that sees only the old name does not error: it falls back to `~/.lee/wallet`". The rule for future renames: "add the new name, keep the old one, never swap" (ADR, `constants.rs:60-74`, *specified*).
- **A default address.** It lives in `.scaffold/state/wallet.state`. `setup` seeds it from the first public account in LEZ's debug wallet config, or creates an account when the config ships none.
- **A default password.** It is `logos-scaffold-v0`, overridden by `LOGOS_SCAFFOLD_WALLET_PASSWORD` (`constants.rs:59`), and scaffold pipes it to the wallet on stdin.
- **`topup` funds from the faucet.** It reads the account, runs `auth-transfer init` only if the account is uninitialised, then runs `pinata claim --to` (`wallet.rs:240-262`).

### 3.6 Deploy

- **Discovery.** There is one program per `methods/guest/src/bin/*.rs`. Its `.bin` is searched for under `target/riscv-guest-docker`, `target/riscv-guest` and `methods/target` (`deploy.rs:32-36`). A `docker` path component ranks above `release`, which ranks above debug, and the shallowest match wins (ADR "Build Output Discovery").
- **One `wallet deploy-program <bin>` per program** (`deploy.rs:238`).
- **One program per block.** The comment explains why: "A block that batches several deployment transactions — each carrying a full guest ELF, ~370 KiB per template program — exceeds that cap and the sequencer panics fatally" (`deploy.rs:43-66`, *specified*). After each deploy, scaffold waits for the head block to move. After 90 s it gives up and doesn't submit the rest.
- **The program id comes from SPEL.** Scaffold runs the vendored `spel program-id <bin>` and parses its `ImageID (hex bytes):` line (`deploy.rs:41`). ADR "Vendoring spel" explains the choice: it avoids a risc0 dependency in scaffold, and the id is "byte-for-byte the value the sequencer will use". A timeout and an "unavailable" fallback stop a broken `spel` from failing a deploy (*specified*).
- **"Submitted", not "confirmed."** Scaffold prints: "Submission confirmed by wallet exit status; deploy inclusion receipt is not currently exposed by LEZ wallet/RPC for scaffold" (`deploy.rs:374`, *specified*).
- **`--json` prints exactly one JSON object.** Absent values are left out rather than set to `null` (FURPS Functionality 9).

### 3.7 `lgs run`

- **The steps run in a fixed order**, and a new step joins the chain (ADR "lgs run is a Single-Command Inner Loop"). Only `topup` and `deploy` can be skipped, because "a step becomes skippable only when the project can legitimately own that concern itself". A skipped step still prints its header and keeps its number (*specified*).
- **The deploy cache.** `.scaffold/state/run_deploy.json` stores a SHA-256 for each guest `.bin`, combined with its IDL and a config digest, plus the sequencer's pid. The deploy is skipped when they all match (`run_state.rs:1-13`). A localnet restart changes the pid and forces a redeploy, on the premise that the restart left "the empty chain" (*specified*).
- **PR #273 keeps that premise**, although its own ADR says a LEZ 0.3 restart "replays every block since genesis". So on 0.3 a restart redeploys programs that are still on chain, under new ids (*inferred*).
- **Hooks.** `[run].post_deploy` commands run under `sh -c` in the project root, with a documented environment (ADR "Hook Env Contract", docs/configuration.md, *specified*):
  - `SEQUENCER_URL`;
  - both wallet-home names;
  - `SCAFFOLD_PROJECT_ROOT` and `SCAFFOLD_IDL_DIR`;
  - `SCAFFOLD_TOPUP_SKIPPED` and `SCAFFOLD_DEPLOY_SKIPPED`;
  - `SCAFFOLD_PROGRAM_ID` and `SCAFFOLD_GUEST_BIN`, but only when exactly one program is deployable, "so hooks fail loudly rather than silently picking up the wrong program".
- **Options:** `--profile`, `--reset`, `--watch` with a debounce, and `--localnet-timeout`.

### 3.8 IDL and clients (lez-framework only)

- **`build idl` doesn't call `spel`.** It runs `cargo test --workspace __lssa_idl_print -- --show-output` and cuts the JSON from between the `--- LSSA IDL BEGIN <name> ---` and `--- LSSA IDL END <name> ---` lines (`idl.rs:18-20`, `:100-110`). The test that prints `PROGRAM_IDL_JSON` is in the template's `src/lib.rs`. That makes it a fifth IDL producer, next to the four in SPEL.md §2 (*implied*).
- **So the template holds the program twice.** One copy is the guest, `methods/guest/src/bin/lez_counter.rs`. The other is a host copy in `src/lib.rs` that exists only to extract the IDL. A comment says "Keep this body in step with methods/guest/src/bin/lez_counter.rs", and nothing checks that (*implied*).
- **The IDL is cached** by a hash of every `.rs`, `Cargo.toml` and `Cargo.lock` plus the `[framework.idl]` config. The reason: the test build "dominates `lgs run` wall time (~30s)" (`idl.rs:22-36`, `:80-82`, *specified*).
- **`build client`** regenerates the IDL, then runs the template's own `crates/lez-client-gen`, which wraps `spel-client-gen` to take a whole directory of IDLs. It writes to `src/generated/` (`client.rs`).
- **The docs disagree with the code here.** docs/commands.md says both commands use "the vendored `spel` binary", but the code uses cargo and the template's crate (*implied*).

### 3.9 Test nodes

- **Isolated sequencers.** `lgs test-node start` runs a sequencer with its own port (a free one by default), config, rocksdb, log and directory under `.scaffold/test-nodes/<id>`. `run -- <cmd>` wraps a command: it exports `LGS_TEST_NODE_RPC_URL` and related variables, and returns the command's exit status. `--serial` and `--parallel N` cap how many nodes run at once (docs/commands.md).
- **Outcomes are never merged.** `tx submit-and-wait --json` returns exactly one of these (`testnode/client.rs:1-33`, *specified*):
  - `committed`, with the block id and timestamp;
  - `rejected`, either `stateless` (at submission) or `stateful` (after `rejection_blocks` new blocks without it, 3 by default);
  - `timeout`;
  - `transport_error`;
  - `wire_mismatch`.
- **Scaffold reads the wire format by hand.** It reads the block header at fixed offsets: `block_id` at bytes 0..8 and `timestamp` at 72..80. It finds a transaction by searching for its bytes inside the block's bytes, "without a full borsh decoder" (`testnode/client.rs:18-24`). PR #285 (2026-10-21) also scans the blocks before reporting `stateful`, because the sequencer doesn't rebuild its `getTransaction` index after a restart.
- **Reads are block-consistent.** Scaffold reads the head before and after each read, and retries when a clock block lands in between. `clock wait-stable` is a read barrier for an "always-ticking sequencer". Accounts are decoded into balance, nonce, owner and data.
- **State seeding.** `state seed` builds a node's genesis from a snapshot: `lgs-state-snapshot/1`, `lgs-account-snapshot/1`, or a rocksdb directory. It refuses public-account data, which the genesis config can't seed.
- **The same surface is Rust:** `logos_scaffold::api::testnode::{TestNode, TestNodeConfig}`.

### 3.10 Diagnostics, skills and the API

- **`doctor`** checks the tools, the pins, the spel/LEZ alignment, the guest-build strategy, the wallet and the localnet. `--json` is for CI.
- **`report`** builds a `.tar.gz` from an allowlist of files, with redaction.
- **AI skills.** `new` and `init` write four skills into the project: `lgs-cli`, `lez-template`, `lez-framework-template` and `basecamp`. Each goes to `.claude/skills/<name>/SKILL.md` and `.cursor/rules/<name>.mdc`, and an `AGENTS.md` lists them (`template/skills.rs`). PR #273 drops the `lez-framework` skill, and `init` retires it from existing projects.
- **The API.** `logos_scaffold::api::Project` has a method per command: `setup`, `build`, `build_idl`, `deploy`, `run`, `localnet_*`, `wallet_*`, `spel`, `basecamp`, `doctor` and `report`. Each returns typed results and errors (`src/api/mod.rs`). AGENTS.md: "A change to command behaviour usually needs a matching change to the API surface."

### 3.11 How changes are checked

- **CI** runs `cargo fmt --check`, `cargo check` and `cargo test` (AGENTS.md). `template-e2e.yml` renders and builds both templates. `guest-reproducibility.yml` checks the Docker build.
- **`DOGFOODING.md`** (~1.9k lines) is the manual runbook. Its scenarios are:
  - D1–D8: the default template;
  - L1–L4: lez-framework;
  - E1–E3: CLI errors, project flags and skills;
  - B1–B7: Basecamp;
  - A1: the Rust API;
  - T1–T4: test nodes.

  A PR must rerun the scenarios it touches (CONTRIBUTING.md).
- **ADR.md and FURPS.md** are "a record, not a description of the current state". You add to them and don't rewrite them (CONTRIBUTING.md). Section 6.1 shows the cost.

---

## 4. With and without scaffold

Scaffold works around a program, not inside it. So the contrasts here are of the workflow and of the client code. The program-level contrasts are in SPEL.md §4.

### 4a. The first run

The middle column is what scaffold runs at the v0.1.2 pin (*implied*, from the code). The right column is the by-hand path on `dev` (WALKTHROUGH.md).

| Step | With scaffold | What scaffold runs (v0.1.2) | By hand on `dev` |
|---|---|---|---|
| Get LEZ | `lgs new my-app` | Clone LEZ into the cache at the pin; copy `examples/program_deployment`; render the overlay | `git clone`, `git checkout jzaki/fix-examples`, `nix develop` |
| Build the tools | `lgs run`, step 1 (through `setup`) | `cargo build --release --features standalone -p sequencer_service`, `-p wallet`, and `-p spel` in the spel checkout | `just` builds them on first use |
| Start a chain | Step 3 | `./target/release/sequencer_service <patched config>` under `setsid`; wait for the port | `just clean`, then `RISC0_DEV_MODE=1 just run-sequencer-standalone` in its own terminal |
| Fund a wallet | `setup` seeds it, step 4 tops it up | `wallet account get`, `auth-transfer init`, `pinata claim --to <default>` | `just run-wallet check-health`, `just wallet-import-test-accounts`, `account new public`; export `LEE_WALLET_HOME_DIR`, `PAYER` and `ACCOUNT` |
| Build the guests | Step 1 | `cargo build --workspace`, then `cargo risczero build` in Docker | `cargo build --release -p example_program_deployment_methods`; set `BIN=…/riscv32im-risc0-zkvm-elf/release` |
| Deploy | Step 5 | `wallet deploy-program <bin>` per guest, one per block, then `spel program-id <bin>` | The runner deploys. Or create four accounts and run `program-loader deploy --elf --header --segments --payer` |
| Call | A hook, or `cargo run --bin run_hello_world -- <account>` | | `cargo run --release --bin run_hello_world -- "$BIN/hello_world.bin" "$ACCOUNT" "$PAYER"` |
| Check | `lgs wallet -- account get …` | | `account get --scope all`, or `getAccount` over curl when the call reverted |
| Next edit | `lgs run` again, or `--watch`. Unchanged programs aren't redeployed | | Rebuild and rerun the runner, which deploys again |

**The difference is less the commands than who remembers them**: the paths, the env var, the password, the pin. Scaffold also turns fixes found while dogfooding into code: the `standalone` feature, the `r0vm` path, the block-size patch, the deploy pacing (*inferred*).

### 4b. The client side: one runner at three LEZ lines

**v0.1.2: scaffold's default overlay runner** (`templates/default/src/bin/run_hello_world.rs`, trimmed):

```rust
let program = load_program(cli.program_path.as_deref(), HELLO_WORLD_ELF, "hello_world")?;
// ...
let nonces = wallet_core.get_accounts_nonces(vec![account_id]).await?;
let message = Message::try_new(program.id(), vec![account_id], nonces, greeting)?;
let witness_set = WitnessSet::for_message(&message, &[signing_key]);
let tx = PublicTransaction::new(message, witness_set);
let response = wallet_core.sequencer_client.send_transaction(tx.into()).await?;
println!("submitted transaction: tx_hash={}", hex::encode(response.0));
```

The program id is `program.id()`, the image id of the ELF embedded in the runner. There is no fee, and the runner reports a submission, not a result.

**v0.3.1: PR #273.** The runners keep their message building, and only the submit step changes. It goes to a helper in the overlay's `src/lib.rs`, `send_paid_public_transaction`. The helper's doc comment says it does this (*specified*):
- re-issues the message with a fee declaration paid by `payer`;
- signs with the given keys, plus the payer's key when the payer isn't already a signer;
- submits the transaction and waits until it is in a block.

Its ADR: "Drop the overlay once upstream examples pay fees."

**`dev`: LEZ's own runner on `jzaki/fix-examples`** (`examples/program_deployment/src/bin/run_hello_world.rs:55-85`, trimmed). It deploys through the program loader, then builds the call with a fee and admission evidence:

```rust
// Deploy the program through `program_loader`; future calls dispatch to the returned header.
let program_account_id = deploy_program(&mut wallet_core, bytecode, payer).await.unwrap();
// ...
let (fee, payer_nonce, payer_key) = fee_paid_by(&wallet_core, payer).await.unwrap();
let nonces = BTreeMap::from([(payer, payer_nonce)]);
let hello = Actor::new(account_id, program_account_id);
// `account_id` doesn't sign and may not exist on chain yet, so the sequencer only admits it
// as a public actor with evidence: its public key, which hashes to the account id. Without
// it the call reverts with `UnadmittedPublicActor`, but is still included and charged.
let message = Message {
    admission_evidence: vec![PublicAccountEvidence::Key(PublicKey::new_from_private_key(
        account_key,
    ))],
    ..Message::try_new_with_fees(hello, vec![hello], nonces, greeting, fee).unwrap()
};
```

**An overlay runner depends on the LEZ line twice**: through the API it calls, and through what counts as a valid transaction. On `dev`, scaffold's v0.1.2 overlay would replace working runners with broken ones. Once runners like these are upstream, `lgs new` could keep LEZ's own (*inferred*).

### 4c. The program: the lez-framework counter

The template's guest (`templates/lez-framework/methods/guest/src/bin/lez_counter.rs`, trimmed):

```rust
#[lez_program]
mod lez_counter {
    #[instruction]
    pub fn initialize(
        #[account(init, pda = literal("counter"))]
        mut counter: AccountWithMetadata,
        #[account(signer)]
        authority: AccountWithMetadata,
    ) -> SpelResult {
        counter.account.data = Data::try_from(0u64.to_le_bytes().to_vec()).map_err(/* … */)?;
        Ok(SpelOutput::execute(vec![counter, authority], vec![]))
    }
    // increment(counter, authority, amount: u64): read 8 bytes LE, checked_add, write back
}
```

**It works at the pin through a rewrite.** SPEL v0.5.0 has no `IntoPostState` for `AccountWithMetadata`. Its macro rewrites `SpelOutput::execute(vec![a, b], calls)` into `execute_with_claims(&[a.account.clone(), …], &__claims_*(…), calls)`, which claims `counter` with `Claim::Pda` (spel `v0.5.0`: `spel-framework-macros/src/lib.rs:965-1150`, `spel-framework-core/src/spel_output.rs`, *implied*). SPEL `main` removed claims and the rewrite. There the same body turns into `AccountStateDiff::unchanged(acc)` with an edited pre-state, which v0.2.5 rejects with `InconsistentAccountPreState` (SPEL.md §6.1, *inferred*). So moving the SPEL pin past v0.7 means changing the template body as well as the pin.

**The same body is in `src/lib.rs` too**, for the IDL harness (§3.8). The version without SPEL, and the v0.4 actor version, are in SPEL.md §4a.

### 4d. Hooks: scripted calls after deploy

From docs/configuration.md:

```toml
[run]
post_deploy = [
  "lgs spel -- --idl $SCAFFOLD_IDL_DIR/counter.json -p $SCAFFOLD_GUEST_BIN init",
  "lgs spel -- --idl $SCAFFOLD_IDL_DIR/counter.json -p $SCAFFOLD_GUEST_BIN increment --by 5",
]
```

**This passes the binary as the program.** It works at SPEL v0.5, where the CLI derives the program id from the binary. Since SPEL's v0.2.5 port, `-p` takes a binary only for the privacy path, because the address isn't in the binary (SPEL.md §3.9). Under the program loader, a hook needs the header address in `SCAFFOLD_PROGRAM_ID`, which PR #273 provides (*inferred*).

---

## 5. Integrations

Scaffold is mostly integration code: it pins, builds and drives other repos' tools. This section is the counterpart of SPEL.md §5.

### 5.1 The pattern

- **A `[repos.<name>]` entry** holds `source`, `pin`, `build` (`"cargo"` or `"nix-flake"`), `attr`, and an optional `path`. "Adding a fifth dep is a one-section addition with no code dispatch on section name". The build targets stay in code, because "making them data-driven didn't earn its complexity" (ADR, *specified*).
- **Cache clones are replaced, your clones aren't.** A cache-managed clone is cloned again when its origin changes. A `path` override or a vendored clone fails instead, so "a developer's checkout is never silently rewritten" (ADR, *specified*).
- **Tools run as subprocesses, and scaffold parses their text output** (*implied*):
  - the wallet's transaction id;
  - the `ImageID` line from `spel program-id`;
  - the `cargo test` markers;
  - `nix build --print-out-paths`.
- **`doctor` reports drift** for each pin.

### 5.2 LEZ

What scaffold uses from LEZ, and where each one already broke or will break:

| Scaffold uses | Where | Status |
|---|---|---|
| `examples/program_deployment`: guests, runners, the `example_program_deployment_methods` crate | `new.rs:199` | Present on `dev`, with v0.4 guests |
| `HELLO_WORLD_PROGRAM_ID_HEX` inside `simple_tail_call.rs` | `template/copy.rs` | Gone at v0.3.1; the patch does nothing |
| `sequencer_service` with `standalone`; its debug config at either of two paths; config keys `port`, `max_block_size`, `home` | `setup.rs`, `sequencer_config.rs`, `localnet.rs` | The CLI changed at v0.3.1 and again on `dev` (§0) |
| Wallet CLI: `account list\|get`, `auth-transfer init`, `pinata claim`, `deploy-program`, `check-health` | `wallet.rs`, `deploy.rs`, `setup.rs`, `doctor.rs` | `pinata`, `auth-transfer init` and `deploy-program` are gone at v0.3.1 and on `dev` |
| The debug wallet config and its preconfigured accounts | `WALLET_CONFIG_REL_PATHS`, `setup.rs` | PR #273 imports the published funder key instead |
| Env var names: wallet home, `RISC0_DEV_MODE`, `RISC0_SERVER_PATH` | `constants.rs:74`, `localnet.rs` | `NSSA_WALLET_HOME_DIR` is gone at v0.3.1 |
| RPC: `sendTransaction`, `getTransaction`, `getBlock`, `getLastBlockId`, `getAccount`, the proof method, and the `/LEZ/ClockProgramAccount/…` accounts | `src/testnode/` | `getTransaction` returns a pair, and the proof method became `getProofsAndRoot`, at v0.3.1 |
| The borsh layout of blocks and transactions | `testnode/client.rs`, `testnode/blocks.rs` | PR #273 rewrote the readers: an 80-byte header without a signature, and fee-distribution and clock transactions at the end of each block |
| LEZ's `Cargo.lock`, for the risc0 version and so the `r0vm` path | `localnet.rs` | PR #273 also seeds each new project's `Cargo.lock` from it |
| The circuits release matching LEZ's `flake.lock` | `circuits.rs`, `constants.rs:36-54` | PR #273 deletes it |

### 5.3 SPEL

**On `master`** (*implied* unless marked):
- **The CLI is vendored into every project**, for both templates. `[repos.spel]` is a required section, and a config without it is refused with a pointer to `init` (ADR "Schema Migration via init"). `setup` clones and builds `spel` even for the default template (`setup.rs:88-97`). A default-template project uses it for one thing: `spel program-id` in `deploy`. The ADR chose this over a risc0 dependency (ADR "Vendoring spel Per-Project for Program ID Surfacing", *specified*).
- **`lgs spel -- <args>`** runs it with the project's wallet (FURPS Functionality 8).
- **Template crates.** `lez-framework` depends on `spel-framework`, `spel-framework-core` and `spel-client-gen` as git dependencies at the same pin (`templates/lez-framework/Cargo.toml.template`).
- **IDL and clients come through the template's own harness and crate** (§3.8), not from `spel generate-idl` or `spel-client-gen` directly.
- **The IDL spec string** is `lssa-idl/0.1.0` (`constants.rs:84`). It is the lssa-lang name, which SPEL keeps for compatibility (SPEL.md §3.8).
- **The alignment check** compares `spel-cli/Cargo.toml` with the LEZ pin (§3.1).

**PR #199** (SPEL v0.7.0, LEZ v0.2.4). From its ADR additions (*specified*):
- **`--template spel` runs `spel init`.** Scaffold builds that `spel` from `DEFAULT_SPEL` in the cache, not from `PATH`: "The pin is the only version that can be used".
- **`spel init` has two constraints.** It refuses a directory that already exists, and it resolves the project name against its own working directory. So scaffold creates nothing in the project directory first, and derives the invocation from the target path alone. Both went wrong twice before tests pinned them down.
- **Builds go through the project's Makefile.** `cargo build --workspace` panics on a `spel init` project: its `methods/Cargo.toml` runs `embed_methods()` without a `[package.metadata.risc0]` section (logos-co/spel#287). So `build` delegates:
  - `make build` builds the guest in Docker, to `methods/guest/target/<triple>/docker/<name>.bin`;
  - `make idl` writes the IDL;
  - `make ffi-gen` writes the FFI, with `SPEL_CLIENT_GEN` pointed at the vendored build.
- **No duplicate program.** That route drops the host copy and the `cargo test` harness, because `spel generate-idl` reads the source (SPEL.md §3.7).

**PR #273** (LEZ v0.3.1) removes the template, `lgs spel`, `build idl`, `build client`, `[repos.spel]` and `[framework.idl]`. Projects that use them "are refused with an explanation; the template can return once spel ships a LEZ 0.3 release" (*specified*). The reason for vendoring SPEL is gone too, because the program id is now the header account.

**Where SPEL and scaffold meet** (*inferred*):
- **No scaffold branch can use SPEL `main`.** SPEL `main` targets v0.2.5-rc2, which no branch pins.
- **If both PRs merged, scaffold would be on LEZ v0.3.1 with no SPEL.** A v0.4 SPEL would come back either by PR #199's route (`spel init` and the Makefile) or by `master`'s (template crates and the harness). PR #199's route is the one that doesn't duplicate the program.
- **SPEL's Basecamp output doesn't fit scaffold's Basecamp input.** SPEL's module generator writes `module.yaml` and `manifest.json` (SPEL.md §3.10). Scaffold's Basecamp commands need a flake exposing `packages.<system>.lgx`, built by `logos-module-builder` 0.3.0 (§5.4). I found nothing that turns one into the other, and the generator never mentions `lgx`.

### 5.4 Basecamp and lgpm

- **The largest part of scaffold, and the one with no LEZ dependency.** It is ~7.6k lines. Basecamp modules are Qt/QML desktop modules that the Logos Basecamp app loads, not LEZ programs (*implied*).
- **The module contract** (docs/basecamp-module-requirements.md, *specified*):
  - a flake exposing `packages.<system>.lgx`, built by `logos-module-builder` 0.3.0;
  - a `metadata.json` with `name` and `dependencies`;
  - modules that bind sockets take their port from an env var.
- **The commands:**
  - `basecamp setup` builds the pinned Basecamp 0.3.0 and `lgpm` with `nix build`, and seeds `alice` and `bob` profiles;
  - `modules` records the project's modules in `[modules.*]` and resolves their dependencies;
  - `install` builds the modules and installs them into both profiles with `lgpm`;
  - `launch <profile>` scrubs the profile, reinstalls, assigns ports, and `exec`s Basecamp with profile-scoped XDG dirs and `LOGOS_USER_DIR`;
  - `build-portable` stages `.#lgx-portable` artefacts in load order, for loading into an AppImage.
- **It needs Nix with flakes** ("nix with flakes enabled, for basecamp subcommands", README).
- **v0.4 leaves it alone**, except where a module talks to a LEZ program through its own client (*inferred*).

### 5.5 risc0 and the guest toolchain

- **The guest builder tag is scaffold's own constant**, `r0.1.97.0`. Scaffold sets it explicitly because risc0-build's default, `r0.1.88.0` with rustc 1.88, "is too old for the template's dependency graph" (`constants.rs:95-118`, *specified*). Changing the tag changes every image id.
- **The sequencer's `r0vm`** is found from the risc0 version in LEZ's `Cargo.lock` (§3.4).

---

## 6. Other things worth knowing, and trade-offs

### 6.1 The pins and the docs lag

- **No release matches a LEZ version jz-progs recommends.** Every release pins LEZ v0.1.2. jz-progs calls v0.2.4 legacy and v0.3 short-lived, and points new work at v0.4 ([README.md](README.md)) (*inferred*).
- **Docs that disagree with the code on `master`:**

| Doc | Says | Code |
|---|---|---|
| ADR.md:248 and :265, FURPS.md:57 | The spel pin is `v0.2.0-rc.5`, which pins LEZ `v0.2.0-rc1` | `v0.5.0` and LEZ `v0.1.2`, since PR #198 (2026-06-11) |
| docs/commands.md:16 | The flags were "verified against `logos-scaffold 0.3.0`" | The crate is 0.4.0 |
| docs/commands.md:172-173 | `build idl` and `build client` use the vendored `spel` binary | `cargo test __lssa_idl_print` and the template's `lez-client-gen` crate |
| README.md:207 | The template is the "LEZ Framework" at `jimmy-claw/lez-framework` | The template's README and `Cargo.toml` use `logos-co/spel` |
| FURPS.md:7, ADR "Network Configuration" | DevNet targeting through env files | Local standalone sequencer only (README:249) |

ADR.md and FURPS.md are records by design (§3.11), which explains part of this. But ADR "Vendoring spel" states the current pin as a fact.

### 6.2 "Committed" doesn't mean the call worked

- **`deploy` on `master` says so itself.** It reports only what the wallet's exit status says (§3.6).
- **`test-node` reports `committed` when the transaction is in a block.** On v0.1.2 a transaction that fails execution isn't included, so there `committed` does mean success (*inferred*, because v0.1.2 has no `is_chargeable`). From v0.2.5 on, a call that panics or runs out of gas is "charged and reverted rather than rejecting the block" (LEZ v0.3.1 `lee/state_machine/src/error.rs`, test comment). So it lands in a block, and `test-node` would report it as `committed` (*inferred*, not run). PR #273 keeps the same outcomes.
- **`dev`'s RPC has no receipt or execution status.** `getTransaction` returns the transaction and its block id (`lez/sequencer/service/rpc/src/lib.rs:73-77`). A client learns of a revert only from the resulting state, or from the sequencer's log (WALKTHROUGH.md). `getPublications` is new on `dev` and may carry events. I haven't checked.
- **SPEL has the same gap** (SPEL.md §0, "Failed calls are included and charged"). On v0.4 both need some way to report a revert.

### 6.3 The image id is built in

On `master`, "program id = image id" runs through:
- the `deploy` output, and the `program_id` field of `--json`;
- `SCAFFOLD_PROGRAM_ID` for hooks;
- the vendored `spel` and its alignment check, which exist to compute it;
- FURPS, on `lgs run`: "when the guest ELF changes, its risc0 image ID changes, and the new program's storage starts empty. Scaffold relies on this for the default cycle and adds no per-run reset" (*specified*);
- the patched `HELLO_WORLD_PROGRAM_ID_HEX`;
- the reproducible-build ADR.

From v0.2.5-rc on, a program is a `program_loader` header account, with a new one per deploy. Deploying an unchanged ELF twice gives two ids and two empty states. So the id has to be recorded rather than recomputed, which is what PR #273's `.scaffold/state/deployments.json` does. On v0.4 an actor is an (account, program) pair, so a redeploy also starts every actor's state from empty (*inferred*).

### 6.4 Coupled to LEZ's internals

Section 5.2 lists what scaffold depends on in LEZ that isn't an interface:
- the example directory;
- a constant inside an example's source;
- the debug configs and their paths;
- `Cargo.lock`;
- the wallet's text output;
- borsh byte offsets.

Each one is cheap to use and breaks without a compile error (*inferred*). PR #273 shows the cost: 86 files and about 12.5k changed lines for one LEZ line.

### 6.5 One release at a time

- **The benefit is one code path**, with defaults that dogfooding has actually run. "A compatibility branch nobody runs is a claim, not a feature" (ADR "Scaffold Supports One Basecamp Release", *specified*).
- **The cost is that a team keeping a v0.2 program alive while porting to v0.4 needs two scaffold versions.** And `init` "cannot port user program code" (PR #273's ADR, *specified*). LEZ went from v0.2.4 to v0.3 to v0.4 within months, so a one-release rule leaves scaffold a migration behind (*inferred*).

### 6.6 Weight

- **The first `setup` builds LEZ's sequencer and wallet, and SPEL, from source.** PR #199's ADR puts SPEL's cache `target/` at about 1.7 GB. The Docker guest build is the default.
- **~38.7k lines of `src/`**, of which Basecamp and the test nodes are about 37%.

### 6.7 What scaffold does well, and should keep

- One inner-loop command with numbered steps, and skips that keep the pipeline's shape visible.
- Commit pins, local builds, nothing on `PATH`, and drift reported by `doctor`.
- `--json` wherever a tool reads the output, with documented shapes.
- A documented hook environment.
- Test-node outcomes that tell committed, rejected, timeout and transport failure apart.
- Reproducible guest builds, checked in CI.
- Dogfooding scenarios and ADRs that record why, including the alternatives rejected.
- A Rust API that mirrors the CLI.

---

## 7. What this means for v0.4

These are observations, not a design.

| Part | `master` (v0.1.2) | PR #273 (v0.3.1) | `dev` (v0.4) | Outcome |
|---|---|---|---|---|
| Pins, cache, `doctor` | Commit pins, built locally | Same | Same mechanism | Keep |
| `new`, default template | LEZ's example plus v0.1.2 runners | LEZ's example plus fee-paying runners | `dev`'s example is already actor code. Its runners need the three fixes on `jzaki/fix-examples`: segment count, fee, admission | Changed. Keep LEZ's runners once they're fixed upstream, or overlay fixed ones |
| Tail-call id patch | Writes the image id | Does nothing | Does nothing | Gone |
| `build`, Docker guests | Image id is the program id | Image id still matters for private execution | Same as v0.3.1 | Keep |
| localnet and test-node start | Config as an argument, patched | `--port --listen-address --home` | `setup <home> --config --funding-public-key`, then `start <home> --rpc-listen-address` | Changed again |
| `wallet topup` | Piñata | `auth-transfer send` from the dev funder | Same funder key. An unused account may need `--to-pk` (*inferred*) | Mostly carries over from PR #273 |
| Deploy | `deploy-program`, paced | `program-loader deploy`, recorded | Same wallet command as v0.3.1 | Carries over from PR #273 |
| Program id for hooks and clients | Image id | Recorded header id | Recorded header id | Carries over from PR #273 |
| Account reads (`test-node account get`, `snapshot accounts`, `state export`) | Balance, owner, data | Shards | Nonce plus actor states, read with `getAccountView(actor)` | Changed: a read names an actor, not an account |
| Wire readers | v0.1.2 layout | v0.3 layout | Must be re-derived | Changed |
| Outcomes | Committed means success (*inferred*) | Committed doesn't mean success | Same, and no receipt RPC | Needs a revert signal |
| Events | None | Not checked | `Response::event`, and the RPC's `getPublications` | New. Test nodes could expose them |
| IDL, clients, `lgs spel` | SPEL v0.5 through the template | Removed | Depends on the SPEL redesign (SPEL.md §7) | Open |
| Basecamp | | | | Unaffected |

### Open questions

1. **Which LEZ line does scaffold move to next?** PR #273 takes it to v0.3.1, which jz-progs advises against porting to. Under the one-release rule, scaffold would stay on v0.3 until another migration of the same size. Is the plan to merge PR #273 and then port to `dev`, or to go straight to v0.4 once it's tagged?
2. **PR #199 or PR #273, on SPEL?** PR #199 makes SPEL the framework template on v0.2.4, and PR #273 removes it. A v0.4 SPEL needs one of the two routes back in (§5.3).
3. **Which tool owns deploy?** `spel deploy` (SPEL.md §3.9), `lgs deploy` and LEZ's runners each create segment and header accounts. On v0.4, one of them should record the program id where the others can read it.
4. **How is a v0.4 project's IDL built?** `spel generate-idl` from source, PR #199's route, avoids the duplicate host copy. A v0.4 program has one handler and one message type, which prog-tui already reads from the source by finding `actor_entrypoint!` ([prog-tui/README.md](prog-tui/README.md)). Does that make a source-read IDL enough?
5. **How should scaffold report a reverted call on v0.4?** From the resulting state, from events, or from a receipt that LEZ would have to add?
6. **Who admits accounts?** On `dev`, a public account that isn't on chain yet needs admission evidence. The templates and `wallet topup` must either supply it or create accounts that are already admitted.
7. **Which LEZ ref should a v0.4 template pin?** `dev` moves and has no tag, and its runners fail without the fixes on `jzaki/fix-examples`.
