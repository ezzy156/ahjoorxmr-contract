#![cfg(test)]
use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, token
};

fn setup_with_members<'a>(n: usize) -> (Env, AhjoorContractClient<'a>, Address, Address, soroban_sdk::Vec<Address>) {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AhjoorContract, ());
    let client = AhjoorContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token_admin = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    let mut members = soroban_sdk::Vec::new(&env);
    for _ in 0..n {
        members.push_back(Address::generate(&env));
    }

    client.init(
        &admin,
        &members,
        &100, // contribution amount
        &token_admin,
        &3600,
        &RoscaConfig {
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

    (env, client, admin, token_admin, members)
}

#[test]
fn test_set_and_clear_payout_beneficiary() {
    let (env, client, _admin, _token_addr, members) = setup_with_members(2);
    let member2 = members.get(1).unwrap();
    let beneficiary = Address::generate(&env);

    assert_eq!(client.get_payout_beneficiary(&member2), None);

    client.set_payout_beneficiary(&member2, &beneficiary);
    assert_eq!(client.get_payout_beneficiary(&member2), Some(beneficiary));

    client.clear_payout_beneficiary(&member2);
    assert_eq!(client.get_payout_beneficiary(&member2), None);
}

#[test]
fn test_beneficiary_rejected_for_non_member() {
    let (env, client, _admin, _token_addr, _members) = setup_with_members(2);
    let outsider = Address::generate(&env);
    let beneficiary = Address::generate(&env);

    let res = client.try_set_payout_beneficiary(&outsider, &beneficiary);
    assert!(res.is_err());
}

#[test]
fn test_beneficiary_cannot_be_contract() {
    let (_env, client, _admin, _token_addr, members) = setup_with_members(2);
    let member2 = members.get(1).unwrap();

    let res = client.try_set_payout_beneficiary(&member2, &client.address);
    assert!(res.is_err());
}

#[test]
fn test_beneficiary_change_locked_during_payout_round() {
    let (env, client, _admin, _token_addr, members) = setup_with_members(2);
    // Round 0: member1 is the scheduled recipient.
    let member1 = members.get(0).unwrap();
    let beneficiary = Address::generate(&env);

    let res = client.try_set_payout_beneficiary(&member1, &beneficiary);
    assert!(res.is_err());

    let res = client.try_clear_payout_beneficiary(&member1);
    assert!(res.is_err());
}

#[test]
fn test_payout_routed_to_beneficiary() {
    let (env, client, _admin, token_addr, members) = setup_with_members(2);
    let token_client = token::Client::new(&env, &token_addr);
    let token_admin_client = token::StellarAssetClient::new(&env, &token_addr);

    let member1 = members.get(0).unwrap();
    let member2 = members.get(1).unwrap();
    let beneficiary = Address::generate(&env);

    token_admin_client.mint(&member1, &1000);
    token_admin_client.mint(&member2, &1000);

    // member2 nominates a beneficiary before their payout round (round 1).
    client.set_payout_beneficiary(&member2, &beneficiary);

    // Round 0: member1 has no beneficiary and is paid directly.
    client.contribute(&member1, &token_addr, &100);
    client.contribute(&member2, &token_addr, &100);
    assert_eq!(token_client.balance(&member1), 1100);

    // Round 1: member2's payout is routed to the beneficiary.
    env.ledger().with_mut(|li| li.sequence_number += 1);
    client.contribute(&member1, &token_addr, &100);
    client.contribute(&member2, &token_addr, &100);

    assert_eq!(token_client.balance(&beneficiary), 200);
    assert_eq!(token_client.balance(&member2), 800);
}

#[test]
fn test_reinvest_preference_takes_priority_over_beneficiary() {
    let (env, client, _admin, token_addr, members) = setup_with_members(2);
    let token_client = token::Client::new(&env, &token_addr);
    let token_admin_client = token::StellarAssetClient::new(&env, &token_addr);

    let member1 = members.get(0).unwrap();
    let member2 = members.get(1).unwrap();
    let beneficiary = Address::generate(&env);

    token_admin_client.mint(&member1, &1000);
    token_admin_client.mint(&member2, &1000);

    client.set_payout_beneficiary(&member2, &beneficiary);
    client.set_reinvest_preference(&member2, &true);

    // Round 0: member1 is paid.
    client.contribute(&member1, &token_addr, &100);
    client.contribute(&member2, &token_addr, &100);

    // Round 1: member2 reinvests; the beneficiary receives nothing.
    client.contribute(&member1, &token_addr, &100);
    client.contribute(&member2, &token_addr, &100);

    assert_eq!(token_client.balance(&beneficiary), 0);
}
