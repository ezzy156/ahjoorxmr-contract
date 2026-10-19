//! Per-group ban list. A banned address cannot join or rejoin the group by
//! any path: admin `add_member`, invites, the waitlist, successor
//! designation, migration or merge. Active members must be removed before
//! they can be banned.

use crate::errors::ExtError3;
use crate::{
    events, DataKey, DataKey2, DataKey5, PERSISTENT_BUMP_AMOUNT, PERSISTENT_LIFETIME_THRESHOLD,
};
use soroban_sdk::{panic_with_error, Address, BytesN, Env, Symbol, Vec};

pub(crate) fn is_banned(env: &Env, addr: &Address) -> bool {
    env.storage()
        .persistent()
        .has(&DataKey5::Banned(addr.clone()))
}

pub(crate) fn require_not_banned(env: &Env, addr: &Address) {
    if is_banned(env, addr) {
        panic_with_error!(env, ExtError3::MemberBanned);
    }
}

/// An address on the member list that has not exited.
fn is_active_member(env: &Env, addr: &Address) -> bool {
    let members: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::Members)
        .unwrap_or(Vec::new(env));
    let exited: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::ExitedMembers)
        .unwrap_or(Vec::new(env));
    members.contains(addr) && !exited.contains(addr)
}

pub(crate) fn ban(env: &Env, member: &Address, reason_hash: &BytesN<32>, banned_by: &Address) {
    if is_active_member(env, member) {
        panic_with_error!(env, ExtError3::MemberStillActive);
    }
    if is_banned(env, member) {
        panic_with_error!(env, ExtError3::AlreadyBanned);
    }
    let key = DataKey5::Banned(member.clone());
    env.storage().persistent().set(&key, reason_hash);
    env.storage().persistent().extend_ttl(
        &key,
        PERSISTENT_LIFETIME_THRESHOLD,
        PERSISTENT_BUMP_AMOUNT,
    );

    // Drop any pending ways back in.
    let waitlist: Vec<(Address, u64)> = env
        .storage()
        .instance()
        .get(&DataKey2::Waitlist)
        .unwrap_or(Vec::new(env));
    let mut new_waitlist: Vec<(Address, u64)> = Vec::new(env);
    for entry in waitlist.iter() {
        if entry.0 != *member {
            new_waitlist.push_back(entry);
        }
    }
    if new_waitlist.len() != waitlist.len() {
        env.storage()
            .instance()
            .set(&DataKey2::Waitlist, &new_waitlist);
        events::emit_waitlist_updated(env, member.clone(), false, new_waitlist.len());
    }

    let invites_key = Symbol::new(env, "approved_invitees");
    let invitees: Option<Vec<Address>> = env.storage().temporary().get(&invites_key);
    if let Some(invitees) = invitees {
        if invitees.contains(member) {
            let mut remaining: Vec<Address> = Vec::new(env);
            for addr in invitees.iter() {
                if addr != *member {
                    remaining.push_back(addr);
                }
            }
            env.storage().temporary().set(&invites_key, &remaining);
        }
    }

    events::emit_member_banned(env, member.clone(), banned_by.clone(), reason_hash.clone());
}

pub(crate) fn unban(env: &Env, member: &Address, unbanned_by: &Address) {
    if !is_banned(env, member) {
        panic_with_error!(env, ExtError3::NotBanned);
    }
    env.storage()
        .persistent()
        .remove(&DataKey5::Banned(member.clone()));
    events::emit_member_unbanned(env, member.clone(), unbanned_by.clone());
}
