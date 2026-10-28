# lez-dryrun

Plans a LEZ public call by running it locally, through LEZ's own engine, against the chain's state. It works out what the transaction needs and ends with the `wallet program call …` line that sends it:
- the actors to declare;
- who signs;
- the admission evidence for new accounts.

It sends nothing and holds no keys. [LEZCALL.md](../LEZCALL.md) §3 is its design.

```
lez-dryrun [--rpc <url>] [--json] plan --payer <account> --program <id> --message-hex <hex> --to <actor>
           [--actor <actor>]… [--signer <account>]… [--seed [NAME=]<seed>]… [--mention <account>]…
           [--label <name>=<id>]… [--sign | --no-sign] [--hinted-signed] [--leave-pending] [--budget <n>]
lez-dryrun [--rpc <url>] [--json] source-of <program-id> --bin-dir <dir>
```

- **The message:** `lez-call encode`'s output. Its `mentioned` ids are what `--mention` takes.
- **`--signer`:** the accounts the wallet holds keys for. Only these are tried as signers, and given key evidence when new.
- **`--seed`:** the source's `PdaSeed` constants as `NAME=0x…` (from `lez-call --json iface`), and any other seeds to try for a new PDA.
- **The rest:**
  - `--rpc` defaults to `http://127.0.0.1:3040`;
  - `--budget` is the wallet's gas limit, default 2000000;
  - exit status 1 means the dry run failed, and nothing should be sent.

`source-of` names the local guest build whose image id is in a deployed program's header.

## Building

It builds against a LEZ checkout at `../../logos-execution-zone`, by path, in its own Cargo workspace. `Cargo.lock` and `rust-toolchain.toml` are copied from LEZ, so the engine and risc0 build as LEZ builds them.

```sh
cargo build --release      # target/release/lez-dryrun
```

prog-tui's `b` builds it too.
