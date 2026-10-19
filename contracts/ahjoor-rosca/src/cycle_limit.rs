//! Optional maximum cycle count. Once the group completes `max_cycles`
//! cycles it moves to `GroupStatus::Completed`: new contributions and round
//! advances are rejected, and the remaining insurance and emergency reserve
//! balances are paid back to members with the dissolution pro-rata logic.
//! Holdbacks, vesting and other per-member balances stay claimable.

use crate::errors::ExtError3;
use crate::{charter, events, internals, DataKey, DataKey2, DataKey3, DataKey5, GroupStatus};
use soroban_sdk::{panic_with_error, token, Address, Env, Vec};

pub(crate) fn max_cycles(env: &Env) -> Option<u32> {
    env.storage().instance().get(&DataKey5::MaxCycles)
}

/// Sets (or clears, with `None`) the cycle limit. Only allowed before the
/// group is activated.
pub(crate) fn set_max_cycles(env: &Env, max_cycles: Option<u32>) {
    if charter::is_group_activated(env) {
        panic_with_error!(env, ExtError3::ConfigLockedAfterActivation);
    }
    match max_cycles {
        Some(0) => panic_with_error!(env, ExtError3::InvalidMaxCycles),
        Some(n) => env.storage().instance().set(&DataKey5::MaxCycles, &n),
        None => env.storage().instance().remove(&DataKey5::MaxCycles),
    }
    events::emit_max_cycles_set(env, max_cycles);
}

pub(crate) fn is_completed(env: &Env) -> bool {
    let status: GroupStatus = env
        .storage()
        .instance()
        .get(&DataKey2::GroupStatus)
        .unwrap_or(GroupStatus::Active);
    status == GroupStatus::Completed
}

pub(crate) fn check_not_completed(env: &Env) {
    if is_completed(env) {
        panic_with_error!(env, ExtError3::GroupCompleted);
    }
}

/// True when finishing `completed_cycles` cycles reaches the configured limit.
pub(crate) fn limit_reached(env: &Env, completed_cycles: u32) -> bool {
    match max_cycles(env) {
        Some(limit) => completed_cycles >= limit,
        None => false,
    }
}

/// Marks the group completed and pays the insurance pool and emergency
/// reserve back to members pro rata.
pub(crate) fn complete_group(env: &Env, completed_cycles: u32) {
    env.storage()
        .instance()
        .set(&DataKey2::GroupStatus, &GroupStatus::Completed);

    let insurance_pool: i128 = env
        .storage()
        .instance()
        .get(&DataKey2::InsurancePool)
        .unwrap_or(0);
    let reserve: i128 = env
        .storage()
        .persistent()
        .get(&DataKey3::EmergencyReserveBalance)
        .unwrap_or(0);
    if insurance_pool != 0 {
        env.storage()
            .instance()
            .set(&DataKey2::InsurancePool, &0i128);
    }
    if reserve != 0 {
        env.storage()
            .persistent()
            .set(&DataKey3::EmergencyReserveBalance, &0i128);
    }

    let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
    let balance = token::Client::new(env, &token_addr).balance(&env.current_contract_address());
    let distributable = (insurance_pool.max(0) + reserve.max(0)).min(balance);
    if distributable > 0 {
        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .unwrap_or(Vec::new(env));
        internals::distribute_pro_rata(env, &members, distributable);
    }

    events::emit_group_completed(env, completed_cycles, distributable.max(0));
}
