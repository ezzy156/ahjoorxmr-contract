#![cfg(test)]
//! Coverage for emergency payouts (`set_emergency_payout_config`,
//! `request_emergency_payout`, `vote_emergency_payout`,
//! `execute_emergency_payout`) and treasury payments
//! (`execute_treasury_payment`, `get_treasury_balance`).
use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient as TokenAdminClient},
    Address, BytesN, Env, Map, Vec,
};

const CONTRIBUTION: i128 = 100;
const START_TS: u64 = 1_000;

struct Setup<'a> {
    env: Env,
    client: AhjoorContractClient<'a>,
    admin: Address,
    members: Vec<Address>,
    token: TokenClient<'a>,
    token_admin: TokenAdminClient<'a>,
}

fn setup<'a>(member_count: u32) -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(START_TS);

    let contract_id = env.register(AhjoorContract, ());
    let client = AhjoorContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token = TokenClient::new(&env, &token_addr);
    let token_admin = TokenAdminClient::new(&env, &token_addr);

    let mut members = Vec::new(&env);
    for _ in 0..member_count {
        let m = Address::generate(&env);
        token_admin.mint(&m, &1_000);
        members.push_back(m);
    }

    client.init(
        &admin,
        &members,
        &CONTRIBUTION,
        &token_addr,
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
        },
        &None,
    );

    Setup {
        env,
        client,
        admin,
        members,
        token,
        token_admin,
    }
}

fn reason(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[7u8; 32])
}

/// Funds the contract by having the first `n` members contribute for the
/// current round.
fn contribute_from_first(s: &Setup, n: u32) {
    for i in 0..n {
        s.client
            .contribute(&s.members.get(i).unwrap(), &s.token.address, &CONTRIBUTION);
    }
}

/// Reads the stored emergency payout request for `requester` in round 0.
fn stored_request(s: &Setup, requester: &Address) -> EmergencyPayoutRequest {
    s.env.as_contract(&s.client.address, || {
        let requests: Map<(u32, Address), EmergencyPayoutRequest> = s
            .env
            .storage()
            .instance()
            .get(&DataKey2::EmergencyPayoutRequests)
            .unwrap();
        requests.get((0u32, requester.clone())).unwrap()
    })
}

/// The treasury balance is only ever written by `enable_group_treasury`
/// (reset to 0) and `execute_treasury_payment` (debited); nothing credits it.
/// Seed the stored balance and back it with real tokens so payments can be
/// exercised.
fn seed_treasury(s: &Setup, amount: i128) {
    s.token_admin.mint(&s.client.address, &amount);
    s.env.as_contract(&s.client.address, || {
        s.env
            .storage()
            .instance()
            .set(&DataKey3::TreasuryBalance, &amount);
    });
}

// ===========================================================================
//  set_emergency_payout_config
// ===========================================================================

#[test]
fn test_set_emergency_payout_config_rejects_invalid_values() {
    let s = setup(5);

    let low_quorum = s.client.try_set_emergency_payout_config(&s.admin, &999, &100, &1);
    assert_eq!(
        low_quorum.unwrap_err().unwrap(),
        ExtError::InvalidEmergencyConfig.into()
    );
    let high_quorum = s
        .client
        .try_set_emergency_payout_config(&s.admin, &10_001, &100, &1);
    assert_eq!(
        high_quorum.unwrap_err().unwrap(),
        ExtError::InvalidEmergencyConfig.into()
    );
    let zero_window = s.client.try_set_emergency_payout_config(&s.admin, &6000, &0, &1);
    assert_eq!(
        zero_window.unwrap_err().unwrap(),
        ExtError::InvalidEmergencyConfig.into()
    );
    let zero_max = s.client.try_set_emergency_payout_config(&s.admin, &6000, &100, &0);
    assert_eq!(
        zero_max.unwrap_err().unwrap(),
        ExtError::InvalidEmergencyConfig.into()
    );

    // Boundary values are accepted.
    s.client.set_emergency_payout_config(&s.admin, &1000, &1, &1);
    s.client.set_emergency_payout_config(&s.admin, &10_000, &1, &1);
}

#[test]
#[should_panic(expected = "Only admin can set emergency payout config")]
fn test_set_emergency_payout_config_non_admin_panics() {
    let s = setup(5);
    let outsider = Address::generate(&s.env);
    s.client
        .set_emergency_payout_config(&outsider, &6000, &100, &1);
}

// ===========================================================================
//  Full emergency payout flow
// ===========================================================================

#[test]
fn test_emergency_payout_full_flow_moves_funds_to_requester() {
    let s = setup(5);
    contribute_from_first(&s, 3);
    // 5 members * 60% => 3 votes required.
    s.client.set_emergency_payout_config(&s.admin, &6000, &86_400, &1);

    let requester = s.members.get(4).unwrap();
    s.client.request_emergency_payout(&requester, &reason(&s.env));

    let req = stored_request(&s, &requester);
    assert_eq!(req.requester, requester);
    assert_eq!(req.created_at, START_TS);
    assert_eq!(req.deadline, START_TS + 86_400);
    assert!(!req.executed);

    for i in 0..3 {
        s.client
            .vote_emergency_payout(&s.members.get(i).unwrap(), &requester, &true);
    }
    let req = stored_request(&s, &requester);
    assert_eq!(req.votes_for, 3);
    assert_eq!(req.votes_against, 0);

    let requester_before = s.token.balance(&requester);
    let contract_before = s.token.balance(&s.client.address);
    assert_eq!(contract_before, 3 * CONTRIBUTION);

    s.client.execute_emergency_payout(&requester);

    assert_eq!(s.token.balance(&requester), requester_before + CONTRIBUTION);
    assert_eq!(
        s.token.balance(&s.client.address),
        contract_before - CONTRIBUTION
    );
    assert!(stored_request(&s, &requester).executed);

    // Requester is marked as paid for the round.
    let (_round, paid_members, _, _, _) = s.client.get_state();
    assert!(paid_members.contains(&requester));

    // A second execution is rejected and moves no funds.
    let again = s.client.try_execute_emergency_payout(&requester);
    assert_eq!(
        again.unwrap_err().unwrap(),
        ExtError::EmergencyPayoutAlreadyExecuted.into()
    );
    assert_eq!(s.token.balance(&requester), requester_before + CONTRIBUTION);
}

#[test]
fn test_execute_emergency_payout_without_enough_votes_fails() {
    let s = setup(5);
    contribute_from_first(&s, 3);

    // Default quorum is 66.67% => ceil(5 * 0.6667) = 4 votes required.
    let requester = s.members.get(4).unwrap();
    s.client.request_emergency_payout(&requester, &reason(&s.env));
    for i in 0..3 {
        s.client
            .vote_emergency_payout(&s.members.get(i).unwrap(), &requester, &true);
    }

    let requester_before = s.token.balance(&requester);
    let contract_before = s.token.balance(&s.client.address);

    let res = s.client.try_execute_emergency_payout(&requester);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError::EmergencyPayoutQuorumNotMet.into()
    );
    assert_eq!(s.token.balance(&requester), requester_before);
    assert_eq!(s.token.balance(&s.client.address), contract_before);
    assert!(!stored_request(&s, &requester).executed);

    // A fourth vote reaches quorum and execution succeeds.
    s.client
        .vote_emergency_payout(&s.members.get(3).unwrap(), &requester, &true);
    s.client.execute_emergency_payout(&requester);
    assert_eq!(s.token.balance(&requester), requester_before + CONTRIBUTION);
    assert_eq!(
        s.token.balance(&s.client.address),
        contract_before - CONTRIBUTION
    );
}

#[test]
fn test_execute_emergency_payout_with_no_votes_fails() {
    let s = setup(5);
    contribute_from_first(&s, 2);
    let requester = s.members.get(4).unwrap();
    s.client.request_emergency_payout(&requester, &reason(&s.env));

    let res = s.client.try_execute_emergency_payout(&requester);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError::EmergencyPayoutQuorumNotMet.into()
    );
}

#[test]
fn test_execute_emergency_payout_rejected_by_majority_moves_no_funds() {
    let s = setup(5);
    contribute_from_first(&s, 3);
    s.client.set_emergency_payout_config(&s.admin, &6000, &86_400, &1);

    let requester = s.members.get(4).unwrap();
    s.client.request_emergency_payout(&requester, &reason(&s.env));
    s.client
        .vote_emergency_payout(&s.members.get(0).unwrap(), &requester, &true);
    s.client
        .vote_emergency_payout(&s.members.get(1).unwrap(), &requester, &false);
    s.client
        .vote_emergency_payout(&s.members.get(2).unwrap(), &requester, &false);

    let requester_before = s.token.balance(&requester);
    let contract_before = s.token.balance(&s.client.address);

    // Quorum is met but the vote failed: no panic, no transfer.
    s.client.execute_emergency_payout(&requester);

    assert_eq!(s.token.balance(&requester), requester_before);
    assert_eq!(s.token.balance(&s.client.address), contract_before);
    assert!(!stored_request(&s, &requester).executed);
    let (_round, paid_members, _, _, _) = s.client.get_state();
    assert!(!paid_members.contains(&requester));
}

#[test]
fn test_request_emergency_payout_rejects_non_member_and_duplicate() {
    let s = setup(5);
    let outsider = Address::generate(&s.env);
    let res = s
        .client
        .try_request_emergency_payout(&outsider, &reason(&s.env));
    assert_eq!(res.unwrap_err().unwrap(), Error::NotAMember.into());

    let requester = s.members.get(0).unwrap();
    s.client.request_emergency_payout(&requester, &reason(&s.env));
    let dup = s
        .client
        .try_request_emergency_payout(&requester, &reason(&s.env));
    assert_eq!(
        dup.unwrap_err().unwrap(),
        ExtError::EmergencyPayoutRequested.into()
    );
}

#[test]
fn test_emergency_payout_limit_per_cycle() {
    let s = setup(5);
    contribute_from_first(&s, 3);
    s.client.set_emergency_payout_config(&s.admin, &6000, &86_400, &1);

    let first = s.members.get(4).unwrap();
    s.client.request_emergency_payout(&first, &reason(&s.env));
    for i in 0..3 {
        s.client
            .vote_emergency_payout(&s.members.get(i).unwrap(), &first, &true);
    }
    s.client.execute_emergency_payout(&first);

    // max_emergency_per_cycle = 1 has been used up.
    let second = s.members.get(3).unwrap();
    let res = s
        .client
        .try_request_emergency_payout(&second, &reason(&s.env));
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError::EmergencyPayoutLimitReached.into()
    );
}

#[test]
fn test_vote_and_execute_after_deadline_fail() {
    let s = setup(5);
    contribute_from_first(&s, 3);
    s.client.set_emergency_payout_config(&s.admin, &6000, &100, &1);

    let requester = s.members.get(4).unwrap();
    s.client.request_emergency_payout(&requester, &reason(&s.env));
    for i in 0..3 {
        s.client
            .vote_emergency_payout(&s.members.get(i).unwrap(), &requester, &true);
    }

    s.env.ledger().set_timestamp(START_TS + 101);

    let vote = s
        .client
        .try_vote_emergency_payout(&s.members.get(3).unwrap(), &requester, &true);
    assert_eq!(
        vote.unwrap_err().unwrap(),
        ExtError::EmergencyPayoutVoteExpired.into()
    );

    let requester_before = s.token.balance(&requester);
    let exec = s.client.try_execute_emergency_payout(&requester);
    assert_eq!(
        exec.unwrap_err().unwrap(),
        ExtError::EmergencyPayoutVoteExpired.into()
    );
    assert_eq!(s.token.balance(&requester), requester_before);
}

#[test]
#[should_panic(expected = "Cannot vote on your own emergency payout request")]
fn test_requester_cannot_vote_on_own_request() {
    let s = setup(5);
    let requester = s.members.get(0).unwrap();
    s.client.request_emergency_payout(&requester, &reason(&s.env));
    s.client
        .vote_emergency_payout(&requester, &requester, &true);
}

#[test]
#[should_panic(expected = "Already voted on this emergency payout request")]
fn test_double_vote_panics() {
    let s = setup(5);
    let requester = s.members.get(0).unwrap();
    let voter = s.members.get(1).unwrap();
    s.client.request_emergency_payout(&requester, &reason(&s.env));
    s.client.vote_emergency_payout(&voter, &requester, &true);
    s.client.vote_emergency_payout(&voter, &requester, &false);
}

#[test]
fn test_non_member_cannot_vote() {
    let s = setup(5);
    let requester = s.members.get(0).unwrap();
    s.client.request_emergency_payout(&requester, &reason(&s.env));
    let outsider = Address::generate(&s.env);
    let res = s
        .client
        .try_vote_emergency_payout(&outsider, &requester, &true);
    assert_eq!(res.unwrap_err().unwrap(), Error::OnlyMembersAllowed.into());
}

#[test]
#[should_panic(expected = "Emergency payout request not found")]
fn test_execute_without_request_panics() {
    let s = setup(5);
    s.client
        .execute_emergency_payout(&s.members.get(0).unwrap());
}

// ===========================================================================
//  Treasury payments
// ===========================================================================

#[test]
fn test_treasury_balance_is_zero_before_and_after_enable() {
    let s = setup(3);
    assert_eq!(s.client.get_treasury_balance(), 0);
    assert!(s.client.get_treasury_config().is_none());

    let treasury_admin = Address::generate(&s.env);
    s.client.enable_group_treasury(&s.admin, &treasury_admin);
    assert_eq!(s.client.get_treasury_balance(), 0);
    let config = s.client.get_treasury_config().unwrap();
    assert_eq!(config.treasury_admin, treasury_admin);
    assert!(config.enabled);
}

#[test]
fn test_execute_treasury_payment_debits_balance_and_pays_recipient() {
    let s = setup(3);
    let treasury_admin = Address::generate(&s.env);
    s.client.enable_group_treasury(&s.admin, &treasury_admin);
    seed_treasury(&s, 500);
    assert_eq!(s.client.get_treasury_balance(), 500);

    let recipient = Address::generate(&s.env);
    s.client
        .execute_treasury_payment(&treasury_admin, &recipient, &200, &reason(&s.env));

    assert_eq!(s.client.get_treasury_balance(), 300);
    assert_eq!(s.token.balance(&recipient), 200);
    assert_eq!(s.token.balance(&s.client.address), 300);

    // Paying out exactly the remaining balance is allowed.
    s.client
        .execute_treasury_payment(&treasury_admin, &recipient, &300, &reason(&s.env));
    assert_eq!(s.client.get_treasury_balance(), 0);
    assert_eq!(s.token.balance(&recipient), 500);
    assert_eq!(s.token.balance(&s.client.address), 0);
}

#[test]
#[should_panic(expected = "Insufficient treasury balance")]
fn test_treasury_payment_cannot_exceed_treasury_balance() {
    let s = setup(3);
    let treasury_admin = Address::generate(&s.env);
    s.client.enable_group_treasury(&s.admin, &treasury_admin);
    seed_treasury(&s, 300);

    // The contract also holds member contributions, so it has enough tokens
    // in total; the payment must still be capped by the treasury balance.
    contribute_from_first(&s, 2);
    assert!(s.token.balance(&s.client.address) >= 301);

    let recipient = Address::generate(&s.env);
    s.client
        .execute_treasury_payment(&treasury_admin, &recipient, &301, &reason(&s.env));
}

#[test]
fn test_treasury_payment_over_balance_leaves_state_unchanged() {
    let s = setup(3);
    let treasury_admin = Address::generate(&s.env);
    s.client.enable_group_treasury(&s.admin, &treasury_admin);
    seed_treasury(&s, 300);

    let recipient = Address::generate(&s.env);
    let res = s.client.try_execute_treasury_payment(
        &treasury_admin,
        &recipient,
        &(s.client.get_treasury_balance() + 1),
        &reason(&s.env),
    );
    assert!(res.is_err());
    assert_eq!(s.client.get_treasury_balance(), 300);
    assert_eq!(s.token.balance(&recipient), 0);
    assert_eq!(s.token.balance(&s.client.address), 300);
}

#[test]
#[should_panic(expected = "Only treasury admin can execute payments")]
fn test_treasury_payment_requires_treasury_admin() {
    let s = setup(3);
    let treasury_admin = Address::generate(&s.env);
    s.client.enable_group_treasury(&s.admin, &treasury_admin);
    seed_treasury(&s, 300);

    // Even the group admin cannot spend the treasury.
    s.client.execute_treasury_payment(
        &s.admin,
        &Address::generate(&s.env),
        &100,
        &reason(&s.env),
    );
}

#[test]
#[should_panic(expected = "Treasury not enabled")]
fn test_treasury_payment_requires_enabled_treasury() {
    let s = setup(3);
    s.client.execute_treasury_payment(
        &s.admin,
        &Address::generate(&s.env),
        &1,
        &reason(&s.env),
    );
}
