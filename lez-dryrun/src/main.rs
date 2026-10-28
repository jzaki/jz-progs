//! Plans a LEZ public call by running it locally, against the chain's state.
//!
//! The sequencer executes a public transaction as `WholeExecution` over an
//! `ExecutionEnvironment`. This runs the same engine over one that reads the chain over RPC and
//! runs guests in the risc0 executor, and adds what each run finds missing:
//! - an actor a message reaches is declared;
//! - a receiver that panics unsigned signs, if the wallet holds its key;
//! - an account that isn't on chain gets key or PDA evidence.
//!
//! What it ends with is a `wallet program call …` line that sends that transaction as it is.
//! It sends nothing itself and holds no keys: the caller says which accounts the wallet can sign
//! for (`--signer`).
//!
//! With `--json`, the plan is one `LEZDRYRUN {json}` line.

use std::{collections::BTreeSet, path::PathBuf, process::ExitCode};

use anyhow::Context as _;
use clap::{Args, Parser, Subcommand};
use lee::{AccountId, PublicKey};
use lee_core::{account::Actor, native_token::NATIVE_TOKEN_PROGRAM_ID, program::PdaSeed};
use serde_json::json;

use crate::{
    chain::Chain,
    guests::Guests,
    plan::{Request, Spec, short},
};

mod chain;
mod dry_run;
mod guests;
mod plan;

#[derive(Parser)]
#[command(about = "Plan a LEZ public call with local dry runs against chain state")]
struct Cli {
    /// Print one `LEZDRYRUN {json}` line instead of text.
    #[arg(long, global = true)]
    json: bool,

    /// The sequencer's RPC address.
    #[arg(long, global = true, default_value = "http://127.0.0.1:3040")]
    rpc: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Work out a call's actors, signers and evidence, without sending it.
    Plan(PlanArgs),
    /// The local build a deployed program runs: its guest name, by image id.
    SourceOf {
        program: String,
        /// Where the guests' `.bin` builds are.
        #[arg(long)]
        bin_dir: PathBuf,
    },
}

#[derive(Args)]
struct PlanArgs {
    /// The account that pays the fee.
    #[arg(long)]
    payer: String,
    /// The program called.
    #[arg(long)]
    program: String,
    /// What to call the program in the plan's text.
    #[arg(long)]
    program_name: Option<String>,
    /// The message, Borsh-encoded (`lez-call encode`).
    #[arg(long)]
    message_hex: String,
    /// The root actor: the message goes to it.
    #[arg(long)]
    to: String,
    /// Another actor to declare. Repeatable.
    #[arg(long)]
    actor: Vec<String>,
    /// An account the wallet holds the key of, so it can sign. Repeatable.
    #[arg(long)]
    signer: Vec<String>,
    /// A seed to try for a PDA that isn't on chain, as `[NAME=]<0x hex | base58>`; a named one
    /// may stand for its seed in `pda:` actors. Repeatable.
    #[arg(long)]
    seed: Vec<String>,
    /// An account id the message names. Repeatable.
    #[arg(long)]
    mention: Vec<String>,
    /// A wallet label, as `<name>=<id>`. Repeatable.
    #[arg(long)]
    label: Vec<String>,
    /// The root's account signs.
    #[arg(long, conflicts_with = "no_sign")]
    sign: bool,
    /// The root's account never signs, even when a run fails without it.
    #[arg(long)]
    no_sign: bool,
    /// The source hints that the message is signed: try the root signing first.
    #[arg(long)]
    hinted_signed: bool,
    /// Leave async messages to undeclared actors pending, instead of delivering them now.
    #[arg(long)]
    leave_pending: bool,
    /// The cycles the transaction may use: the wallet's gas limit.
    #[arg(long, default_value_t = 2_000_000)]
    budget: u64,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("Error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Command::Plan(args) => plan_command(&cli.rpc, &args, cli.json),
        Command::SourceOf { program, bin_dir } => {
            let program = account(&program, &[])?;
            let chain = Chain::new(&cli.rpc, BTreeSet::new())?;
            let header = chain
                .header(program)?
                .with_context(|| format!("no program is deployed at {program}"))?;
            let guest = Guests { bin_dir }.by_image_id(header.image_id)?;
            if cli.json {
                println!("LEZDRYRUN {}", json!({ "guest": guest }));
            } else if let Some(guest) = &guest {
                println!("{guest}");
            }
            Ok(if guest.is_some() {
                ExitCode::SUCCESS
            } else {
                eprintln!("no local build runs program {program}");
                ExitCode::FAILURE
            })
        }
    }
}

fn plan_command(rpc: &str, args: &PlanArgs, json: bool) -> anyhow::Result<ExitCode> {
    let labels = args
        .label
        .iter()
        .map(|text| {
            let (name, id) = text
                .split_once('=')
                .with_context(|| format!("--label {text}: expected <name>=<id>"))?;
            Ok((name.to_owned(), account(id, &[])?))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let account = |text: &str| account(text, &labels);
    let named_seeds = args
        .seed
        .iter()
        .map(|text| match text.split_once('=') {
            Some((name, seed)) => Ok((Some(name.to_owned()), parse_seed(seed, &[])?)),
            None => Ok((None, parse_seed(text, &[])?)),
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let names: Vec<(String, PdaSeed)> = named_seeds
        .iter()
        .filter_map(|(name, seed)| Some((name.clone()?, *seed)))
        .collect();

    let program = account(&args.program)?;
    let spec = |text: &str| parse_spec(text, program, &names, &labels);
    let request = Request {
        program,
        root: spec(&args.to)?,
        actors: args
            .actor
            .iter()
            .map(|text| spec(text))
            .collect::<anyhow::Result<_>>()?,
        message: hex::decode(args.message_hex.trim_start_matches("0x"))
            .context("--message-hex isn't hex")?,
        payer: account(&args.payer)?,
        sign: if args.sign {
            Some(true)
        } else if args.no_sign {
            Some(false)
        } else {
            None
        },
        hinted_signed: args.hinted_signed,
        seeds: named_seeds.iter().map(|(_, seed)| *seed).collect(),
        mentioned: args
            .mention
            .iter()
            .map(|text| account(text))
            .collect::<anyhow::Result<_>>()?,
        leave_pending: args.leave_pending,
        budget: args.budget,
    };
    let signers = args
        .signer
        .iter()
        .map(|text| account(text))
        .collect::<anyhow::Result<_>>()?;
    let chain = Chain::new(rpc, signers)?;
    let plan = plan::plan(&chain, &request)?;

    let name = |id: AccountId| {
        if id == program {
            args.program_name.clone().unwrap_or_else(|| short(id))
        } else if id == NATIVE_TOKEN_PROGRAM_ID {
            "native".to_owned()
        } else {
            short(id)
        }
    };
    let lines = plan.lines(&name);
    if json {
        println!("LEZDRYRUN {}", json!({ "plan": plan, "lines": lines }));
    } else {
        for line in lines {
            println!("{line}");
        }
    }
    Ok(if plan.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// An account id: base58 (with or without `Public/`), `0x` hex, `pk:<hex>` for the account of a
/// public key, or a label.
fn account(text: &str, labels: &[(String, AccountId)]) -> anyhow::Result<AccountId> {
    let text = text.trim();
    if let Some(key) = text.strip_prefix("pk:") {
        return Ok(AccountId::from(&public_key(key)?));
    }
    if let Some(hex) = text.strip_prefix("0x") {
        let bytes: [u8; 32] = hex::decode(hex)?
            .try_into()
            .map_err(|bytes: Vec<u8>| anyhow::anyhow!("expected 32 bytes, got {}", bytes.len()))?;
        return Ok(AccountId::new(bytes));
    }
    let bare = text.strip_prefix("Public/").unwrap_or(text);
    if let Ok(account_id) = bare.parse() {
        return Ok(account_id);
    }
    labels
        .iter()
        .find(|(label, _)| label == text)
        .map(|(_, id)| *id)
        .with_context(|| format!("{text:?} isn't base58, 0x hex, or a label given with --label"))
}

fn public_key(hex: &str) -> anyhow::Result<PublicKey> {
    hex.parse()
        .with_context(|| format!("{hex:?} isn't a public key (64 hex digits)"))
}

/// A seed: a name given with `--seed NAME=…`, 0x hex, or base58.
fn parse_seed(text: &str, names: &[(String, PdaSeed)]) -> anyhow::Result<PdaSeed> {
    if let Some((_, seed)) = names.iter().find(|(name, _)| name == text) {
        return Ok(*seed);
    }
    Ok(PdaSeed::new(*account(text, &[])?.value()))
}

/// `<account>[@program][:sign]`, `pda:[<owner>/]<seed>[@program]`, or `pk:<hex>[@program]`, as
/// `wallet program call` takes them.
fn parse_spec(
    text: &str,
    program: AccountId,
    seeds: &[(String, PdaSeed)],
    labels: &[(String, AccountId)],
) -> anyhow::Result<Spec> {
    let (text, sign) = text
        .trim()
        .strip_suffix(":sign")
        .map_or_else(|| (text.trim(), false), |text| (text, true));
    let (ident, actor_program) = match text.rsplit_once('@') {
        Some((ident, actor_program)) => (ident, account(actor_program, labels)?),
        None => (text, program),
    };
    if let Some(pda) = ident.strip_prefix("pda:") {
        let (owner, seed) = match pda.rsplit_once('/') {
            Some((owner, seed)) => (account(owner, labels)?, seed),
            None => (program, pda),
        };
        let seed = parse_seed(seed, seeds)?;
        return Ok(Spec {
            actor: Actor::new(AccountId::for_public_pda(&owner, &seed), actor_program),
            sign,
            pda: Some((owner, seed)),
            key: None,
        });
    }
    if let Some(hex) = ident.strip_prefix("pk:") {
        let key = public_key(hex)?;
        return Ok(Spec {
            actor: Actor::new(AccountId::from(&key), actor_program),
            sign,
            pda: None,
            key: Some(key),
        });
    }
    Ok(Spec {
        actor: Actor::new(account(ident, labels)?, actor_program),
        sign,
        pda: None,
        key: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_parse_as_the_wallet_takes_them() {
        let program = AccountId::new([1; 32]);
        let other = AccountId::new([2; 32]);
        let seeds = [("SEAT".to_owned(), PdaSeed::new([5; 32]))];
        let labels = [("alice".to_owned(), AccountId::new([9; 32]))];

        let spec = parse_spec(&format!("alice@{other}:sign"), program, &seeds, &labels).unwrap();
        assert_eq!(spec.actor, Actor::new(AccountId::new([9; 32]), other));
        assert!(spec.sign);

        let spec = parse_spec("pda:SEAT", program, &seeds, &labels).unwrap();
        assert_eq!(spec.pda, Some((program, PdaSeed::new([5; 32]))));
        assert_eq!(
            spec.actor.account_id,
            AccountId::for_public_pda(&program, &PdaSeed::new([5; 32]))
        );

        let spec = parse_spec(
            &format!("pda:Public/{other}/0x{}", "07".repeat(32)),
            program,
            &seeds,
            &labels,
        )
        .unwrap();
        assert_eq!(spec.pda, Some((other, PdaSeed::new([7; 32]))));
        assert!(parse_spec("bob", program, &seeds, &labels).is_err());
    }
}
