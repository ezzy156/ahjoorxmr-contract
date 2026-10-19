#![cfg(test)]
//! Coverage for group cloning (`clone_group`, `is_cloned_group`,
//! `get_clone_origin`) and concurrent-membership caps
//! (`set_membership_cap`, `get_membership_cap`, `get_membership_count`).
use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient as TokenAdminClient},
    Address, Env, Map, Vec,
};

/// Minimal source group exposing the read interface `clone_group` consumes
/// (`RoscaCloneInterface`) with fixed, non-default config values.
mod source {
    use crate::VotingMode;
    use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env};

    pub const CONTRIBUTION: i128 = 250;
    pub const ROUND_DURATION: u64 = 7_200;
    pub const FEE_BPS: u32 = 150;
    pub const MAX_DEFAULTS: u32 = 5;
    pub const GRACE_PERIOD_SECONDS: u64 = 600;
    pub const MAX_MEMBERS: u32 = 20;

    #[contract]
    pub struct SourceGroup;

    #[contractimpl]
    impl SourceGroup {
        pub fn set_token(env: Env, token: Address) {
            env.storage().instance().set(&symbol_short!("token"), &token);
        }
        pub fn get_token(env: Env) -> Address {
            env.storage()
                .instance()
                .get(&symbol_short!("token"))
                .unwrap()
        }
        pub fn get_contribution_amount(_env: Env) -> i128 {
            CONTRIBUTION
        }
        pub fn get_round_duration(_env: Env) -> u64 {
            ROUND_DURATION
        }
        pub fn get_use_timestamp_schedule(_env: Env) -> bool {
            false
        }
        pub fn get_round_duration_seconds(_env: Env) -> u64 {
            0
        }
        pub fn get_fee_bps(_env: Env) -> u32 {
            FEE_BPS
        }
        pub fn get_penalty_amount(_env: Env) -> i128 {
            10
        }
        pub fn get_exit_penalty_bps(_env: Env) -> u32 {
            200
        }
        pub fn get_max_defaults(_env: Env) -> u32 {
            MAX_DEFAULTS
        }
        pub fn get_grace_period_ledgers(_env: Env) -> u32 {
            0
        }
        pub fn get_grace_period_seconds(_env: Env) -> u64 {
            GRACE_PERIOD_SECONDS
        }
        pub fn get_skip_fee(_env: Env) -> i128 {
            0
        }
        pub fn get_max_skips_per_cycle(_env: Env) -> u32 {
            0
        }
        pub fn get_voting_mode(_env: Env) -> VotingMode {
            VotingMode::Equal
        }
        pub fn get_auction_enabled(_env: Env) -> bool {
            false
        }
        pub fn get_auction_window_ledgers(_env: Env) -> u64 {
            0
        }
        pub fn get_max_members_opt(_env: Env) -> Option<u32> {
            Some(MAX_MEMBERS)
        }
        pub fn get_reserve_enabled(_env: Env) -> bool {
            false
        }
        pub fn get_reserve_contribution_bps(_env: Env) -> u32 {
            0
        }
    }
}

fn no_overrides() -> GroupCloneOverrides {
    GroupCloneOverrides {
        contribution_amount: None,
        token: None,
        round_duration: None,
        fee_bps: None,
        fee_recipient: None,
        max_defaults: None,
        grace_period_ledgers: None,
        grace_period_seconds: None,
        penalty_amount: None,
        exit_penalty_bps: None,
        skip_fee: None,
        max_skips_per_cycle: None,
        use_timestamp_schedule: None,
        round_duration_seconds: None,
        voting_mode: None,
        auction_enabled: None,
        auction_window_ledgers: None,
        max_members: None,
        reserve_enabled: None,
        reserve_contribution_bps: None,
        strategy: None,
        custom_order: None,
        collective_goal: None,
        member_goals: None,
        late_fee_bps: None,
        randomize_payout_order: None,
    }
}

fn default_config() -> RoscaConfig {
    RoscaConfig {
        strategy: PayoutStrategy::RoundRobin,
        custom_order: None,
        penalty_amount: 0,
        exit_penalty_bps: 0,
        collective_goal: None,
        member_goals: None,
        fee_bps: 0,
        fee_recipient: None,
        max_defaults: 3,
        grace_period_ledgers: 0,
        grace_period_seconds: 0,
        use_timestamp_schedule: false,
        round_duration_seconds: 0,
        max_members: None,
        skip_fee: 0,
        max_skips_per_cycle: 0,
        voting_mode: VotingMode::Equal,
        late_fee_bps: 0,
        auction_enabled: false,
        auction_window_ledgers: 0,
        randomize_payout_order: false,
        reserve_enabled: false,
        reserve_contribution_bps: 0,
    }
}

fn generate_members(env: &Env, n: u32) -> Vec<Address> {
    let mut members = Vec::new(env);
    for _ in 0..n {
        members.push_back(Address::generate(env));
    }
    members
}

struct CloneSetup<'a> {
    env: Env,
    source_id: Address,
    source_members: Vec<Address>,
    token: TokenClient<'a>,
    token_admin: TokenAdminClient<'a>,
}

/// A source group with its own members and a non-zero token balance.
fn clone_setup<'a>() -> CloneSetup<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let token_issuer = Address::generate(&env);
    let token_addr = env
        .register_stellar_asset_contract_v2(token_issuer)
        .address();
    let token = TokenClient::new(&env, &token_addr);
    let token_admin = TokenAdminClient::new(&env, &token_addr);

    let source_id = env.register(source::SourceGroup, ());
    source::SourceGroupClient::new(&env, &source_id).set_token(&token_addr);
    token_admin.mint(&source_id, &5_000);

    let source_members = generate_members(&env, 4);

    CloneSetup {
        env,
        source_id,
        source_members,
        token,
        token_admin,
    }
}

// ===========================================================================
//  clone_group / is_cloned_group / get_clone_origin
// ===========================================================================

#[test]
fn test_clone_group_copies_source_configuration() {
    let s = clone_setup();
    let clone_id = s.env.register(AhjoorContract, ());
    let clone = AhjoorContractClient::new(&s.env, &clone_id);
    let admin = Address::generate(&s.env);
    let members = generate_members(&s.env, 3);

    clone.clone_group(&admin, &members, &s.source_id, &no_overrides(), &None);

    let info = clone.get_group_info();
    assert_eq!(info.contribution_amount, source::CONTRIBUTION);
    assert_eq!(info.token, s.token.address);
    assert_eq!(clone.get_token(), s.token.address);
    assert_eq!(clone.get_fee_bps(), source::FEE_BPS);
    assert_eq!(clone.get_max_defaults(), source::MAX_DEFAULTS);
    assert_eq!(clone.get_grace_period_seconds(), source::GRACE_PERIOD_SECONDS);
    assert_eq!(clone.get_max_members(), source::MAX_MEMBERS);
}

#[test]
fn test_clone_group_does_not_copy_members_or_balances() {
    let s = clone_setup();
    let clone_id = s.env.register(AhjoorContract, ());
    let clone = AhjoorContractClient::new(&s.env, &clone_id);
    let admin = Address::generate(&s.env);
    let members = generate_members(&s.env, 3);

    let source_balance_before = s.token.balance(&s.source_id);
    clone.clone_group(&admin, &members, &s.source_id, &no_overrides(), &None);

    // Only the caller-supplied members are in the clone.
    let info = clone.get_group_info();
    assert_eq!(info.members, members);
    for m in s.source_members.iter() {
        assert!(!info.members.contains(&m));
    }
    assert_eq!(info.current_round, 0);
    assert!(info.paid_members.is_empty());

    // No funds move: the clone starts empty, the source keeps its balance.
    assert_eq!(s.token.balance(&clone_id), 0);
    assert_eq!(s.token.balance(&s.source_id), source_balance_before);
    assert_eq!(clone.get_treasury_balance(), 0);

    // The clone is a working group in its own right.
    let m0 = members.get(0).unwrap();
    s.token_admin.mint(&m0, &1_000);
    clone.contribute(&m0, &s.token.address, &source::CONTRIBUTION);
    assert_eq!(s.token.balance(&clone_id), source::CONTRIBUTION);
    assert_eq!(s.token.balance(&s.source_id), source_balance_before);
}

#[test]
fn test_get_clone_origin_returns_source_id() {
    let s = clone_setup();
    s.env.ledger().set_timestamp(12_345);
    s.env.ledger().set_sequence_number(678);

    let clone_id = s.env.register(AhjoorContract, ());
    let clone = AhjoorContractClient::new(&s.env, &clone_id);
    let admin = Address::generate(&s.env);

    assert!(!clone.is_cloned_group());
    assert!(clone.get_clone_origin().is_none());

    clone.clone_group(
        &admin,
        &generate_members(&s.env, 2),
        &s.source_id,
        &no_overrides(),
        &None,
    );

    assert!(clone.is_cloned_group());
    let origin = clone.get_clone_origin().unwrap();
    assert_eq!(origin.source_contract, s.source_id);
    assert_eq!(origin.cloned_by, admin);
    assert_eq!(origin.cloned_at_timestamp, 12_345);
    assert_eq!(origin.cloned_at_ledger, 678);
}

#[test]
fn test_group_created_with_init_is_not_a_clone() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(AhjoorContract, ());
    let client = AhjoorContractClient::new(&env, &contract_id);
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    client.init(
        &Address::generate(&env),
        &generate_members(&env, 3),
        &100,
        &token,
        &3600,
        &default_config(),
        &None,
    );

    assert!(!client.is_cloned_group());
    assert!(client.get_clone_origin().is_none());
}

#[test]
fn test_clone_group_overrides_take_precedence() {
    let s = clone_setup();
    let clone_id = s.env.register(AhjoorContract, ());
    let clone = AhjoorContractClient::new(&s.env, &clone_id);

    let mut overrides = no_overrides();
    overrides.contribution_amount = Some(999);
    overrides.fee_bps = Some(25);
    overrides.max_members = Some(10);

    clone.clone_group(
        &Address::generate(&s.env),
        &generate_members(&s.env, 3),
        &s.source_id,
        &overrides,
        &None,
    );

    assert_eq!(clone.get_group_info().contribution_amount, 999);
    assert_eq!(clone.get_fee_bps(), 25);
    assert_eq!(clone.get_max_members(), 10);
    // Fields without an override still come from the source.
    assert_eq!(clone.get_max_defaults(), source::MAX_DEFAULTS);
}

#[test]
fn test_clone_group_from_self_rejected() {
    let s = clone_setup();
    let clone_id = s.env.register(AhjoorContract, ());
    let clone = AhjoorContractClient::new(&s.env, &clone_id);

    let res = clone.try_clone_group(
        &Address::generate(&s.env),
        &generate_members(&s.env, 2),
        &clone_id,
        &no_overrides(),
        &None,
    );
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError2::CloneSourceInvalid.into()
    );
    assert!(!clone.is_cloned_group());
}

#[test]
fn test_clone_group_into_initialized_group_rejected() {
    let s = clone_setup();
    let clone_id = s.env.register(AhjoorContract, ());
    let clone = AhjoorContractClient::new(&s.env, &clone_id);
    clone.clone_group(
        &Address::generate(&s.env),
        &generate_members(&s.env, 2),
        &s.source_id,
        &no_overrides(),
        &None,
    );

    let res = clone.try_clone_group(
        &Address::generate(&s.env),
        &generate_members(&s.env, 2),
        &s.source_id,
        &no_overrides(),
        &None,
    );
    assert_eq!(res.unwrap_err().unwrap(), Error::AlreadyInitialized.into());
}

// ===========================================================================
//  set_membership_cap / get_membership_cap / get_membership_count
// ===========================================================================

struct GroupSetup<'a> {
    env: Env,
    client: AhjoorContractClient<'a>,
    admin: Address,
    members: Vec<Address>,
}

fn group_setup<'a>() -> GroupSetup<'a> {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(AhjoorContract, ());
    let client = AhjoorContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let members = generate_members(&env, 3);
    client.init(
        &admin,
        &members,
        &100,
        &token,
        &3600,
        &default_config(),
        &None,
    );
    GroupSetup {
        env,
        client,
        admin,
        members,
    }
}

/// Simulates `member` already holding `count` active memberships in this
/// contract's tracked store (no public API sets the count directly).
fn seed_membership_count(s: &GroupSetup, member: &Address, count: u32) {
    s.env.as_contract(&s.client.address, || {
        let mut counts: Map<Address, u32> = s
            .env
            .storage()
            .persistent()
            .get(&DataKey5::MembershipCount)
            .unwrap_or(Map::new(&s.env));
        counts.set(member.clone(), count);
        s.env
            .storage()
            .persistent()
            .set(&DataKey5::MembershipCount, &counts);
    });
}

#[test]
fn test_membership_cap_defaults_to_unlimited_and_round_trips() {
    let s = group_setup();
    assert_eq!(s.client.get_membership_cap(), 0);

    s.client.set_membership_cap(&s.admin, &3);
    assert_eq!(s.client.get_membership_cap(), 3);

    s.client.set_membership_cap(&s.admin, &255);
    assert_eq!(s.client.get_membership_cap(), 255);

    s.client.set_membership_cap(&s.admin, &0);
    assert_eq!(s.client.get_membership_cap(), 0);
}

#[test]
fn test_membership_cap_above_255_rejected() {
    let s = group_setup();
    let res = s.client.try_set_membership_cap(&s.admin, &256);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError2::InvalidMembershipCap.into()
    );
    assert_eq!(s.client.get_membership_cap(), 0);
}

#[test]
fn test_membership_cap_requires_admin() {
    let s = group_setup();
    let outsider = Address::generate(&s.env);
    let res = s.client.try_set_membership_cap(&outsider, &2);
    assert!(res.is_err());
    assert_eq!(s.client.get_membership_cap(), 0);
}

#[test]
fn test_membership_count_set_for_initial_members() {
    let s = group_setup();
    for m in s.members.iter() {
        assert_eq!(s.client.get_membership_count(&m), 1);
    }
    assert_eq!(
        s.client.get_membership_count(&Address::generate(&s.env)),
        0
    );
}

#[test]
fn test_membership_count_updates_on_join_and_exit() {
    let s = group_setup();
    let newcomer = Address::generate(&s.env);
    assert_eq!(s.client.get_membership_count(&newcomer), 0);

    s.client.add_member(&newcomer);
    assert_eq!(s.client.get_membership_count(&newcomer), 1);

    s.client.remove_member(&newcomer);
    assert_eq!(s.client.get_membership_count(&newcomer), 0);

    // Rejoining after exit counts again.
    s.client.add_member(&newcomer);
    assert_eq!(s.client.get_membership_count(&newcomer), 1);
}

#[test]
fn test_membership_count_decrements_on_voluntary_exit() {
    let s = group_setup();
    let member = s.members.get(0).unwrap();
    assert_eq!(s.client.get_membership_count(&member), 1);

    s.env.ledger().set_sequence_number(100);
    s.client.set_exit_notice_ledgers(&s.admin, &10u32);
    s.client.request_voluntary_exit(&member);
    // Still a member during the notice window.
    assert_eq!(s.client.get_membership_count(&member), 1);

    s.env.ledger().set_sequence_number(110);
    s.client.finalize_voluntary_exit(&member);
    assert!(!s.client.get_group_info().members.contains(&member));
    assert_eq!(s.client.get_membership_count(&member), 0);
}

#[test]
fn test_joining_past_membership_cap_fails() {
    let s = group_setup();
    s.client.set_membership_cap(&s.admin, &1);

    let busy = Address::generate(&s.env);
    seed_membership_count(&s, &busy, 1);

    let res = s.client.try_add_member(&busy);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError2::MembershipCapReached.into()
    );
    assert!(!s.client.get_group_info().members.contains(&busy));
    assert_eq!(s.client.get_membership_count(&busy), 1);

    // An address below the cap can still join.
    let fresh = Address::generate(&s.env);
    s.client.add_member(&fresh);
    assert_eq!(s.client.get_membership_count(&fresh), 1);
}

#[test]
fn test_raising_membership_cap_allows_join() {
    let s = group_setup();
    let busy = Address::generate(&s.env);
    seed_membership_count(&s, &busy, 1);

    s.client.set_membership_cap(&s.admin, &1);
    assert!(s.client.try_add_member(&busy).is_err());

    s.client.set_membership_cap(&s.admin, &2);
    s.client.add_member(&busy);
    assert!(s.client.get_group_info().members.contains(&busy));
    assert_eq!(s.client.get_membership_count(&busy), 2);
}

#[test]
fn test_unlimited_cap_never_blocks_join() {
    let s = group_setup();
    let busy = Address::generate(&s.env);
    seed_membership_count(&s, &busy, 200);

    s.client.add_member(&busy);
    assert_eq!(s.client.get_membership_count(&busy), 201);
}
