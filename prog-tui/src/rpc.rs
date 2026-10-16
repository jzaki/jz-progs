//! The sequencer's JSON-RPC, and how to read what it returns.

use std::{collections::BTreeMap, fmt::Write as _, time::Duration};

use anyhow::{anyhow, bail};
use serde_json::{Value, json};

/// The native token program; an account's balance is its actor state here.
pub const NATIVE_TOKEN: &str = "11111111111111111111111111111111";

/// What `getAccount` returns: the nonce and every program's actor state on the account.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountView {
    pub nonce: u64,
    pub actor_states: BTreeMap<String, Vec<u8>>,
}

impl AccountView {
    pub fn from_json(value: &Value) -> Option<Self> {
        let states = value.get("data")?.get("actor_states")?.as_object()?;
        let actor_states = states
            .iter()
            .map(|(program, bytes)| {
                let bytes = bytes
                    .as_array()
                    .map(|bytes| {
                        bytes
                            .iter()
                            .filter_map(|byte| u8::try_from(byte.as_u64()?).ok())
                            .collect()
                    })
                    .unwrap_or_default();
                (program.clone(), bytes)
            })
            .collect();
        Some(Self {
            nonce: value.get("nonce").and_then(Value::as_u64).unwrap_or_default(),
            actor_states,
        })
    }

    pub fn balance(&self) -> u128 {
        self.actor_states
            .get(NATIVE_TOKEN)
            .map_or(0, |bytes| decode_u128_le(bytes))
    }
}

/// The native token stores a balance as a little-endian u128.
pub fn decode_u128_le(bytes: &[u8]) -> u128 {
    let mut buf = [0_u8; 16];
    let len = bytes.len().min(16);
    buf[..len].copy_from_slice(&bytes[..len]);
    u128::from_le_bytes(buf)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Quoted text when the bytes are printable UTF-8, otherwise hex (shortened past 48 bytes).
pub fn describe_bytes(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "(empty)".to_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(text) if !text.chars().any(char::is_control) => format!("{text:?}"),
        _ if bytes.len() > 48 => format!("0x{}…", hex(&bytes[..48])),
        _ => format!("0x{}", hex(bytes)),
    }
}

/// Balances are large (the genesis accounts hold 10^13), so show big ones in scientific form.
pub fn format_balance(balance: u128) -> String {
    if balance < 1_000_000 {
        return balance.to_string();
    }
    #[expect(clippy::cast_precision_loss, reason = "display only")]
    let value = balance as f64;
    format!("{value:.2e}")
}

#[derive(Clone, Debug)]
pub struct Rpc {
    client: reqwest::Client,
    pub url: String,
}

impl Rpc {
    pub fn new(url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap_or_default();
        Self { client, url }
    }

    async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let response: Value = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        if let Some(error) = response.get("error") {
            bail!("{method}: {error}");
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    /// `checkHealth` answers `null` when the sequencer is healthy.
    pub async fn healthy(&self) -> bool {
        self.call("checkHealth", json!([])).await.is_ok()
    }

    /// The current base fees, `(execution, storage)`, from `getFeeState`.
    pub async fn base_fees(&self) -> Option<(u64, u64)> {
        let state = self.call("getFeeState", json!([])).await.ok()?;
        Some((
            state["base_fee_exec"].as_u64()?,
            state["base_fee_stor"].as_u64()?,
        ))
    }

    pub async fn last_block(&self) -> Option<u64> {
        self.call("getLastBlockId", json!([])).await.ok()?.as_u64()
    }

    /// `None` when the account isn't on chain.
    pub async fn account(&self, account_id: &str) -> anyhow::Result<Option<AccountView>> {
        let result = self.call("getAccount", json!([account_id])).await?;
        if result.is_null() {
            return Ok(None);
        }
        AccountView::from_json(&result)
            .map(Some)
            .ok_or_else(|| anyhow!("unexpected getAccount result: {result}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_account_and_balance() {
        let value: Value = serde_json::from_str(
            r#"{"nonce":9,"data":{"actor_states":{"11111111111111111111111111111111":[208,252,160,156,48,18,0,0,0,0,0,0,0,0,0,0],"7WWX":[72,111,108,97]}}}"#,
        )
        .unwrap();
        let view = AccountView::from_json(&value).unwrap();
        assert_eq!(view.nonce, 9);
        assert_eq!(view.balance(), 19_999_995_526_352);
        assert_eq!(view.actor_states["7WWX"], b"Hola");
    }

    #[test]
    fn describes_bytes() {
        assert_eq!(describe_bytes(b"Hola mundo!"), "\"Hola mundo!\"");
        assert_eq!(describe_bytes(&[0, 1, 255]), "0x0001ff");
        assert_eq!(describe_bytes(&[]), "(empty)");
    }

    #[test]
    fn hex_round_trip() {
        assert_eq!(unhex(&hex(b"Hola mundo!")).unwrap(), b"Hola mundo!");
        assert_eq!(unhex("abc"), None);
    }

    #[test]
    fn formats_balances() {
        assert_eq!(format_balance(42), "42");
        assert_eq!(format_balance(20_000_000_000_000), "2.00e13");
    }
}
