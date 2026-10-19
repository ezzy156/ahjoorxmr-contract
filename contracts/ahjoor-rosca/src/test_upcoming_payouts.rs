#![cfg(test)]
use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env, Vec,
};

const CONTRIBUTION: i128 = 100;
const MEMBERS: u32 = 3;
const ROUND_SECONDS: u64 = 3_600;
const START_BALANCE: i128 = 10_000;

struct Setup<'a> {
    env: Env,
    client: AhjoorContractClient<'a>,
    admin: Address,
    token: Address,
    members: Vec<Address>,
}

fn setup<'a>() -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AhjoorContract, ());
    let client = AhjoorContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = token::StellarAssetClient::new(&env, &token);

    let mut members = Vec::new(&env);
    for _ in 0..MEMBERS {
        let m = Address::generate(&env);
        token_admin.mint(&m, &START_BALANCE);
        members.push_back(m);
    }

    client.init(
        &admin,
        &members,
        &CONTRIBUTION,
        &token,
        &ROUND_SECONDS,
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
            max_skips_per_cycle: 1,
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

    Setup {
        env,
        client,
        admin,
        token,
        members,
    }
}

impl<'a> Setup<'a> {
    fn member(&self, i: u32) -> Address {
        self.members.get(i).unwrap()
    }

    fn full_round(&self) {
        for m in self.members.iter() {
            self.client.contribute(&m, &self.token, &CONTRIBUTION);
        }
    }

    fn paid_recipients(&self) -> Vec<Address> {
        let mut recipients = Vec::new(&self.env);
        for record in self.client.get_round_history(&0, &0, &100).iter() {
            recipients.push_back(record.recipient);
        }
        recipients
    }
}

#[test]
fn test_round_robin_order_and_deadlines() {
    let s = setup();
    let deadline = s.client.get_group_info().round_deadline;

    let upcoming = s.client.get_upcoming_payouts(&0, &3);
    assert_eq!(upcoming.len(), 3);
    for i in 0..3u32 {
        let (recipient, round, expected_at) = upcoming.get(i).unwrap();
        assert_eq!(recipient, s.member(i));
        assert_eq!(round, i);
        assert_eq!(expected_at, deadline + i as u64 * ROUND_SECONDS);
    }

    // Matches get_upcoming_deadlines.
    let deadlines = s.client.get_upcoming_deadlines(&3);
    for (_, round, expected_at) in upcoming.iter() {
        assert_eq!(deadlines.get(round).unwrap(), expected_at);
    }
}

#[test]
fn test_count_is_capped() {
    let s = setup();
    let upcoming = s.client.get_upcoming_payouts(&0, &1_000);
    assert_eq!(upcoming.len(), MAX_UPCOMING_PAYOUTS);
    assert_eq!(s.client.get_upcoming_payouts(&0, &0).len(), 0);
}

#[test]
fn test_slot_swap_changes_order_and_matches_payouts() {
    let s = setup();
    let (m0, m1, m2) = (s.member(0), s.member(1), s.member(2));

    // m1 (round 1) and m2 (round 2) swap; no admin approval required.
    let swap_id = s.client.request_slot_swap(&m1, &1, &2, &m2);
    s.client.accept_slot_swap(&m2, &swap_id);

    let upcoming = s.client.get_upcoming_payouts(&0, &3);
    let mut predicted = Vec::new(&s.env);
    for (recipient, _, _) in upcoming.iter() {
        predicted.push_back(recipient);
    }
    assert_eq!(predicted, Vec::from_array(&s.env, [m0, m2, m1]));

    s.full_round();
    s.full_round();
    s.full_round();
    assert_eq!(s.paid_recipients(), predicted);
}

#[test]
fn test_view_advances_with_rounds() {
    let s = setup();
    s.full_round();

    let upcoming = s.client.get_upcoming_payouts(&0, &2);
    let (recipient, round, expected_at) = upcoming.get(0).unwrap();
    assert_eq!(recipient, s.member(1));
    assert_eq!(round, 1);
    assert_eq!(expected_at, s.client.get_group_info().round_deadline);
    assert_eq!(upcoming.get(1).unwrap().0, s.member(2));
}

#[test]
fn test_skip_reflected_and_matches_payout() {
    let s = setup();
    let (m0, m1, m2) = (s.member(0), s.member(1), s.member(2));
    s.full_round();

    // m1 skips round 1, so the payout logic moves on to m2.
    s.client.request_skip(&m1, &1);
    let (recipient, round, _) = s.client.get_upcoming_payouts(&0, &1).get(0).unwrap();
    assert_eq!((recipient.clone(), round), (m2.clone(), 1));

    s.client.contribute(&m0, &s.token, &CONTRIBUTION);
    s.client.contribute(&m2, &s.token, &CONTRIBUTION);
    let deadline = s.client.get_group_info().round_deadline;
    s.env.ledger().set_timestamp(deadline + 1);
    s.client.finalize_round();

    assert_eq!(s.paid_recipients().get(1).unwrap(), recipient);
}

#[test]
fn test_stops_at_max_cycles() {
    let s = setup();
    s.client.set_max_cycles(&s.admin, &Some(1));
    assert_eq!(s.client.get_upcoming_payouts(&0, &10).len(), MEMBERS);

    s.full_round();
    assert_eq!(s.client.get_upcoming_payouts(&0, &10).len(), MEMBERS - 1);
}
