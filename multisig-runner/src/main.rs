//! Drives the multisig example (`example_progs/multisig`) with private transactions.
//!
//! Each member runs this against their **own** wallet (`LEE_WALLET_HOME_DIR`). Their seat is a
//! private account of that wallet (default label `seat`): it signs every call and pays its fee.
//!
//! Members share what they must through a **mailbox**, a directory standing in for out-of-band
//! messages. It holds only what members may see: a vault's `create_key` and placement, each
//! member's pseudonym, and a private recipient's public keys. Group secrets travel sealed, through
//! `wallet group invite` and `wallet group join`.
//!
//! Each member also keeps a local record of the vault they act on
//! (`$LEE_WALLET_HOME_DIR/multisig-<vault>.txt`): its placement, `create_key`, generation, group,
//! and address. After a hidden vault rotates, members who hold the new group secret follow it on
//! their next call.

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    path::{Path, PathBuf},
    str::FromStr as _,
};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use lee::{
    AccountId, privacy_preserving_transaction::circuit::ProgramCatalog, program::Program,
};
use lee_core::{
    NullifierPublicKey, PrivateAccountKind, encryption::ViewingPublicKey, program::ProgramId,
};
use multisig_core::{Action, Message, Placement, VaultState, pseudonym, seed_for};
use rand::RngCore as _;
use wallet::{
    AccountIdentity, AccountMention, AsyncDelivery, WalletCore, WalletSequencerClient,
    account::{AccountIdWithPrivacy, Label},
    cli::{Command, account::AccountSubcommand, execute_subcommand},
    program_facades::{CreditDelivery, native_token_transfer::NativeTokenTransfer},
    storage::key_chain::{SharedAccountDerivation, SharedAccountEntry},
};

const DEFAULT_ELF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../example_progs/multisig/target/riscv-guest/multisig-methods/multisig-guest/",
    "riscv32im-risc0-zkvm-elf/release/multisig.bin"
);

#[derive(Parser)]
#[command(about = "Drive the multisig example with private transactions")]
struct Cli {
    /// The deployed multisig program: its header account id.
    #[arg(long, env = "PROGRAM_MULTISIG")]
    program: AccountId,
    /// This member's seat: a private account of this wallet, as a label, `Private/<id>` or id.
    #[arg(long, default_value = "seat")]
    seat: String,
    /// This member's name in the mailbox.
    #[arg(long, env = "MULTISIG_ME")]
    me: Option<String>,
    /// The out-of-band mailbox directory.
    #[arg(long, env = "MULTISIG_MAILBOX")]
    mailbox: PathBuf,
    /// The vault's name, in the mailbox and in this member's local record.
    #[arg(long, default_value = "vault")]
    vault: String,
    /// The multisig guest the program was deployed from (the proof runs it).
    #[arg(long, env = "MULTISIG_ELF", default_value = DEFAULT_ELF)]
    elf: PathBuf,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum PlacementArg {
    Public,
    Hidden,
}

#[derive(Subcommand)]
enum Cmd {
    /// Import the local chain's genesis private account A, labelled `genesis-private`.
    ImportGenesisPrivate,
    /// Write this seat's secret, once. Every pseudonym derives from it.
    InitSeat,
    /// Start a vault: pick its `create_key` and post it, with its placement, to the mailbox.
    Announce {
        #[arg(long, value_enum)]
        placement: PlacementArg,
    },
    /// Compute this member's pseudonym for the announced vault and post it to the mailbox.
    Pseudonym,
    /// Register the announced vault in this wallet. A hidden vault needs `--group`: the wallet
    /// group holding its secret (`wallet group join`).
    Join {
        #[arg(long)]
        group: Option<String>,
    },
    /// Create the vault: `--members` are mailbox names (their pseudonyms are read from it).
    Create {
        #[arg(long, value_delimiter = ',')]
        members: Vec<String>,
        #[arg(long)]
        threshold: u8,
    },
    /// Move `amount` from the seat's balance into the vault.
    Fund {
        #[arg(long)]
        amount: u128,
    },
    /// Propose an action, approving it.
    Propose {
        #[command(subcommand)]
        action: ActionArg,
    },
    /// Approve proposal `id`. The approval that meets the threshold runs the action.
    Approve { id: u64 },
    /// Forward a retired vault's balance to its successor. `--retired` names it (a label such as
    /// `vault-gen0`, or its id); the default is this member's current vault.
    Sweep {
        #[arg(long)]
        retired: Option<String>,
    },
    /// Post a private account of this wallet's public keys to the mailbox, so any member can pay
    /// it.
    PostKeys { account: String },
    /// Post this wallet's sealing public key (`wallet group new-sealing-key`) to the mailbox, as
    /// `<me>.sealing-pk`, for `wallet group invite --key`.
    SealingKey,
    /// Print the vault's state and balance.
    Show,
    /// Search every transaction's public bytes, from `from_block`, for `amount` (16 bytes LE), or
    /// for an account id (its 32 bytes).
    Audit {
        #[arg(long, required_unless_present = "account")]
        amount: Option<u128>,
        #[arg(long, conflicts_with = "amount")]
        account: Option<AccountId>,
        #[arg(long, default_value_t = 1)]
        from_block: u64,
    },
}

#[derive(Subcommand)]
enum ActionArg {
    /// Pay `amount` to `to`: a label or id of this wallet, `Private/<id>` with keys in the mailbox
    /// (`post-keys`), or a public id.
    Transfer {
        #[arg(long)]
        to: String,
        #[arg(long)]
        amount: u128,
    },
    /// Add the member named `member` in the mailbox. `--successor-group` rotates a hidden vault to
    /// that group's next-generation vault.
    Add {
        #[arg(long)]
        member: String,
        #[arg(long)]
        threshold: u8,
        #[arg(long)]
        successor_group: Option<String>,
    },
    /// Remove the member named `member`. A hidden vault needs `--successor-group`.
    Remove {
        #[arg(long)]
        member: String,
        #[arg(long)]
        threshold: u8,
        #[arg(long)]
        successor_group: Option<String>,
    },
}

/// This member's record of the vault they act on.
#[derive(Clone)]
struct Local {
    placement: Placement,
    create_key: [u8; 32],
    generation: u64,
    group: Option<String>,
    vault: AccountId,
}

struct Ctx {
    program: AccountId,
    image: ProgramId,
    programs: ProgramCatalog,
    seat: AccountId,
    mailbox: PathBuf,
    vault_name: String,
    me: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut wallet = WalletCore::from_env().context("loading the wallet")?;
    if matches!(cli.command, Cmd::ImportGenesisPrivate) {
        return import_genesis_private(&mut wallet);
    }
    if let Cmd::Audit {
        amount,
        account,
        from_block,
    } = cli.command
    {
        let (needle, what) = match (amount, account) {
            (Some(amount), _) => (amount.to_le_bytes().to_vec(), amount.to_string()),
            (None, Some(account)) => (account.value().to_vec(), account.to_string()),
            (None, None) => bail!("--amount or --account"),
        };
        return audit(&wallet, &needle, &what, from_block).await;
    }
    sync(&mut wallet, false).await?;
    std::fs::create_dir_all(&cli.mailbox)
        .with_context(|| format!("creating the mailbox {}", cli.mailbox.display()))?;
    let elf = std::fs::read(&cli.elf)
        .with_context(|| format!("reading the multisig guest {}", cli.elf.display()))?;
    let guest = Program::new(elf.into()).map_err(|e| anyhow!("not a guest binary: {e:?}"))?;
    let ctx = Ctx {
        program: cli.program,
        image: guest.id(),
        programs: ProgramCatalog::from([(cli.program, guest)]),
        seat: private_account(&wallet, &cli.seat)?,
        mailbox: cli.mailbox,
        vault_name: cli.vault,
        me: cli.me,
    };
    let seat_actor = AccountIdentity::PrivateOwned(ctx.seat).select_program_actor_state(ctx.program);

    match cli.command {
        Cmd::ImportGenesisPrivate | Cmd::Audit { .. } => unreachable!("handled above"),
        Cmd::InitSeat => {
            let mut secret = [0_u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut secret);
            let message = Message::InitSeat { secret };
            send(&mut wallet, &ctx, vec![seat_actor], &message, AsyncDelivery::default()).await?;
        }
        Cmd::Announce { placement } => {
            let mut create_key = [0_u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut create_key);
            let placement = match placement {
                PlacementArg::Public => "public",
                PlacementArg::Hidden => "hidden",
            };
            let text = format!("placement={placement}\ncreate_key={}\n", hex::encode(create_key));
            let path = ctx.mailbox.join(format!("{}.vault", ctx.vault_name));
            ensure!(!path.exists(), "{} is already announced", path.display());
            std::fs::write(&path, text)?;
            println!("announced `{}` ({placement}) in {}", ctx.vault_name, path.display());
        }
        Cmd::Pseudonym => {
            let me = ctx.me.as_deref().context("--me (or MULTISIG_ME) names this member")?;
            let (_, create_key) = announced(&ctx)?;
            let pseudonym = my_pseudonym(&wallet, &ctx, create_key)?;
            let path = ctx.mailbox.join(format!("{}.member.{me}", ctx.vault_name));
            std::fs::write(&path, pseudonym.to_string())?;
            println!("{me}'s pseudonym at `{}`: {pseudonym}", ctx.vault_name);
        }
        Cmd::Join { group } => {
            let (placement, create_key) = announced(&ctx)?;
            let local = match placement {
                Placement::Public => Local {
                    placement,
                    create_key,
                    generation: 0,
                    group: None,
                    vault: AccountId::for_public_pda(&ctx.program, &seed_for(create_key, 0)),
                },
                Placement::Hidden => {
                    let group = group.context("a hidden vault needs --group")?;
                    let vault = register_hidden(&mut wallet, &ctx, &group, create_key, 0)?;
                    Local {
                        placement,
                        create_key,
                        generation: 0,
                        group: Some(group),
                        vault,
                    }
                }
            };
            write_local(&ctx, &local)?;
            if matches!(placement, Placement::Hidden) {
                // The vault may already be on chain: read its history from genesis.
                sync(&mut wallet, true).await?;
            }
            println!("vault `{}`: {}", ctx.vault_name, local.vault);
        }
        Cmd::Create { members, threshold } => {
            let local = read_local(&ctx)?;
            let members = members
                .iter()
                .map(|name| member_pseudonym(&ctx, name))
                .collect::<Result<Vec<_>>>()?;
            let message = Message::Create {
                vault: local.vault,
                placement: local.placement,
                create_key: local.create_key,
                members,
                threshold,
            };
            let vault_actor = vault_identity(&wallet, &local)?.select_program_actor_state(ctx.program);
            let accounts = vec![seat_actor, vault_actor];
            send(&mut wallet, &ctx, accounts, &message, AsyncDelivery::default()).await?;
        }
        Cmd::Fund { amount } => {
            let local = current(&mut wallet, &ctx).await?;
            let (hash, _) = NativeTokenTransfer(&wallet)
                .transfer(
                    AccountIdentity::PrivateOwned(ctx.seat),
                    vault_identity(&wallet, &local)?,
                    amount,
                    CreditDelivery::Automatic,
                )
                .await?;
            settle(&mut wallet, hash).await?;
        }
        Cmd::Propose { action } => {
            let local = current(&mut wallet, &ctx).await?;
            let state = vault_state(&wallet, &ctx, &local).await?;
            let action = build_action(&mut wallet, &ctx, &local, action).await?;
            println!("proposing #{}: {}", state.next_id, describe(&ctx, &action));
            let executes = state.config.threshold <= 1;
            let message = Message::Propose {
                vault: local.vault,
                create_key: local.create_key,
                action: action.clone(),
            };
            act(&mut wallet, &ctx, &local, &message, executes.then_some(&action)).await?;
        }
        Cmd::Approve { id } => {
            let local = current(&mut wallet, &ctx).await?;
            let state = vault_state(&wallet, &ctx, &local).await?;
            let proposal = state
                .proposal(id)
                .with_context(|| format!("no pending proposal #{id}"))?
                .clone();
            println!("approving #{id}: {}", describe(&ctx, &proposal.action));
            check_successor(&mut wallet, &ctx, &local, &proposal.action).await?;
            let executes =
                proposal.approvals.len().saturating_add(1) >= usize::from(state.config.threshold);
            let message = Message::Approve {
                vault: local.vault,
                create_key: local.create_key,
                id,
            };
            act(&mut wallet, &ctx, &local, &message, executes.then_some(&proposal.action)).await?;
        }
        Cmd::Sweep { retired } => {
            let mut local = read_local(&ctx)?;
            if let Some(retired) = retired {
                local.vault = any_account(&wallet, &retired)?;
            }
            let state = vault_state(&wallet, &ctx, &local).await?;
            let successor = state.retired.context("this vault isn't retired")?;
            let accounts = vec![
                seat_actor,
                vault_identity(&wallet, &local)?.select_program_actor_state(ctx.program),
                vault_identity(&wallet, &local)?.balance(),
                successor_identity(&wallet, successor)?.balance(),
            ];
            let message = Message::Sweep {
                vault: local.vault,
                create_key: local.create_key,
            };
            send(&mut wallet, &ctx, accounts, &message, AsyncDelivery::default()).await?;
        }
        Cmd::PostKeys { account } => {
            let id = private_account(&wallet, &account)?;
            let found = wallet
                .storage()
                .key_chain()
                .private_account(id)
                .context("the account's keys")?;
            let text = format!(
                "npk={}\nvpk={}\n",
                hex::encode(found.key_chain.nullifier_public_key.0),
                hex::encode(found.key_chain.viewing_public_key.to_bytes())
            );
            std::fs::write(ctx.mailbox.join(format!("keys.{id}")), text)?;
            println!("posted the keys of Private/{id}");
        }
        Cmd::SealingKey => {
            let me = ctx.me.as_deref().context("--me (or MULTISIG_ME) names this member")?;
            let sk = wallet
                .storage()
                .key_chain()
                .sealing_secret_key()
                .context("no sealing key: run `wallet group new-sealing-key` first")?;
            let pk = ViewingPublicKey::from_seed(&sk.d, &sk.z);
            let path = ctx.mailbox.join(format!("{me}.sealing-pk"));
            std::fs::write(&path, hex::encode(pk.to_bytes()))?;
            println!("posted {me}'s sealing public key to {}", path.display());
        }
        Cmd::Show => {
            let local = current(&mut wallet, &ctx).await?;
            show(&wallet, &ctx, &local).await?;
        }
    }
    Ok(())
}

/// Sends `message` to the seat, adding what the action it runs (if any) needs.
async fn act<C: WalletSequencerClient>(
    wallet: &mut WalletCore<C>,
    ctx: &Ctx,
    local: &Local,
    message: &Message,
    executes: Option<&Action>,
) -> Result<()> {
    let vault = vault_identity(wallet, local)?;
    let mut accounts = vec![
        AccountIdentity::PrivateOwned(ctx.seat).select_program_actor_state(ctx.program),
        vault.clone().select_program_actor_state(ctx.program),
    ];
    let mut delivery = AsyncDelivery::default();
    if let Some(action) = executes {
        println!("this approval meets the threshold: the action runs now");
        match action {
            Action::Transfer { to, .. } => {
                accounts.push(vault.balance());
                match recipient(wallet, ctx, local, *to)? {
                    Recipient::Joins(mention) => accounts.push(mention),
                    Recipient::Sealed(sealed) => delivery = sealed,
                }
            }
            Action::AddMember { successor, .. } | Action::RemoveMember { successor, .. } => {
                if let Some(successor) = successor {
                    let next = successor_identity(wallet, *successor)?;
                    accounts.push(vault.balance());
                    // Both mentions alike: the wallet refuses one account mentioned two ways.
                    accounts.push(next.clone().select_program_actor_state(ctx.program));
                    accounts.push(next.balance());
                }
            }
        }
    }
    send(wallet, ctx, accounts, message, delivery).await
}

enum Recipient {
    /// The recipient takes part in the transaction, so the credit applies at once.
    Joins(AccountMention),
    /// The recipient is private and not in the transaction: the credit waits for them to collect
    /// it (`wallet pending receive`).
    Sealed(AsyncDelivery),
}

/// How a transfer reaches `to`: to a private account whose keys this wallet holds or the mailbox
/// has, else as a public account in the transaction.
///
/// A private recipient's credit comes from the vault's native balance. From a hidden vault that
/// is private execution, so the credit is *sealed*: nobody but the recipient reads it. From a
/// public vault it is public execution, which can't seal: the credit is published with a recovery
/// binding for the recipient, and its amount is public (as the vault's balance already is).
fn recipient<C: WalletSequencerClient>(
    wallet: &WalletCore<C>,
    ctx: &Ctx,
    local: &Local,
    to: AccountId,
) -> Result<Recipient> {
    let keys = match wallet.storage().key_chain().private_account(to) {
        Some(found) => Some((
            found.key_chain.nullifier_public_key,
            found.key_chain.viewing_public_key.clone(),
        )),
        None => posted_keys(ctx, to)?,
    };
    let Some((npk, vpk)) = keys else {
        return Ok(Recipient::Joins(AccountIdentity::PublicNoSign(to).balance()));
    };
    let foreign = AccountIdentity::PrivateForeign {
        npk,
        vpk,
        kind: PrivateAccountKind::Regular,
    };
    let (_, mut delivery) = match local.placement {
        Placement::Hidden => wallet.seal_destination(foreign.balance())?,
        Placement::Public => wallet.async_destination(foreign.balance())?,
    };
    delivery.promotions.listed_only = true;
    Ok(Recipient::Sealed(delivery))
}

fn posted_keys(ctx: &Ctx, to: AccountId) -> Result<Option<(NullifierPublicKey, ViewingPublicKey)>> {
    let path = ctx.mailbox.join(format!("keys.{to}"));
    if !path.exists() {
        return Ok(None);
    }
    let fields = read_fields(&path)?;
    let npk: [u8; 32] = hex::decode(field(&fields, "npk")?)?
        .try_into()
        .map_err(|_| anyhow!("npk is 32 bytes"))?;
    let vpk = ViewingPublicKey::from_bytes(hex::decode(field(&fields, "vpk")?)?)
        .map_err(|e| anyhow!("vpk: {e:?}"))?;
    let npk = NullifierPublicKey(npk);
    ensure!(
        AccountId::from((&npk, &vpk)) == to,
        "the keys posted for {to} don't derive it"
    );
    Ok(Some((npk, vpk)))
}

/// Builds the proposed action. A rotation derives and registers the successor from
/// `--successor-group`.
async fn build_action<C: WalletSequencerClient>(
    wallet: &mut WalletCore<C>,
    ctx: &Ctx,
    local: &Local,
    action: ActionArg,
) -> Result<Action> {
    Ok(match action {
        ActionArg::Transfer { to, amount } => Action::Transfer {
            to: any_account(wallet, &to)?,
            amount,
        },
        ActionArg::Add {
            member,
            threshold,
            successor_group,
        } => Action::AddMember {
            member: member_pseudonym(ctx, &member)?,
            threshold,
            successor: successor(wallet, ctx, local, successor_group).await?,
        },
        ActionArg::Remove {
            member,
            threshold,
            successor_group,
        } => Action::RemoveMember {
            member: member_pseudonym(ctx, &member)?,
            threshold,
            successor: successor(wallet, ctx, local, successor_group).await?,
        },
    })
}

async fn successor<C: WalletSequencerClient>(
    wallet: &mut WalletCore<C>,
    ctx: &Ctx,
    local: &Local,
    group: Option<String>,
) -> Result<Option<AccountId>> {
    let Some(group) = group else {
        return Ok(None);
    };
    ensure!(
        matches!(local.placement, Placement::Hidden),
        "a public vault changes members in place"
    );
    let next = local.generation.checked_add(1).context("generations overflow")?;
    let successor = register_hidden(wallet, ctx, &group, local.create_key, next)?;
    println!("successor (generation {next}, group `{group}`): {successor}");
    Ok(Some(successor))
}

/// Before approving a rotation: the successor must derive from a group secret this member holds,
/// or the funds would move to a vault they can't verify. Registers it on a match.
async fn check_successor<C: WalletSequencerClient>(
    wallet: &mut WalletCore<C>,
    ctx: &Ctx,
    local: &Local,
    action: &Action,
) -> Result<()> {
    let (Action::AddMember {
        successor: Some(successor),
        ..
    }
    | Action::RemoveMember {
        successor: Some(successor),
        ..
    }) = action
    else {
        return Ok(());
    };
    let next = local.generation.checked_add(1).context("generations overflow")?;
    let Some(group) = group_deriving(wallet, ctx, local.create_key, next, *successor)? else {
        bail!(
            "refusing to approve: the successor {successor} doesn't derive from any group secret \
             this wallet holds (join the new group first)"
        )
    };
    register_hidden(wallet, ctx, &group, local.create_key, next)?;
    println!("the successor {successor} is generation {next} of group `{group}`: checked");
    Ok(())
}

/// The wallet group whose secret derives `vault` as generation `generation` of `create_key`.
fn group_deriving<C: WalletSequencerClient>(
    wallet: &WalletCore<C>,
    ctx: &Ctx,
    create_key: [u8; 32],
    generation: u64,
    vault: AccountId,
) -> Result<Option<String>> {
    let groups: Vec<String> = wallet
        .storage()
        .key_chain()
        .group_key_holders_iter()
        .map(|(label, _)| label.to_string())
        .collect();
    for group in groups {
        let (id, _) = hidden_vault(wallet, ctx, &group, create_key, generation)?;
        if id == vault {
            return Ok(Some(group));
        }
    }
    Ok(None)
}

/// A hidden vault's address and wallet entry: generation `generation` of `create_key`, under
/// `group`'s secret.
fn hidden_vault<C: WalletSequencerClient>(
    wallet: &WalletCore<C>,
    ctx: &Ctx,
    group: &str,
    create_key: [u8; 32],
    generation: u64,
) -> Result<(AccountId, SharedAccountEntry)> {
    let label = Label::new(group);
    let holder = wallet
        .storage()
        .key_chain()
        .group_key_holder(&label)
        .with_context(|| format!("no wallet group `{group}` (wallet group join)"))?;
    let seed = seed_for(create_key, generation);
    let keys = holder.derive_keys_for_pda(&ctx.image, &seed);
    let npk = keys.generate_nullifier_public_key();
    let vpk = keys.generate_viewing_public_key();
    let id = AccountId::for_private_pda(&ctx.program, &seed, &npk, &vpk);
    let entry = SharedAccountEntry {
        group_label: label,
        derivation: SharedAccountDerivation::Pda {
            seed,
            program_id: ctx.image,
        },
        kind: PrivateAccountKind::Pda {
            account_id: ctx.program,
            seed,
        },
        account: lee::Account::default(),
    };
    Ok((id, entry))
}

/// Registers a hidden vault as a shared private PDA of the wallet, so it can witness and sync it.
fn register_hidden<C: WalletSequencerClient>(
    wallet: &mut WalletCore<C>,
    ctx: &Ctx,
    group: &str,
    create_key: [u8; 32],
    generation: u64,
) -> Result<AccountId> {
    let (id, entry) = hidden_vault(wallet, ctx, group, create_key, generation)?;
    if wallet.storage().key_chain().shared_private_account(id).is_none() {
        wallet
            .storage_mut()
            .key_chain_mut()
            .insert_shared_private_account(id, entry);
        let label = Label::new(format!("{}-gen{generation}", ctx.vault_name));
        if wallet.storage().resolve_label(&label).is_none() {
            wallet
                .storage_mut()
                .add_label(label, AccountIdWithPrivacy::Private(id))?;
        }
        wallet.store_persistent_data()?;
    }
    Ok(id)
}

fn vault_identity<C: WalletSequencerClient>(
    wallet: &WalletCore<C>,
    local: &Local,
) -> Result<AccountIdentity> {
    Ok(match local.placement {
        Placement::Public => AccountIdentity::PublicNoSign(local.vault),
        Placement::Hidden => wallet
            .resolve_private_account(local.vault)
            .context("the vault isn't registered in this wallet (join)")?,
    })
}

fn successor_identity<C: WalletSequencerClient>(
    wallet: &WalletCore<C>,
    successor: AccountId,
) -> Result<AccountIdentity> {
    wallet
        .resolve_private_account(successor)
        .with_context(|| format!("this wallet can't witness the successor {successor}"))
}

async fn vault_state<C: WalletSequencerClient>(
    wallet: &WalletCore<C>,
    ctx: &Ctx,
    local: &Local,
) -> Result<VaultState> {
    let account = account_of(wallet, local).await?;
    let state = account.data.actor_state(ctx.program);
    ensure!(!state.is_empty(), "the vault isn't created yet");
    Ok(VaultState::decode(state))
}

async fn account_of<C: WalletSequencerClient>(
    wallet: &WalletCore<C>,
    local: &Local,
) -> Result<lee::Account> {
    Ok(match local.placement {
        Placement::Public => wallet.get_account_public(local.vault).await?.unwrap_or_default(),
        Placement::Hidden => wallet
            .get_account_private(local.vault)
            .context("the vault isn't registered in this wallet (join)")?,
    })
}

/// This member's record, after following a rotation they can follow.
async fn current<C: WalletSequencerClient>(wallet: &mut WalletCore<C>, ctx: &Ctx) -> Result<Local> {
    let mut local = read_local(ctx)?;
    loop {
        let Ok(state) = vault_state(wallet, ctx, &local).await else {
            return Ok(local);
        };
        let Some(successor) = state.retired else {
            return Ok(local);
        };
        let next = local.generation.checked_add(1).context("generations overflow")?;
        let group = match wallet.storage().key_chain().shared_private_account(successor) {
            Some(entry) => entry.group_label.to_string(),
            None => {
                // Not registered yet: it may derive from a group this member has joined.
                let Some(group) = group_deriving(wallet, ctx, local.create_key, next, successor)?
                else {
                    println!(
                        "vault {} is retired, succeeded by {successor}, which this wallet can't see",
                        local.vault
                    );
                    return Ok(local);
                };
                register_hidden(wallet, ctx, &group, local.create_key, next)?;
                // Its history is already on chain.
                sync(wallet, true).await?;
                group
            }
        };
        local.group = Some(group);
        local.generation = next;
        local.vault = successor;
        write_local(ctx, &local)?;
        println!("followed the rotation to generation {}: {successor}", local.generation);
    }
}

async fn show<C: WalletSequencerClient>(wallet: &WalletCore<C>, ctx: &Ctx, local: &Local) -> Result<()> {
    let account = account_of(wallet, local).await?;
    let balance = account
        .data
        .native_balance()
        .map_or_else(|_| "?".to_owned(), |b| b.to_string());
    let placement = match local.placement {
        Placement::Public => "public",
        Placement::Hidden => "hidden",
    };
    println!(
        "vault `{}` ({placement}, generation {}): {}  balance {balance}",
        ctx.vault_name, local.generation, local.vault
    );
    let state = vault_state(wallet, ctx, local).await?;
    let names = member_names(ctx)?;
    let name = |id: &AccountId| names.get(id).cloned().unwrap_or_else(|| id.to_string());
    let members: Vec<String> = state.config.members.iter().map(&name).collect();
    println!(
        "  {}-of-{}: {}",
        state.config.threshold,
        members.len(),
        members.join(", ")
    );
    if let Some(successor) = state.retired {
        println!("  RETIRED: succeeded by {successor}");
    }
    for proposal in &state.proposals {
        let approvals: Vec<String> = proposal.approvals.iter().map(&name).collect();
        println!(
            "  #{} {}  approved by {} ({}/{})",
            proposal.id,
            describe(ctx, &proposal.action),
            approvals.join(", "),
            approvals.len(),
            state.config.threshold
        );
    }
    Ok(())
}

fn describe(ctx: &Ctx, action: &Action) -> String {
    let names = member_names(ctx).unwrap_or_default();
    let name = |id: &AccountId| names.get(id).cloned().unwrap_or_else(|| id.to_string());
    let mut text = String::new();
    match action {
        Action::Transfer { to, amount } => {
            let _ = write!(text, "transfer {amount} to {to}");
        }
        Action::AddMember {
            member,
            threshold,
            successor,
        } => {
            let _ = write!(text, "add {} (threshold {threshold})", name(member));
            if let Some(s) = successor {
                let _ = write!(text, ", rotating to {s}");
            }
        }
        Action::RemoveMember {
            member,
            threshold,
            successor,
        } => {
            let _ = write!(text, "remove {} (threshold {threshold})", name(member));
            if let Some(s) = successor {
                let _ = write!(text, ", rotating to {s}");
            }
        }
    }
    text
}

fn my_pseudonym<C: WalletSequencerClient>(
    wallet: &WalletCore<C>,
    ctx: &Ctx,
    create_key: [u8; 32],
) -> Result<AccountId> {
    let account = wallet
        .get_account_private(ctx.seat)
        .context("the seat isn't in this wallet")?;
    let state = account.data.actor_state(ctx.program);
    ensure!(!state.is_empty(), "the seat isn't initialised (init-seat)");
    let seat = multisig_core::SeatState::decode(state);
    Ok(pseudonym(ctx.seat, seat.secret, create_key))
}

fn member_pseudonym(ctx: &Ctx, name: &str) -> Result<AccountId> {
    let path = ctx.mailbox.join(format!("{}.member.{name}", ctx.vault_name));
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("no pseudonym from `{name}` ({})", path.display()))?;
    AccountId::from_str(text.trim()).map_err(|e| anyhow!("{}: {e:?}", path.display()))
}

/// Pseudonym → mailbox name, for display.
fn member_names(ctx: &Ctx) -> Result<BTreeMap<AccountId, String>> {
    let prefix = format!("{}.member.", ctx.vault_name);
    let mut names = BTreeMap::new();
    for entry in std::fs::read_dir(&ctx.mailbox)? {
        let entry = entry?;
        let file = entry.file_name().to_string_lossy().into_owned();
        if let Some(name) = file.strip_prefix(&prefix)
            && let Ok(id) = member_pseudonym(ctx, name)
        {
            names.insert(id, name.to_owned());
        }
    }
    Ok(names)
}

fn announced(ctx: &Ctx) -> Result<(Placement, [u8; 32])> {
    let path = ctx.mailbox.join(format!("{}.vault", ctx.vault_name));
    let fields = read_fields(&path)?;
    let placement = match field(&fields, "placement")?.as_str() {
        "public" => Placement::Public,
        "hidden" => Placement::Hidden,
        other => bail!("unknown placement `{other}`"),
    };
    Ok((placement, key32(&field(&fields, "create_key")?)?))
}

fn local_path(ctx: &Ctx) -> Result<PathBuf> {
    let home = std::env::var("LEE_WALLET_HOME_DIR").context("LEE_WALLET_HOME_DIR")?;
    Ok(Path::new(&home).join(format!("multisig-{}.txt", ctx.vault_name)))
}

fn write_local(ctx: &Ctx, local: &Local) -> Result<()> {
    let placement = match local.placement {
        Placement::Public => "public",
        Placement::Hidden => "hidden",
    };
    let text = format!(
        "placement={placement}\ncreate_key={}\ngeneration={}\ngroup={}\nvault={}\n",
        hex::encode(local.create_key),
        local.generation,
        local.group.as_deref().unwrap_or(""),
        local.vault
    );
    std::fs::write(local_path(ctx)?, text)?;
    Ok(())
}

fn read_local(ctx: &Ctx) -> Result<Local> {
    let path = local_path(ctx)?;
    let fields = read_fields(&path).context("run `join` first")?;
    let group = field(&fields, "group")?;
    Ok(Local {
        placement: match field(&fields, "placement")?.as_str() {
            "public" => Placement::Public,
            _ => Placement::Hidden,
        },
        create_key: key32(&field(&fields, "create_key")?)?,
        generation: field(&fields, "generation")?.parse()?,
        group: (!group.is_empty()).then_some(group),
        vault: AccountId::from_str(&field(&fields, "vault")?).map_err(|e| anyhow!("{e:?}"))?,
    })
}

fn read_fields(path: &Path) -> Result<BTreeMap<String, String>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(text
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect())
}

fn field(fields: &BTreeMap<String, String>, name: &str) -> Result<String> {
    fields.get(name).cloned().with_context(|| format!("missing `{name}`"))
}

fn key32(text: &str) -> Result<[u8; 32]> {
    hex::decode(text)?
        .try_into()
        .map_err(|_| anyhow!("expected 32 bytes of hex"))
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

/// A private account of this wallet, by label, `Private/<id>` or bare id.
fn private_account<C: WalletSequencerClient>(wallet: &WalletCore<C>, name: &str) -> Result<AccountId> {
    let id = match any_account_with_privacy(wallet, name)? {
        AccountIdWithPrivacy::Private(id) => id,
        AccountIdWithPrivacy::Public(_) => bail!("`{name}` is a public account"),
    };
    ensure!(
        wallet.storage().key_chain().private_account(id).is_some(),
        "`{name}` is not a private account of this wallet"
    );
    Ok(id)
}

/// Any account, by label, `Private/<id>`, `Public/<id>` or bare id.
fn any_account<C: WalletSequencerClient>(wallet: &WalletCore<C>, name: &str) -> Result<AccountId> {
    Ok(match any_account_with_privacy(wallet, name)? {
        AccountIdWithPrivacy::Private(id) | AccountIdWithPrivacy::Public(id) => id,
    })
}

fn any_account_with_privacy<C: WalletSequencerClient>(
    wallet: &WalletCore<C>,
    name: &str,
) -> Result<AccountIdWithPrivacy> {
    let label = Label::new(name);
    if let Some(found) = wallet.storage().resolve_label(&label) {
        return Ok(found);
    }
    if let Ok(found) = AccountIdWithPrivacy::from_str(name) {
        return Ok(found);
    }
    let id = AccountId::from_str(name).map_err(|e| anyhow!("`{name}`: {e:?}"))?;
    Ok(if wallet.storage().key_chain().private_account(id).is_some() {
        AccountIdWithPrivacy::Private(id)
    } else {
        AccountIdWithPrivacy::Public(id)
    })
}

/// A private transaction rooted at the seat (the first account), then [`settle`].
async fn send<C: WalletSequencerClient>(
    wallet: &mut WalletCore<C>,
    ctx: &Ctx,
    accounts: Vec<AccountMention>,
    message: &Message,
    delivery: AsyncDelivery,
) -> Result<()> {
    let data = borsh::to_vec(message)?;
    let (hash, _) = wallet
        .send_tx(accounts, 0, data, &ctx.programs, None, delivery)
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
    sync(wallet, false).await?;
    if reverted {
        bail!("transaction {hash} reverted in block {block_id}: its fee was charged, nothing else applied");
    }
    println!("included in block {block_id}");
    Ok(())
}

async fn sync<C: WalletSequencerClient>(wallet: &mut WalletCore<C>, rescan: bool) -> Result<()> {
    execute_subcommand(
        wallet,
        Command::Account(AccountSubcommand::SyncPrivate { rescan, yes: true }),
    )
    .await?;
    wallet.store_persistent_data()
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
