//! Late-fee waiver: a `WaivePenalty` proposal targets `target_member` with the
//! penalty round in `execution_data`.
//!
//! On approval a pending penalty (queued in `PendingPenalties` during the
//! grace window) is cleared before it is ever charged. A penalty that was
//! already collected is refunded from the base-token custody it was paid
//! into (the round pot, net of reward / insurance / reserved balances), and
//! the default count and reputation change it caused are reversed, so the
//! member's score is not reduced for a waived penalty.

use crate::errors::ExtError3;
use crate::{
    events, streak, vesting, AhjoorContract, CollectedPenaltyRecord, DataKey, DataKey2,
    DataKey4, DataKey6, PERSISTENT_BUMP_AMOUNT, PERSISTENT_LIFETIME_THRESHOLD,
};
use soroban_sdk::{panic_with_error, token, Address, Env, Map, Vec};

pub(crate) fn get_collected(env: &Env, member: &Address, round: u32) -> Option<CollectedPenaltyRecord> {
    env.storage()
        .persistent()
        .get(&DataKey6::CollectedPenalty(member.clone(), round))
}

fn save_collected(env: &Env, record: &CollectedPenaltyRecord) {
    let key = DataKey6::CollectedPenalty(record.member.clone(), record.round);
    env.storage().persistent().set(&key, record);
    env.storage()
        .persistent()
        .extend_ttl(&key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
}

/// Called by `apply_penalty` each time a penalty is charged.
pub(crate) fn record_collected(
    env: &Env,
    member: &Address,
    round: u32,
    token: &Address,
    amount: i128,
    reputation_delta: i128,
) {
    let record = match get_collected(env, member, round) {
        Some(mut existing) if !existing.refunded => {
            existing.amount += amount;
            existing.reputation_delta += reputation_delta;
            existing.applications += 1;
            existing
        }
        _ => CollectedPenaltyRecord {
            member: member.clone(),
            round,
            token: token.clone(),
            amount,
            reputation_delta,
            applications: 1,
            refunded: false,
        },
    };
    save_collected(env, &record);
}

fn pending_round(env: &Env, member: &Address) -> Option<u32> {
    let pending: Map<Address, u32> = env
        .storage()
        .instance()
        .get(&DataKey4::PendingPenalties)
        .unwrap_or(Map::new(env));
    pending.get(member.clone())
}

fn parse_round(env: &Env, execution_data: Option<i128>) -> u32 {
    match execution_data {
        Some(r) if r >= 0 && r <= u32::MAX as i128 => r as u32,
        _ => panic_with_error!(env, ExtError3::PenaltyNotFound),
    }
}

/// Panics with `PenaltyNotFound` unless `member` has a pending penalty for
/// `round` or a collected one that has not been refunded. Returns the round.
pub(crate) fn require_penalty_exists(env: &Env, member: &Address, execution_data: Option<i128>) -> u32 {
    let round = parse_round(env, execution_data);
    if pending_round(env, member) == Some(round) {
        return round;
    }
    match get_collected(env, member, round) {
        Some(record) if !record.refunded => round,
        _ => panic_with_error!(env, ExtError3::PenaltyNotFound),
    }
}

/// Base-token balance available for refunds: the same custody the penalty
/// was paid into, less funds earmarked for other pools.
fn free_balance(env: &Env, token_addr: &Address) -> i128 {
    let balance = token::Client::new(env, token_addr).balance(&env.current_contract_address());
    let base_token: Address = env.storage().instance().get(&DataKey::Token).unwrap();
    if *token_addr != base_token {
        return balance;
    }
    let reward_pool: i128 = env
        .storage()
        .instance()
        .get(&DataKey::RewardPool)
        .unwrap_or(0);
    let insurance_pool: i128 = env
        .storage()
        .instance()
        .get(&DataKey2::InsurancePool)
        .unwrap_or(0);
    balance
        - reward_pool
        - insurance_pool
        - vesting::locked_balance(env)
        - streak::reserved_balance(env)
}

/// Called from `execute_proposal` for an approved `WaivePenalty` proposal.
pub(crate) fn execute(env: &Env, proposal_id: u32, member: &Address, execution_data: Option<i128>) {
    let round = require_penalty_exists(env, member, execution_data);

    if pending_round(env, member) == Some(round) {
        let mut pending: Map<Address, u32> = env
            .storage()
            .instance()
            .get(&DataKey4::PendingPenalties)
            .unwrap_or(Map::new(env));
        pending.remove(member.clone());
        env.storage()
            .instance()
            .set(&DataKey4::PendingPenalties, &pending);

        // Drop the member from the defaulters list so the waived penalty
        // cannot be charged again through `penalise_defaulter`.
        let defaulters: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Defaulters)
            .unwrap_or(Vec::new(env));
        let mut kept: Vec<Address> = Vec::new(env);
        for d in defaulters.iter() {
            if d != *member {
                kept.push_back(d);
            }
        }
        env.storage().instance().set(&DataKey::Defaulters, &kept);

        events::emit_penalty_waived(env, proposal_id, member.clone(), round, false, 0);
        return;
    }

    let mut record = get_collected(env, member, round).unwrap();
    if free_balance(env, &record.token) < record.amount {
        panic_with_error!(env, ExtError3::PenaltyRefundUnavailable);
    }
    token::Client::new(env, &record.token).transfer(
        &env.current_contract_address(),
        member,
        &record.amount,
    );

    let mut default_count: Map<Address, u32> = env
        .storage()
        .instance()
        .get(&DataKey::DefaultCount)
        .unwrap_or(Map::new(env));
    let count = default_count.get(member.clone()).unwrap_or(0);
    default_count.set(member.clone(), count.saturating_sub(record.applications));
    env.storage()
        .instance()
        .set(&DataKey::DefaultCount, &default_count);

    if record.reputation_delta != 0 {
        AhjoorContract::apply_reputation_delta(
            env,
            member.clone(),
            -record.reputation_delta,
            "penalty_waived",
        );
    }

    record.refunded = true;
    save_collected(env, &record);

    events::emit_penalty_waived(env, proposal_id, member.clone(), round, true, record.amount);
}
