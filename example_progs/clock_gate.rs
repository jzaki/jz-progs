use borsh::{BorshDeserialize, BorshSerialize};
use clock_core::{Reply, clock_actor};
use lee_core::program::{ReceiveInput, Response};

// Clock gate: two timestamps that can be set at most once every 5 seconds, gated by the chain's
// clock. A block closes every `block_create_timeout` (the sequencer's network config, e.g.
// `lez/sequencer/service/networks/local.json`).
//
// The state is `(last_set, next_set)`. A set reads the clock's current time `now`. Once the state
// exists, `now` must be at least `next_set`. The set then stores `(now, now + 5s)`.
//
// **The block timestamp.** Each block header carries a timestamp: the sequencer's wall clock when it
// starts building the block, in Unix milliseconds. The last transaction of every block is the clock
// tick, which writes that block's id and timestamp into the clock account
// (`clock_core::ClockAccountData`). So a transaction in block N finds block N-1's timestamp there,
// about one block time behind. That is the "current" time a program can use.
//
// **Reading it.** A program only learns another actor's state when that actor sends it. The clock
// answers `ReadTimestamp { granularity }` by sending the sender `Reply::Timestamp(t)`, with `t`
// rounded down to a multiple of `granularity` (1 here: the exact timestamp). So `Set` asks the
// clock, and the clock's reply sets the timestamps, in the same transaction:
//
// 1. `Set`, from a transaction, to the account's gate: send `ReadTimestamp` to the clock.
// 2. The clock's reply, to the same gate: check it is from the clock actor, then check `now`
//    against `next_set` and store the new timestamps.
//
// A reply isn't a `Message`, so the gate uses `actor_entrypoint!(raw …)` and dispatches on `from`:
// only the clock actor can be the sender of a reply, so anything else is decoded as a `Message`.
// This is LEZ's `test_programs/guest/src/bin/cooldown.rs` pattern. Only an actor can read the clock:
// a transaction that sends `ReadTimestamp` straight to it fails.
//
// **Past timestamps** aren't kept on chain: read a block's header timestamp off chain with RPC
// `getBlock(id)`. A program can't check that one, so to use a past time later, store it in your own
// state while it's current, as this program does.
//
// A response can also carry `timestamp_window(..)`, which the engine checks against the current
// block's own timestamp. It only bounds when the transaction may land: the program never learns
// the time from it.
//
// Calls declare the clock actor: the clock account under the clock program
// (`clock_core::clock_actor()`). Anyone may set any account's timestamps here: the clock is the only
// gate.
//
// prog-tui builds it as the guest `jz_clock_gate` (key u, or b for every file here).

/// How long after a set the next one may come: 5 s, in the clock's milliseconds.
const COOLDOWN_MS: u64 = 5_000;

/// The messages a transaction sends the gate. The clock's reply is a `clock_core::Reply`.
#[derive(Clone, Copy, BorshSerialize, BorshDeserialize)]
enum Message {
    /// Set the timestamps to the clock's time. Refused before the next set is due.
    Set,
}

/// The actor's state. Empty until the first set.
#[derive(Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
struct Timestamps {
    /// When it was last set.
    last_set: u64,
    /// The earliest time the next set may come: `last_set + COOLDOWN_MS`.
    next_set: u64,
}

// The state layout, for the IDL: an alias named `State` or ending in `State` declares one.
/// The gate's state.
type GateState = Timestamps;

lee_core::actor_entrypoint!(raw handle_message);

fn handle_message(input: &ReceiveInput) -> Response {
    if input.from == Some(clock_actor()) {
        let Reply::Timestamp(now) = Reply::from_message(&input.message) else {
            panic!("the gate reads only the clock's timestamp");
        };
        return set(input, now);
    }
    let message: Message = borsh::from_slice(&input.message).expect("not a gate message");
    match message {
        Message::Set => Response::keep_state().send_sync(
            clock_actor(),
            &clock_core::Message::ReadTimestamp { granularity: 1 },
        ),
    }
}

/// The clock's answer: store `now` if the next set is due.
fn set(input: &ReceiveInput, now: u64) -> Response {
    if !input.pre_state.as_ref().is_empty() {
        let previous = GateState::try_from_slice(input.pre_state.as_ref()).expect("the state decodes");
        assert!(
            now >= previous.next_set,
            "too soon: {now} is before the next set, at {}",
            previous.next_set
        );
    }
    let timestamps = Timestamps {
        last_set: now,
        next_set: now.checked_add(COOLDOWN_MS).expect("now + 5s fits u64"),
    };
    Response::set_state(borsh::to_vec(&timestamps).expect("the state encodes"))
}

#[cfg(test)]
mod tests {
    use lee_core::{
        account::{AccountId, Actor, ActorState},
        program::SyncMessage,
    };

    use super::*;

    const PROGRAM: AccountId = AccountId::new([7; 32]);
    const ACCOUNT: AccountId = AccountId::new([1; 32]);
    const NOW: u64 = 1_700_000_000_000;

    fn input(from: Option<Actor>, pre: Option<&Timestamps>, message: Vec<u8>) -> ReceiveInput {
        ReceiveInput {
            receiver: Actor::new(ACCOUNT, PROGRAM),
            from,
            is_authorized: false,
            pre_state: ActorState::from(pre.map(|pre| borsh::to_vec(pre).unwrap()).unwrap_or_default()),
            message,
        }
    }

    /// The clock's reply of `now` to the gate.
    fn reply(pre: Option<&Timestamps>, now: u64) -> Response {
        handle_message(&input(
            Some(clock_actor()),
            pre,
            borsh::to_vec(&Reply::Timestamp(now)).unwrap(),
        ))
    }

    fn stored(response: &Response) -> Timestamps {
        borsh::from_slice(response.post_state.as_ref().unwrap().as_ref()).unwrap()
    }

    #[test]
    fn a_set_reads_the_clock() {
        let response = handle_message(&input(None, None, borsh::to_vec(&Message::Set).unwrap()));
        assert_eq!(response.post_state, None);
        assert_eq!(
            response.sync_messages,
            [SyncMessage::new(
                clock_actor(),
                &clock_core::Message::ReadTimestamp { granularity: 1 }
            )]
        );
    }

    #[test]
    fn the_first_reply_stores_now() {
        assert_eq!(
            stored(&reply(None, NOW)),
            Timestamps {
                last_set: NOW,
                next_set: NOW + 5_000
            }
        );
    }

    #[test]
    fn a_reply_at_the_next_set_time_is_allowed() {
        let previous = Timestamps {
            last_set: NOW,
            next_set: NOW + 5_000,
        };
        assert_eq!(stored(&reply(Some(&previous), NOW + 5_000)).next_set, NOW + 10_000);
    }

    #[test]
    #[should_panic(expected = "too soon")]
    fn a_reply_before_the_next_set_time_is_refused() {
        let previous = Timestamps {
            last_set: NOW,
            next_set: NOW + 5_000,
        };
        let _response = reply(Some(&previous), NOW + 4_999);
    }

    #[test]
    #[should_panic(expected = "not a gate message")]
    fn a_forged_reply_from_anyone_else_is_not_a_reply() {
        // A transaction (or another program) sending reply bytes is decoded as a `Message`, and
        // `Reply::Timestamp` (tag 1) isn't one.
        let _response = handle_message(&input(
            None,
            None,
            borsh::to_vec(&Reply::Timestamp(NOW)).unwrap(),
        ));
    }

    #[test]
    #[should_panic(expected = "reads only the clock's timestamp")]
    fn a_block_id_reply_is_refused() {
        let _response = handle_message(&input(
            Some(clock_actor()),
            None,
            borsh::to_vec(&Reply::BlockId(57)).unwrap(),
        ));
    }
}
