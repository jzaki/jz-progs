//! Local-chain helpers for testing LEZ programs, outside any one program:
//! - `import-genesis-private`: the local chain's genesis private account, to bootstrap private funds;
//! - `audit`: search every transaction's public bytes for an amount, an account or any bytes;
//! - `sealing-key`: this wallet's sealing public key, for `wallet group invite --key`;
//! - `block`: the latest block's id and timestamp.
//!
//! Calls to programs, private or public, go through `spel`.

use std::str::FromStr as _;

use anyhow::{Context as _, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use lee_core::{account::AccountId, encryption::ViewingPublicKey};
use wallet::{
    WalletCore, WalletSequencerClient,
    account::{AccountIdWithPrivacy, Label},
};

#[derive(Parser)]
#[command(about = "Local-chain helpers for testing LEZ programs (the wallet from LEE_WALLET_HOME_DIR)")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Import the local chain's genesis private account A, labelled `genesis-private`. A private
    /// transaction's fee comes from a private account, so a fresh chain's first private funds
    /// come from here: collect its genesis credit with `wallet pending receive`.
    ImportGenesisPrivate,
    /// Search every transaction's public bytes, from `--from-block`, for an amount (16 bytes LE),
    /// an account id (its 32 bytes) or hex bytes.
    Audit {
        #[arg(long, group = "needle")]
        amount: Option<u128>,
        #[arg(long, group = "needle")]
        account: Option<String>,
        #[arg(long, group = "needle")]
        hex: Option<String>,
        #[arg(long, default_value_t = 1)]
        from_block: u64,
    },
    /// Print this wallet's sealing public key (made by `wallet group new-sealing-key`) as hex.
    SealingKey,
    /// Print the latest block's id and timestamp (ms), tab-separated.
    Block,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut wallet = WalletCore::from_env().context("loading the wallet")?;
    match cli.command {
        Cmd::ImportGenesisPrivate => import_genesis_private(&mut wallet),
        Cmd::Audit {
            amount,
            account,
            hex,
            from_block,
        } => {
            let (needle, what) = match (amount, account, hex) {
                (Some(amount), _, _) => (amount.to_le_bytes().to_vec(), amount.to_string()),
                (_, Some(account), _) => {
                    let id = account_id(&wallet, &account)?;
                    (id.value().to_vec(), account)
                }
                (_, _, Some(text)) => (
                    hex::decode(text.trim_start_matches("0x"))?,
                    format!("0x{}", text.trim_start_matches("0x")),
                ),
                _ => bail!("--amount, --account or --hex"),
            };
            audit(&wallet, &needle, &what, from_block).await
        }
        Cmd::SealingKey => {
            let sk = wallet
                .storage()
                .key_chain()
                .sealing_secret_key()
                .context("no sealing key: run `wallet group new-sealing-key` first")?;
            println!("{}", hex::encode(ViewingPublicKey::from_seed(&sk.d, &sk.z).to_bytes()));
            Ok(())
        }
        Cmd::Block => {
            let last = wallet.get_last_block_id().await?;
            let block = wallet.get_block(last).await?.context("the latest block")?;
            println!("{last}\t{}", block.header.timestamp);
            Ok(())
        }
    }
}

/// An account by label, `Private/<id>`, `Public/<id>` or bare id.
fn account_id<C: WalletSequencerClient>(wallet: &WalletCore<C>, name: &str) -> Result<AccountId> {
    if let Some(found) = wallet.storage().resolve_label(&Label::new(name)) {
        let (AccountIdWithPrivacy::Private(id) | AccountIdWithPrivacy::Public(id)) = found;
        return Ok(id);
    }
    if let Ok(AccountIdWithPrivacy::Private(id) | AccountIdWithPrivacy::Public(id)) =
        AccountIdWithPrivacy::from_str(name)
    {
        return Ok(id);
    }
    AccountId::from_str(name).map_err(|e| anyhow!("`{name}`: {e:?}"))
}

fn import_genesis_private<C: WalletSequencerClient>(wallet: &mut WalletCore<C>) -> Result<()> {
    let genesis = testnet_initial_state::initial_priv_accounts_private_keys()
        .into_iter()
        .next()
        .context("the genesis private accounts")?;
    let account_id = genesis.account_id();
    if wallet.storage().key_chain().private_account(account_id).is_none() {
        wallet.storage_mut().key_chain_mut().add_imported_private_account(
            genesis.key_chain,
            genesis.chain_index,
            genesis.account,
        );
        wallet.storage_mut().rewind_skipped_history();
    }
    let label = Label::new("genesis-private");
    if wallet.storage().resolve_label(&label).is_none() {
        wallet
            .storage_mut()
            .add_label(label, AccountIdWithPrivacy::Private(account_id))?;
    }
    wallet.store_persistent_data()?;
    println!("imported Private/{account_id} (label `genesis-private`)");
    Ok(())
}

async fn audit<C: WalletSequencerClient>(
    wallet: &WalletCore<C>,
    needle: &[u8],
    what: &str,
    from_block: u64,
) -> Result<()> {
    let last = wallet.get_last_block_id().await?;
    let mut found = 0_usize;
    for block_id in from_block..=last {
        let Some(block) = wallet.get_block(block_id).await? else {
            continue;
        };
        for tx in &block.body.transactions {
            let bytes = borsh::to_vec(tx)?;
            if bytes.windows(needle.len()).any(|window| window == needle) {
                found = found.saturating_add(1);
                let kind = match tx {
                    common::transaction::LeeTransaction::Public(_) => "public",
                    common::transaction::LeeTransaction::PrivacyPreserving(_) => "privacy-preserving",
                };
                println!("found {what} in block {block_id}, {kind} transaction {}", tx.hash());
            }
        }
    }
    println!(
        "{what}: {} in the public bytes of blocks {from_block}..={last}",
        if found == 0 { "not found".to_owned() } else { format!("found in {found} transaction(s)") }
    );
    Ok(())
}
