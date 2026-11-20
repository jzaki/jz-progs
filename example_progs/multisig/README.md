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

No CLI at LEZ v0.4.0-rc3 sends a private call to a custom program. [`jz-progs/multisig-runner`](../../multisig-runner/README.md) drives it through the wallet library, one wallet per member, and its README has the sessions this was tested with.
