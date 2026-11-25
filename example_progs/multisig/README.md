# Multisig

A LEZ v0.4 (actor model) program: an M-of-N vault whose members are equal. There are no executors. It is laid out like lez-programs' `programs/token`, and like [payroll](../payroll/README.md) next to it. prog-tui doesn't build it.

| Path | What |
|---|---|
| `core/` (`multisig_core`) | `Message` (to a seat), `ToVault` / `VaultMessage` (to a vault), `Action`, `VaultState`, `SeatState`, and the two hashes clients share with the guest: `seed_for` and `pseudonym` |
| `src/` (`multisig_program`) | `handle_message`, plus one file per step (`seat`, `create`, `propose`, `members`, `rotate`), each with its unit tests |
| `methods/` (`multisig-methods`) | the one-line guest, `actor_entrypoint!(raw multisig_program::handle_message)` |

The crate docs in `src/lib.rs` explain the design. In short:

- **Actors.**
  - A **seat** is the program's actor on a member's own private account. It holds one secret.
  - The **vault** is the program's actor on a PDA. It holds the members, the threshold, and the pending proposals. The PDA's native balance holds the funds, and only the program can move them, by granting the vault's seed.
- **Like a Safe.** A member *proposes* an action, which the vault stores with their approval, and tells the others out of band. Each member who agrees *approves* it in a transaction of their own. The approval that meets the threshold runs the action in that same transaction. Actions:
  - `Transfer`;
  - `AddMember` and `RemoveMember`, which can also change M.
- **Every message goes member → seat → vault**, as a sync message in the member's one private transaction.
- **How the vault knows who approved.** A private sender reaches the vault under a fresh random alias (the wallet's `presenter`, at rc3 and on `dev`), so the vault can't recognise a seat by `from`. Instead, the seat names itself, and the vault trusts the name because of who can send it:
  - The vault accepts a seat's message only from this program.
  - The seat runs only on its owner's signed transaction.
  - The seat only ever names its own **pseudonym**, `seat.blinded(H(secret ‖ create_key))`, computed from its own address and its own private state.

  Nobody can link a pseudonym to an account without the seat's secret, and no signatures are involved.
- **Two placements, one handler.**
  - **Hidden**: a private PDA whose keys come from a group secret every member holds (`wallet group`). Only members can see the vault or put it in a transaction. The secret lets them *operate* it, not spend from it. Removing a member **rotates**: the members make a new group secret without the removed member. In the approving transaction, the vault retires, hands its configuration to the next generation (`Adopt`, granting its seed), and moves its whole balance there. The removed member keeps sight of the old vault, up to "retired, succeeded by X", and nothing after. An add can rotate too, if the members don't want the new member to read the history.
  - **Public**: a public PDA. Its state, balance, payouts and approvals are public, under pseudonyms. A removed member loses control at once, in place, but keeps sight of it, as everyone has.
- **Rotation costs no extra transactions.**
  - The approval that meets the threshold does it all. That one proof covers two vaults and two balances.
  - Off chain, the members share a new group secret.
  - Pending proposals don't carry over.
  - A deposit that still reaches the old address needs a `Sweep`, which forwards it to the successor; any old member may send it.

## Options considered

| | |
|---|---|
| Who holds a hidden vault's keys | **Every member** (no executors), so revoking means rotating. With a few executors and sealed notices to the other seats, revoking a non-executor would need no rotation, but it needs executors |
| How the vault knows who approved | **A self-naming seat**. LEZ's actor book (`docs/actor-program-development-book/src/catalog/multisig.md`, branch `moudy/lez-books`) registers each seat's fixed blinded address, which needs the wallet to reuse a blinding factor; rc3 doesn't. Off-chain BIP-340 signatures would work, but they skip the actor model |
| How an approval reaches the vault | **Sync, in the approver's own transaction.** A cast (`send_async`) would let a member approve without touching the vault, but someone must then collect it: 2k proofs per action |

## Build and test

```bash
cd jz-progs/example_progs/multisig
RISC0_DEV_MODE=1 cargo test -p multisig_program        # handler unit tests
cargo clippy -p multisig_program -p multisig_core --all-targets
cargo build --release -p multisig-methods              # the guest, local RISC Zero toolchain, no Docker
# -> target/riscv-guest/multisig-methods/multisig-guest/riscv32im-risc0-zkvm-elf/release/multisig.bin
```

`Cargo.lock`, `rust-toolchain.toml` (Rust 1.94.0) and the lints come from payroll, which takes them from lez-programs. `lee_core` comes from git tag `v0.4.0-rc3`. There are no other dependencies: both hashes use `AccountId::blinded`'s domain-separated SHA-256.

## Run it on a chain

[`multisig.sh`](multisig.sh) drives it with **spel's private calls** (spel branch `feat/private-calls`) and LEZ's wallet CLI. It keeps only the multisig's own client logic: the mailbox, each member's record of their vault, following a rotation, and checking a successor before approving. Everything sent goes through `spel send`, `spel transfer` and `spel pda`. It needs spel built at `../../../spel` and [`lez-devtools`](../../lez-devtools/README.md); the IDL is `multisig-idl.json` (`spel generate-idl core --message Message --state VaultState`).

**Each member runs it with their own wallet** (`LEE_WALLET_HOME_DIR`, `LEE_WALLET_PASSWORD`), with `PROGRAM_MULTISIG`, `MULTISIG_ME` (their mailbox name) and `MULTISIG_MAILBOX` set. Their seat is the wallet's private account `seat`.

**The mailbox** is a directory standing in for out-of-band messages. It holds:
- a vault's placement and `create_key` (`<vault>.vault`);
- each member's pseudonym (`<vault>.member.<name>`);
- sealing public keys (`<name>.sealing-pk`);
- a private payee's public keys (`keys.<id>`, `wallet account show-keys` output).

Group secrets travel sealed: `wallet group invite` / `join`. Each member's record of the vault they act on is `$LEE_WALLET_HOME_DIR/multisig-<vault>.txt`.

| Command | What it does, through spel |
|---|---|
| `init-seat` | `send --root seat -- init-seat --secret <random>` |
| `announce public\|hidden` | posts a fresh `create_key` |
| `pseudonym` | `seat.blinded(AccountId(secret).blinded(create_key))` with two `spel blind`s, the secret read from the seat (`spel inspect seat`) |
| `sealing-key` | posts `lez-devtools sealing-key` |
| `join [group]` | a public vault: `spel pda <seed>`; a hidden one: `spel pda <seed> --group G --register --rescan`. The seed is `spel blind 0x<create_key> le64:<generation> --hex` |
| `create <M> <name>...` | `send --root seat -- create …`; the vault is found by simulation |
| `fund <amount>` | `spel transfer --from seat --to <vault>` |
| `propose transfer <to> <amount>` | `send … propose`; `to` is a label, `Private/<id>` or `Public/<id>` |
| `propose add\|remove <name> <M> [group]` | with a group, a hidden vault rotates to that group's next generation, registered with `spel pda --register` |
| `approve <id>` | `send … approve`. If it meets the threshold, it adds what the action needs: the payee's credit (`--seal` from a hidden vault, `--recover` from a public one; `keys:<mailbox file>` for someone else's account), or the successor's balance (`--actor <successor>@native`). Before approving a rotation it checks the successor derives from a group this wallet holds, and refuses otherwise |
| `sweep [retired]` | forwards a retired vault's balance to its successor |
| `post-keys <account>`, `show`, `audit …` | |

Commands follow a hidden vault's rotation first, if this wallet holds the new group's secret.

### Tested 2026-11-20

Each set ran on a fresh local chain, with four wallets (alice, bob, carol, dave), the program deployed with `spel deploy`, and every seat funded privately from the genesis private account (`spel transfer --to keys:<file>`, then `wallet pending receive`).

**Hidden vault, 2-of-3.**
- alice: `announce hidden`, `group new g1`, invited bob and carol; all three `join g1` and derived `Cajd…`. `create 2 alice bob carol`: all three `show` 2-of-3.
- `fund 1000000`; alice proposed 123,457 to dave (`Private/<dave>`, keys posted to the mailbox): stored at 1/2, nothing moved. alice's second approval was refused in simulation ("this member already approved the proposal", exit 2).
- bob approved: the transfer ran in his transaction, sealed to dave's keys (`--seal keys:…`). The vault held 876,543; dave's credit was pending from a blinded alias, and he collected it.
- A 5,000,000 transfer was refused in simulation when approved (`InsufficientBalance` on the vault); proposal #1 stayed pending.
- dave's `join g1` failed before g1 was shared with him. Added in place as 2-of-4 (alice proposed, carol approved); after `join g1` dave saw the vault, pending #1 included.
- **Remove carol with rotation:** alice made g2 and shared it with bob and dave; `propose remove carol 2 g2` named the successor `FzUU…`. carol's `approve` was refused by her script (the successor derives from no group she holds); bob's checked it ("generation 1 of group `g2`") and ran the rotation in one transaction.
- alice, bob and dave followed it to generation 1: 2-of-3, holding all 876,543. carol sees only the old vault, balance 0, "RETIRED: succeeded by FzUU…, which this wallet can't see"; her `propose` there was refused ("this vault is retired").
- carol's late deposit of 4,242 to the old vault was swept by alice (`sweep vault-gen0`): the new vault held 880,785. It then paid carol 55,555 (bob proposed, dave approved), leaving 825,230; carol's credit was sealed.
- `audit`: 123457, 876543, 1000000, 5000000, 4242, 880785, 55555 and 825230 were all "not found"; a public transfer of 777,777 was found.

**Public vault.**
- `announce public`, every member `join`ed (the public PDA, no group); `create 2 alice bob carol`. spel added the vault as a public actor; anyone can read it.
- `fund 2000000`. 314,159 to a public account (alice proposed, bob approved), and 271,828 to dave's private seat (alice proposed, carol approved). The private credit went with a recovery binding (`--recover keys:…`), naming the vault as sender, and dave collected it.
- A public payee must exist on chain: paying a brand-new public account was refused in simulation (`UnadmittedPublicActor`) until it held a balance.
- Add dave as 3-of-4 (alice, bob), then remove carol in place as 2-of-3 (alice, bob, dave). carol's `propose` was refused ("not a member of this vault"); a rotation was refused ("a public vault changes members in place").
- `audit`: the payout amounts were found, as expected for a public vault. Each member's pseudonym was found in 2 to 5 transactions; no seat account was, except dave's, as a payee.


### What stays public

| | Hidden vault | Public vault |
|---|---|---|
| Balance, pending proposals, members | no | yes, as pseudonyms |
| Who approved | no | pseudonyms only |
| Payout amount and payee | no, to a private payee | yes |
| That transactions happened (nullifiers, commitments, fees), and the program's image | yes | yes |
