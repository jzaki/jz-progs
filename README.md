# jz-progs

Notes for writing Logos Execution Zone (LEZ) programs. The target is **v0.4**, the actor model that is now merged into `dev` of [`logos-blockchain/logos-execution-zone`](https://github.com/logos-blockchain/logos-execution-zone). There is no v0.4 tag yet. Checked against `dev` at `e2913677b` (2026-10-14).

## Reading path

1. **[INTRO.md](INTRO.md), section 4.** Build, deploy and call a first actor program locally on `dev`. Includes a known problem with the example runners (they send calls without a fee). Sections 2 and 3 cover the v0.2.4 and v0.3.0 tags if you need them.
2. **The actor-model manual**, written by schouhy: `docs/actor-program-development-book/` on LEZ branch [`schouhy/lez-guide-mdbook`](https://github.com/logos-blockchain/logos-execution-zone/tree/schouhy/lez-guide-mdbook/docs/actor-program-development-book/src) (build with `mdbook`). The model, authorization, transactions, private accounts, patterns, a program catalog and the API cheat sheet. It is the main reference, and these notes link to it instead of repeating it. It uses `Call`/`Cast` and `define_actor_logic!`, which `dev` now calls `SyncMessage`/`AsyncMessage` and `actor_entrypoint!`.
3. **[GUIDE_v0.2_v0.4.md](GUIDE_v0.2_v0.4.md).** Only if you are porting a v0.2 program. What changed, why, a porting checklist, and before/after code for the common patterns (write, authorize, move data, transfer, native funds, depend on another program, call with PDA authority, events).
4. **[MODEL_CHANGES.md](MODEL_CHANGES.md).** History: the v0.3.0 `plan`/`apply` model and the testnet v0.3 notes. Superseded on `dev`.

## What these notes add to the manual

- The v0.2 → v0.4 mapping.
- The `dev` names where the manual is one rename behind.
- A local build-and-run path, with fees and the program loader.
- Where the repo's own example README and runners are out of date.
