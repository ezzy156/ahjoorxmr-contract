#![cfg(test)]
use super::*;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, Vec,
};

fn setup_group<'a>(
    exit_penalty_bps: u32,
) -> (
    Env,
    AhjoorContractClient<'a>,
    Address,
    Vec<Address>,
    Address,
    TokenAdminClient<'a>,
) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(AhjoorContract, ());
    let client = AhjoorContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let mut members = Vec::new(&env);
    for _ in 0..3 {
        members.push_back(Address::generate(&env));
    }
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let tac = TokenAdminClient::new(&env, &token_addr);
    for i in 0..members.len() {
        tac.mint(&members.get(i).unwrap(), &10_000);
    }
    client.init(
        &admin,
        &members,
        &100i128,
        &token_addr,
        &3600u64,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 3,
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,
            late_fee_bps: 0,
            grace_period_seconds: 0,
            auction_enabled: false,
            auction_window_ledgers: 0,
            randomize_payout_order: false,
            reserve_enabled: false,
            reserve_contribution_bps: 0,
        },
        &None,
    );
    (env, client, admin, members, token_addr, tac)
}

#[test]
fn test_voluntary_exit_notice_period_flow() {
    let (env, client, admin, members, _token, _tac) = setup_group(0);
    let member = members.get(0).unwrap();

    env.ledger().set_sequence_number(100);
    client.set_exit_notice_ledgers(&admin, &20u32);
    assert_eq!(client.get_exit_notice_ledgers(), 20);

    client.request_voluntary_exit(&member);

    // Still a member during notice window
    let info = client.get_group_info();
    assert!(info.members.contains(&member));
    let req = client.get_voluntary_exit_request(&member).unwrap();
    assert_eq!(req.effective_ledger, 120);

    // Too early to finalize
    let early = client.try_finalize_voluntary_exit(&member);
    assert_eq!(
        early.unwrap_err().unwrap(),
        ExtError2::ExitNoticeNotElapsed.into()
    );

    // Advance to effective ledger and finalize (permissionless)
    env.ledger().set_sequence_number(120);
    client.finalize_voluntary_exit(&member);

    let exited = client.get_exited_members(&0u32, &10u32);
    assert!(exited.contains(&member));
    assert!(client.get_voluntary_exit_request(&member).is_none());
}

#[test]
fn test_cancel_voluntary_exit() {
    let (env, client, admin, members, _token, _tac) = setup_group(0);
    let member = members.get(1).unwrap();

    env.ledger().set_sequence_number(50);
    client.set_exit_notice_ledgers(&admin, &30u32);
    client.request_voluntary_exit(&member);
    assert!(client.get_voluntary_exit_request(&member).is_some());

    client.cancel_voluntary_exit(&member);
    assert!(client.get_voluntary_exit_request(&member).is_none());

    // Member remains active
    let info = client.get_group_info();
    assert!(info.members.contains(&member));
}

#[test]
fn test_zero_notice_immediate_exit() {
    let (env, client, admin, members, _token, _tac) = setup_group(0);
    let member = members.get(2).unwrap();

    env.ledger().set_sequence_number(10);
    client.set_exit_notice_ledgers(&admin, &0u32);
    client.request_voluntary_exit(&member);

    // Immediate removal — no pending request
    assert!(client.get_voluntary_exit_request(&member).is_none());
    assert!(client.get_exited_members(&0u32, &10u32).contains(&member));
}

#[test]
fn test_member_liable_during_notice_window() {
    let (env, client, admin, members, token, _tac) = setup_group(0);
    let member = members.get(0).unwrap();

    env.ledger().set_sequence_number(100);
    client.set_exit_notice_ledgers(&admin, &50u32);
    client.request_voluntary_exit(&member);

    // Still able to contribute during notice
    client.contribute(&member, &token, &100i128);
    let (paid, remaining) = client.get_member_contribution_status(&member);
    assert_eq!(paid, 100);
    assert_eq!(remaining, 0);
}

fn seed_exited_members(env: &Env, client: &AhjoorContractClient<'_>) -> Vec<Address> {
    let mut seeded = Vec::new(env);
    for _ in 0..7 {
        seeded.push_back(Address::generate(env));
    }
    let stored = seeded.clone();
    env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .set(&DataKey::ExitedMembers, &stored);
    });
    seeded
}

#[test]
fn test_get_exited_members_pagination_middle_page() {
    let (env, client, _admin, _members, _token, _tac) = setup_group(0);
    let seeded = seed_exited_members(&env, &client);

    let page = client.get_exited_members(&2u32, &3u32);
    assert_eq!(page.len(), 3);
    assert_eq!(page.get(0).unwrap(), seeded.get(2).unwrap());
    assert_eq!(page.get(1).unwrap(), seeded.get(3).unwrap());
    assert_eq!(page.get(2).unwrap(), seeded.get(4).unwrap());
}

#[test]
fn test_get_exited_members_pagination_runs_past_end() {
    let (env, client, _admin, _members, _token, _tac) = setup_group(0);
    let _seeded = seed_exited_members(&env, &client);

    let partial = client.get_exited_members(&5u32, &4u32);
    assert_eq!(partial.len(), 2);

    let empty = client.get_exited_members(&7u32, &4u32);
    assert_eq!(empty.len(), 0);

    let zero_limit = client.get_exited_members(&0u32, &0u32);
    assert_eq!(zero_limit.len(), 0);
}
