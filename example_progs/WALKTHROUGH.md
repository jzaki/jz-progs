# Walkthrough: LEZ v0.4 (actor model) locally

The commands from [INTRO.md](../INTRO.md) section 4, in the order you run them. The explanations stay in INTRO.md. prog-tui's `W` runs the same steps.

It assumes the section 1 setup is done, with three checkouts side by side in `logos-repos`:
- `logos-execution-zone` at **`v0.4.0-rc3`**, the tag lez-programs pins;
- `lez-programs`;
- `spel` on its `refactor/lez-v0.4.x` branch, which pins the same tag.

A local sequencer may already be running.

## 1. Reset to a clean chain

In the sequencer's terminal, stop it (Ctrl-C) if it is running, then from the `logos-execution-zone` root inside `nix develop`:

```bash
just clean                                       # deletes chain data and wallet storage; the `docker compose down` error at the end is harmless
RISC0_DEV_MODE=1 just run-sequencer-standalone   # 127.0.0.1:3040; dev mode skips proving
```

Leave it running. Revert reasons only appear in this terminal's log.

## 2. Fresh terminal: build and environment

```bash
cd logos-execution-zone && nix develop          # from logos-repos
cargo build --release -p wallet                 # the wallet
WALLET=$PWD/target/release/wallet
export LEE_WALLET_HOME_DIR=$PWD/lez/wallet/configs/debug   # same home as `just run-wallet`; spel reads it too

(cd ../lez-programs && cargo build --release -p token-methods -p nft-methods && make idl)   # the guests (local risc0, no Docker) and their IDLs
(cd ../spel && cargo build --release --bin spel)
SPEL=$PWD/../spel/target/release/spel
TOKEN_BIN=$PWD/../lez-programs/target/riscv-guest/token-methods/token-guest/riscv32im-risc0-zkvm-elf/release/token.bin
TOKEN_IDL=$PWD/../lez-programs/artifacts/token-idl.json
```

lez-programs' `make build-programs` builds the same guests in Docker, into `target/guest/`. The methods crates build them with the local RISC Zero toolchain instead.

## 3. Wallet and accounts

`just clean` wiped the wallet storage, so this runs on every reset.

```bash
export LEE_WALLET_PASSWORD=             # empty: no storage encryption and no prompts; set one to encrypt it
$WALLET check-health                     # creates the wallet
just wallet-import-test-accounts         # imports the two genesis-funded test accounts and lists them
PAYER=<first account id listed above>
$WALLET account new public --label account   # the account the token's supply goes to
ACCOUNT=<account id printed above>
```

## 4. Deploy token

The program loader stores a program as a header account (its address) and one segment account per 96 KiB of its user ELF. `token.bin` needs 2.

```bash
$WALLET account new public --label hdr      # its id becomes the program's address
$WALLET account new public --label seg0
$WALLET account new public --label seg1
$WALLET program-loader deploy --elf "$TOKEN_BIN" \
  --header Public/<hdr id> \
  --segments Public/<seg0 id> Public/<seg1 id> \
  --payer Public/$PAYER
TOKEN=<hdr id>
```

**Expect a lot of output.** The deploy prints every transaction in full, including the program bytes in each segment upload, and that's normal. On success it ends with `Program deployed. Header account id: <hdr id>`. With the wrong number of segments it stops before sending anything, with `Program bytecode splits into N segment(s) but M were supplied`. Create N segment accounts and run it again.

## 5. Create a token

A token's definition lives on an account of its own, which signs for it. The whole supply goes to `ACCOUNT`'s holding, which signs too.

```bash
$WALLET account new public --label jz-token
$SPEL --idl "$TOKEN_IDL" --program "$TOKEN" --fee-payer "$PAYER" \
  send --root jz-token --signer jz-token --signer "$ACCOUNT" --dry-run -- \
  new-definition --name JZ --total-supply 1000 --mint-authority none --holding "$ACCOUNT"
```

The dry run prints the access list (each actor, and why it's there), who signs, and the encoded message, and sends nothing. Drop `--dry-run` to send. spel simulates the transaction first, and doesn't send one that would be refused. Once it's included, it prints each declared actor that changed:

```
Included in block 203. Changed:
  (jz-token (7SDm…), 3Yk4…)
    before: none
    after:  {"Definition":{"authority":null,"name":"JZ","total_supply":"1000"}}
  (account (qJxT…), 3Yk4…)
    before: none
    after:  {"Holding":{"balance":"1000","definition_id":"7SDm…"}}
```

## 6. Check the result

```bash
$SPEL --idl "$TOKEN_IDL" --program "$TOKEN" inspect "$ACCOUNT"     # {"Holding": {"balance": "1000", …}}
$WALLET account get --raw --scope all --account-id "Public/$ACCOUNT"   # the raw actor states
```

Then move some. The recipient needs a holding of this token first:

```bash
$WALLET account new public --label bob
$SPEL --idl "$TOKEN_IDL" --program "$TOKEN" --fee-payer "$PAYER" \
  send --root bob --signer bob -- initialize-holding --definition jz-token
$SPEL --idl "$TOKEN_IDL" --program "$TOKEN" --fee-payer "$PAYER" \
  send --root "$ACCOUNT" --signer "$ACCOUNT" -- transfer --to bob --amount 300
```

A transfer to an account with no holding, or of more than the balance, is refused in simulation: spel prints the guest's panic (`recipient has no token holding; initialize it first`, `insufficient balance`), sends nothing, and exits with 2. lez-programs' [token runbook](https://github.com/logos-blockchain/lez-programs/blob/main/docs/token/README.md) covers every token message.

## Your own guests

`jz-progs/example_progs/*.rs` build in a guest package prog-tui keeps in lez-programs, `lez-programs/jz/` (from `prog-tui/templates/jz/`). prog-tui's `b` writes it, links each file in as `jz/methods/guest/src/bin/jz_<name>.rs`, and builds it. By hand, once prog-tui has written it:

```bash
(cd ../lez-programs && cargo build --release --manifest-path jz/methods/Cargo.toml --target-dir target)
JZ_BIN=$PWD/../lez-programs/target/riscv-guest/jz-methods/jz-guest/riscv32im-risc0-zkvm-elf/release
```

Deploy `$JZ_BIN/jz_<name>.bin` as in step 4 (each needs 2 segments today), then call it with spel, like token. `b` also writes each guest's spel IDL to `lez-programs/jz/idl/jz_<name>.json`, with `spel generate-idl` over a one-file crate that links to the guest:

```bash
IDL=$PWD/../lez-programs/jz/idl/jz_hello_world.json
$SPEL --idl "$IDL" --program <jz_hello_world hdr id> --fee-payer "$PAYER" \
  send --root "$ACCOUNT" --signer "$ACCOUNT" -- hello --greeting 'Hola mundo!'
```

[prog-tui/README.md](../prog-tui/README.md), "Calling with spel", lists what each guest's calls take: which actors to add for admin_seat's PDA, simple_token's treasury, and the clock clock_gate reads.
