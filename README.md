# jz-progs

Rolling info towards upcoming LEZ program updates.

INTRO.md and ongoing changes.

MODEL_CHANGE.md whats coming...

Ten scenarios, each with code from both versions:
- A. Writing to an account: the old version rewrote the whole account; the new one changes only the program's own data slot.
- B. Requiring authorization: almost the same code, but the engine now decides who is authorized.
- C. Moving data between accounts: the instruction now has to carry the bytes, and the program checks they were really there.
- D. Token transfer: the instruction carries the token's definition, and each side checks its own holding against it.
- E. Moving native balance: the old version edited balances directly; the new one calls the native-token program, and only from accounts the program controls.
- F. Creating an account: "claim a blank account" becomes "my data slot on it is empty".
- G. Calling another program: callers no longer pass account values, and callees aren't identified by a hard-coded ID.
- H. Depending on another program's state: the program states what it expects, and the owning program verifies it with plan.inspect. This is the main new rule for reviewers.
- I. Events, which are new.
- J. Deploying and submitting, which now cost fees and need a payer

## Towards v0.4 (research notes, 2026-10-09)

- ACTOR_MODEL_0.4.md: the "transactional actors" model in draft PR #935, how it maps from plan/apply and shards, what breaks, stated benefits. Public sources only.
- CORE_API_STABILITY.md: will the `lee_core` API (and the wallet/FFI layers above it) change? Module by module.
- PRIVATE_TX_CONTENTION.md: was the one-private-tx limit lifted in v0.3? Partially; what remains and what actors change.

