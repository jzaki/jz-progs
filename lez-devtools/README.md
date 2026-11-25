# lez-devtools

Local-chain helpers for testing LEZ programs, outside any one program. Calls to programs, public or private, go through `spel` (its `feat/private-calls` branch for private ones); these cover what isn't a program call.

| Command | What it does |
|---|---|
| `import-genesis-private` | Imports the local chain's genesis private account A as `genesis-private`. A private transaction's fee comes from an owned private account, so a fresh chain's first private funds come from here: collect its genesis credit with `wallet pending receive` |
| `audit (--amount N \| --account ID \| --hex H) [--from-block B]` | Searches every transaction's public bytes for an amount (16 bytes LE), an account id (32 bytes, label or id) or any bytes |
| `sealing-key` | Prints this wallet's sealing public key (made by `wallet group new-sealing-key`), for `wallet group invite --key`. The rc3 wallet doesn't print it again |
| `block` | Prints the latest block's id and timestamp (ms), tab-separated |

The wallet is `LEE_WALLET_HOME_DIR`'s, with `LEE_WALLET_PASSWORD`. Set `RISC0_DEV_MODE=1` on a dev chain.

It builds against a LEZ checkout at `../../logos-execution-zone` (v0.4.0-rc3), by path, in its own Cargo workspace, with LEZ's `Cargo.lock` and toolchain:

```sh
cargo build --release      # target/release/lez-devtools
```

It holds the helpers the payroll and multisig examples' former bespoke runners each carried.
