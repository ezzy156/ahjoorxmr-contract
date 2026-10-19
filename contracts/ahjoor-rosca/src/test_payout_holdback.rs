#![cfg(test)]
use super::*;
use crate::errors::ExtError3;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env, Vec,
};

const CONTRIBUTION: i128 = 100;
const MEMBERS: u32 = 3;
const POT: i128 = MEMBERS as i128 * CONTRIBUTION;
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

    fn pay(&self, i: u32) {
        self.client
            .contribute(&self.member(i), &self.token, &CONTRIBUTION);
    }

    fn full_round(&self) {
        for i in 0..MEMBERS {
            self.pay(i);
        }
    }

    fn pass_deadline(&self) {
        let deadline = self.client.get_group_info().round_deadline;
        self.env.ledger().set_timestamp(deadline + 1);
    }
}

#[test]
fn test_zero_bps_pays_full_payout() {
    let s = setup();
    let m0 = s.member(0);
    assert_eq!(s.client.get_payout_holdback_bps(), 0);

    s.full_round();

    assert_eq!(s.balance(&m0), START_BALANCE - CONTRIBUTION + POT);
    assert_eq!(s.client.get_holdback(&m0, &0), None);
    let res = s.client.try_release_holdback(&0, &m0, &0);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError3::NoHoldbackRecord.into()
    );
}

#[test]
fn test_bps_bounds_and_lock_after_activation() {
    let s = setup();
    let res = s.client.try_set_payout_holdback_bps(&s.admin, &5_001);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError3::InvalidHoldbackBps.into()
    );

    s.client.set_payout_holdback_bps(&s.admin, &5_000);
    assert_eq!(s.client.get_payout_holdback_bps(), 5_000);

    s.pay(0);
    let res = s.client.try_set_payout_holdback_bps(&s.admin, &1_000);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError3::ConfigLockedAfterActivation.into()
    );
}

#[test]
fn test_holdback_released_after_remaining_contributions() {
    let s = setup();
    let (m0, m1, m2) = (s.member(0), s.member(1), s.member(2));
    s.client.set_payout_holdback_bps(&s.admin, &2_000);
    let held = POT * 2_000 / 10_000;

    // Round 0: m0 is paid; 20% is withheld until m0 pays rounds 1 and 2.
    s.full_round();
    assert_eq!(s.balance(&m0), START_BALANCE - CONTRIBUTION + POT - held);
    let record = s.client.get_holdback(&m0, &0).unwrap();
    assert_eq!(record.amount, held);
    assert_eq!(record.payout_round, 0);
    assert_eq!(record.last_round, 2);
    assert_eq!(record.status, HoldbackStatus::Held);

    let res = s.client.try_release_holdback(&0, &m0, &0);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError3::HoldbackNotReleasable.into()
    );

    // Round 1: m1's pot excludes m0's withheld funds.
    s.full_round();
    assert_eq!(
        s.balance(&m1),
        START_BALANCE - 2 * CONTRIBUTION + POT - held
    );
    let res = s.client.try_release_holdback(&0, &m0, &0);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError3::HoldbackNotReleasable.into()
    );

    // Round 2: once m0 has paid the last round of the cycle, it can release.
    s.pay(0);
    let before = s.balance(&m0);
    assert_eq!(s.client.release_holdback(&0, &m0, &0), held);
    assert_eq!(s.balance(&m0), before + held);
    assert_eq!(
        s.client.get_holdback(&m0, &0).unwrap().status,
        HoldbackStatus::Released
    );
    let res = s.client.try_release_holdback(&0, &m0, &0);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::HoldbackSettled.into());

    // m2 is paid in the last round of the cycle: nothing left to protect.
    s.pay(1);
    s.pay(2);
    assert_eq!(s.client.get_holdback(&m2, &0), None);
    assert_eq!(s.balance(&m2), START_BALANCE - 3 * CONTRIBUTION + POT);

    assert_eq!(s.client.release_holdback(&0, &m1, &0), held);
    assert_eq!(s.balance(&m1), START_BALANCE - 3 * CONTRIBUTION + POT);
}

#[test]
fn test_default_covered_by_holdback_and_remainder_forfeited() {
    let s = setup();
    let (m0, m1) = (s.member(0), s.member(1));
    s.client.set_payout_holdback_bps(&s.admin, &5_000);
    let held = POT / 2;

    // Seed the insurance pool so we can see it is not drawn on.
    s.client.contribute_to_insurance(&m1, &s.token, &40);
    s.full_round();

    // Round 1: m0 (already paid out) stops contributing.
    s.pay(1);
    s.pay(2);
    s.pass_deadline();
    let m1_before = s.balance(&m1);
    s.client.finalize_round();

    // The holdback covered the missed contribution; no default is recorded.
    let record = s.client.get_holdback(&m0, &0).unwrap();
    assert_eq!(record.status, HoldbackStatus::Forfeited);
    assert_eq!(record.covered, CONTRIBUTION);
    assert_eq!(record.forfeited, held - CONTRIBUTION);
    assert_eq!(s.client.get_member_status(&m0).default_count, 0);

    // m1 received a full pot (less its own holdback); insurance was untouched
    // and gained the forfeited remainder.
    assert_eq!(s.balance(&m1), m1_before + POT - POT / 2);
    assert_eq!(s.client.get_insurance_pool(), 40 + held - CONTRIBUTION);

    let res = s.client.try_release_holdback(&0, &m0, &0);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::HoldbackSettled.into());
}

#[test]
fn test_partial_cover_still_defaults() {
    let s = setup();
    let m0 = s.member(0);
    s.client.set_payout_holdback_bps(&s.admin, &1_000);
    let held = POT / 10; // 30, less than the 100 owed

    s.full_round();
    s.pay(1);
    s.pay(2);
    s.pass_deadline();
    s.client.finalize_round();

    let record = s.client.get_holdback(&m0, &0).unwrap();
    assert_eq!(record.status, HoldbackStatus::Forfeited);
    assert_eq!(record.covered, held);
    assert_eq!(record.forfeited, 0);
    assert_eq!(s.client.get_member_status(&m0).default_count, 1);
}

#[test]
fn test_removed_member_forfeits_holdback() {
    let s = setup();
    let m0 = s.member(0);
    s.client.set_payout_holdback_bps(&s.admin, &2_000);
    let held = POT * 2_000 / 10_000;

    s.full_round();
    s.client.remove_member(&m0);

    // Remaining members complete round 1 without m0.
    s.pay(1);
    s.pay(2);

    let record = s.client.get_holdback(&m0, &0).unwrap();
    assert_eq!(record.status, HoldbackStatus::Forfeited);
    assert_eq!(record.forfeited, held);
    assert_eq!(s.client.get_insurance_pool(), held);
}
