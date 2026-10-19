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
    /// Any other type: one Borsh value of it.
    Single { ty: String, fields: Option<String> },
}

#[derive(Debug, PartialEq, Eq)]
pub struct Variant {
    pub name: String,
    /// `""`, `"(Vec<u8>)"` or `"{ data: Vec<u8>, to: AccountId }"`.
    pub fields: String,
    /// The first line of its doc comment.
    pub doc: Option<String>,
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
        .unwrap_or_else(|| Message::Single {
            ty: tidy(&ty.to_token_stream().to_string()),
            fields: None,
        });
    Ok(Interface { handler, message })
}

impl Interface {
    /// What the Guests pane's `v` shows.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!("handler: {}", self.handler), String::new()];
        match &self.message {
            Message::Raw => {
                lines.push("raw: the handler decodes the message bytes itself.".to_owned());
                lines.push("Read its source for the formats it accepts.".to_owned());
            }
            Message::Single { ty, fields } => {
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
                    doc: doc(&variant.attrs),
                })
                .collect(),
        }),
        Item::Struct(item) if item.ident == name => Some(Message::Single {
            ty: name.to_owned(),
            fields: Some(fields(&item.fields)),
        }),
        _ => None,
    })
}

fn fields(fields: &Fields) -> String {
    let ty = |ty: &Type| tidy(&ty.to_token_stream().to_string());
    match fields {
        Fields::Unit => String::new(),
        Fields::Unnamed(fields) => {
            let types: Vec<String> = fields.unnamed.iter().map(|f| ty(&f.ty)).collect();
            format!("({})", types.join(", "))
        }
        Fields::Named(fields) => {
            let named: Vec<String> = fields
                .named
                .iter()
                .map(|f| {
                    let name = f.ident.as_ref().map(ToString::to_string).unwrap_or_default();
                    format!("{name}: {}", ty(&f.ty))
                })
                .collect();
            format!(" {{ {} }}", named.join(", "))
        }
    }
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
                fields: None
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
                fields: Some(" { to: AccountId }".to_owned())
            }
        );
    }

    #[test]
    fn a_file_without_an_entry_point_is_an_error() {
        assert!(parse("fn main() {}").is_err());
    }
}
