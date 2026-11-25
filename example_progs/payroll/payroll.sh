#!/usr/bin/env bash
# Drive the payroll example with private transactions, through spel's private path.
#
#   PROGRAM_PAYROLL=<header id> ./payroll.sh <command> [args]
#
# The wallet is LEE_WALLET_HOME_DIR's (with LEE_WALLET_PASSWORD). The seat is its private account
# labelled $SEAT (default `seat`); it authorizes every call and pays its fee. Employees are private
# accounts of the same wallet, by label.
#
# Commands:
#   treasury                    register the seat's treasury (a private PDA keyed by the seat) as
#                               `treasury`, and print it; every other command needs it
#   fund <amount>               move <amount> from the seat to the treasury
#   open <period-ms>            open the payroll
#   set-employee <emp> <pay>    set an employee's pay per period
#   accrue [period]             accrue pay up to <period> (default: the chain's current period)
#   pay <emp>                   pay an employee what they're owed, sealed to them
#   withdraw <amount>           return <amount> nobody is owed to the seat
#   show                        the treasury's state and the balances
#   period                      the chain's current period
#   audit <amount>              search every transaction's public bytes for <amount>
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPOS=$(cd "$HERE/../../.." && pwd)
SPEL=${SPEL:-$REPOS/spel/target/release/spel}
DEVTOOLS=${DEVTOOLS:-$REPOS/jz-progs/lez-devtools/target/release/lez-devtools}
IDL=$HERE/payroll-idl.json
BIN=${PAYROLL_BIN:-$HERE/target/riscv-guest/payroll-methods/payroll-guest/riscv32im-risc0-zkvm-elf/release/payroll.bin}
SEAT=${SEAT:-seat}
: "${PROGRAM_PAYROLL:?set PROGRAM_PAYROLL to the deployed payroll program}"

# spel, for this program.
sp() { "$SPEL" --idl "$IDL" --binary "$BIN" --program "$PROGRAM_PAYROLL" "$@"; }

# The treasury: the payroll program's private PDA of seed = the seat's id, under the seat's keys.
treasury() { sp pda "account:$SEAT" --owner "$SEAT" --register --label treasury; }

current_period() {
  local period_ms timestamp
  period_ms=$(sp inspect treasury | jq -r .period_ms)
  timestamp=$("$DEVTOOLS" block | cut -f2)
  echo $((timestamp / period_ms))
}

cmd=${1:?usage: payroll.sh <command> [args] (see the header)}
shift
case $cmd in
  treasury) treasury ;;
  fund) treasury >/dev/null; "$SPEL" transfer --from "$SEAT" --to treasury --amount "${1:?amount}" ;;
  open)
    treasury >/dev/null
    sp send --root "$SEAT" -- open --treasury treasury --period-ms "${1:?period-ms}" ;;
  set-employee)
    sp send --root "$SEAT" -- set-employee --employee "${1:?employee}" --amount "${2:?pay}" ;;
  accrue)
    period=${1:-$(current_period)}
    echo "accruing up to period $period" >&2
    sp send --root "$SEAT" -- accrue --period "$period" ;;
  pay)
    # The credit is sealed to the employee, and waits for them (`wallet pending receive`).
    sp send --root "$SEAT" --seal "${1:?employee}" -- pay --employee "$1" ;;
  withdraw)
    # The seat's balance takes part, so its credit applies in this transaction.
    sp send --root "$SEAT" --actor "$SEAT@native" -- withdraw --amount "${1:?amount}" ;;
  show)
    sp inspect treasury
    for account in "$SEAT" treasury $(sp inspect treasury 2>/dev/null | jq -r '.employees[].account'); do
      printf '%s\t%s\n' "$account" "$("$SPEL" inspect "$account@native" 2>/dev/null | jq -r .balance)"
    done ;;
  period) current_period ;;
  audit) "$DEVTOOLS" audit --amount "${1:?amount}" ;;
  *) echo "unknown command '$cmd' (see the header of $0)" >&2; exit 1 ;;
esac
