# Walkthrough: LEZ v0.4 (actor model) locally

The commands from [INTRO.md](../INTRO.md) section 4, in the order you run them. The explanations stay in INTRO.md. It assumes the section 1 setup is done (toolchain, `logos-execution-zone` on `dev`) and that a local sequencer may already be running.

## 1. Reset to a clean chain

In the sequencer's terminal, stop it (Ctrl-C) if it is running, then from the `logos-execution-zone` root inside `nix develop`:

```bash
just clean                                       # deletes chain data and wallet storage; the `docker compose down` error at the end is harmless
RISC0_DEV_MODE=1 just run-sequencer-standalone   # 127.0.0.1:3040; dev mode skips proving
```

Leave it running. Revert reasons only appear in this terminal's log.

## 2. Fresh terminal: environment

```bash
cd logos-execution-zone && nix develop          # from logos-repos

export LEE_WALLET_HOME_DIR=$PWD/lez/wallet/configs/debug   # same home as `just run-wallet`, so the runner reads it
cargo build --release -p example_program_deployment_methods   # builds the guests; quick if already built
BIN=$PWD/target/riscv-guest/example_program_deployment_methods/example_program_deployment_programs/riscv32im-risc0-zkvm-elf/release
```

## 3. Wallet and accounts

`just clean` wiped the wallet storage, so this runs on every reset.

```bash
just run-wallet check-health             # asks for a new password
just wallet-import-test-accounts         # imports the two genesis-funded test accounts and lists them
PAYER=<first account id listed above>
just run-wallet account new public       # the account hello_world writes to
ACCOUNT=<account id printed above>
```

## 4. Deploy and call hello_world

The runner deploys through the program loader, then sends the call.

```bash
(cd examples/program_deployment && \
  cargo run --release --bin run_hello_world -- "$BIN/hello_world.bin" "$ACCOUNT" "$PAYER")
```

**Expect about 1 MB of output.** The runner prints every transaction in full, including the program bytes in each segment upload, and that's normal. Look for four `Transaction hash is …` / `Transaction is included in block N` pairs: three segments and the header. The hello-world call prints nothing, so step 5 is how you check it. To see only those, append `| grep -E 'Transaction (hash|is included)|rror|panicked'` inside the parentheses.

On `dev` at `e2913677b` the runner needs three fixes in `examples/program_deployment/` first (details in INTRO.md section 4, "Run it locally"):

1. **Segment count** (`src/lib.rs`): count segments from `ProgramBinary::decode(&bytecode)?.user_elf`, not the whole `.bin`. Otherwise: `SegmentCountMismatch`.
2. **Fee:** build the call with `Message::try_new_with_fees(.., FeeDeclaration::new(payer, ..))`, include the payer's nonce, and sign with the payer. Otherwise: `MissingFeeDeclaration`.
3. **Admission:** add `PublicAccountEvidence::Key(..)` for `ACCOUNT` to `admission_evidence`, or use an account already on chain. Otherwise: `UnadmittedPublicActor`.

Fixes 2 and 3 fail silently: the runner doesn't check the call's result.

## 5. Check the result

```bash
just run-wallet account get --scope all --account-id "Public/$ACCOUNT"        # decoded
just run-wallet account get --raw --scope all --account-id "Public/$ACCOUNT"  # hex under actor_states
echo <hex> | xxd -r -p                                                        # -> Hola mundo!

# "Account is Uninitialized" means the call reverted. Confirm on the sequencer:
curl -s -X POST http://127.0.0.1:3040 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getAccount","params":["'$ACCOUNT'"]}'   # null: never written
```

## Optional: deploy without the runner

```bash
just run-wallet account new public --label hdr      # header; its id becomes the program's address
just run-wallet account new public --label seg0     # one segment account per 96 KiB chunk of the user ELF
just run-wallet account new public --label seg1
just run-wallet account new public --label seg2
just run-wallet program-loader deploy --elf "$BIN/hello_world.bin" \
  --header Public/<hdr id> \
  --segments Public/<seg0 id> Public/<seg1 id> Public/<seg2 id> \
  --payer Public/$PAYER
```

`hello_world.bin` needs exactly 3 segments. If a program needs a different number, the deploy stops before sending anything, with `Program bytecode splits into N segment(s) but M were supplied`. Create N segment accounts and run it again. It prints the same large transaction dump as the runner, and on success ends with `Program deployed. Header account id: <hdr id>`.
