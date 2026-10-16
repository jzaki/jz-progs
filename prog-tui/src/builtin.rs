//! The programs that are part of LEZ itself, and their addresses.
//!
//! Two sit at fixed addresses (`lee/state_machine/core/src`): the native token (`[0; 32]`) and the
//! program loader (`[0xFE; 32]`). Both are run by the engine rather than interpreted from a guest.
//! The rest are guests in `lez/programs/`, at `SHA-256(PREFIX ‖ name)`, which is
//! `AccountId::from_builtin_program_name`. The standalone dev chain deploys only some of them.

use sha2::{Digest as _, Sha256};

/// `BUILTIN_PROGRAM_NAME_PREFIX` in `lee/state_machine/core/src/program/mod.rs`.
const NAME_PREFIX: &[u8; 32] = b"/LEE-BuiltinProgram/v1/AccountId";

/// The `*_NAME` constants in `lez/programs/*/core/src/lib.rs`.
const GUEST_NAMES: [&str; 10] = [
    "clock",
    "fee",
    "bridge",
    "bridge_lock",
    "cross_zone_inbox",
    "cross_zone_outbox",
    "ping_sender",
    "ping_receiver",
    "sequencer_stake",
    "wrapped_token",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Builtin {
    pub name: &'static str,
    pub id: String,
    /// Run by the engine itself, so always available. The others are guests that a chain has to
    /// deploy at genesis, and the dev chain deploys only some.
    pub engine: bool,
    /// Where the address comes from.
    pub origin: &'static str,
}

pub fn native_token_id() -> String {
    bs58::encode([0_u8; 32]).into_string()
}

pub fn program_loader_id() -> String {
    bs58::encode([0xFE_u8; 32]).into_string()
}

fn named_id(name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(NAME_PREFIX);
    hasher.update(name.as_bytes());
    bs58::encode(hasher.finalize()).into_string()
}

/// Every built-in program, fixed ones first.
pub fn builtins() -> Vec<Builtin> {
    let mut all = vec![
        Builtin {
            name: "native token",
            id: native_token_id(),
            engine: true,
            origin: "fixed address: 32 zero bytes (NATIVE_TOKEN_PROGRAM_ID). Holds every balance; run by the engine, not a guest",
        },
        Builtin {
            name: "program_loader",
            id: program_loader_id(),
            engine: true,
            origin: "fixed address: 32 bytes of 0xFE (PROGRAM_LOADER_ACCOUNT_ID). Stores deployed programs as header and segment accounts; run by the engine, not a guest",
        },
    ];
    all.extend(GUEST_NAMES.iter().map(|name| Builtin {
        name,
        id: named_id(name),
        engine: false,
        origin: "a guest in lez/programs/, at SHA-256(\"/LEE-BuiltinProgram/v1/AccountId\" + name). The dev chain deploys only some of them",
    }));
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_addresses() {
        assert_eq!(native_token_id(), "1".repeat(32));
        assert_eq!(
            program_loader_id(),
            "JAQ5MVHbCkSYRzXunsrNuM2m1LS859PGveHfoYPAmcvZ"
        );
    }

    #[test]
    fn named_addresses_match_the_live_chain() {
        // Computed independently, and each exists on the running dev chain via `getAccount`.
        let expected = [
            ("clock", "2hwaRW8sMnyKLLRbDRw8k68tiv9bnfq98oxcdtcp157Z"),
            ("fee", "2TsGdKDWTY92X4vSBZzjzemZpThhyqNEdZJj3xBrQ9AZ"),
            ("bridge", "99GUyPapsAwpRKEoEQKFNAT4WJtJJyCdBnSby7Ht7qp4"),
            (
                "sequencer_stake",
                "5NbpF2r22SE97w1PhFcy5JBwfRrRrUvJToLzUR94EtyW",
            ),
        ];
        for (name, id) in expected {
            assert_eq!(named_id(name), id, "{name}");
        }
    }

    #[test]
    fn lists_everything_once() {
        let all = builtins();
        assert_eq!(all.len(), 2 + GUEST_NAMES.len());
        let mut ids: Vec<&str> = all.iter().map(|b| b.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
    }

    #[test]
    fn only_the_fixed_ones_are_engine_programs() {
        let engine: Vec<&str> = builtins().iter().filter(|b| b.engine).map(|b| b.name).collect();
        assert_eq!(engine, ["native token", "program_loader"]);
    }
}
