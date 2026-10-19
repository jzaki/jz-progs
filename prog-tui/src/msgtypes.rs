//! The messages a guest accepts, read from its source file.
//!
//! A built guest carries no description of its messages: Borsh encodes an enum variant as its
//! index and its fields, with no names. So this parses the guest's source with `syn`: it finds
//! the handler that `actor_entrypoint!` names, takes the type of the handler's second parameter,
//! and lists that type's variants if the file defines it.

use quote::ToTokens as _;
use syn::{Attribute, Expr, Fields, FnArg, Item, ItemFn, Lit, Meta, Type};

#[derive(Debug, PartialEq, Eq)]
pub struct Interface {
    pub handler: String,
    pub message: Message,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Message {
    /// `actor_entrypoint!(raw handler)`: the handler decodes the bytes itself.
    Raw,
    /// An enum defined in the file.
    Enum { name: String, variants: Vec<Variant> },
    /// Any other type: one Borsh value of it. `fields` is set for a struct defined in the file.
    Single {
        ty: String,
        fields: Option<String>,
        params: Vec<Param>,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct Variant {
    pub name: String,
    /// `""`, `"(Vec<u8>)"` or `"{ data: Vec<u8>, to: AccountId }"`.
    pub fields: String,
    pub params: Vec<Param>,
    /// The first line of its doc comment.
    pub doc: Option<String>,
}

/// One value of a message, in Borsh order. `name` is `None` for a tuple field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: Option<String>,
    pub ty: String,
}

/// The type of a raw handler's one param: the message bytes, as hex.
pub const RAW: &str = "raw bytes";

/// One message that can be sent: what the picker lists, and what the form asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Method {
    pub label: String,
    /// The Borsh variant index, for an enum.
    pub index: Option<u8>,
    pub params: Vec<Param>,
}

pub fn parse(source: &str) -> anyhow::Result<Interface> {
    let file = syn::parse_file(source)?;
    let items = all_items(&file.items);
    let (raw, handler) = items
        .iter()
        .find_map(|item| match item {
            Item::Macro(item) => entry_handler(&item.mac),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("no actor_entrypoint! in this file"))?;
    if raw {
        return Ok(Interface {
            handler,
            message: Message::Raw,
        });
    }
    let function = items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.sig.ident == handler => Some(function),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("the handler `{handler}` isn't defined in this file"))?;
    let ty = message_type(function)
        .ok_or_else(|| anyhow::anyhow!("`{handler}` has no message parameter"))?;
    let name = match ty {
        Type::Path(path) => path.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    };
    let message = name
        .and_then(|name| local_type(&items, &name))
        .unwrap_or_else(|| {
            let ty = tidy(&ty.to_token_stream().to_string());
            Message::Single {
                params: vec![Param {
                    name: None,
                    ty: ty.clone(),
                }],
                ty,
                fields: None,
            }
        });
    Ok(Interface { handler, message })
}

impl Interface {
    /// The messages that can be sent, in variant order.
    pub fn methods(&self) -> Vec<Method> {
        match &self.message {
            Message::Raw => vec![Method {
                label: "raw message bytes".to_owned(),
                index: None,
                params: vec![Param {
                    name: Some("hex".to_owned()),
                    ty: RAW.to_owned(),
                }],
            }],
            Message::Single { ty, fields, params } => vec![Method {
                label: format!("{ty}{}", fields.as_deref().unwrap_or("")),
                index: None,
                params: params.clone(),
            }],
            Message::Enum { variants, .. } => variants
                .iter()
                .enumerate()
                .map(|(index, variant)| Method {
                    label: format!("{index:>2}  {}{}", variant.name, variant.fields),
                    index: u8::try_from(index).ok(),
                    params: variant.params.clone(),
                })
                .collect(),
        }
    }

    /// What the Guests pane's `v` shows.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!("handler: {}", self.handler), String::new()];
        match &self.message {
            Message::Raw => {
                lines.push("raw: the handler decodes the message bytes itself.".to_owned());
                lines.push("Read its source for the formats it accepts.".to_owned());
            }
            Message::Single { ty, fields, .. } => {
                lines.push(format!("one message type: {ty}{}", fields.as_deref().unwrap_or("")));
                lines.push(String::new());
                lines.push(if ty == "Vec<u8>" {
                    "Borsh: a u32 length, then the bytes.".to_owned()
                } else {
                    "Borsh: one encoded value of this type.".to_owned()
                });
            }
            Message::Enum { name, variants } => {
                lines.push(format!("enum {name}, {} variant(s):", variants.len()));
                lines.push(String::new());
                for (index, variant) in variants.iter().enumerate() {
                    lines.push(format!("  {index:>2}  {}{}", variant.name, variant.fields));
                    if let Some(doc) = &variant.doc {
                        lines.push(format!("        {doc}"));
                    }
                }
                lines.push(String::new());
                lines.push("Borsh: the variant's index as one byte, then its fields in order.".to_owned());
            }
        }
        lines
    }
}

/// Top-level items, and those of inline modules.
fn all_items(items: &[Item]) -> Vec<&Item> {
    let mut found = Vec::new();
    for item in items {
        found.push(item);
        if let Item::Mod(syn::ItemMod {
            content: Some((_, inner)),
            ..
        }) = item
        {
            found.extend(all_items(inner));
        }
    }
    found
}

/// `(raw, handler)` from `actor_entrypoint!(raw path::to::handler)`.
fn entry_handler(mac: &syn::Macro) -> Option<(bool, String)> {
    if mac.path.segments.last()?.ident != "actor_entrypoint" {
        return None;
    }
    let args = tidy(&mac.tokens.to_string());
    let args = args.trim().trim_end_matches(',').trim();
    let (raw, path) = match args.strip_prefix("raw ") {
        Some(path) => (true, path.trim()),
        None => (false, args),
    };
    let handler = path.rsplit("::").next()?.to_owned();
    Some((raw, handler))
}

fn message_type(function: &ItemFn) -> Option<&Type> {
    match function.sig.inputs.iter().nth(1)? {
        FnArg::Typed(arg) => Some(&arg.ty),
        FnArg::Receiver(_) => None,
    }
}

fn local_type(items: &[&Item], name: &str) -> Option<Message> {
    items.iter().find_map(|item| match item {
        Item::Enum(item) if item.ident == name => Some(Message::Enum {
            name: name.to_owned(),
            variants: item
                .variants
                .iter()
                .map(|variant| Variant {
                    name: variant.ident.to_string(),
                    fields: fields(&variant.fields),
                    params: params(&variant.fields),
                    doc: doc(&variant.attrs),
                })
                .collect(),
        }),
        Item::Struct(item) if item.ident == name => Some(Message::Single {
            ty: name.to_owned(),
            fields: Some(fields(&item.fields)),
            params: params(&item.fields),
        }),
        _ => None,
    })
}

fn params(fields: &Fields) -> Vec<Param> {
    fields
        .iter()
        .map(|field| Param {
            name: field.ident.as_ref().map(ToString::to_string),
            ty: tidy(&field.ty.to_token_stream().to_string()),
        })
        .collect()
}

/// `""`, `"(Vec<u8>)"` or `" { data: Vec<u8>, to: AccountId }"`.
fn fields(fields: &Fields) -> String {
    let params = params(fields);
    match fields {
        Fields::Unit => String::new(),
        Fields::Unnamed(_) => {
            let types: Vec<&str> = params.iter().map(|p| p.ty.as_str()).collect();
            format!("({})", types.join(", "))
        }
        Fields::Named(_) => {
            let named: Vec<String> = params
                .iter()
                .map(|p| format!("{}: {}", p.name.as_deref().unwrap_or_default(), p.ty))
                .collect();
            format!(" {{ {} }}", named.join(", "))
        }
    }
}

// ---- Encoding what the call form holds ----

/// The Borsh-encoded message: the variant index (for an enum), then each param's value.
pub fn encode_message(index: Option<u8>, params: &[(&str, &str)]) -> anyhow::Result<Vec<u8>> {
    let mut bytes: Vec<u8> = index.into_iter().collect();
    for (ty, input) in params {
        let encoded = encode(ty, input).map_err(|error| anyhow::anyhow!("{ty}: {error}"))?;
        bytes.extend(encoded);
    }
    Ok(bytes)
}

/// The Borsh encoding of one value typed into the form. Types it doesn't know take the hex of
/// their encoding.
pub fn encode(ty: &str, input: &str) -> anyhow::Result<Vec<u8>> {
    let input = input.trim();
    let with_length = |data: &[u8]| -> anyhow::Result<Vec<u8>> {
        let mut bytes = u32::try_from(data.len())?.to_le_bytes().to_vec();
        bytes.extend(data);
        Ok(bytes)
    };
    let short = ty.rsplit("::").next().unwrap_or(ty);
    match short {
        "Vec<u8>" => match input.strip_prefix("0x") {
            Some(hex) => with_length(&unhex(hex)?),
            None => with_length(input.as_bytes()),
        },
        "String" => with_length(input.as_bytes()),
        "bool" => match input {
            "true" => Ok(vec![1]),
            "false" => Ok(vec![0]),
            _ => anyhow::bail!("expected true or false, not {input:?}"),
        },
        "u8" => Ok(input.parse::<u8>()?.to_le_bytes().to_vec()),
        "u16" => Ok(input.parse::<u16>()?.to_le_bytes().to_vec()),
        "u32" => Ok(input.parse::<u32>()?.to_le_bytes().to_vec()),
        "u64" => Ok(input.parse::<u64>()?.to_le_bytes().to_vec()),
        "u128" => Ok(input.parse::<u128>()?.to_le_bytes().to_vec()),
        "i8" => Ok(input.parse::<i8>()?.to_le_bytes().to_vec()),
        "i16" => Ok(input.parse::<i16>()?.to_le_bytes().to_vec()),
        "i32" => Ok(input.parse::<i32>()?.to_le_bytes().to_vec()),
        "i64" => Ok(input.parse::<i64>()?.to_le_bytes().to_vec()),
        "i128" => Ok(input.parse::<i128>()?.to_le_bytes().to_vec()),
        "AccountId" => account_id(input),
        "Actor" => {
            let (account, program) = input
                .split_once(',')
                .ok_or_else(|| anyhow::anyhow!("expected `account,program`"))?;
            Ok([account_id(account)?, account_id(program)?].concat())
        }
        _ => {
            let bytes = unhex(input.strip_prefix("0x").unwrap_or(input))?;
            if let Some(len) = byte_array_len(short) {
                anyhow::ensure!(bytes.len() == len, "expected {len} bytes, got {}", bytes.len());
            }
            Ok(bytes)
        }
    }
}

/// A form hint for a param's type.
pub fn hint(ty: &str) -> &'static str {
    match ty.rsplit("::").next().unwrap_or(ty) {
        "Vec<u8>" => "text, or 0x… hex",
        "String" => "text",
        "bool" => "true / false",
        "u8" | "u16" | "u32" | "u64" | "u128" | "i8" | "i16" | "i32" | "i64" | "i128" => "decimal",
        "AccountId" => "base58",
        "Actor" => "account,program (base58)",
        _ => "hex of its Borsh encoding",
    }
}

/// `N` for `[u8; N]`.
fn byte_array_len(ty: &str) -> Option<usize> {
    ty.strip_prefix("[u8;")?.strip_suffix(']')?.trim().parse().ok()
}

fn account_id(text: &str) -> anyhow::Result<Vec<u8>> {
    let text = text.trim();
    let text = text.strip_prefix("Public/").unwrap_or(text);
    let bytes = bs58::decode(text)
        .into_vec()
        .map_err(|error| anyhow::anyhow!("not base58: {error}"))?;
    anyhow::ensure!(bytes.len() == 32, "an account id is 32 bytes, this is {}", bytes.len());
    Ok(bytes)
}

fn unhex(text: &str) -> anyhow::Result<Vec<u8>> {
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    anyhow::ensure!(text.len() % 2 == 0, "hex has an odd number of digits");
    (0..text.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&text[i..i + 2], 16)
                .map_err(|_| anyhow::anyhow!("not hex: {:?}", &text[i..i + 2]))
        })
        .collect()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn doc(attrs: &[Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| match &attr.meta {
        Meta::NameValue(meta) if meta.path.is_ident("doc") => match &meta.value {
            Expr::Lit(expr) => match &expr.lit {
                Lit::Str(text) => Some(text.value().trim().to_owned()),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    })
}

/// Undoes the spacing of a token stream's `to_string`: `Vec < u8 >` → `Vec<u8>`.
fn tidy(tokens: &str) -> String {
    let mut text = tokens.to_owned();
    for (from, to) in [
        (" :: ", "::"),
        (":: ", "::"),
        (" ::", "::"),
        (" < ", "<"),
        ("< ", "<"),
        (" <", "<"),
        (" >", ">"),
        (" ,", ","),
        (" ;", ";"),
        ("& ", "&"),
        ("[ ", "["),
        (" ]", "]"),
        ("( ", "("),
        (" )", ")"),
    ] {
        text = text.replace(from, to);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_greeting_is_one_vec() {
        let source = r"
            use lee_core::program::{ReceiveInput, Response};
            lee_core::actor_entrypoint!(handle_message);
            fn handle_message(input: &ReceiveInput, greeting: Vec<u8>) -> Response { todo!() }
        ";
        let interface = parse(source).unwrap();
        assert_eq!(interface.handler, "handle_message");
        assert_eq!(
            interface.message,
            Message::Single {
                ty: "Vec<u8>".to_owned(),
                fields: None,
                params: vec![Param {
                    name: None,
                    ty: "Vec<u8>".to_owned()
                }],
            }
        );
    }

    #[test]
    fn an_enum_lists_its_variants_in_borsh_order() {
        let source = r"
            #[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
            enum Message {
                /// Appends the bytes.
                Write(Vec<u8>),
                MoveData { data: Vec<u8>, to: AccountId },
                Stamp,
                Fixed([u8; 32], lee_core::account::AccountId),
            }
            lee_core::actor_entrypoint!(handle_message);
            fn handle_message(input: &ReceiveInput, message: Message) -> Response { todo!() }
        ";
        let Message::Enum { name, variants } = parse(source).unwrap().message else {
            panic!("not an enum");
        };
        assert_eq!(name, "Message");
        let shown: Vec<String> = variants
            .iter()
            .map(|v| format!("{}{}", v.name, v.fields))
            .collect();
        assert_eq!(
            shown,
            [
                "Write(Vec<u8>)",
                "MoveData { data: Vec<u8>, to: AccountId }",
                "Stamp",
                "Fixed([u8; 32], lee_core::account::AccountId)",
            ]
        );
        assert_eq!(variants[0].doc.as_deref(), Some("Appends the bytes."));
        assert_eq!(variants[1].doc, None);
    }

    #[test]
    fn the_raw_form_and_a_path_handler() {
        let raw = parse("lee_core::actor_entrypoint!(raw handle);").unwrap();
        assert_eq!(
            raw,
            Interface {
                handler: "handle".to_owned(),
                message: Message::Raw
            }
        );
        let source = r"
            lee_core::actor_entrypoint!(self::inner::run);
            mod inner {
                pub struct Ping { pub to: AccountId }
                pub fn run(input: &ReceiveInput, ping: Ping) -> Response { todo!() }
            }
        ";
        assert_eq!(
            parse(source).unwrap().message,
            Message::Single {
                ty: "Ping".to_owned(),
                fields: Some(" { to: AccountId }".to_owned()),
                params: vec![Param {
                    name: Some("to".to_owned()),
                    ty: "AccountId".to_owned()
                }],
            }
        );
    }

    #[test]
    fn a_file_without_an_entry_point_is_an_error() {
        assert!(parse("fn main() {}").is_err());
    }

    const HELLO_ACCOUNT: &str = r"
        enum Message {
            Hello(Vec<u8>),
            WriteAccountId,
            MoveData { data: Vec<u8>, to: AccountId },
        }
        lee_core::actor_entrypoint!(handle_message);
        fn handle_message(input: &ReceiveInput, message: Message) -> Response { todo!() }
    ";

    /// Encodes `method`'s params from `inputs`, as the call form does.
    fn send(method: &Method, inputs: &[&str]) -> anyhow::Result<String> {
        let params: Vec<(&str, &str)> = method
            .params
            .iter()
            .map(|p| p.ty.as_str())
            .zip(inputs.iter().copied())
            .collect();
        encode_message(method.index, &params).map(|bytes| hex(&bytes))
    }

    #[test]
    fn methods_encode_their_variant_and_params() {
        let methods = parse(HELLO_ACCOUNT).unwrap().methods();
        assert_eq!(methods[0].label, " 0  Hello(Vec<u8>)");
        assert_eq!(send(&methods[0], &["Hi"]).unwrap(), "00020000004869");
        assert_eq!(send(&methods[0], &["0xff00"]).unwrap(), "0002000000ff00");
        assert_eq!(send(&methods[1], &[]).unwrap(), "01");

        let id = bs58::encode([7_u8; 32]).into_string();
        let moved = send(&methods[2], &["Hi", &format!("Public/{id}")]).unwrap();
        assert_eq!(moved, format!("0202000000{}{}", "4869", "07".repeat(32)));
    }

    #[test]
    fn values_are_checked() {
        assert_eq!(hex(&encode("u64", "258").unwrap()), "0201000000000000");
        assert_eq!(hex(&encode("bool", "true").unwrap()), "01");
        assert_eq!(hex(&encode("[u8; 2]", "0xabcd").unwrap()), "abcd");
        let id = bs58::encode([1_u8; 32]).into_string();
        assert_eq!(encode("Actor", &format!("{id},{id}")).unwrap(), vec![1; 64]);

        assert!(encode("u8", "256").is_err());
        assert!(encode("u64", "ten").is_err());
        assert!(encode("[u8; 2]", "abc").is_err());
        assert!(encode("[u8; 2]", "abcdef").is_err());
        assert!(encode("AccountId", "not-base58!").is_err());
        assert!(encode("lee_core::account::AccountId", &bs58::encode([1_u8; 31]).into_string()).is_err());
    }

    #[test]
    fn a_raw_handler_takes_hex() {
        let methods = parse("lee_core::actor_entrypoint!(raw handle);").unwrap().methods();
        assert_eq!(methods.len(), 1);
        assert_eq!(send(&methods[0], &["0a 0b"]).unwrap(), "0a0b");
    }
}
