#![cfg(test)]
use super::*;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, Map, Vec,
};

fn setup_group<'a>() -> (Env, AhjoorContractClient<'a>, Address, Vec<Address>, Address) {
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
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 5,
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
    env.ledger().set_timestamp(100);
    client.set_succession_trigger_rounds(&admin, &2);
    (env, client, admin, members, token_addr)
}

/// Members 0 and 1 contribute, member 2 misses, then the round is finalized.
fn run_round_missing_last(env: &Env, client: &AhjoorContractClient, members: &Vec<Address>, token: &Address) {
    client.contribute(&members.get(0).unwrap(), token, &100);
    client.contribute(&members.get(1).unwrap(), token, &100);
    let deadline = client.get_group_info().round_deadline;
    env.ledger().set_timestamp(deadline + 1);
    client.finalize_round();
}

#[test]
fn test_designate_and_accept_successor() {
    let (env, client, _admin, members, _token) = setup_group();
    let member = members.get(2).unwrap();
    let successor = Address::generate(&env);

    client.designate_successor(&member, &successor);
    let d = client.get_successor(&member).unwrap();
    assert_eq!(d.successor, successor);
    assert!(!d.accepted);

    client.accept_succession(&successor, &member);
    assert!(client.get_successor(&member).unwrap().accepted);
}

#[test]
fn test_existing_member_cannot_be_successor() {
    let (_env, client, _admin, members, _token) = setup_group();
    let res = client.try_designate_successor(&members.get(2).unwrap(), &members.get(0).unwrap());
    assert_eq!(res.unwrap_err().unwrap(), ExtError2::InvalidSuccessor.into());
}

#[test]
fn test_member_cannot_designate_self() {
    let (_env, client, _admin, members, _token) = setup_group();
    let m = members.get(2).unwrap();
    let res = client.try_designate_successor(&m, &m);
    assert_eq!(res.unwrap_err().unwrap(), ExtError2::InvalidSuccessor.into());
}

#[test]
fn test_only_designated_successor_can_accept() {
    let (env, client, _admin, members, _token) = setup_group();
    let member = members.get(2).unwrap();
    client.designate_successor(&member, &Address::generate(&env));

    let res = client.try_accept_succession(&Address::generate(&env), &member);
    assert_eq!(res.unwrap_err().unwrap(), ExtError2::SuccessorNotDesignated.into());
}

#[test]
fn test_claim_requires_acceptance() {
    let (env, client, _admin, members, token) = setup_group();
    let member = members.get(2).unwrap();
    let successor = Address::generate(&env);
    client.designate_successor(&member, &successor);

    run_round_missing_last(&env, &client, &members, &token);
    run_round_missing_last(&env, &client, &members, &token);

    let res = client.try_claim_succession(&successor, &member);
    assert_eq!(res.unwrap_err().unwrap(), ExtError2::SuccessionNotAccepted.into());
}

#[test]
fn test_early_claim_rejected() {
    let (env, client, _admin, members, token) = setup_group();
    let member = members.get(2).unwrap();
    let successor = Address::generate(&env);
    client.designate_successor(&member, &successor);
    client.accept_succession(&successor, &member);

    // No missed rounds yet.
    let res = client.try_claim_succession(&successor, &member);
    assert_eq!(res.unwrap_err().unwrap(), ExtError2::SuccessionThresholdNotMet.into());

    // One missed round, trigger is two.
    run_round_missing_last(&env, &client, &members, &token);
    assert_eq!(client.get_consecutive_misses(&member), 1);
    let res = client.try_claim_succession(&successor, &member);
    assert_eq!(res.unwrap_err().unwrap(), ExtError2::SuccessionThresholdNotMet.into());
}

#[test]
fn test_contribution_resets_consecutive_misses() {
    let (env, client, _admin, members, token) = setup_group();
    let member = members.get(2).unwrap();

    run_round_missing_last(&env, &client, &members, &token);
    assert_eq!(client.get_consecutive_misses(&member), 1);

    // Member contributes in the next round alongside a missing member 1.
    client.contribute(&members.get(0).unwrap(), &token, &100);
    client.contribute(&member, &token, &100);
    let deadline = client.get_group_info().round_deadline;
    env.ledger().set_timestamp(deadline + 1);
    client.finalize_round();

    assert_eq!(client.get_consecutive_misses(&member), 0);
    assert_eq!(client.get_consecutive_misses(&members.get(1).unwrap()), 1);
}

#[test]
fn test_successful_claim_transfers_slot_history_and_debt() {
    let (env, client, _admin, members, token) = setup_group();
    let member = members.get(2).unwrap();
    let successor = Address::generate(&env);

    client.designate_successor(&member, &successor);
    client.accept_succession(&successor, &member);

    run_round_missing_last(&env, &client, &members, &token);
    run_round_missing_last(&env, &client, &members, &token);
    assert_eq!(client.get_consecutive_misses(&member), 2);

    // Seed outstanding catch-up debt for the inactive member.
    env.as_contract(&client.address, || {
        let mut debts: Map<Address, i128> = Map::new(&env);
        debts.set(member.clone(), 250);
        env.storage().instance().set(&DataKey2::CatchUpDebt, &debts);
    });

    let history_before = client.get_member_contribution_history(&member, &None, &None);
    let order_before = client.get_payout_order();
    let slot_index = order_before.first_index_of(&member).unwrap();

    client.claim_succession(&successor, &member);

    // Slot: successor replaces the member at the same position.
    let info = client.get_group_info();
    assert!(info.members.contains(&successor));
    assert!(!info.members.contains(&member));
    let order_after = client.get_payout_order();
    assert_eq!(order_after.get(slot_index).unwrap(), successor);
    assert_eq!(order_after.len(), order_before.len());

    // Debt transfers with the slot.
    assert_eq!(client.get_catch_up_debt(&successor), 250);
    assert_eq!(client.get_catch_up_debt(&member), 0);

    // History is linked to the successor.
    assert_eq!(client.get_succeeded_from(&successor), Some(member.clone()));
    let history_after = client.get_member_contribution_history(&successor, &None, &None);
    assert_eq!(history_after, history_before);

    // Designation consumed, miss counter cleared.
    assert!(client.get_successor(&member).is_none());
    assert_eq!(client.get_consecutive_misses(&successor), 0);

    // Successor can now contribute in the member's place.
    let tac = TokenAdminClient::new(&env, &token);
    tac.mint(&successor, &1_000);
    client.contribute(&successor, &token, &100);
}

#[test]
fn test_claim_rejected_if_successor_joined_meanwhile() {
    let (env, client, _admin, members, token) = setup_group();
    let member = members.get(2).unwrap();
    let successor = Address::generate(&env);
    client.designate_successor(&member, &successor);
    client.accept_succession(&successor, &member);

    run_round_missing_last(&env, &client, &members, &token);
    run_round_missing_last(&env, &client, &members, &token);

    // Successor became a member of this group through another path.
    client.add_member(&successor);

    let res = client.try_claim_succession(&successor, &member);
    assert_eq!(res.unwrap_err().unwrap(), ExtError2::InvalidSuccessor.into());
}

#[test]
fn test_invalid_trigger_rounds_rejected() {
    let (_env, client, admin, _members, _token) = setup_group();
    let res = client.try_set_succession_trigger_rounds(&admin, &0);
    assert_eq!(res.unwrap_err().unwrap(), ExtError2::InvalidSuccessionTrigger.into());
    assert_eq!(client.get_succession_trigger_rounds(), 2);
}
