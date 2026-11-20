//! Core types for the multisig program: its messages, its actors' states, and the two hashes
//! clients must compute the same way as the guest (`seed_for` and `pseudonym`).
//!
//! State and messages outlive code: state stays on accounts, and async messages wait in mailboxes.
//! Borsh encodes an enum variant as a leading tag byte, so every enum here is append-only. A
//! changed layout is a new variant, never an edit of an existing one.

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{account::AccountId, program::PdaSeed};

/// The most members a vault holds. Bounds the state, which a hidden vault keeps in one note.
pub const MAX_MEMBERS: usize = 16;
/// The most proposals a vault keeps pending at once.
pub const MAX_PENDING: usize = 8;

/// The seed of a vault's PDA: generation `generation` of the vault created with `create_key`.
///
/// Generation 0 is the vault `Create` makes. A hidden vault's rotation moves it to the next
/// generation, under a new group secret, so a new address.
///
/// The hash is `AccountId::blinded`'s domain-separated SHA-256, so neither the guest nor a client
/// needs another hashing crate.
#[must_use]
pub fn seed_for(create_key: [u8; 32], generation: u64) -> PdaSeed {
    let mut word = [0_u8; 32];
    let (head, _) = word.split_at_mut(8);
    head.copy_from_slice(&generation.to_le_bytes());
    PdaSeed::new(*AccountId::new(create_key).blinded(&word).value())
}

/// A member's name at the vaults created with `create_key`: their seat's account, blinded by a
/// factor only their seat's private state knows.
///
/// Nobody can link the pseudonym to the seat account without `secret`. Only the seat can claim it:
/// the program's seat code computes it from its own address and its own state. It stays the same
/// across a hidden vault's rotations, since `create_key` does.
#[must_use]
pub fn pseudonym(seat: AccountId, secret: [u8; 32], create_key: [u8; 32]) -> AccountId {
    let factor = AccountId::new(secret).blinded(&create_key);
    seat.blinded(factor.value())
}

/// Where the vault lives. Chosen at `Create`, stored, and never changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Placement {
    /// `AccountId::for_public_pda(program, seed)`. Its state, balance and payouts are public; its
    /// members are pseudonyms. Revocation drops the member in place.
    Public,
    /// `AccountId::for_private_pda(program, seed, npk, vpk)`, with keys derived from a group secret
    /// every member holds. Only members see it. Revocation rotates to a new vault.
    Hidden,
}

/// Messages a member's transaction sends their own seat: the program's actor on the member's own
/// account. The seat checks the member signed (`is_authorized`, no sender), names itself by its
/// pseudonym, and forwards a [`ToVault`] to `vault`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Message {
    /// Write the seat's secret, once. Every pseudonym derives from it.
    InitSeat { secret: [u8; 32] },
    /// Create `vault`: `members` (pseudonyms, the caller's included) and `threshold` approvals.
    Create {
        vault: AccountId,
        placement: Placement,
        create_key: [u8; 32],
        members: Vec<AccountId>,
        threshold: u8,
    },
    /// Propose `action`, approving it.
    Propose {
        vault: AccountId,
        create_key: [u8; 32],
        action: Action,
    },
    /// Approve proposal `id`. The approval that meets the threshold runs the action.
    Approve {
        vault: AccountId,
        create_key: [u8; 32],
        id: u64,
    },
    /// Forward whatever a retired hidden vault holds to its successor.
    Sweep {
        vault: AccountId,
        create_key: [u8; 32],
    },
}

/// What a seat tells a vault, naming the seat by its pseudonym `member`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum ToVault {
    Create {
        member: AccountId,
        placement: Placement,
        create_key: [u8; 32],
        members: Vec<AccountId>,
        threshold: u8,
    },
    Propose { member: AccountId, action: Action },
    Approve { member: AccountId, id: u64 },
    Sweep { member: AccountId },
}

/// Every message one of this program's actors sends a vault.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum VaultMessage {
    /// From a member's seat.
    Seat(ToVault),
    /// From the hidden vault this one succeeds, granting this vault's seed.
    Adopt(Config),
}

/// What M of the members can make the vault do.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Action {
    /// Pay `amount` of the vault's native balance to `to`.
    Transfer { to: AccountId, amount: u128 },
    /// Add `member` (a pseudonym) and require `threshold` approvals from now on.
    ///
    /// `successor`: `None` adds them in place, and they can read this vault's history. `Some`
    /// rotates a hidden vault to `successor`, so they see only what comes after. A public vault
    /// takes `None` only.
    AddMember {
        member: AccountId,
        threshold: u8,
        successor: Option<AccountId>,
    },
    /// Remove `member` and require `threshold` approvals from now on.
    ///
    /// A hidden vault must rotate (`Some(successor)`): the removed member holds this vault's keys.
    /// A public vault removes in place (`None`).
    RemoveMember {
        member: AccountId,
        threshold: u8,
        successor: Option<AccountId>,
    },
}

/// Who the vault's members are and how many must agree. A rotation hands this to the successor.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Config {
    pub create_key: [u8; 32],
    pub generation: u64,
    pub members: Vec<AccountId>,
    pub threshold: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Proposal {
    pub id: u64,
    pub action: Action,
    /// The members (pseudonyms) who approved it, the proposer first.
    pub approvals: Vec<AccountId>,
}

/// Work waiting for the vault's native balance to answer `ReadState`, in the same transaction.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Pending {
    /// Hand `config` to the successor, and move the whole balance there.
    Rotate(Config),
    /// Move the whole balance to the successor.
    Sweep,
}

/// The vault's state: its program actor on the vault PDA.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct VaultState {
    pub placement: Placement,
    pub config: Config,
    /// The vault's PDA seed, which every custody transfer out of it carries.
    pub seed: PdaSeed,
    pub proposals: Vec<Proposal>,
    pub next_id: u64,
    /// The vault this one was rotated to. A retired vault only forwards funds.
    pub retired: Option<AccountId>,
    pub pending: Option<Pending>,
}

/// A seat's state: its program actor on the member's own account. Only the member sees it.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SeatState {
    pub secret: [u8; 32],
}

impl VaultState {
    #[must_use]
    pub fn new(placement: Placement, config: Config) -> Self {
        let seed = seed_for(config.create_key, config.generation);
        Self {
            placement,
            config,
            seed,
            proposals: Vec::new(),
            next_id: 0,
            retired: None,
            pending: None,
        }
    }

    /// Decodes the vault's actor state.
    ///
    /// # Panics
    /// If it is empty (no vault was created here) or isn't a `VaultState`.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Self {
        assert!(!bytes.is_empty(), "no vault was created here");
        borsh::from_slice(bytes).expect("vault state must decode")
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("borsh serialization is infallible")
    }

    #[must_use]
    pub fn is_member(&self, member: AccountId) -> bool {
        self.config.members.contains(&member)
    }

    #[must_use]
    pub fn proposal(&self, id: u64) -> Option<&Proposal> {
        self.proposals.iter().find(|p| p.id == id)
    }
}

impl SeatState {
    /// Decodes the seat's actor state.
    ///
    /// # Panics
    /// If it is empty (the seat isn't initialised) or isn't a `SeatState`.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Self {
        assert!(!bytes.is_empty(), "this seat isn't initialised (InitSeat)");
        borsh::from_slice(bytes).expect("seat state must decode")
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("borsh serialization is infallible")
    }
}
