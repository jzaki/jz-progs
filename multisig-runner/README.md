# multisig-runner

Drives the [multisig example](../example_progs/multisig/README.md) on a LEZ v0.4.0-rc3 chain with private transactions. No CLI at that tag sends a private call to a custom program, so this links the LEZ wallet library by path, with LEZ's `Cargo.lock` and toolchain (Rust 1.98.1), as `payroll-runner` does.

**Each member runs it with their own wallet** (`LEE_WALLET_HOME_DIR`, `LEE_WALLET_PASSWORD`). Their seat is a private account of that wallet (`--seat`, default `seat`). It signs every call and pays its fee.

**Members share what they must through a mailbox** (`--mailbox` / `$MULTISIG_MAILBOX`), a directory standing in for out-of-band messages. It holds:
- a vault's placement and `create_key` (`<vault>.vault`);
- each member's pseudonym (`<vault>.member.<name>`);
- sealing public keys (`<name>.sealing-pk`);
- a private payee's public keys (`keys.<id>`).

Group secrets travel sealed: `wallet group invite` / `join`. Each member keeps a local record of the vault they act on (`$LEE_WALLET_HOME_DIR/multisig-<vault>.txt`). After a hidden vault rotates, members who hold the new group secret follow it on their next call.

| Command | What it does |
|---|---|
| `import-genesis-private` | Imports the local chain's genesis private account A as `genesis-private`, to bootstrap private funds |
| `init-seat` | Writes the seat's random secret, once |
| `announce --placement hidden\|public` | Picks the vault's `create_key` and posts it |
| `pseudonym` | Computes this member's pseudonym for the announced vault and posts it |
| `sealing-key` | Posts this wallet's sealing public key (made by `wallet group new-sealing-key`) |
| `join [--group G]` | Registers the vault. A hidden vault derives from group G's secret and is added to the wallet as a shared private PDA; it rescans history, since the vault may already be on chain |
| `create --members a,b,c --threshold M` | `Create`, with members named in the mailbox |
| `fund --amount N` | A transfer from the seat into the vault |
| `propose transfer --to X --amount N` | Propose a transfer. `X` is a label, `Private/<id>` (keys from this wallet or the mailbox), or a public id |
| `propose add\|remove --member NAME --threshold M [--successor-group G2]` | With `--successor-group`, a hidden vault rotates to the generation G2 derives |
| `approve <id>` | Approves. If this approval meets the threshold, it adds what the action needs (balances, the payee, the successor). Before approving a rotation, it checks that the successor derives from a group this wallet holds, and refuses otherwise |
| `sweep [--retired vault-gen0]` | Forwards a retired vault's balance to its successor |
| `post-keys <account>` | Posts a private account's public keys, so any member can pay it |
| `show` | Shows the vault (decrypted, or public): members by mailbox name, threshold, proposals with approvals, balance, and retired → successor |
| `audit (--amount N \| --account ID)` | Searches every transaction's public bytes for an amount (16 bytes LE) or an account id |

`--program` / `$PROGRAM_MULTISIG` is the deployed header id; `--me` / `$MULTISIG_ME` is this member's mailbox name; `--vault` is the vault's name, default `vault`. Each call waits for its block, fails if the transaction reverted, and syncs the wallet's private accounts.

## Tested sessions

These ran on 2026-11-20, each on a fresh local chain (`RISC0_DEV_MODE=1 just run-sequencer-standalone`, with `sequencer_home` removed in between). Four wallets: alice, bob, carol and dave.

Build everything before starting the sequencer, since heavy builds starve its executor:

```bash
(cd ../example_progs/multisig && cargo build --release -p multisig-methods)
cargo build --release
```

A helper puts each member in their own wallet:

```bash
export LEE_WALLET_PASSWORD=multisig RISC0_DEV_MODE=1 MULTISIG_MAILBOX=$PWD/mailbox
W=$LEZ/target/release/wallet R=$PWD/target/release/multisig-runner
as() { local who=$1; shift; LEE_WALLET_HOME_DIR=$HOMES/$who MULTISIG_ME=$who "$@"; }   # each home holds a copy of wallet_config.json
```

**Setup.**
1. Every wallet: `check-health` and `account new private --label seat`.
2. alice imports a genesis test public key and deploys `multisig.bin` with 3 segments (`program-loader deploy`).
3. Private funds:
   - alice runs `$R import-genesis-private` and collects its pending credit.
   - She sends 500M to her seat, and to each other seat with `auth-transfer send --to-keys` (`wallet account show-keys` gives the keys).
   - Each member collects with `wallet pending receive`.
4. Every member runs `$R init-seat`.

### Session 1: a hidden vault

**Your flow, 2-of-3.**
```bash
as alice $R announce --placement hidden
for m in alice bob carol dave; do as $m $R pseudonym; done
as alice $W group new g1
for m in bob carol dave; do as $m $W group new-sealing-key; as $m $R sealing-key; done
as alice $W group invite g1 --key $(cat mailbox/bob.sealing-pk)    # → bob: group join g1 --sealed …; same for carol
for m in alice bob carol; do as $m $R join --group g1; done          # all three derive F6zQ…
as alice $R create --members alice,bob,carol --threshold 2           # all three `show` 2-of-3
as alice $R fund --amount 1000000
as dave $R post-keys seat                                           # dave, not a member, is the payee
as alice $R propose transfer --to Private/<dave> --amount 123457     # stored, 1/2, nothing moved
as alice $R approve 0      # refused: "this member already approved the proposal"
as bob $R approve 0        # meets the threshold: the transfer runs in bob's transaction
```
- The vault then held 876,543.
- dave's `pending list` showed a sealed credit from a blinded alias, not the vault's address, and `pending receive` collected it.

Refusals:
- A transfer of 5,000,000 was proposed and approved, and refused ("holds less than the transferred amount"). Proposal #1 stayed pending, at 1/2.
- dave's `join --group g1` failed before g1 was shared with him: "no wallet group `g1`".

Privacy: 123457, 876543, 1000000 and 5000000 were all "not found" in public bytes; a public transfer of 777777 was found.

**Membership.**
- **Add dave in place, 2-of-4:** alice proposes and carol approves. dave joins g1 and sees the vault, including pending proposal #1.
- **Remove carol with rotation:**
  ```bash
  as alice $W group new g2                     # sealed to bob and dave only
  as alice $R propose remove --member carol --threshold 2 --successor-group g2   # successor 5t4h…
  as carol $R approve 3   # refused by her runner: the successor derives from no group she holds
  as bob $R approve 3     # "the successor 5t4h… is generation 1 of group `g2`: checked"; executes
  ```
- After the rotation:
  - alice, bob and dave follow it to generation 1, `5t4h…`: 2-of-3 alice, bob, dave, holding all 876,543. Pending #1 was dropped.
  - carol sees only the old vault: balance 0, "RETIRED: succeeded by 5t4h…", "which this wallet can't see".
  - carol's `propose` at the old vault was refused: "this vault is retired". Her `sweep` failed, because she can't witness the successor.
- **A late deposit:**
  - carol (still holding g1) funded the old vault with 4,242.
  - alice ran `sweep --retired vault-gen0`, and the new vault held 880,785.
  - carol could see that deposit, as she can see everything at the old address.
- **The new vault pays:** bob proposes 55,555 to carol, dave approves, and carol gets a sealed credit. New proposal ids restart at 0.
- **Privacy:** 876543, 880785, 825230, 55555 and 4242 were all "not found"; the control was found.

### Session 2: a public vault

```bash
as alice $R --vault pub announce --placement public
for m in alice bob carol dave; do as $m $R --vault pub pseudonym; as $m $R --vault pub join; done
as alice $R --vault pub create --members alice,bob,carol --threshold 2
```
- A private seat's *sync* message reaches a public vault in one private transaction, so both placements use the same delivery.
- Anyone can read the vault (`wallet account get --scope all`); it lists only pseudonyms.

Payouts and membership:
- Fund 2,000,000.
- 314,159 to a public account: alice proposes, bob approves.
- 271,828 to dave's private seat: alice proposes, carol approves.
  - From public execution the credit can't be sealed, so it carries a recovery binding (`async_destination`), and it names the vault as sender.
  - The runner's first try sealed it, and the transaction reverted.
- Add dave as 3-of-4 (alice and bob), then remove carol in place as 2-of-3 (alice, bob, dave). No rotation.
- carol's `propose` was refused: "not a member of this vault". A rotation (`--successor-group`) was refused: "a public vault changes members in place".

Audit:
- The payout amounts were *found*, as expected for a public vault.
- Each member's pseudonym was found in 2 to 5 transactions. Each member's seat account was not found, except dave's: he was a payee, which a public vault publishes.

## What stays public

| | Hidden vault | Public vault |
|---|---|---|
| Balance, pending proposals, members | no | yes, as pseudonyms |
| Who approved | no | pseudonyms only |
| Payout amount and payee | no, to a private payee | yes |
| That transactions happened (nullifiers, commitments, fees), and the program's image | yes | yes |
