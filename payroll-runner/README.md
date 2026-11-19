# payroll-runner

Drives the [payroll example](../example_progs/payroll/README.md) on a LEZ v0.4.0-rc3 chain with private transactions. No CLI at that tag sends a private call to a custom program, so this links the LEZ wallet library by path, with LEZ's `Cargo.lock` and toolchain (Rust 1.98.1), as `lez-dryrun` does. It reads the same wallet as the `wallet` CLI (`LEE_WALLET_HOME_DIR`, `LEE_WALLET_PASSWORD`).

| Command | What it does |
|---|---|
| `import-genesis-private` | Imports the local chain's genesis private account A as `genesis-private`. The first private funds on a fresh chain come from here (see below) |
| `treasury` | Prints the treasury address, `for_private_pda(program, seed_of(seat), seat keys)`. Every command first registers it in the wallet, as the `Pda` kind of the seat's own key node, labelled `treasury` |
| `fund --amount N` | A private native transfer from the seat to the treasury |
| `open --period-ms N` | `Open { treasury, period_ms }` |
| `set-employee <account> --amount N` | `SetEmployee` |
| `accrue [--period P]` | `Accrue`. The default is the period of the latest block's timestamp |
| `pay <account>` | `Pay`. The employee's credit is sealed to their keys (`seal_destination`), and waits for them to collect it |
| `withdraw --amount N` | `Withdraw`. The seat's balance takes part, so the credit applies in the same transaction |
| `show` | The treasury's decrypted state, plus the native balances the wallet knows |
| `period` | The chain's current period |
| `audit --amount N [--from-block B]` | Searches every transaction's public bytes for `N` as 16 little-endian bytes |

Shared options:
- `--program`, or `$PROGRAM_PAYROLL`: the deployed header id;
- `--seat`: label or id, default `seat`;
- `--elf`: the guest, default the payroll crate's release build.

Each call waits for its block, fails if the transaction reverted, and syncs the wallet's private accounts. Accounts are wallet labels, `Private/<id>`, or ids. Employees must be private accounts of this wallet, because `pay` takes their public keys from it.

## A tested session

This ran on a fresh local chain (`just clean`, then `RISC0_DEV_MODE=1 just run-sequencer-standalone`), on 2026-11-19. Build everything first, before starting the sequencer, since heavy builds starve its executor.

```bash
(cd ../example_progs/payroll && cargo build --release -p payroll-methods)
cargo build --release
R=$PWD/target/release/payroll-runner
source ../prog-tui/.state/env.sh          # LEE_WALLET_HOME_DIR, LEE_WALLET_PASSWORD, LEZ_WALLET; wallet set up (prog-tui `i`)
export RISC0_DEV_MODE=1                   # the wallet proves private transactions: without it, a real proof takes many minutes
W=$LEZ_WALLET
```

**Deploy.** `payroll.bin` needs 3 segments:

```bash
for l in pay-hdr pay-seg0 pay-seg1 pay-seg2; do $W account new public --label $l; done
$W program-loader deploy --elf ../example_progs/payroll/target/riscv-guest/payroll-methods/payroll-guest/riscv32im-risc0-zkvm-elf/release/payroll.bin \
  --header pay-hdr --segments pay-seg0 pay-seg1 pay-seg2 --payer Public/$PAYER
export PROGRAM_PAYROLL=<header id>
```

**Private funds.** A private transaction's fee comes from a private account, which rules out shielding from a public account into a fresh private one: the recipient would pay, and it holds nothing. `--to-keys` doesn't help, because publishing a recovery binding makes the transaction private too. Genesis does fund two private accounts, with pending credits, so:

```bash
$W account new private --label seat
$W account new private --label alice
$W account new private --label bob
$R import-genesis-private
$W pending list                           # the genesis credit to genesis-private
$W pending receive --position <N>
$W auth-transfer send --from genesis-private --to seat --amount 500000000
```

The seat pays every payroll transaction's fee. That was about 5M each on the dev chain, so give it plenty.

**Payroll:**

```bash
$R open --period-ms 10000
$R fund --amount 500000
$R set-employee alice --amount 12345
$R set-employee bob --amount 23456
$R accrue                                 # owed: alice 12345, bob 23456
$R accrue --period <same>                 # refused before sending: "period … is already accrued"
$R accrue --period <current + 1000>       # not included: "Execution outside of the validity window" (sequencer log)
$R accrue --period <current + 1>          # once it has begun: owed 24690 and 46912
$R pay alice && $R pay bob                # owed 0, paid 24690 / 46912, treasury 428398
$R show
```

**A failed payment loses nothing.** Pay Alice 10,000,000 a period, accrue, and pay her with only 428,398 in the treasury:
- `pay alice` is refused before sending ("Program violated execution rules": the native debit's insufficient balance). Her owed amount, the treasury balance and the seat's balance are all unchanged.
- `pay bob` still goes through.
- `fund --amount 10000000`, then `pay alice` again, succeeds.

**Employees collect.** `$W pending list` shows one sealed credit per payment. Each comes from a different blinded alias of the treasury, so the payments can't be linked to it or to each other. `$W pending receive --position <N>` collects each one, and `$W account get --account-id alice` then shows exactly what she was paid.

**Privacy check.** As a control, a public transfer of 777777 is found by `audit --amount 777777`. Every payroll amount came back "not found" in blocks 1 to 681:
- pay rates 12345 and 23456;
- payments 24690, 46912 and 10000000;
- totals 71602, 10024690 and 70368;
- the treasury funding, 500000.

**Withdraw.** With 35801 owed and 404942 held:
- `withdraw --amount 369142` is refused ("exceeds the 369141 nobody is owed");
- `withdraw --amount 369141` leaves the treasury holding exactly 35801.

## What stays public

- That private transactions happened, with their nullifiers, commitments and fees.
- The program's image (`ProgramCatalog::from` discloses it).
- Funds entering private accounts: here, only genesis.
- The approximate size of the treasury state. The wallet pads notes to 512 bytes, and a payroll with more than about five employees outgrows that.
