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
  - Payments come from private execution, so each employee's credit is published sealed. Run on a local chain, no amount appeared in any transaction's public bytes (see `payroll.sh audit`).
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

[`payroll.sh`](payroll.sh) drives it with **spel's private calls** (spel branch `feat/private-calls`), one spel command per step. It needs spel built at `../../../spel` and [`lez-devtools`](../../lez-devtools/README.md) for the local-chain helpers; the IDL is `payroll-idl.json` (`spel generate-idl core --message Message --state PayrollState`).

```bash
export LEE_WALLET_HOME_DIR=… LEE_WALLET_PASSWORD=… RISC0_DEV_MODE=1
export PROGRAM_PAYROLL=$(spel deploy target/riscv-guest/…/payroll.bin --fee-payer <funded public account>)
./payroll.sh treasury        # spel pda account:seat --owner seat --register --label treasury
./payroll.sh fund 1000000    # spel transfer --from seat --to treasury
./payroll.sh open 60000      # spel send --root seat -- open --treasury treasury --period-ms 60000
./payroll.sh set-employee emp1 1234
./payroll.sh accrue          # up to the chain's current period
./payroll.sh pay emp1        # spel send --root seat --seal emp1 -- pay --employee emp1
./payroll.sh withdraw 500000 # spel send --root seat --actor seat@native -- withdraw --amount 500000
./payroll.sh show
./payroll.sh audit 1234
```

Each `send` names only the seat: spel finds the treasury from the wallet's simulation (`+ (treasury …) reached in simulation, private`) and prints the equivalent fully declared command. The seat's own balance is declared for `withdraw`, since an async credit isn't found by simulation.

**Tested 2026-11-20** on a fresh local chain (`RISC0_DEV_MODE=1 just run-sequencer-standalone`), one wallet with private accounts `seat`, `emp1`, `emp2`, funded from the genesis private account (`lez-devtools import-genesis-private`, `wallet pending receive`, `spel transfer`):
- open, two employees (1,234 and 2,500 per period), fund 1,000,000, accrue one period;
- `pay emp1`: the treasury dropped to 998,766, and emp1's credit was pending from a blinded alias, not the treasury; `wallet pending receive` collected 1,234;
- `withdraw 997000` was refused in simulation and not sent ("exceeds the 996266 nobody is owed", exit 2); `withdraw 500000` left the treasury at 498,766;
- `audit`: 1234, 2500, 998766, 500000 and 498766 were all "not found" in public bytes.

