//! Cross-group member obligations.
//!
//! Each contract instance manages a single group, so a member's view across
//! groups is built from a per-address index of other group contracts kept on
//! this contract (`DataKey6::MemberGroupIndex`). `get_member_obligations`
//! reports this group's obligation plus one per linked group, queried through
//! `RoscaObligationClient`. Groups the member has exited, groups that are no
//! longer active (dissolved / merged / split) and rounds already fully paid
//! are left out.

use crate::errors::ExtError3;
use crate::{
    events, DataKey, DataKey2, DataKey4, DataKey6, GroupStatus, MemberObligation,
    PERSISTENT_BUMP_AMOUNT, PERSISTENT_LIFETIME_THRESHOLD,
};
use soroban_sdk::{contractclient, panic_with_error, Address, Env, Map, Vec};

/// Maximum number of other groups a member may link to their index.
pub(crate) const MAX_LINKED_GROUPS: u32 = 20;

/// Read-only interface other ROSCA group contracts expose for the index.
#[contractclient(name = "RoscaObligationClient")]
pub trait RoscaObligationInterface {
    fn get_group_obligation(env: Env, member: Address) -> Option<MemberObligation>;
    fn is_active_group_member(env: Env, member: Address) -> bool;
}

/// `true` when the group is active and `member` belongs to it and has not
/// exited or been suspended.
pub(crate) fn is_active_member(env: &Env, member: &Address) -> bool {
    let status: GroupStatus = env
        .storage()
        .instance()
        .get(&DataKey2::GroupStatus)
        .unwrap_or(GroupStatus::Active);
    if status != GroupStatus::Active {
        return false;
    }
    let members: Vec<Address> = match env.storage().instance().get(&DataKey::Members) {
        Some(m) => m,
        None => return false,
    };
    if !members.contains(member) {
        return false;
    }
    let exited: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::ExitedMembers)
        .unwrap_or(Vec::new(env));
    if exited.contains(member) {
        return false;
    }
    let suspended: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::SuspendedMembers)
        .unwrap_or(Vec::new(env));
    !suspended.contains(member)
}

/// Current round contribution deadline (timestamp or ledger-mode deadline).
pub(crate) fn current_deadline(env: &Env) -> u64 {
    let use_timestamp: bool = env
        .storage()
        .instance()
        .get(&DataKey2::UseTimestampSchedule)
        .unwrap_or(false);
    if use_timestamp {
        env.storage()
            .instance()
            .get(&DataKey::RoundDeadlineTimestamp)
            .unwrap_or(0)
    } else {
        env.storage()
            .instance()
            .get(&DataKey::RoundDeadline)
            .unwrap_or(0)
    }
}

/// What `member` still owes this group for the current round, if anything.
pub(crate) fn local_obligation(env: &Env, member: &Address) -> Option<MemberObligation> {
    if !is_active_member(env, member) {
        return None;
    }

    let paid_members: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::PaidMembers)
        .unwrap_or(Vec::new(env));
    if paid_members.contains(member) {
        return None;
    }

    let current_round: u32 = env
        .storage()
        .instance()
        .get(&DataKey::CurrentRound)
        .unwrap_or(0);
    let skip_requests: Map<(Address, u32), bool> = env
        .storage()
        .instance()
        .get(&DataKey2::SkipRequests)
        .unwrap_or(Map::new(env));
    if skip_requests
        .get((member.clone(), current_round))
        .unwrap_or(false)
    {
        return None;
    }

    let base_amount: i128 = env
        .storage()
        .instance()
        .get(&DataKey::ContributionAmt)
        .unwrap_or(0);
    let tiers: Map<Address, u32> = env
        .storage()
        .instance()
        .get(&DataKey2::MemberTiers)
        .unwrap_or(Map::new(env));
    let tier_bps = tiers.get(member.clone()).unwrap_or(10_000);
    let required = (base_amount * tier_bps as i128) / 10_000;

    let contributions: Map<Address, i128> = env
        .storage()
        .instance()
        .get(&DataKey::MemberContributions)
        .unwrap_or(Map::new(env));
    let contributed = contributions.get(member.clone()).unwrap_or(0);
    let prepaid: Map<Address, i128> = env
        .storage()
        .instance()
        .get(&DataKey4::PrepaidBalances)
        .unwrap_or(Map::new(env));
    let prepaid_balance = prepaid.get(member.clone()).unwrap_or(0);

    let amount_due = required - contributed - prepaid_balance;
    if amount_due <= 0 {
        return None;
    }

    let token: Address = env.storage().instance().get(&DataKey::Token)?;
    Some(MemberObligation {
        group_id: env.current_contract_address(),
        token,
        amount_due,
        deadline: current_deadline(env),
    })
}

pub(crate) fn linked_groups(env: &Env, member: &Address) -> Vec<Address> {
    env.storage()
        .persistent()
        .get(&DataKey6::MemberGroupIndex(member.clone()))
        .unwrap_or(Vec::new(env))
}

fn save_linked_groups(env: &Env, member: &Address, groups: &Vec<Address>) {
    let key = DataKey6::MemberGroupIndex(member.clone());
    if groups.is_empty() {
        env.storage().persistent().remove(&key);
        return;
    }
    env.storage().persistent().set(&key, groups);
    env.storage()
        .persistent()
        .extend_ttl(&key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
}

/// Adds `group` to `member`'s index after confirming the member is active
/// there. Linking an already-linked group is a no-op.
pub(crate) fn link_group(env: &Env, member: &Address, group: &Address) {
    if *group == env.current_contract_address() {
        panic_with_error!(env, ExtError3::InvalidLinkedGroup);
    }

    let mut groups = linked_groups(env, member);
    if groups.contains(group) {
        return;
    }
    if groups.len() >= MAX_LINKED_GROUPS {
        panic_with_error!(env, ExtError3::LinkedGroupLimitReached);
    }

    let client = RoscaObligationClient::new(env, group);
    let active = matches!(client.try_is_active_group_member(member), Ok(Ok(true)));
    if !active {
        panic_with_error!(env, ExtError3::NotActiveInLinkedGroup);
    }

    groups.push_back(group.clone());
    save_linked_groups(env, member, &groups);
    events::emit_member_group_linked(env, member.clone(), group.clone());
}

pub(crate) fn unlink_group(env: &Env, member: &Address, group: &Address) {
    let groups = linked_groups(env, member);
    let mut kept: Vec<Address> = Vec::new(env);
    let mut removed = false;
    for g in groups.iter() {
        if g == *group {
            removed = true;
        } else {
            kept.push_back(g);
        }
    }
    if removed {
        save_linked_groups(env, member, &kept);
        events::emit_member_group_unlinked(env, member.clone(), group.clone());
    }
}

/// This group's obligation followed by one per linked group that still has
/// an outstanding contribution. Groups that cannot be queried are skipped.
pub(crate) fn member_obligations(env: &Env, member: &Address) -> Vec<MemberObligation> {
    let mut out: Vec<MemberObligation> = Vec::new(env);
    if let Some(own) = local_obligation(env, member) {
        out.push_back(own);
    }
    for group in linked_groups(env, member).iter() {
        let client = RoscaObligationClient::new(env, &group);
        if let Ok(Ok(Some(obligation))) = client.try_get_group_obligation(member) {
            out.push_back(obligation);
        }
    }
    out
}
