//! Membership succession for inactive members.
//!
//! A member designates a successor, the successor accepts, and once the member
//! has missed `succession_trigger_rounds` consecutive contributions the
//! successor can claim the slot. The successor takes over the member's
//! position in the member list and payout order (and therefore any payout
//! still owed to that slot), per-member contribution state, and outstanding
//! catch-up debt. Contribution history recorded under the original member is
//! linked through `DataKey5::SucceededFrom`.

use crate::errors::ExtError2;
use crate::{events, DataKey, DataKey2, DataKey3, DataKey4, DataKey5, SuccessorDesignation};
use soroban_sdk::{panic_with_error, Address, Env, IntoVal, Map, TryFromVal, Val, Vec};

const PERSISTENT_LIFETIME_THRESHOLD: u32 = 100_000;
const PERSISTENT_BUMP_AMOUNT: u32 = 120_000;

/// Default number of consecutive missed contributions before a successor may claim.
pub(crate) const DEFAULT_SUCCESSION_TRIGGER_ROUNDS: u32 = 3;

/// Upper bound on the predecessor chain walked when merging contribution history.
pub(crate) const MAX_SUCCESSION_CHAIN: u32 = 10;

pub(crate) fn trigger_rounds(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey5::SuccessionTriggerRounds)
        .unwrap_or(DEFAULT_SUCCESSION_TRIGGER_ROUNDS)
}

pub(crate) fn consecutive_misses(env: &Env, member: &Address) -> u32 {
    let misses: Map<Address, u32> = env
        .storage()
        .instance()
        .get(&DataKey5::ConsecutiveMisses)
        .unwrap_or(Map::new(env));
    misses.get(member.clone()).unwrap_or(0)
}

/// Called when a round is closed or finalized: members in `defaulters` get
/// their consecutive-miss counter incremented, everyone else is reset.
pub(crate) fn track_consecutive_misses(env: &Env, members: &Vec<Address>, defaulters: &Vec<Address>) {
    let mut misses: Map<Address, u32> = env
        .storage()
        .instance()
        .get(&DataKey5::ConsecutiveMisses)
        .unwrap_or(Map::new(env));
    for member in members.iter() {
        if defaulters.contains(&member) {
            let count = misses.get(member.clone()).unwrap_or(0).saturating_add(1);
            misses.set(member, count);
        } else if misses.contains_key(member.clone()) {
            misses.remove(member);
        }
    }
    env.storage()
        .instance()
        .set(&DataKey5::ConsecutiveMisses, &misses);
}

pub(crate) fn get_designation(env: &Env, member: &Address) -> Option<SuccessorDesignation> {
    env.storage()
        .persistent()
        .get(&DataKey5::Successor(member.clone()))
}

fn save_designation(env: &Env, designation: &SuccessorDesignation) {
    let key = DataKey5::Successor(designation.member.clone());
    env.storage().persistent().set(&key, designation);
    env.storage()
        .persistent()
        .extend_ttl(&key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
}

fn members(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&DataKey::Members)
        .expect("Not initialized")
}

fn require_valid_successor(env: &Env, member: &Address, successor: &Address) {
    if successor == member || members(env).contains(successor) {
        panic_with_error!(env, ExtError2::InvalidSuccessor);
    }
    crate::bans::require_not_banned(env, successor);
}

pub(crate) fn designate(env: &Env, member: &Address, successor: &Address) {
    if !members(env).contains(member) {
        panic_with_error!(env, crate::errors::Error::NotAMember);
    }
    require_valid_successor(env, member, successor);
    let designation = SuccessorDesignation {
        member: member.clone(),
        successor: successor.clone(),
        accepted: false,
        designated_at_ledger: env.ledger().sequence(),
    };
    save_designation(env, &designation);
    events::emit_successor_designated(env, member.clone(), successor.clone());
}

pub(crate) fn accept(env: &Env, successor: &Address, member: &Address) {
    let mut designation = match get_designation(env, member) {
        Some(d) if d.successor == *successor => d,
        _ => panic_with_error!(env, ExtError2::SuccessorNotDesignated),
    };
    if designation.accepted {
        panic_with_error!(env, ExtError2::SuccessionAlreadyAccepted);
    }
    require_valid_successor(env, member, successor);
    designation.accepted = true;
    save_designation(env, &designation);
    events::emit_successor_accepted(env, member.clone(), successor.clone());
}

/// Checks the successor against this group's concurrent-membership cap and
/// moves one active-membership count from `member` to `successor`.
fn transfer_membership_count(env: &Env, member: &Address, successor: &Address) {
    let cap: u32 = env
        .storage()
        .instance()
        .get(&DataKey5::MaxConcurrentMemberships)
        .unwrap_or(0);
    let mut counts: Map<Address, u32> = env
        .storage()
        .persistent()
        .get(&DataKey5::MembershipCount)
        .unwrap_or(Map::new(env));
    let successor_count = counts.get(successor.clone()).unwrap_or(0);
    if cap > 0 && successor_count >= cap {
        panic_with_error!(env, ExtError2::MembershipCapReached);
    }
    let member_count = counts.get(member.clone()).unwrap_or(0);
    if member_count > 0 {
        counts.set(member.clone(), member_count - 1);
        counts.set(successor.clone(), successor_count + 1);
        env.storage()
            .persistent()
            .set(&DataKey5::MembershipCount, &counts);
    }
}

/// Replaces `from` with `to` in the `Vec<Address>` stored under `key`.
fn replace_in_vec<K: IntoVal<Env, Val>>(env: &Env, key: &K, from: &Address, to: &Address) {
    let list: Option<Vec<Address>> = env.storage().instance().get(key);
    if let Some(list) = list {
        if !list.contains(from) {
            return;
        }
        let mut updated: Vec<Address> = Vec::new(env);
        for addr in list.iter() {
            if addr == *from {
                updated.push_back(to.clone());
            } else {
                updated.push_back(addr);
            }
        }
        env.storage().instance().set(key, &updated);
    }
}

/// Removes `addr` from the `Vec<Address>` stored under `key`.
fn remove_from_vec<K: IntoVal<Env, Val>>(env: &Env, key: &K, addr: &Address) {
    let list: Option<Vec<Address>> = env.storage().instance().get(key);
    if let Some(list) = list {
        if !list.contains(addr) {
            return;
        }
        let mut updated: Vec<Address> = Vec::new(env);
        for a in list.iter() {
            if a != *addr {
                updated.push_back(a);
            }
        }
        env.storage().instance().set(key, &updated);
    }
}

/// Moves the entry for `from` to `to` in the `Map<Address, V>` stored under `key`.
fn move_map_entry<K, V>(env: &Env, key: &K, from: &Address, to: &Address)
where
    K: IntoVal<Env, Val>,
    V: IntoVal<Env, Val> + TryFromVal<Env, Val>,
{
    let map: Option<Map<Address, V>> = env.storage().instance().get(key);
    if let Some(mut map) = map {
        if let Some(value) = map.get(from.clone()) {
            map.remove(from.clone());
            map.set(to.clone(), value);
            env.storage().instance().set(key, &map);
        }
    }
}

/// Removes the entry for `addr` from the `Map<Address, V>` stored under `key`.
fn remove_map_entry<K, V>(env: &Env, key: &K, addr: &Address)
where
    K: IntoVal<Env, Val>,
    V: IntoVal<Env, Val> + TryFromVal<Env, Val>,
{
    let map: Option<Map<Address, V>> = env.storage().instance().get(key);
    if let Some(mut map) = map {
        if map.contains_key(addr.clone()) {
            map.remove(addr.clone());
            env.storage().instance().set(key, &map);
        }
    }
}

pub(crate) fn claim(env: &Env, successor: &Address, member: &Address) {
    let designation = match get_designation(env, member) {
        Some(d) if d.successor == *successor => d,
        _ => panic_with_error!(env, ExtError2::SuccessorNotDesignated),
    };
    if !designation.accepted {
        panic_with_error!(env, ExtError2::SuccessionNotAccepted);
    }
    if !members(env).contains(member) {
        panic_with_error!(env, crate::errors::Error::NotAMember);
    }
    require_valid_successor(env, member, successor);

    let missed = consecutive_misses(env, member);
    if missed < trigger_rounds(env) {
        panic_with_error!(env, ExtError2::SuccessionThresholdNotMet);
    }

    // Successor joins the group, so the usual join gates apply.
    crate::charter::require_charter_acknowledged(env, successor);
    transfer_membership_count(env, member, successor);

    // Slot: same position in the member list and payout order.
    replace_in_vec(env, &DataKey::Members, member, successor);
    replace_in_vec(env, &DataKey::PayoutOrder, member, successor);
    replace_in_vec(env, &DataKey::PaidMembers, member, successor);

    // Per-member contribution state and reward accounting.
    move_map_entry::<_, i128>(env, &DataKey::MemberContributions, member, successor);
    move_map_entry::<_, u32>(env, &DataKey::MemberParticipation, member, successor);
    move_map_entry::<_, i128>(env, &DataKey::MemberCollected, member, successor);
    move_map_entry::<_, i128>(env, &DataKey::MemberGoals, member, successor);
    move_map_entry::<_, i128>(env, &DataKey::ClaimedRewards, member, successor);
    move_map_entry::<_, u32>(env, &DataKey::RewardWeights, member, successor);
    move_map_entry::<_, u32>(env, &DataKey2::MemberTiers, member, successor);
    move_map_entry::<_, u32>(env, &DataKey3::MemberTierIndex, member, successor);
    move_map_entry::<_, i128>(env, &DataKey4::PrepaidBalances, member, successor);

    // Outstanding catch-up debt transfers with the slot.
    let mut debts: Map<Address, i128> = env
        .storage()
        .instance()
        .get(&DataKey2::CatchUpDebt)
        .unwrap_or(Map::new(env));
    let debt = debts.get(member.clone()).unwrap_or(0);
    if debt > 0 {
        debts.remove(member.clone());
        let existing = debts.get(successor.clone()).unwrap_or(0);
        debts.set(successor.clone(), existing + debt);
        env.storage().instance().set(&DataKey2::CatchUpDebt, &debts);
    }

    // The successor takes over instead of the member being penalised, so the
    // inactive member's default / penalty state does not carry over.
    remove_from_vec(env, &DataKey::Defaulters, member);
    remove_from_vec(env, &DataKey::SuspendedMembers, member);
    remove_map_entry::<_, u32>(env, &DataKey::DefaultCount, member);
    remove_map_entry::<_, u32>(env, &DataKey4::PendingPenalties, member);
    remove_map_entry::<_, u32>(env, &DataKey3::LateContributionCount, member);
    remove_map_entry::<_, u32>(env, &DataKey5::ConsecutiveMisses, member);

    env.storage()
        .persistent()
        .remove(&DataKey5::Successor(member.clone()));
    let link_key = DataKey5::SucceededFrom(successor.clone());
    env.storage().persistent().set(&link_key, member);
    env.storage().persistent().extend_ttl(
        &link_key,
        PERSISTENT_LIFETIME_THRESHOLD,
        PERSISTENT_BUMP_AMOUNT,
    );

    events::emit_succession_claimed(env, member.clone(), successor.clone(), missed, debt);
}

pub(crate) fn predecessor(env: &Env, successor: &Address) -> Option<Address> {
    env.storage()
        .persistent()
        .get(&DataKey5::SucceededFrom(successor.clone()))
}
