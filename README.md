# jz-progs

Notes for writing Logos Execution Zone (LEZ) programs. The target is **v0.4**, the actor model that is now merged into `dev` of [`logos-blockchain/logos-execution-zone`](https://github.com/logos-blockchain/logos-execution-zone). There is no v0.4 release yet. The build-and-run path uses the pre-release tag **`v0.4.0-rc3`**, with [lez-programs](https://github.com/logos-blockchain/lez-programs)' programs and spel's `refactor/lez-v0.4.x` branch, which pin the same tag (checked 2026-11-17).

## Which version?

| Version | Status | What to do |
|---|---|---|
| **0.4**, actor model | **How programs will be written.** On `dev`, with a pre-release tag (`v0.4.0-rc3`) and no testnet yet | Build new programs here |
| **0.3**, `plan`/`apply` | The current testnet, but **very short-lived** | Don't start on it. For a test deployment, use 0.2 for now |
| **0.2.4** | **Legacy** | Only for maintaining existing programs and test deployments. Port to 0.4 when available |

Programs written for 0.2 or 0.3 will need porting to 0.4. The program interface differs, not just the names.

## Reading path

1. **[INTRO.md](INTRO.md), section 4.** Build lez-programs' token program against LEZ `v0.4.0-rc3`, deploy it locally, and call it with spel. Sections 2 and 3 cover the v0.2.4 and v0.3.0 tags, including deploying to their public testnets. Testnet RPCs and explorers come from `docs/get-started/testnet-endpoints.md` on `logos-docs` branch `docs/zone-endpoint-ref`. 0.4 has no testnet yet.
2. **[example_progs/WALKTHROUGH.md](example_progs/WALKTHROUGH.md).** Only the commands of INTRO.md section 4, in the order you run them, from a clean chain to a token whose supply sits on your account. It assumes the toolchain is installed and skips the explanations. Use it as a checklist once you've read INTRO.md.
3. **[prog-tui](prog-tui/README.md).** A terminal UI that runs those steps for you, and lets you deploy and call lez-programs' programs (token, nft). It starts and resets the local sequencer, shows the accounts and each program's actor state, and writes an `env.sh` with `PAYER`, `ACCOUNT` and the program ids for your own shell. Run it with `cargo run --release` from `prog-tui/`. It calls lez-programs' programs with spel, from their IDLs. You can also put your own single-file guests in `example_progs/` (`hello_world.rs` adapts LEZ's) and build and deploy them with `u`. prog-tui builds them in a package it keeps in the lez-programs checkout, and calls them with spel too, from IDLs it generates. A guest spel can't describe falls back to [`lez-call`](https://github.com/jzaki/lez-prog-call) and [`lez-dryrun`](lez-dryrun/README.md), which need the wallet's `program call` subcommand to send (on `dev`, not in `v0.4.0-rc3`). Its README says how.
4. **The actor-model manual**, written by schouhy: `docs/actor-program-development-book/` on LEZ branch [`moudy/guide-mdbook-fixes`](https://github.com/logos-blockchain/logos-execution-zone/tree/moudy/guide-mdbook-fixes/docs/actor-program-development-book/src), which fixes schouhy's `schouhy/lez-guide-mdbook` (build with `mdbook`). The model, authorization, transactions, private accounts, patterns, a program catalog and the API cheat sheet. It is the main reference, and these notes link to it instead of repeating it. It uses `Call`/`Cast` and `define_actor_logic!`, which `dev` now calls `SyncMessage`/`AsyncMessage` and `actor_entrypoint!`. The same branch still has the plan/apply book it was copied from (`docs/program-development-book/`). [mdbook-diffs.md](mdbook-diffs.md) lists what changed between the two, chapter by chapter, and which parts of the actor book are not on `dev`.
5. **[GUIDE_v0.2_v0.4.md](GUIDE_v0.2_v0.4.md).** Only if you are porting a v0.2 program. What changed, why, a porting checklist, and before/after code for the common patterns (write, authorize, move data, transfer, native funds, depend on another program, call with PDA authority, events).
6. **[SPEL.md](SPEL.md).** SPEL, the Anchor-like framework that lez-programs uses, as it is today. It targets LEZ `v0.2.5-rc2`, a release candidate from before v0.3, and has no actor-model support. It covers what SPEL solves, how the macros, IDL, CLI and extensions work (spel-admin-authority in depth), code with and without SPEL, the trade-offs, and what carries over to v0.4.
7. **[SCAFFOLD.md](SCAFFOLD.md).** `logos-scaffold` (`lgs`), the CLI that runs the build, localnet, fund and deploy loop for you, as it is today. Every release pins LEZ `v0.1.2`, and its open PRs move it to v0.2.4 or v0.3.1, not v0.4. It covers what scaffold solves, how its commands and templates work, the workflow with and without it, how it integrates LEZ, SPEL and Basecamp, the trade-offs, and what carries over to v0.4.
8. **[MODEL_CHANGES.md](MODEL_CHANGES.md).** History: the v0.3.0 `plan`/`apply` model and the testnet v0.3 notes. Superseded on `dev`.

## What these notes add to the manual

- The v0.2 → v0.4 mapping.
- The `dev` names where the manual is one rename behind.
- What the actor manual changed from the plan/apply manual ([mdbook-diffs.md](mdbook-diffs.md)).
- A local build-and-run path for lez-programs' programs, with fees, the program loader and spel, as prose (INTRO.md), as a command list (WALKTHROUGH.md) and as a TUI (`prog-tui`).
- Where the repo's own example README and runners are out of date.
