#!/usr/bin/env bash
# Drive the multisig example with private transactions, through spel's private path.
#
#   PROGRAM_MULTISIG=<header id> MULTISIG_ME=<name> MULTISIG_MAILBOX=<dir> ./multisig.sh <command> [args]
#
# Each member runs it with their own wallet (LEE_WALLET_HOME_DIR, LEE_WALLET_PASSWORD). Their seat is
# the wallet's private account labelled $SEAT (default `seat`): it authorizes every call and pays
# its fee. $VAULT (default `vault`) names the vault in the mailbox and in this member's record.
#
# The mailbox stands in for out-of-band messages between members. It holds:
#   <vault>.vault            the vault's placement and create_key (announce)
#   <vault>.member.<name>    each member's pseudonym (pseudonym)
#   keys.<id>                a private payee's public keys (post-keys)
#   <name>.sealing-pk        a member's sealing public key, for `wallet group invite --key`
# Group secrets travel sealed: `wallet group invite` / `wallet group join`.
#
# This member's record of the vault they act on is $LEE_WALLET_HOME_DIR/multisig-<vault>.txt. After a
# hidden vault rotates, members holding the new group secret follow it on their next command.
#
# Commands:
#   init-seat                          write the seat's random secret, once
#   announce public|hidden             pick the vault's create_key and post it
#   pseudonym                          post this member's pseudonym for the announced vault
#   sealing-key                        post this wallet's sealing public key
#   join [group]                       register the announced vault (a hidden one needs its group)
#   create <threshold> <name>...       create the vault with these members (mailbox names)
#   fund <amount>                      move <amount> from the seat into the vault
#   propose transfer <to> <amount>     <to>: a label, Private/<id>, Public/<id> or an id
#   propose add|remove <name> <threshold> [successor-group]
#                                      a successor group rotates a hidden vault to its next generation
#   approve <id>                       approve a proposal; the approval meeting the threshold runs it
#   sweep [retired-vault]              forward a retired vault's balance to its successor
#   post-keys <account>                post a private account's public keys, so members can pay it
#   show                               the vault, its members, proposals and balance
#   audit (--amount N | --account ID)  search every transaction's public bytes
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPOS=$(cd "$HERE/../../.." && pwd)
SPEL=${SPEL:-$REPOS/spel/target/release/spel}
WALLET=${WALLET:-$REPOS/logos-execution-zone/target/release/wallet}
DEVTOOLS=${DEVTOOLS:-$REPOS/jz-progs/lez-devtools/target/release/lez-devtools}
IDL=$HERE/multisig-idl.json
BIN=${MULTISIG_BIN:-$HERE/target/riscv-guest/multisig-methods/multisig-guest/riscv32im-risc0-zkvm-elf/release/multisig.bin}
SEAT=${SEAT:-seat}
VAULT=${VAULT:-vault}
: "${PROGRAM_MULTISIG:?set PROGRAM_MULTISIG to the deployed multisig program}"
MB=${MULTISIG_MAILBOX:?set MULTISIG_MAILBOX to the shared mailbox directory}
mkdir -p "$MB"

die() { echo "multisig: $*" >&2; exit 1; }
sp() { "$SPEL" --idl "$IDL" --binary "$BIN" --program "$PROGRAM_MULTISIG" "$@"; }
me() { echo "${MULTISIG_ME:?set MULTISIG_ME to the mailbox name of this member}"; }
random32() { head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n'; }

# key=value files: the mailbox's vault announcement and this member's record.
field() { sed -n "s/^$1=//p" "$2"; }
record() { echo "$LEE_WALLET_HOME_DIR/multisig-$VAULT.txt"; }
write_record() { # placement create_key generation group vault
  printf 'placement=%s\ncreate_key=%s\ngeneration=%s\ngroup=%s\nvault=%s\n' "$@" > "$(record)"
}
load() {
  [ -f "$(record)" ] || die "no record of \`$VAULT\` in this wallet: run join first"
  PLACEMENT=$(field placement "$(record)"); CK=$(field create_key "$(record)")
  GEN=$(field generation "$(record)"); GROUP=$(field group "$(record)"); V=$(field vault "$(record)")
}

# The vault's PDA seed: AccountId(create_key) blinded by its generation (multisig_core::seed_for).
seed() { "$SPEL" blind "0x$1" "le64:$2" --hex; }
# A hidden vault: generation $3 of create_key $2 under group $1's secret. $4 = register.
hidden_vault() {
  if [ "${4:-}" = register ]; then
    sp pda "$(seed "$2" "$3")" --group "$1" --register --label "$VAULT-gen$3" ${5:+--rescan} 2>/dev/null
  else
    sp pda "$(seed "$2" "$3")" --group "$1" 2>/dev/null
  fi
}
# The wallet group whose secret derives $3 as generation $2 of create_key $1, if any.
group_deriving() {
  local group
  for group in $("$WALLET" group list 2>/dev/null | grep -v '^No groups found$'); do
    [ "$(hidden_vault "$group" "$1" "$2")" = "$3" ] && { echo "$group"; return; }
  done
  return 0
}

# The vault's state as JSON (decrypted, for a hidden vault).
state() { sp inspect "$1" 2>/dev/null; }

# This member's record, after following any rotation they can follow.
current() {
  load
  local successor next group
  while :; do
    successor=$(state "$V" | jq -r '.retired // empty' 2>/dev/null || true)
    [ -n "$successor" ] || return 0
    next=$((GEN + 1))
    group=$(group_deriving "$CK" "$next" "$successor")
    if [ -z "$group" ]; then
      echo "vault $V is retired, succeeded by $successor, which this wallet can't see" >&2
      return 0
    fi
    hidden_vault "$group" "$CK" "$next" register rescan >/dev/null
    write_record "$PLACEMENT" "$CK" "$next" "$group" "$successor"
    echo "followed the rotation to generation $next: $successor" >&2
    load
  done
}

my_pseudonym() {
  local secret factor
  secret=$("$SPEL" --program "$PROGRAM_MULTISIG" inspect "$SEAT" 2>/dev/null | tail -1)
  [ "$secret" != null ] && [ -n "$secret" ] || die "the seat isn't initialised (init-seat)"
  # multisig_core::pseudonym: seat.blinded(AccountId(secret).blinded(create_key))
  factor=$("$SPEL" blind "0x$secret" "0x$1")
  "$SPEL" blind "$SEAT" "$factor"
}
member() { cat "$MB/$VAULT.member.$1" 2>/dev/null || die "no pseudonym from \`$1\` in the mailbox"; }
# Pseudonym → mailbox name, for display.
name_of() {
  local file
  for file in "$MB/$VAULT".member.*; do
    [ "$(cat "$file" 2>/dev/null)" = "$1" ] && { echo "${file##*.member.}"; return; }
  done
  echo "$1"
}
# An account id, from a label, Private/<id>, Public/<id> or an id.
account_id() {
  case $1 in Private/*|Public/*) echo "${1#*/}" ;; *) "$WALLET" account id --account-id "$1" 2>/dev/null | tail -1 ;; esac
}

# What an action needs when it runs in this transaction.
#   Transfer: its payee's credit, sealed from a hidden vault, with a recovery binding from a public
#   one (public execution can't seal). A public payee is declared instead.
#   A rotation: the successor's balance, which the vault's whole balance moves to.
extras() {
  local action=$1 to flag
  case $(jq -r 'keys[0]' <<<"$action") in
    Transfer)
      to=$(jq -r '.Transfer.to' <<<"$action")
      flag=--seal; [ "$PLACEMENT" = public ] && flag=--recover
      if [ -f "$MB/keys.$to" ]; then echo "$flag keys:$MB/keys.$to"
      elif "$WALLET" account show-keys --account-id "Private/$to" >/dev/null 2>&1; then echo "$flag Private/$to"
      else echo "$flag Public/$to"; fi ;;
    AddMember|RemoveMember)
      to=$(jq -r '.[].successor // empty' <<<"$action")
      [ -z "$to" ] || echo "--actor $to@native" ;;
  esac
}

# Before approving a rotation: the successor must derive from a group secret this member holds,
# or the funds would move to a vault they can't verify. Registers it on a match.
check_successor() {
  local successor next group
  successor=$(jq -r '.[].successor // empty' <<<"$1")
  [ -n "$successor" ] || return 0
  next=$((GEN + 1))
  group=$(group_deriving "$CK" "$next" "$successor")
  [ -n "$group" ] || die "refusing to approve: the successor $successor doesn't derive from any group secret this wallet holds (join the new group first)"
  hidden_vault "$group" "$CK" "$next" register >/dev/null
  echo "the successor $successor is generation $next of group \`$group\`: checked" >&2
}

# Send <message> to the seat, adding what <action> needs if this call runs it.
act() {
  local runs=$1 action=$2; shift 2
  local extra=()
  if [ "$runs" = yes ]; then
    echo "this call meets the threshold: the action runs now" >&2
    read -r -a extra <<<"$(extras "$action")"
  fi
  sp send --root "$SEAT" "${extra[@]}" -- "$@"
}

placement_variant() { [ "$1" = public ] && echo Public || echo Hidden; }

cmd=${1:?usage: multisig.sh <command> [args] (see the header)}
shift
case $cmd in
  init-seat) sp send --root "$SEAT" -- init-seat --secret "$(random32)" ;;
  announce)
    placement=${1:?public or hidden}
    [ "$placement" = public ] || [ "$placement" = hidden ] || die "placement is public or hidden"
    [ ! -e "$MB/$VAULT.vault" ] || die "$MB/$VAULT.vault is already announced"
    printf 'placement=%s\ncreate_key=%s\n' "$placement" "$(random32)" > "$MB/$VAULT.vault"
    echo "announced \`$VAULT\` ($placement) in $MB/$VAULT.vault" ;;
  pseudonym)
    p=$(my_pseudonym "$(field create_key "$MB/$VAULT.vault")")
    echo "$p" > "$MB/$VAULT.member.$(me)"
    echo "$(me)'s pseudonym at \`$VAULT\`: $p" ;;
  sealing-key)
    "$DEVTOOLS" sealing-key > "$MB/$(me).sealing-pk"
    echo "posted $(me)'s sealing public key to $MB/$(me).sealing-pk" ;;
  join)
    placement=$(field placement "$MB/$VAULT.vault"); ck=$(field create_key "$MB/$VAULT.vault")
    if [ "$placement" = public ]; then
      v=$(sp pda "$(seed "$ck" 0 | sed 's/^0x//')" 2>/dev/null)
      write_record public "$ck" 0 "" "$v"
    else
      group=${1:?a hidden vault needs the group holding its secret}
      # The vault may already be on chain: read its history from genesis.
      v=$(hidden_vault "$group" "$ck" 0 register rescan) || die "no wallet group \`$group\` (wallet group join)"
      write_record hidden "$ck" 0 "$group" "$v"
    fi
    echo "vault \`$VAULT\`: $v" ;;
  create)
    load
    threshold=${1:?threshold}; shift
    [ $# -gt 0 ] || die "name the members"
    members=$(for m in "$@"; do member "$m"; done | paste -sd,)
    sp send --root "$SEAT" -- create --vault "$V" --placement "$(placement_variant "$PLACEMENT")" \
      --create-key "$CK" --members "$members" --threshold "$threshold" ;;
  fund) current; "$SPEL" transfer --from "$SEAT" --to "$V" --amount "${1:?amount}" ;;
  propose)
    current
    st=$(state "$V") || die "the vault isn't created yet"
    kind=${1:?transfer, add or remove}; shift
    case $kind in
      transfer)
        to=$(account_id "${1:?payee}"); [ -n "$to" ] || die "no account \`$1\`"
        action=$(jq -nc --arg to "$to" --arg amount "${2:?amount}" '{Transfer: {to: $to, amount: $amount}}') ;;
      add|remove)
        m=$(member "${1:?member}"); threshold=${2:?threshold}; successor=null
        if [ -n "${3:-}" ]; then
          [ "$PLACEMENT" = hidden ] || die "a public vault changes members in place"
          successor=$(hidden_vault "$3" "$CK" $((GEN + 1)) register) || die "no wallet group \`$3\`"
          echo "successor (generation $((GEN + 1)), group \`$3\`): $successor" >&2
          successor="\"$successor\""
        fi
        variant=AddMember; [ "$kind" = remove ] && variant=RemoveMember
        action=$(jq -nc --arg v "$variant" --arg m "$m" --argjson t "$threshold" --argjson s "$successor" \
          '{($v): {member: $m, threshold: $t, successor: $s}}') ;;
      *) die "propose transfer|add|remove" ;;
    esac
    echo "proposing #$(jq -r .next_id <<<"$st"): $action" >&2
    runs=no; [ "$(jq -r '.config.threshold | tonumber' <<<"$st")" -le 1 ] && runs=yes
    act "$runs" "$action" propose --vault "$V" --create-key "$CK" --action "$action" ;;
  approve)
    current
    id=${1:?proposal id}
    st=$(state "$V") || die "the vault isn't created yet"
    proposal=$(jq -c --arg id "$id" '.proposals[] | select(.id == $id or (.id|tostring) == $id)' <<<"$st")
    [ -n "$proposal" ] || die "no pending proposal #$id"
    action=$(jq -c .action <<<"$proposal")
    echo "approving #$id: $action" >&2
    check_successor "$action"
    approvals=$(jq '.approvals | length' <<<"$proposal")
    runs=no; [ $((approvals + 1)) -ge "$(jq -r '.config.threshold | tonumber' <<<"$st")" ] && runs=yes
    act "$runs" "$action" approve --vault "$V" --create-key "$CK" --id "$id" ;;
  sweep)
    load
    [ -z "${1:-}" ] || V=$(account_id "$1")
    successor=$(state "$V" | jq -r '.retired // empty')
    [ -n "$successor" ] || die "vault $V isn't retired"
    sp send --root "$SEAT" --actor "$successor@native" -- sweep --vault "$V" --create-key "$CK" ;;
  post-keys)
    id=$(account_id "${1:?account}")
    "$WALLET" account show-keys --account-id "Private/$id" 2>/dev/null | tail -2 > "$MB/keys.$id"
    echo "posted the keys of Private/$id" ;;
  show)
    current
    balance=$("$SPEL" inspect "$V@native" 2>/dev/null | jq -r .balance)
    echo "vault \`$VAULT\` ($PLACEMENT, generation $GEN): $V  balance $balance"
    st=$(state "$V") || { echo "  not created yet"; exit 0; }
    names=$(jq -r '.config.members[]' <<<"$st" | while read -r p; do name_of "$p"; done | paste -sd, | sed 's/,/, /g')
    echo "  $(jq -r .config.threshold <<<"$st")-of-$(jq '.config.members | length' <<<"$st"): $names"
    retired=$(jq -r '.retired // empty' <<<"$st")
    [ -z "$retired" ] || echo "  RETIRED: succeeded by $retired"
    jq -c '.proposals[]' <<<"$st" | while read -r p; do
      approvals=$(jq -r '.approvals[]' <<<"$p" | while read -r a; do name_of "$a"; done | paste -sd, | sed 's/,/, /g')
      echo "  #$(jq -r .id <<<"$p") $(jq -c .action <<<"$p")  approved by $approvals"
    done ;;
  audit) "$DEVTOOLS" audit "$@" ;;
  *) die "unknown command '$cmd' (see the header of $0)" ;;
esac
