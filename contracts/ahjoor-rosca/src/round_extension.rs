//! Member-voted extension of the current round's contribution deadline.
//!
//! An `ExtendRound` proposal carries `extra_seconds` in `execution_data`. It
//! is bounded by the admin-set `MaxRoundExtensionSeconds` (0 disables the
//! feature), may only be applied while the current deadline has not passed,
//! and at most once per cycle. The stored round deadline itself is moved, so
//! contribution, late-contribution and penalty checks all see the extended
//! deadline (`finalize_round` / `close_round` copy it into
//! `LastRoundDeadline`, which the penalty grace logic reads).

use crate::errors::ExtError3;
use crate::obligations::current_deadline;
use crate::{events, streak, DataKey, DataKey6};
use soroban_sdk::{panic_with_error, Env};

pub(crate) fn max_extension(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey6::MaxRoundExtensionSeconds)
        .unwrap_or(0)
}

pub(crate) fn set_max_extension(env: &Env, max_extra_seconds: u64) {
    env.storage()
        .instance()
        .set(&DataKey6::MaxRoundExtensionSeconds, &max_extra_seconds);
}

pub(crate) fn last_extended_cycle(env: &Env) -> Option<u32> {
    env.storage().instance().get(&DataKey6::LastExtendedCycle)
}

/// Checks an `ExtendRound` request against the current state and returns
/// `extra_seconds`. Used both when the proposal is opened and when executed.
pub(crate) fn validate(env: &Env, execution_data: Option<i128>) -> u64 {
    let max = max_extension(env);
    if max == 0 {
        panic_with_error!(env, ExtError3::RoundExtensionDisabled);
    }
    let extra = execution_data.unwrap_or(0);
    if extra <= 0 || extra > max as i128 {
        panic_with_error!(env, ExtError3::InvalidRoundExtension);
    }
    if env.ledger().timestamp() > current_deadline(env) {
        panic_with_error!(env, ExtError3::RoundDeadlineAlreadyPassed);
    }
    if last_extended_cycle(env) == Some(streak::current_cycle(env)) {
        panic_with_error!(env, ExtError3::RoundAlreadyExtended);
    }
    extra as u64
}

/// Called from `execute_proposal` for an approved `ExtendRound` proposal.
pub(crate) fn execute(env: &Env, proposal_id: u32, execution_data: Option<i128>) {
    let extra = validate(env, execution_data);

    let round_deadline: u64 = env
        .storage()
        .instance()
        .get(&DataKey::RoundDeadline)
        .unwrap_or(0);
    if round_deadline > 0 {
        env.storage()
            .instance()
            .set(&DataKey::RoundDeadline, &round_deadline.saturating_add(extra));
    }

    let current_round: u32 = env
        .storage()
        .instance()
        .get(&DataKey::CurrentRound)
        .unwrap_or(0);
    let timestamp_deadline: u64 = env
        .storage()
        .instance()
        .get(&DataKey::RoundDeadlineTimestamp)
        .unwrap_or(0);
    if timestamp_deadline > 0 {
        let next = timestamp_deadline.saturating_add(extra);
        env.storage()
            .instance()
            .set(&DataKey::RoundDeadlineTimestamp, &next);
        events::emit_round_deadline_timestamp_set(env, current_round, next);
    }

    let cycle = streak::current_cycle(env);
    env.storage()
        .instance()
        .set(&DataKey6::LastExtendedCycle, &cycle);

    events::emit_round_deadline_extended(
        env,
        proposal_id,
        current_round,
        cycle,
        extra,
        current_deadline(env),
    );
}
