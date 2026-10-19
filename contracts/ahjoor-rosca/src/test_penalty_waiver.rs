#![cfg(test)]
use super::*;
use crate::errors::{Error, ExtError3};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env, Vec,
};

const CONTRIBUTION: i128 = 100;
const ROUND_SECONDS: u64 = 3_600;
const START_BALANCE: i128 = 1_000;
const PENALTY: i128 = 50;
const VOTING: u64 = 10;

struct Setup<'a> {
    env: Env,
    contract_id: Address,
    client: AhjoorContractClient<'a>,
    token: Address,
    members: Vec<Address>,
}

fn setup<'a>(grace_period_ledgers: u32) -> Setup<'a> {
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
    for _ in 0..2 {
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
            penalty_amount: PENALTY,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 3,
            grace_period_ledgers,
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

    Setup { env, contract_id, client, token, members }
}

impl<'a> Setup<'a> {
    fn member(&self, i: u32) -> Address {
        self.members.get(i).unwrap()
    }

    fn balance(&self, who: &Address) -> i128 {
        token::Client::new(&self.env, &self.token).balance(who)
    }

    fn now(&self) -> u64 {
        self.env.ledger().timestamp()
    }

    fn set_time(&self, ts: u64) {
        self.env.ledger().set_timestamp(ts);
    }

    fn full_round(&self) {
        for m in self.members.iter() {
            self.client.contribute(&m, &self.token, &CONTRIBUTION);
        }
    }

    /// Round 0 is paid by everyone; in round 1 member 1 defaults and the
    /// round is closed. Returns the deadline of round 1.
    fn default_in_round_one(&self) -> u64 {
        self.full_round();
        let deadline = self.client.get_group_info().round_deadline;
        self.client
            .contribute(&self.member(0), &self.token, &CONTRIBUTION);
        self.set_time(deadline + 1);
        self.client.close_round();
        deadline
    }

    /// Opens a waiver for `member`/`round`, has every member approve it and
    /// executes it.
    fn waive(&self, member: &Address, round: u32) -> u32 {
        let id = self
            .client
            .propose_penalty_waiver(&self.member(0), member, &round, &VOTING);
        for m in self.members.iter() {
            self.client.vote_on_proposal(&m, &id, &true);
        }
        self.set_time(self.now() + VOTING + 1);
        self.client.execute_proposal(&id);
        id
    }
}

#[test]
fn test_waive_pending_penalty() {
    let s = setup(100);
    let m1 = s.member(1);
    s.default_in_round_one();
    let reputation_before = s.client.get_reputation_score(&m1);

    // Within the grace window the penalty is only queued.
    s.client.penalise_defaulter(&m1);
    assert_eq!(s.balance(&m1), START_BALANCE - CONTRIBUTION);
    let pending = s.client.get_pending_penalties(&0);
    assert_eq!(pending.len(), 1);
    let (who, info) = pending.get(0).unwrap();
    assert_eq!(who, m1);

    let id = s.waive(&m1, info.round);
    assert_eq!(
        s.client.get_proposal(&id).unwrap().status,
        ProposalStatus::Executed
    );

    // Cleared before it was ever charged; score untouched.
    assert_eq!(s.client.get_pending_penalties(&0).len(), 0);
    assert_eq!(s.balance(&m1), START_BALANCE - CONTRIBUTION);
    assert_eq!(s.client.get_reputation_score(&m1), reputation_before);
    assert_eq!(s.client.get_member_status(&m1).default_count, 0);
    assert_eq!(s.client.get_collected_penalty(&m1, &info.round), None);

    // The waived penalty cannot be charged again.
    let res = s.client.try_penalise_defaulter(&m1);
    assert_eq!(res.unwrap_err().unwrap(), Error::NotADefaulter.into());
}

#[test]
fn test_waive_collected_penalty_refunds_from_same_pool() {
    let s = setup(0);
    let m1 = s.member(1);
    s.default_in_round_one();
    let reputation_before = s.client.get_reputation_score(&m1);
    let member_before = s.balance(&m1);
    let pool_before = s.balance(&s.contract_id);

    // Grace is zero, so the penalty is collected immediately.
    s.client.penalise_defaulter(&m1);
    assert_eq!(s.balance(&m1), member_before - PENALTY);
    assert_eq!(s.balance(&s.contract_id), pool_before + PENALTY);
    assert_eq!(s.client.get_member_status(&m1).default_count, 1);
    assert!(s.client.get_reputation_score(&m1) < reputation_before);

    let round = s.client.get_group_info().current_round;
    let record = s.client.get_collected_penalty(&m1, &round).unwrap();
    assert_eq!(record.amount, PENALTY);
    assert_eq!(record.token, s.token);
    assert!(!record.refunded);

    s.waive(&m1, round);

    // Refunded in the same token out of the contract pool it was paid into.
    assert_eq!(s.balance(&m1), member_before);
    assert_eq!(s.balance(&s.contract_id), pool_before);
    assert!(s.client.get_collected_penalty(&m1, &round).unwrap().refunded);

    // The waived penalty leaves no mark on the member's score.
    assert_eq!(s.client.get_reputation_score(&m1), reputation_before);
    assert_eq!(s.client.get_member_status(&m1).default_count, 0);
}

#[test]
fn test_only_existing_penalties_can_be_waived() {
    let s = setup(0);
    let m1 = s.member(1);

    // Nothing has been charged yet.
    let res = s
        .client
        .try_propose_penalty_waiver(&s.member(0), &m1, &0, &VOTING);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::PenaltyNotFound.into());

    // The generic entrypoint applies the same check.
    let res = s.client.try_create_proposal(
        &s.member(0),
        &ProposalType::WaivePenalty,
        &String::from_str(&s.env, "waive"),
        &m1,
        &VOTING,
        &Some(0),
    );
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::PenaltyNotFound.into());

    s.default_in_round_one();
    s.client.penalise_defaulter(&m1);
    let round = s.client.get_group_info().current_round;

    // Wrong round or wrong member.
    let res = s
        .client
        .try_propose_penalty_waiver(&s.member(0), &m1, &(round + 1), &VOTING);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::PenaltyNotFound.into());
    let res = s
        .client
        .try_propose_penalty_waiver(&s.member(0), &s.member(0), &round, &VOTING);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::PenaltyNotFound.into());

    // A refunded penalty cannot be waived twice.
    s.waive(&m1, round);
    let res = s
        .client
        .try_propose_penalty_waiver(&s.member(0), &m1, &round, &VOTING);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::PenaltyNotFound.into());
}

#[test]
fn test_refund_limited_to_free_pool_balance() {
    let s = setup(0);
    let m1 = s.member(1);
    s.default_in_round_one();
    s.client.penalise_defaulter(&m1);
    let round = s.client.get_group_info().current_round;

    let id = s
        .client
        .propose_penalty_waiver(&s.member(0), &m1, &round, &VOTING);
    for m in s.members.iter() {
        s.client.vote_on_proposal(&m, &id, &true);
    }
    s.set_time(s.now() + VOTING + 1);

    // Earmark the whole balance for the reward pool: the penalty pool has
    // nothing free to refund from.
    let earmarked = s.balance(&s.contract_id);
    s.env.as_contract(&s.contract_id, || {
        s.env
            .storage()
            .instance()
            .set(&DataKey::RewardPool, &earmarked);
    });

    let res = s.client.try_execute_proposal(&id);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::PenaltyRefundUnavailable.into());
    assert!(!s.client.get_collected_penalty(&m1, &round).unwrap().refunded);
}
