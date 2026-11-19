//! Core types for the payroll program: its messages and its actors' states.
//!
//! State and messages outlive code: state stays on accounts, and async messages wait in mailboxes.
//! Borsh encodes an enum variant as a leading tag byte, so every enum here is append-only. A
//! changed layout is a new variant, never an edit of an existing one.

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{account::AccountId, program::PdaSeed};

/// The seed of `seat`'s treasury PDA: the seat's account id.
#[must_use]
pub fn seed_of(seat: AccountId) -> PdaSeed {
    PdaSeed::new(*seat.value())
}

/// Messages a transaction sends the seat: the payroll's owner, signing with its private account.
///
/// Each one is checked at the seat (`is_authorized`, no sender) and forwarded to the treasury as
/// an [`Internal`] message that grants the treasury's seed.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Message {
    /// Open a payroll whose treasury is `treasury`, paying every `period_ms` milliseconds.
    ///
    /// `treasury` is the private PDA `AccountId::for_private_pda(program, seed, npk, vpk)`, where
    /// `seed` is the seat's account id and `npk`, `vpk` are the seat's keys. The guest can't
    /// derive it (it never sees the keys), so the seat stores it, once.
    Open { treasury: AccountId, period_ms: u64 },

    /// Pay `employee` `amount` each period from now on. `amount` 0 stops their pay, and keeps
    /// what they are owed and were paid.
    SetEmployee { employee: AccountId, amount: u128 },

    /// Add every employee's pay for each period up to and including `period` to what they are
    /// owed. Valid only once `period` has begun: at `period * period_ms` (Unix ms).
    Accrue { period: u64 },

    /// Pay `employee` everything they are owed, from the treasury's native balance.
    Pay { employee: AccountId },

    /// Move `amount` of the treasury's native balance that nobody is owed back to the seat.
    Withdraw { amount: u128 },
}

/// Messages only the payroll's own actors send each other: the seat to its treasury. The guest
/// decodes these only when the sender is this program, and the treasury also requires the seed
/// grant, which only this program can give and only the seat's keys can use.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Internal {
    /// Write the treasury's initial state, once.
    Init { seed: PdaSeed, period_ms: u64 },
    /// Set `employee`'s pay per period.
    Configure { employee: AccountId, amount: u128 },
    /// Accrue every employee's pay up to and including `period`.
    AccrueTo { period: u64 },
    /// Pay `employee` what they are owed.
    Release { employee: AccountId },
    /// Pay `amount` to `to` (the seat), if the treasury holds that much beyond what it owes.
    Recover { amount: u128, to: AccountId },
}

/// The seat's state: its program actor on the seat's own (private) account.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SeatState {
    /// The treasury this seat opened.
    pub treasury: AccountId,
}

/// The treasury's state: its program actor on the treasury PDA. Only the seat can read it.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PayrollState {
    /// The treasury's PDA seed, which every custody transfer out of it carries.
    pub seed: PdaSeed,
    /// How long a pay period lasts, in the clock's milliseconds.
    pub period_ms: u64,
    /// The last period accrued, if any.
    pub last_accrued: Option<u64>,
    /// The sum of every employee's `owed`: the part of the balance the seat can't withdraw.
    pub total_owed: u128,
    /// The sum of every employee's `paid`.
    pub total_paid: u128,
    pub employees: Vec<Employee>,
    /// A withdrawal waiting for the treasury's native balance to answer `ReadState`.
    pub pending_withdraw: Option<Withdrawal>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Employee {
    pub account: AccountId,
    /// Pay per period.
    pub amount: u128,
    /// Accrued and not yet paid.
    pub owed: u128,
    /// Paid so far, in total.
    pub paid: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Withdrawal {
    pub amount: u128,
    pub to: AccountId,
}

impl PayrollState {
    #[must_use]
    pub const fn new(seed: PdaSeed, period_ms: u64) -> Self {
        Self {
            seed,
            period_ms,
            last_accrued: None,
            total_owed: 0,
            total_paid: 0,
            employees: Vec::new(),
            pending_withdraw: None,
        }
    }

    /// Decodes the treasury's actor state.
    ///
    /// # Panics
    /// If it is empty (the payroll isn't open) or isn't a `PayrollState`.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Self {
        assert!(!bytes.is_empty(), "the payroll isn't open");
        borsh::from_slice(bytes).expect("treasury state must decode")
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("borsh serialization is infallible")
    }

    #[must_use]
    pub fn employee(&self, account: AccountId) -> Option<&Employee> {
        self.employees.iter().find(|e| e.account == account)
    }
}

impl SeatState {
    /// Decodes the seat's actor state.
    ///
    /// # Panics
    /// If it is empty (this seat hasn't opened a payroll) or isn't a `SeatState`.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Self {
        assert!(!bytes.is_empty(), "this seat hasn't opened a payroll");
        borsh::from_slice(bytes).expect("seat state must decode")
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("borsh serialization is infallible")
    }
}
