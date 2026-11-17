//! Calls to lez-programs' programs through `spel` (github.com/logos-co/spel, branch
//! `refactor/lez-v0.4.x`), from their spel IDLs (v2, `artifacts/<program>-idl.json`).
//!
//! Their guests decode the message themselves (`actor_entrypoint!(raw …)`), so `lez-call` can't
//! read their messages from source; the IDL has them. `spel send` encodes the message, simulates
//! the transaction and doesn't send one that would be refused, then reports which declared actors
//! changed. It links LEZ's wallet library, so it needs no `wallet program call`.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::Context as _;
use serde_json::Value;

use crate::{
    builtin,
    lez_call::{Method, Param},
    state::shell_quote,
};

/// `spel send`'s exit code when nothing changed: refused in simulation (so not sent), or included
/// with no change to any declared actor.
pub const EXIT_NO_CHANGE: i32 = 2;

/// The messages in a spel IDL, as the call form takes them: `index` is the tag, each param a
/// field with its spel type and how to type it.
pub fn methods(idl: &Path) -> anyhow::Result<Vec<Method>> {
    let text = std::fs::read_to_string(idl)
        .with_context(|| format!("no IDL at {} (key b runs lez-programs' `make idl`)", idl.display()))?;
    let value: Value = serde_json::from_str(&text).with_context(|| format!("{} isn't JSON", idl.display()))?;
    methods_of(&value)
}

fn methods_of(idl: &Value) -> anyhow::Result<Vec<Method>> {
    anyhow::ensure!(
        idl["version"] == "2",
        "not a spel IDL v2 (version {}): rebuild the IDLs (key b)",
        idl["version"]
    );
    let messages = idl["messages"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("the IDL has no messages"))?;
    Ok(messages
        .iter()
        .filter_map(|message| {
            let name = message["name"].as_str()?.to_owned();
            let params: Vec<Param> = message["fields"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|field| {
                    let ty = &field["type"];
                    Some(Param {
                        name: field["name"].as_str()?.to_owned(),
                        ty: type_name(ty),
                        hint: type_hint(ty),
                    })
                })
                .collect();
            let fields = format!(
                "({})",
                params
                    .iter()
                    .map(|param| format!("{}: {}", param.name, param.ty))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            Some(Method {
                name,
                index: message["tag"].as_u64().and_then(|tag| u8::try_from(tag).ok()),
                fields,
                params,
                doc: message["docs"]
                    .as_str()
                    .and_then(|docs| docs.lines().next())
                    .map(str::to_owned),
                hints: Vec::new(),
            })
        })
        .collect())
}

/// A spel IDL type as Rust-ish text: `u128`, `Option<account_id>`, `Vec<u8>`, `[u8; 32]`, a
/// defined type's name.
pub fn type_name(ty: &Value) -> String {
    match ty {
        Value::String(name) => name.clone(),
        Value::Object(map) => {
            if let Some(inner) = map.get("vec") {
                format!("Vec<{}>", type_name(inner))
            } else if let Some(inner) = map.get("option") {
                format!("Option<{}>", type_name(inner))
            } else if let Some(name) = map.get("defined").and_then(Value::as_str) {
                name.to_owned()
            } else if let Some([inner, len]) = map.get("array").and_then(Value::as_array).map(Vec::as_slice) {
                format!("[{}; {len}]", type_name(inner))
            } else {
                ty.to_string()
            }
        }
        _ => ty.to_string(),
    }
}

/// How to type a value of `ty`, as `spel` parses it.
fn type_hint(ty: &Value) -> String {
    match ty {
        Value::String(name) => match name.as_str() {
            "account_id" => "a wallet label, base58, or 0x hex".to_owned(),
            "string" => "text".to_owned(),
            "bool" => "true or false".to_owned(),
            "u8" | "u16" | "u32" | "u64" | "u128" | "i8" | "i16" | "i32" | "i64" | "i128" => {
                "decimal".to_owned()
            }
            other => other.to_owned(),
        },
        Value::Object(map) => {
            if let Some(inner) = map.get("option") {
                format!("empty or `none` for none, or {}", type_hint(inner))
            } else if map.contains_key("vec") {
                "comma-separated".to_owned()
            } else if map.contains_key("defined") {
                "a variant name, or a JSON object".to_owned()
            } else if map.contains_key("array") {
                "hex".to_owned()
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

/// `NewDefinition` → `new-definition`: how `spel send` names a message (its `to_kebab`).
pub fn kebab(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else if c == '_' {
            out.push('-');
        } else {
            out.push(c);
        }
    }
    out
}

/// One `spel send`, as the form gives it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpelCall {
    /// The deployment's name, e.g. `token`.
    pub name: String,
    pub idl: PathBuf,
    pub program_id: String,
    pub payer: String,
    /// The message's IDL name, e.g. `NewDefinition`.
    pub message: String,
    /// The actor the message goes to: an account (on this program), or `account@program`.
    pub root: String,
    pub signers: Vec<String>,
    pub actors: Vec<String>,
    /// `(field name, value)` in IDL order.
    pub fields: Vec<(String, String)>,
}

impl SpelCall {
    /// The arguments after `spel`. Everything after the `--` is the message and its fields: spel
    /// passes those through untouched, so a field never reads as one of its own flags.
    pub fn args(&self, dry_run: bool) -> Vec<String> {
        let mut args = vec![
            "--idl".to_owned(),
            self.idl.display().to_string(),
            "--program".to_owned(),
            self.program_id.clone(),
            "--fee-payer".to_owned(),
            self.payer.clone(),
            "send".to_owned(),
            "--root".to_owned(),
            self.root.clone(),
        ];
        for signer in &self.signers {
            args.extend(["--signer".to_owned(), signer.clone()]);
        }
        for actor in &self.actors {
            args.extend(["--actor".to_owned(), actor.clone()]);
        }
        if dry_run {
            args.push("--dry-run".to_owned());
        }
        args.extend(["--".to_owned(), kebab(&self.message)]);
        for (name, value) in &self.fields {
            args.extend([format!("--{}", name.replace('_', "-")), value.clone()]);
        }
        args
    }

    /// The same call, to paste into a shell that sourced `env.sh`.
    pub fn command_line(&self) -> String {
        let words: Vec<String> = self
            .args(false)
            .iter()
            .map(|word| {
                let plain = !word.is_empty()
                    && word.chars().all(|c| c.is_ascii_alphanumeric() || "-_./@:=".contains(c));
                if plain {
                    word.clone()
                } else {
                    shell_quote(word)
                }
            })
            .collect();
        format!("spel {}", words.join(" "))
    }
}

/// The 32 bytes of an account typed as a wallet label, base58 (`Public/` optional), or `0x` hex.
fn account_bytes(text: &str, labels: &BTreeMap<String, String>) -> Result<[u8; 32], String> {
    let text = text.trim();
    let text = text.strip_prefix("Public/").unwrap_or(text);
    if let Some(hex) = text.strip_prefix("0x") {
        let bytes = crate::rpc::unhex(hex).ok_or_else(|| format!("{text:?} isn't hex"))?;
        return bytes.try_into().map_err(|_| format!("{text:?} isn't 32 bytes"));
    }
    let id = labels.get(text).map_or(text, String::as_str);
    bs58::decode(id)
        .into_vec()
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| format!("{text:?} is neither a wallet label nor a base58 account id"))
}

/// The call with prog-tui's shorthands replaced by what spel takes:
/// - `to` = `pda:<account>`: this program's PDA seeded by that account's id (as admin_seat and
///   simple_token derive theirs);
/// - an actor `pda:<account>`: that PDA under this program, `<pda>@<program>`;
/// - an actor `clock`: the clock's actor, which a program that reads the clock sends to.
///
/// `<account>` is a wallet label, base58 or `0x` hex; `labels` maps labels to ids.
pub fn resolve_shorthands(call: &SpelCall, labels: &BTreeMap<String, String>) -> Result<SpelCall, String> {
    let program = account_bytes(&call.program_id, labels)?;
    let pda = |seed: &str| -> Result<String, String> {
        Ok(builtin::public_pda(&program, &account_bytes(seed, labels)?))
    };
    let root = match call.root.trim().strip_prefix("pda:") {
        Some(seed) => pda(seed)?,
        None => call.root.clone(),
    };
    let actors = call
        .actors
        .iter()
        .map(|actor| match actor.trim() {
            "clock" => Ok(builtin::clock_actor()),
            actor => match actor.strip_prefix("pda:") {
                Some(seed) => Ok(format!("{}@{}", pda(seed)?, call.program_id)),
                None => Ok(actor.to_owned()),
            },
        })
        .collect::<Result<_, String>>()?;
    Ok(SpelCall {
        root,
        actors,
        ..call.clone()
    })
}

/// Whether `spel inspect`'s JSON shows a `Holding` with `balance`. Spel prints u128s as strings.
pub fn shows_holding(inspect: &str, balance: u128) -> bool {
    let compact: String = inspect.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("\"Holding\"") && compact.contains(&format!("\"balance\":\"{balance}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_idls_messages() {
        let idl: Value = serde_json::from_str(
            r#"{"version":"2","name":"token","messages":[
                {"name":"NewDefinition","tag":0,"docs":"Create a fungible token.\n\nMore.","fields":[
                    {"name":"name","type":"string"},
                    {"name":"total_supply","type":"u128"},
                    {"name":"mint_authority","type":{"option":"account_id"}}]},
                {"name":"ReadState","tag":6}]}"#,
        )
        .unwrap();
        let methods = methods_of(&idl).unwrap();
        assert_eq!(methods.len(), 2);
        let new = &methods[0];
        assert_eq!(new.index, Some(0));
        assert_eq!(new.doc.as_deref(), Some("Create a fungible token."));
        assert_eq!(new.fields, "(name: string, total_supply: u128, mint_authority: Option<account_id>)");
        assert_eq!(new.params[2].ty, "Option<account_id>");
        assert!(new.params[2].hint.starts_with("empty or `none`"));
        assert_eq!(methods[1].label(), " 6  ReadState()");
        assert!(methods_of(&serde_json::json!({"version": "1"})).is_err());
    }

    #[test]
    fn names_types() {
        let ty = |text: &str| type_name(&serde_json::from_str(text).unwrap());
        assert_eq!(ty(r#"{"vec":"u8"}"#), "Vec<u8>");
        assert_eq!(ty(r#"{"array":["u8",32]}"#), "[u8; 32]");
        assert_eq!(ty(r#"{"defined":"TokenState"}"#), "TokenState");
    }

    #[test]
    fn kebabs_like_spel() {
        assert_eq!(kebab("NewDefinition"), "new-definition");
        assert_eq!(kebab("Transfer"), "transfer");
        assert_eq!(kebab("set_authority"), "set-authority");
    }

    #[test]
    fn builds_the_send() {
        let call = SpelCall {
            name: "token".into(),
            idl: PathBuf::from("/p/artifacts/token-idl.json"),
            program_id: "PROG".into(),
            payer: "PAY".into(),
            message: "NewDefinition".into(),
            root: "DEF".into(),
            signers: vec!["DEF".into(), "ACC".into()],
            actors: Vec::new(),
            fields: vec![
                ("name".into(), "My token".into()),
                ("total_supply".into(), "1000".into()),
                ("mint_authority".into(), String::new()),
            ],
        };
        assert_eq!(
            call.args(true).join(" "),
            "--idl /p/artifacts/token-idl.json --program PROG --fee-payer PAY send --root DEF \
             --signer DEF --signer ACC --dry-run -- new-definition --name My token \
             --total-supply 1000 --mint-authority "
        );
        assert_eq!(
            call.command_line(),
            "spel --idl /p/artifacts/token-idl.json --program PROG --fee-payer PAY send --root DEF \
             --signer DEF --signer ACC -- new-definition --name 'My token' --total-supply 1000 \
             --mint-authority ''"
        );
    }

    #[test]
    fn resolves_pda_and_clock_shorthands() {
        let program = bs58::encode([7_u8; 32]).into_string();
        let admin = bs58::encode([1_u8; 32]).into_string();
        let labels = BTreeMap::from([("admin".to_owned(), admin.clone())]);
        let call = SpelCall {
            program_id: program.clone(),
            root: "pda:admin".into(),
            actors: vec!["pda:admin".into(), "clock".into(), "X@Y".into()],
            ..SpelCall::default()
        };
        let resolved = resolve_shorthands(&call, &labels).unwrap();
        // lee_core's `for_public_pda([7; 32], [1; 32])`.
        let pda = "B8eG72E2vv9kQRMotzFsRMyL4vvdVHZTArYZwtXLDinp";
        assert_eq!(resolved.root, pda);
        assert_eq!(resolved.actors[0], format!("{pda}@{program}"));
        assert_eq!(resolved.actors[1], builtin::clock_actor());
        assert_eq!(resolved.actors[2], "X@Y");
        // The same seed by id, `Public/` id, or hex.
        for seed in [admin.clone(), format!("Public/{admin}"), format!("0x{}", "01".repeat(32))] {
            let call = SpelCall { root: format!("pda:{seed}"), ..call.clone() };
            assert_eq!(resolve_shorthands(&call, &labels).unwrap().root, pda);
        }
        let unknown = SpelCall { root: "pda:nobody".into(), ..call };
        assert!(resolve_shorthands(&unknown, &labels).is_err());
    }

    #[test]
    fn finds_a_holding_in_inspects_output() {
        let out = "{\n  \"Holding\": {\n    \"balance\": \"1000\",\n    \"definition_id\": \"x\"\n  }\n}\n";
        assert!(shows_holding(out, 1000));
        assert!(!shows_holding(out, 10));
        assert!(!shows_holding("{\"Definition\": {\"balance\": \"1000\"}}", 1000));
    }
}
