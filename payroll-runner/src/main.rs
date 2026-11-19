//! Drives the payroll example (`example_progs/payroll`) with private transactions.
//!
//! Every call is a privacy-preserving transaction over private accounts:
//! - the seat, a private account of this wallet (default label `seat`);
//! - the treasury, the private PDA `for_private_pda(program, seed_of(seat), seat_npk, seat_vpk)`,
//!   which the wallet tracks as a second kind of the seat's own key node;
//! - employees, private accounts. Their pay is sealed to their keys, and each collects it with
//!   `wallet pending receive`.
//!
//! After each call the runner waits for the block, reports a revert, and syncs the wallet's private
//! accounts, so `show` reads the treasury's decrypted state.

use std::{path::PathBuf, str::FromStr as _};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use clap::{Parser, Subcommand};
use lee::{
    AccountId, privacy_preserving_transaction::circuit::ProgramCatalog, program::Program,
};
use lee_core::PrivateAccountKind;
use payroll_core::{Message, PayrollState, SeatState, seed_of};
use wallet::{
    AccountIdentity, AccountMention, AsyncDelivery, WalletCore, WalletSequencerClient,
    account::{AccountIdWithPrivacy, Label},
    cli::{Command, account::AccountSubcommand, execute_subcommand},
    program_facades::{CreditDelivery, native_token_transfer::NativeTokenTransfer},
};

const DEFAULT_ELF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../example_progs/payroll/target/riscv-guest/payroll-methods/payroll-guest/",
    "riscv32im-risc0-zkvm-elf/release/payroll.bin"
);

#[derive(Parser)]
#[command(about = "Drive the payroll example with private transactions")]
struct Cli {
    /// The deployed payroll program: its header account id.
    #[arg(long, env = "PROGRAM_PAYROLL")]
    program: AccountId,
    /// The seat: a private account of this wallet, as a label, `Private/<id>` or a bare id.
    #[arg(long, default_value = "seat")]
    seat: String,
    /// The payroll guest the program was deployed from (the proof runs it).
    #[arg(long, env = "PAYROLL_ELF", default_value = DEFAULT_ELF)]
    elf: PathBuf,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Import the local chain's genesis private account A, labelled `genesis-private`. A private
    /// transaction's fee comes from a private account, so the first private funds on a fresh chain
    /// come from here: collect its genesis credit with `wallet pending receive`, then send to the
    /// seat from it.
    ImportGenesisPrivate,
    /// Print the treasury's address, registering it in the wallet (every command does this).
    Treasury,
    /// Move `amount` from the seat's private balance to the treasury, privately.
    Fund {
        #[arg(long)]
        amount: u128,
    },
    /// Open the payroll: the seat records its treasury, the treasury its period.
    Open {
        #[arg(long)]
        period_ms: u64,
    },
    /// Set an employee's pay per period. The employee is a private account of this wallet.
    SetEmployee {
        employee: String,
        #[arg(long)]
        amount: u128,
    },
    /// Accrue every employee's pay up to `period` (default: the period the chain is in).
    Accrue {
        #[arg(long)]
        period: Option<u64>,
    },
    /// Pay an employee what they are owed, sealed to them.
    Pay { employee: String },
    /// Return `amount` nobody is owed to the seat.
    Withdraw {
        #[arg(long)]
        amount: u128,
    },
    /// Print the treasury's decrypted state, its balance, and the employees' balances.
    Show,
    /// Print the chain's current period, by the latest block's timestamp.
    Period,
    /// Search every transaction's public bytes, from `from_block`, for `amount` (16 bytes LE).
    Audit {
        #[arg(long)]
        amount: u128,
        #[arg(long, default_value_t = 1)]
        from_block: u64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut wallet = WalletCore::from_env().context("loading the wallet")?;
    if matches!(cli.command, Cmd::ImportGenesisPrivate) {
        return import_genesis_private(&mut wallet);
    }
    sync(&mut wallet).await?;
    let seat = private_account(&wallet, &cli.seat)?;
    let treasury = register_treasury(&mut wallet, cli.program, seat)?;
    let program = cli.program;
    let programs = || -> Result<ProgramCatalog> {
        let elf = std::fs::read(&cli.elf)
            .with_context(|| format!("reading the payroll guest {}", cli.elf.display()))?;
        let guest = Program::new(elf.into()).map_err(|e| anyhow!("not a guest binary: {e:?}"))?;
        Ok(ProgramCatalog::from([(program, guest)]))
    };
    let seat_actor = AccountIdentity::PrivateOwned(seat).select_program_actor_state(program);
    let treasury_actor = AccountIdentity::PrivateOwned(treasury).select_program_actor_state(program);
    let treasury_balance = AccountIdentity::PrivateOwned(treasury).balance();

    match cli.command {
        Cmd::ImportGenesisPrivate => unreachable!("handled before the seat is resolved"),
        Cmd::Treasury => println!("treasury: Private/{treasury}"),
        Cmd::Fund { amount } => {
            let (hash, _) = NativeTokenTransfer(&wallet)
                .transfer(
                    AccountIdentity::PrivateOwned(seat),
                    AccountIdentity::PrivateOwned(treasury),
                    amount,
                    CreditDelivery::Automatic,
                )
                .await?;
            settle(&mut wallet, hash).await?;
        }
        Cmd::Open { period_ms } => {
            let message = Message::Open {
                treasury,
                period_ms,
            };
            let accounts = vec![seat_actor, treasury_actor];
            send(&mut wallet, accounts, &message, &programs()?, AsyncDelivery::default()).await?;
        }
        Cmd::SetEmployee { employee, amount } => {
            let employee = private_account(&wallet, &employee)?;
            let message = Message::SetEmployee { employee, amount };
            let accounts = vec![seat_actor, treasury_actor];
            send(&mut wallet, accounts, &message, &programs()?, AsyncDelivery::default()).await?;
        }
        Cmd::Accrue { period } => {
            let period = match period {
                Some(period) => period,
                None => {
                    let period_ms = treasury_state(&wallet, program, treasury)?.period_ms;
                    current_period(&wallet, period_ms).await?
                }
            };
            println!("accruing up to period {period}");
            let accounts = vec![seat_actor, treasury_actor];
            let message = Message::Accrue { period };
            send(&mut wallet, accounts, &message, &programs()?, AsyncDelivery::default()).await?;
        }
        Cmd::Pay { employee } => {
            let employee = private_account(&wallet, &employee)?;
            // The employee as a foreign private account: the credit is sealed to their keys and
            // waits for them, rather than being applied in this transaction.
            let found = wallet
                .storage()
                .key_chain()
                .private_account(employee)
                .context("the employee's keys")?;
            let recipient = AccountIdentity::PrivateForeign {
                npk: found.key_chain.nullifier_public_key,
                vpk: found.key_chain.viewing_public_key.clone(),
                kind: PrivateAccountKind::Regular,
            };
            let (_, mut delivery) = wallet.seal_destination(recipient.balance())?;
            delivery.promotions.listed_only = true;
            let accounts = vec![seat_actor, treasury_actor, treasury_balance];
            let message = Message::Pay { employee };
            send(&mut wallet, accounts, &message, &programs()?, delivery).await?;
        }
        Cmd::Withdraw { amount } => {
            // The seat's own balance takes part, so its credit applies in this transaction.
            let seat_balance = AccountIdentity::PrivateOwned(seat).balance();
            let accounts = vec![seat_actor, treasury_actor, treasury_balance, seat_balance];
            let message = Message::Withdraw { amount };
            send(&mut wallet, accounts, &message, &programs()?, AsyncDelivery::default()).await?;
        }
        Cmd::Show => show(&wallet, program, seat, treasury)?,
        Cmd::Period => {
            let period_ms = treasury_state(&wallet, program, treasury)?.period_ms;
            let period = current_period(&wallet, period_ms).await?;
            println!("period {period} (each {period_ms} ms)");
        }
        Cmd::Audit { amount, from_block } => audit(&wallet, amount, from_block).await?,
    }
    Ok(())
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
    let label = Label::from_str("genesis-private").expect("a label parses from any string");
    if wallet.storage().resolve_label(&label).is_none() {
        wallet
            .storage_mut()
            .add_label(label, AccountIdWithPrivacy::Private(account_id))?;
    }
    wallet.store_persistent_data()?;
    println!("imported Private/{account_id} (label `genesis-private`)");
    Ok(())
}

/// A private account of this wallet, by label, `Private/<id>` or bare id.
fn private_account<C: WalletSequencerClient>(wallet: &WalletCore<C>, name: &str) -> Result<AccountId> {
    let label = Label::from_str(name).expect("a label parses from any string");
    let id = match wallet.storage().resolve_label(&label) {
        Some(AccountIdWithPrivacy::Private(id)) => id,
        Some(AccountIdWithPrivacy::Public(_)) => bail!("`{name}` is a public account"),
        None => match AccountIdWithPrivacy::from_str(name) {
            Ok(AccountIdWithPrivacy::Private(id)) => id,
            Ok(AccountIdWithPrivacy::Public(_)) => bail!("`{name}` is a public account"),
            Err(_) => AccountId::from_str(name).map_err(|e| anyhow!("`{name}`: {e:?}"))?,
        },
    };
    ensure!(
        wallet.storage().key_chain().private_account(id).is_some(),
        "`{name}` is not a private account of this wallet"
    );
    Ok(id)
}

/// The seat's treasury, registered in the wallet under the seat's key node (as the `Pda` kind of
/// the same keys), so the wallet can witness it and syncs follow it.
fn register_treasury<C: WalletSequencerClient>(
    wallet: &mut WalletCore<C>,
    program: AccountId,
    seat: AccountId,
) -> Result<AccountId> {
    let found = wallet
        .storage()
        .key_chain()
        .private_account(seat)
        .context("the seat's keys")?;
    let seed = seed_of(seat);
    let treasury = AccountId::for_private_pda(
        &program,
        &seed,
        &found.key_chain.nullifier_public_key,
        &found.key_chain.viewing_public_key,
    );
    if wallet.storage().key_chain().private_account(treasury).is_none() {
        wallet.storage_mut().key_chain_mut().insert_private_account(
            treasury,
            PrivateAccountKind::Pda {
                account_id: program,
                seed,
            },
            lee::Account::default(),
        )?;
        let label = Label::from_str("treasury").expect("a label parses from any string");
        if wallet.storage().resolve_label(&label).is_none() {
            wallet
                .storage_mut()
                .add_label(label, AccountIdWithPrivacy::Private(treasury))?;
        }
        wallet.store_persistent_data()?;
        println!("registered the treasury Private/{treasury} (label `treasury`)");
    }
    Ok(treasury)
}

/// A private transaction rooted at the first account, then [`settle`].
async fn send<C: WalletSequencerClient>(
    wallet: &mut WalletCore<C>,
    accounts: Vec<AccountMention>,
    message: &Message,
    programs: &ProgramCatalog,
    delivery: AsyncDelivery,
) -> Result<()> {
    let data = borsh::to_vec(message)?;
    let (hash, _) = wallet
        .send_tx(accounts, 0, data, programs, None, delivery)
        .await?;
    settle(wallet, hash).await
}

/// Waits for `hash`'s block, fails on a revert, and syncs the private accounts either way.
async fn settle<C: WalletSequencerClient>(
    wallet: &mut WalletCore<C>,
    hash: common::HashType,
) -> Result<()> {
    let (_, block_id) = wallet.poll_transaction(hash).await?;
    let block = wallet
        .get_block(block_id)
        .await?
        .with_context(|| format!("block {block_id}"))?;
    let reverted = block
        .body
        .transactions
        .iter()
        .position(|tx| tx.hash() == hash)
        .is_some_and(|position| block.body.is_reverted(position));
    sync(wallet).await?;
    if reverted {
        bail!("transaction {hash} reverted in block {block_id}: its fee was charged, nothing else applied");
    }
    println!("included in block {block_id}");
    Ok(())
}

async fn sync<C: WalletSequencerClient>(wallet: &mut WalletCore<C>) -> Result<()> {
    execute_subcommand(
        wallet,
        Command::Account(AccountSubcommand::SyncPrivate {
            rescan: false,
            yes: true,
        }),
    )
    .await?;
    wallet.store_persistent_data()
}

fn treasury_state<C: WalletSequencerClient>(
    wallet: &WalletCore<C>,
    program: AccountId,
    treasury: AccountId,
) -> Result<PayrollState> {
    let account = wallet
        .get_account_private(treasury)
        .context("the treasury isn't in the wallet")?;
    let state = account.data.actor_state(program);
    ensure!(!state.is_empty(), "the payroll isn't open (run `open`)");
    Ok(PayrollState::decode(state))
}

async fn current_period<C: WalletSequencerClient>(wallet: &WalletCore<C>, period_ms: u64) -> Result<u64> {
    let last = wallet.get_last_block_id().await?;
    let block = wallet.get_block(last).await?.context("the latest block")?;
    Ok(block.header.timestamp / period_ms)
}

fn native_balance<C: WalletSequencerClient>(wallet: &WalletCore<C>, account: AccountId) -> String {
    wallet
        .get_account_private(account)
        .and_then(|account| account.data.native_balance().ok())
        .map_or_else(|| "?".to_owned(), |balance| balance.to_string())
}

fn show<C: WalletSequencerClient>(
    wallet: &WalletCore<C>,
    program: AccountId,
    seat: AccountId,
    treasury: AccountId,
) -> Result<()> {
    let seat_state = wallet
        .get_account_private(seat)
        .map(|account| account.data.actor_state(program).clone())
        .filter(|state| !state.is_empty())
        .map(|state| SeatState::decode(&state));
    println!("seat      Private/{seat}  balance {}", native_balance(wallet, seat));
    println!(
        "          opened: {}",
        seat_state.map_or_else(|| "no".to_owned(), |s| format!("treasury {}", s.treasury))
    );
    println!("treasury  Private/{treasury}  balance {}", native_balance(wallet, treasury));
    let state = treasury_state(wallet, program, treasury)?;
    println!(
        "          period {} ms, last accrued {:?}, owed {}, paid {}, withdrawal pending {:?}",
        state.period_ms, state.last_accrued, state.total_owed, state.total_paid, state.pending_withdraw
    );
    for employee in &state.employees {
        let label = wallet
            .storage()
            .labels_for_account(AccountIdWithPrivacy::Private(employee.account))
            .next()
            .map_or_else(String::new, |label| format!(" ({label})"));
        println!(
            "employee  Private/{}{label}  pay {}  owed {}  paid {}  balance {}",
            employee.account,
            employee.amount,
            employee.owed,
            employee.paid,
            native_balance(wallet, employee.account)
        );
    }
    Ok(())
}

async fn audit<C: WalletSequencerClient>(
    wallet: &WalletCore<C>,
    amount: u128,
    from_block: u64,
) -> Result<()> {
    let needle = amount.to_le_bytes();
    let last = wallet.get_last_block_id().await?;
    let mut found = 0_usize;
    for block_id in from_block..=last {
        let Some(block) = wallet.get_block(block_id).await? else {
            continue;
        };
        for tx in &block.body.transactions {
            let bytes = borsh::to_vec(tx)?;
            if bytes.windows(needle.len()).any(|window| window == needle) {
                found += 1;
                let kind = match tx {
                    common::transaction::LeeTransaction::Public(_) => "public",
                    common::transaction::LeeTransaction::PrivacyPreserving(_) => "privacy-preserving",
                };
                println!("found {amount} in block {block_id}, {kind} transaction {}", tx.hash());
            }
        }
    }
    println!(
        "{amount}: {} in the public bytes of blocks {from_block}..={last}",
        if found == 0 { "not found".to_owned() } else { format!("found in {found} transaction(s)") }
    );
    Ok(())
}
