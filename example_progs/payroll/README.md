# Payroll

A LEZ v0.4 (actor model) program: a private treasury pays employees private amounts. It is laid out like lez-programs' `programs/token`. Unlike the single-file examples next to it, prog-tui doesn't build it.

| Path | What |
|---|---|
| `core/` (`payroll_core`) | `Message` (to the seat), `Internal` (seat to treasury), `SeatState`, `PayrollState`, `seed_of` |
| `src/` (`payroll_program`) | `handle_message`, plus one file per message, holding both the seat's step and the treasury's, with its unit tests |
| `methods/` (`payroll-methods`) | the one-line guest, `actor_entrypoint!(raw payroll_program::handle_message)` |

The crate docs in `src/lib.rs` explain the design. In short:

- **Every party is private.** The seat is the program's actor on its owner's private account. The treasury is a private PDA, `for_private_pda(program, seed_of(seat), seat_npk, seat_vpk)`. Employees are private accounts.
  - Only the seat's keys can put the treasury in a transaction, so only the seat sees its state: employees, pay, and what each is owed and was paid.
  - Payments come from private execution, so each employee's credit is published sealed. Run on a local chain, no amount appeared in any transaction's public bytes (see payroll-runner's `audit`).
  - It emits no events, because an event would publish the amounts.
- **Accrue, then pay.** `Accrue { period }` adds each period's pay to what every employee is *owed*. No funds move, and a timestamp window holds the transaction until the period begins.
  - `Pay { employee }` moves one employee's `owed` to `paid` in the same transaction as the custody transfer that pays it.
  - LEZ can't bounce a stored async credit, so everything that can fail must fail before the credit is stored: an insufficient balance, an unknown employee, nothing owed, a missing seal. Any of these reverts the whole transaction, so the funds stay in the treasury and the pay stays owed, ready to retry.
- **`Withdraw`** returns only what nobody is owed. The treasury reads its own native balance (`ReadState`, answered in the same transaction) and pays out at most `balance - total_owed`.

## Build and test

```bash
cd jz-progs/example_progs/payroll
RISC0_DEV_MODE=1 cargo test -p payroll_program         # handler unit tests
cargo clippy -p payroll_program -p payroll_core --all-targets
cargo build --release -p payroll-methods               # the guest, local RISC Zero toolchain, no Docker
# -> target/riscv-guest/payroll-methods/payroll-guest/riscv32im-risc0-zkvm-elf/release/payroll.bin
```

`Cargo.lock` and `rust-toolchain.toml` (Rust 1.94.0) come from lez-programs, as do the lints. `lee_core` comes from git tag `v0.4.0-rc3`, the same as lez-programs.

## Run it on a chain

No CLI at LEZ v0.4.0-rc3 sends a private call to a custom program: there's no `wallet program call`, and spel and lez-call are public-only. [`jz-progs/payroll-runner`](../../payroll-runner/README.md) drives it through the wallet library, and its README has the session this was tested with.
