#![cfg(test)]
use super::*;
use crate::errors::{Error, ExtError2, ExtError3};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env, Vec,
};

const CONTRIBUTION: i128 = 100;
const ROUND_SECONDS: u64 = 3_600;
const START_BALANCE: i128 = 1_000;
const PENALTY: i128 = 50;
const GRACE: u32 = 100;
const MAX_EXTENSION: u64 = 7_200;

struct Setup<'a> {
    env: Env,
    client: AhjoorContractClient<'a>,
    admin: Address,
    token: Address,
    members: Vec<Address>,
}

fn setup<'a>(max_extension: u64) -> Setup<'a> {
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
            grace_period_ledgers: GRACE,
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

    if max_extension > 0 {
        client.set_max_round_extension(&admin, &max_extension);
    }

    Setup { env, client, admin, token, members }
}

impl<'a> Setup<'a> {
    fn member(&self, i: u32) -> Address {
        self.members.get(i).unwrap()
    }

    fn balance(&self, who: &Address) -> i128 {
        token::Client::new(&self.env, &self.token).balance(who)
    }

    fn set_time(&self, ts: u64) {
        self.env.ledger().set_timestamp(ts);
    }

    fn deadline(&self) -> u64 {
        self.client.get_group_info().round_deadline
    }

    fn vote_all(&self, proposal_id: u32) {
        for m in self.members.iter() {
            self.client.vote_on_proposal(&m, &proposal_id, &true);
        }
    }

    fn full_round(&self) {
        for m in self.members.iter() {
            self.client.contribute(&m, &self.token, &CONTRIBUTION);
        }
    }

    /// Opens, approves and executes an extension; voting closes after
    /// `voting` seconds from the current time.
    fn extend(&self, extra: u64, voting: u64) -> u32 {
        let now = self.env.ledger().timestamp();
        let id = self
            .client
            .propose_round_extension(&self.member(0), &extra, &voting);
        self.vote_all(id);
        self.set_time(now + voting + 1);
        self.client.execute_proposal(&id);
        id
    }
}

#[test]
fn test_extension_moves_current_deadline() {
    let s = setup(MAX_EXTENSION);
    assert_eq!(s.client.get_max_round_extension(), MAX_EXTENSION);
    assert_eq!(s.deadline(), ROUND_SECONDS);

    let id = s.extend(1_800, 600);

    assert_eq!(s.deadline(), ROUND_SECONDS + 1_800);
    assert_eq!(s.client.get_last_extended_cycle(), Some(0));
    let proposal = s.client.get_proposal(&id).unwrap();
    assert_eq!(proposal.proposal_type, ProposalType::ExtendRound);
    assert_eq!(proposal.status, ProposalStatus::Executed);
}

#[test]
fn test_extension_disabled_by_default() {
    let s = setup(0);
    let res = s
        .client
        .try_propose_round_extension(&s.member(0), &600, &600);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::RoundExtensionDisabled.into());
}

#[test]
fn test_extension_bounded_by_group_maximum() {
    let s = setup(MAX_EXTENSION);
    let res = s
        .client
        .try_propose_round_extension(&s.member(0), &(MAX_EXTENSION + 1), &600);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::InvalidRoundExtension.into());

    let res = s.client.try_propose_round_extension(&s.member(0), &0, &600);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::InvalidRoundExtension.into());

    // The generic entrypoint applies the same bound.
    let res = s.client.try_create_proposal(
        &s.member(0),
        &ProposalType::ExtendRound,
        &String::from_str(&s.env, "too long"),
        &s.member(0),
        &600,
        &Some((MAX_EXTENSION + 1) as i128),
    );
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::InvalidRoundExtension.into());
}

#[test]
fn test_only_admin_or_round_config_co_admin_sets_maximum() {
    let s = setup(0);
    let outsider = Address::generate(&s.env);
    let res = s.client.try_set_max_round_extension(&outsider, &600);
    assert_eq!(res.unwrap_err().unwrap(), ExtError2::NotACoAdminRole.into());

    let co_admin = Address::generate(&s.env);
    let mut perms = Vec::new(&s.env);
    perms.push_back(CoAdminPermission::ManageRoundConfig);
    s.client.add_co_admin(&s.admin, &co_admin, &perms);
    s.client.set_max_round_extension(&co_admin, &600);
    assert_eq!(s.client.get_max_round_extension(), 600);
}

#[test]
fn test_extension_rejected_after_deadline_passed() {
    let s = setup(MAX_EXTENSION);

    // Opening a proposal once the deadline has passed is rejected.
    s.set_time(ROUND_SECONDS + 1);
    let res = s
        .client
        .try_propose_round_extension(&s.member(0), &600, &60);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::RoundDeadlineAlreadyPassed.into());
}

#[test]
fn test_extension_execution_rejected_after_deadline_passed() {
    let s = setup(MAX_EXTENSION);

    // Proposal opened in time, but voting ends after the round deadline.
    s.set_time(ROUND_SECONDS - 100);
    let id = s
        .client
        .propose_round_extension(&s.member(0), &600, &200);
    s.vote_all(id);
    s.set_time(ROUND_SECONDS + 101);

    let res = s.client.try_execute_proposal(&id);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::RoundDeadlineAlreadyPassed.into());
    assert_eq!(s.deadline(), ROUND_SECONDS);
}

#[test]
fn test_second_extension_in_same_cycle_rejected() {
    let s = setup(MAX_EXTENSION);

    // Two proposals opened before either executes: only the first applies.
    let first = s
        .client
        .propose_round_extension(&s.member(0), &600, &100);
    let second = s
        .client
        .propose_round_extension(&s.member(1), &600, &100);
    s.vote_all(first);
    s.vote_all(second);
    s.set_time(101);
    s.client.execute_proposal(&first);
    assert_eq!(s.deadline(), ROUND_SECONDS + 600);

    let res = s.client.try_execute_proposal(&second);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::RoundAlreadyExtended.into());
    assert_eq!(s.deadline(), ROUND_SECONDS + 600);

    // A new proposal in the same cycle is rejected up front.
    let res = s
        .client
        .try_propose_round_extension(&s.member(0), &600, &100);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::RoundAlreadyExtended.into());

    // Round 1 is still in cycle 0 (two members → two rounds per cycle).
    s.full_round();
    let res = s
        .client
        .try_propose_round_extension(&s.member(0), &600, &100);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::RoundAlreadyExtended.into());

    // Round 2 starts cycle 1, where a new extension is allowed.
    s.full_round();
    let id = s
        .client
        .propose_round_extension(&s.member(0), &600, &100);
    assert_eq!(
        s.client.get_proposal(&id).unwrap().proposal_type,
        ProposalType::ExtendRound
    );
}

#[test]
fn test_late_contribution_uses_extended_deadline() {
    // Without an extension the window closes at the original deadline.
    let control = setup(MAX_EXTENSION);
    control.set_time(ROUND_SECONDS + 400);
    let res = control
        .client
        .try_contribute(&control.member(0), &control.token, &CONTRIBUTION);
    assert_eq!(res.unwrap_err().unwrap(), Error::ContributionWindowClosed.into());

    let s = setup(MAX_EXTENSION);
    s.extend(1_800, 600);

    // Past the original deadline, before the extended one: on time.
    s.set_time(ROUND_SECONDS + 400);
    let m0 = s.member(0);
    s.client.contribute(&m0, &s.token, &CONTRIBUTION);
    assert_eq!(s.client.get_late_contribution_counts().get(m0.clone()), None);
    assert_eq!(s.client.get_reputation_score(&m0), 10);
}

#[test]
fn test_late_penalty_uses_extended_deadline() {
    let s = setup(MAX_EXTENSION);
    s.extend(1_800, 600);
    let extended = ROUND_SECONDS + 1_800;

    let m0 = s.member(0);
    let m1 = s.member(1);
    s.client.contribute(&m0, &s.token, &CONTRIBUTION);

    // The round cannot be closed at the original deadline any more.
    s.set_time(ROUND_SECONDS + GRACE as u64 + 1);
    let res = s.client.try_close_round();
    assert_eq!(res.unwrap_err().unwrap(), Error::DeadlineNotPassed.into());

    // Just past the extended deadline the penalty is still inside the grace
    // window, which now runs from the extended deadline: it is only queued.
    s.set_time(extended + 1);
    s.client.close_round();
    s.client.penalise_defaulter(&m1);
    assert_eq!(s.balance(&m1), START_BALANCE);
    let pending = s.client.get_pending_penalties(&0);
    assert_eq!(pending.len(), 1);
    let (who, info) = pending.get(0).unwrap();
    assert_eq!(who, m1);
    assert_eq!(info.grace_expires_at, extended + GRACE as u64);

    // Once the extended grace window ends the penalty is charged.
    s.set_time(extended + GRACE as u64 + 1);
    s.client.penalise_defaulter(&m1);
    assert!(s.balance(&m1) < START_BALANCE);
}
