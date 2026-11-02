use borsh::{BorshDeserialize, BorshSerialize};
use clock_core::{CLOCK_01_PROGRAM_ACCOUNT_ID, clock_account_id};
use lee_core::{
    account::Actor,
    program::{ReceiveInput, Response},
};

// Clock gate: two timestamps that can be set at most once every 5 seconds, gated by the chain's
// every-block clock, `CLOCK_01`. A block closes every `block_create_timeout` (the sequencer's
// network config, e.g. `lez/sequencer/service/networks/local.json`).
//
// The state is `(last_set, next_set)`. A set claims a current time `now`. Once the state exists,
// `now` must be at least `next_set`. The set then stores `(now, now + 5s)`.
//
// **The block timestamp.** Each block header carries a timestamp: the sequencer's wall clock when it
// starts building the block, in Unix milliseconds. The last transaction of every block is the clock
// tick, which writes that block's id and timestamp into `CLOCK_01` (`clock_core::ClockAccountData`).
// So a transaction in block N finds block N-1's timestamp there, about one block time behind. That
// is the "current" time a program can use.
//
// **No program reads it.** A program only learns another actor's state when that actor sends it.
// The native token answers `ReadState` with a `StateReply` (read and reply), but the clock has no
// such message. Its only read-only message is `AssertTimestamp { at_least, at_most }`, which panics
// unless its timestamp is in that range, and the panic fails the whole transaction. So the time
// travels in the message and the clock checks it, as in LEZ's
// `test_programs/guest/src/bin/time_locked_transfer.rs`.
//
// TODO: read the clock instead, once the clock answers `ReadState` with a `StateReply` of its
// state, as the native token does (`lee_core::native_token`). `Set` would then send `ReadState`
// to `CLOCK_01` and set from the reply, checking it comes from the clock actor. A reply isn't a
// `Message`, so this would need `actor_entrypoint!(raw …)`, dispatching on `from`.
//
// Until then:
// - `Set { now }` asserts one side, like `time_locked_transfer`'s deadline. `at_least: now` means
//   `now` isn't in the future. With `now >= next_set` checked here, the clock is past `next_set`.
//   `now` may be stale: a caller can claim an earlier time, down to `next_set`.
// - `SetExact { now }` asserts both sides, `at_least == at_most`, so `now` is exactly the clock's
//   timestamp. This is the nearest thing to reading it. The caller reads `CLOCK_01` off chain (RPC
//   `getAccount`, decoding `ClockAccountData` from the clock program's actor state) and passes what
//   it read. That only holds until the next tick, so the transaction must land in the block after
//   the read; one that lands later reverts. The shorter the block time, the less time that is.
//
// **Past timestamps.** On chain, the coarser clocks keep older ones. `CLOCK_10` and `CLOCK_50` hold
// the id and timestamp of the latest block whose id is a multiple of 10 or 50, so up to 9 and 49
// blocks old, and `AssertTimestamp` works against them too. Anything older isn't kept on chain: read
// a block's header timestamp off chain with RPC `getBlock(id)`. A program can't check that one, so
// to use a past time later, store it in your own state while it's current, as this program does.
//
// A response can also carry `timestamp_window(..)`, which the engine checks against the current
// block's own timestamp. It only bounds when the transaction may land: the program still never
// learns the time.
//
// Calls declare the clock actor, `CLOCK_01` under the clock program (lez-dryrun adds it). Anyone may
// set any account's timestamps here: the clock is the only gate.
//
// prog-tui builds it as the guest `jz_clock_gate` (key u, or b for every file here).

/// How long after a set the next one may come: 5 s, in the clock's milliseconds.
const COOLDOWN_MS: u64 = 5_000;

#[derive(Clone, Copy, BorshSerialize, BorshDeserialize)]
enum Message {
    /// Set to `now`, a Unix time in ms. The clock asserts `now` isn't in the future.
    Set { now: u64 },
    /// Set to `now`, the clock's timestamp as read off chain. The clock asserts it exactly.
    SetExact { now: u64 },
}

/// The actor's state. Empty until the first set.
#[derive(Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
struct Timestamps {
    /// When it was last set.
    last_set: u64,
    /// The earliest time the next set may claim: `last_set + COOLDOWN_MS`.
    next_set: u64,
}

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, message: Message) -> Response {
    let (now, at_most) = match message {
        Message::Set { now } => (now, u64::MAX),
        Message::SetExact { now } => (now, now),
    };
    if !input.pre_state.as_ref().is_empty() {
        let previous =
            Timestamps::try_from_slice(input.pre_state.as_ref()).expect("the state decodes");
        assert!(
            now >= previous.next_set,
            "too soon: {now} is before the next set, from {}",
            previous.next_set
        );
    }
    let timestamps = Timestamps {
        last_set: now,
        next_set: now.checked_add(COOLDOWN_MS).expect("now + 5s fits u64"),
    };
    // The state is set before the clock checks `now`. If the assertion panics, the whole
    // transaction fails, and this state with it.
    Response::set_state(borsh::to_vec(&timestamps).expect("the state encodes")).send_sync(
        clock(),
        &clock_core::Message::AssertTimestamp {
            at_least: now,
            at_most,
        },
    )
}

/// The every-block clock account, under the clock program.
fn clock() -> Actor {
    Actor::new(CLOCK_01_PROGRAM_ACCOUNT_ID, clock_account_id())
}

#[cfg(test)]
mod tests {
    use lee_core::{
        account::{AccountId, ActorState},
        program::{SyncMessage, Transition},
    };

    use super::*;

    const PROGRAM: AccountId = AccountId::new([7; 32]);
    const ACCOUNT: AccountId = AccountId::new([1; 32]);
    const NOW: u64 = 1_700_000_000_000;

    fn run(pre: Option<&Timestamps>, message: &Message) -> Transition {
        let input = ReceiveInput {
            receiver: Actor::new(ACCOUNT, PROGRAM),
            from: None,
            is_authorized: false,
            pre_state: ActorState::from(
                pre.map(|pre| borsh::to_vec(pre).unwrap())
                    .unwrap_or_default(),
            ),
            message: borsh::to_vec(message).unwrap(),
        };
        handle_message(&input, *message).into_transition(input)
    }

    fn stored(transition: &Transition) -> Timestamps {
        borsh::from_slice(transition.response.post_state.as_ref().unwrap().as_ref()).unwrap()
    }

    fn asserts(at_least: u64, at_most: u64) -> Vec<SyncMessage> {
        vec![SyncMessage::new(
            clock(),
            &clock_core::Message::AssertTimestamp { at_least, at_most },
        )]
    }

    #[test]
    fn the_first_set_stores_now_and_asserts_it_isnt_in_the_future() {
        let transition = run(None, &Message::Set { now: NOW });
        assert_eq!(
            stored(&transition),
            Timestamps {
                last_set: NOW,
                next_set: NOW + 5_000
            }
        );
        assert_eq!(transition.response.sync_messages, asserts(NOW, u64::MAX));
    }

    #[test]
    fn an_exact_set_pins_the_clock() {
        let transition = run(None, &Message::SetExact { now: NOW });
        assert_eq!(transition.response.sync_messages, asserts(NOW, NOW));
    }

    #[test]
    fn a_set_at_the_next_set_time_is_allowed() {
        let previous = Timestamps {
            last_set: NOW,
            next_set: NOW + 5_000,
        };
        let transition = run(Some(&previous), &Message::Set { now: NOW + 5_000 });
        assert_eq!(stored(&transition).next_set, NOW + 10_000);
    }

    #[test]
    #[should_panic(expected = "too soon")]
    fn a_set_before_the_next_set_time_is_refused() {
        let previous = Timestamps {
            last_set: NOW,
            next_set: NOW + 5_000,
        };
        let _transition = run(Some(&previous), &Message::Set { now: NOW + 4_999 });
    }
}
