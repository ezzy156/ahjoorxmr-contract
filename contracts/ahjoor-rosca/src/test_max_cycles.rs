#![cfg(test)]
use super::*;
use crate::errors::ExtError3;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env, Vec,
};

const CONTRIBUTION: i128 = 100;
const MEMBERS: u32 = 2;
const ROUND_SECONDS: u64 = 3_600;
const START_BALANCE: i128 = 1_000;

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

    fn balance(&self, who: &Address) -> i128 {
        token::Client::new(&self.env, &self.token).balance(who)
    }

    fn full_round(&self) {
        for m in self.members.iter() {
            self.client.contribute(&m, &self.token, &CONTRIBUTION);
        }
    }

    fn full_cycle(&self) {
        for _ in 0..MEMBERS {
            self.full_round();
        }
    }
}

#[test]
fn test_none_keeps_unlimited_cycles() {
    let s = setup();
    assert_eq!(s.client.get_max_cycles(), None);

    s.full_cycle();
    s.full_cycle();
    s.full_cycle();

    assert_eq!(s.client.get_group_status(), GroupStatus::Active);
    s.client.contribute(&s.member(0), &s.token, &CONTRIBUTION);
}

#[test]
fn test_set_max_cycles_validation() {
    let s = setup();
    let res = s.client.try_set_max_cycles(&s.admin, &Some(0));
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError3::InvalidMaxCycles.into()
    );

    s.client.set_max_cycles(&s.admin, &Some(2));
    assert_eq!(s.client.get_max_cycles(), Some(2));
    s.client.set_max_cycles(&s.admin, &None);
    assert_eq!(s.client.get_max_cycles(), None);

    s.client.contribute(&s.member(0), &s.token, &CONTRIBUTION);
    let res = s.client.try_set_max_cycles(&s.admin, &Some(3));
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError3::ConfigLockedAfterActivation.into()
    );
}

#[test]
fn test_group_completes_at_cycle_limit() {
    let s = setup();
    let (m0, m1) = (s.member(0), s.member(1));
    s.client.set_max_cycles(&s.admin, &Some(2));

    s.full_cycle();
    assert_eq!(s.client.get_group_status(), GroupStatus::Active);

    s.full_cycle();
    assert_eq!(s.client.get_group_status(), GroupStatus::Completed);
    assert!(!s.client.is_active());

    // Contributions and round advances are rejected with a domain error.
    let res = s.client.try_contribute(&m0, &s.token, &CONTRIBUTION);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::GroupCompleted.into());
    let res = s.client.try_prepay_rounds(&m1, &1);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::GroupCompleted.into());
    let deadline = s.client.get_group_info().round_deadline;
    s.env.ledger().set_timestamp(deadline + 1);
    let res = s.client.try_finalize_round();
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::GroupCompleted.into());
    let res = s.client.try_close_round();
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::GroupCompleted.into());

    // Every member was paid exactly once per cycle.
    assert_eq!(s.balance(&m0), START_BALANCE);
    assert_eq!(s.balance(&m1), START_BALANCE);
    assert_eq!(s.client.get_upcoming_payouts(&0, &5).len(), 0);
}

#[test]
fn test_completion_distributes_insurance_pro_rata() {
    let s = setup();
    let (m0, m1) = (s.member(0), s.member(1));
    s.client.set_max_cycles(&s.admin, &Some(1));
    s.client.contribute_to_insurance(&m0, &s.token, &60);
    assert_eq!(s.client.get_insurance_pool(), 60);

    s.full_cycle();

    assert_eq!(s.client.get_group_status(), GroupStatus::Completed);
    assert_eq!(s.client.get_insurance_pool(), 0);
    // Both members contributed equally, so the pool is split evenly.
    assert_eq!(s.balance(&m0), START_BALANCE - 60 + 30);
    assert_eq!(s.balance(&m1), START_BALANCE + 30);
}

#[test]
fn test_completion_via_finalize_round() {
    let s = setup();
    let (m0, m1) = (s.member(0), s.member(1));
    s.client.set_max_cycles(&s.admin, &Some(1));

    s.full_round();
    // Last round of the only cycle is settled by the admin after a default.
    s.client.contribute(&m0, &s.token, &CONTRIBUTION);
    let deadline = s.client.get_group_info().round_deadline;
    s.env.ledger().set_timestamp(deadline + 1);
    s.client.finalize_round();

    assert_eq!(s.client.get_group_status(), GroupStatus::Completed);
    let res = s.client.try_contribute(&m1, &s.token, &CONTRIBUTION);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::GroupCompleted.into());
}

#[test]
fn test_holdback_releasable_after_completion() {
    let s = setup();
    let m0 = s.member(0);
    s.client.set_max_cycles(&s.admin, &Some(1));
    s.client.set_payout_holdback_bps(&s.admin, &1_000);

    s.full_cycle();
    assert_eq!(s.client.get_group_status(), GroupStatus::Completed);

    let held = 2 * CONTRIBUTION / 10;
    assert_eq!(s.client.release_holdback(&0, &m0, &0), held);
    assert_eq!(s.balance(&m0), START_BALANCE);
}
