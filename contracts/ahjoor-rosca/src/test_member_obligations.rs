#![cfg(test)]
use super::*;
use crate::errors::ExtError3;
use soroban_sdk::{testutils::Address as _, token, Address, BytesN, Env, Vec};

const START_BALANCE: i128 = 10_000;

fn config() -> RoscaConfig {
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
    }
}

struct Group<'a> {
    id: Address,
    client: AhjoorContractClient<'a>,
    admin: Address,
    token: Address,
}

/// Registers a new group contract with its own token.
fn new_group<'a>(env: &'a Env, members: &Vec<Address>, contribution: i128, round_seconds: u64) -> Group<'a> {
    let id = env.register(AhjoorContract, ());
    let client = AhjoorContractClient::new(env, &id);
    let admin = Address::generate(env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = token::StellarAssetClient::new(env, &token);
    for m in members.iter() {
        token_admin.mint(&m, &START_BALANCE);
    }
    client.init(&admin, members, &contribution, &token, &round_seconds, &config(), &None);
    Group { id, client, admin, token }
}

fn two_members(env: &Env) -> (Address, Address, Vec<Address>) {
    let member = Address::generate(env);
    let other = Address::generate(env);
    let mut members = Vec::new(env);
    members.push_back(member.clone());
    members.push_back(other.clone());
    (member, other, members)
}

#[test]
fn test_obligations_across_groups_with_different_tokens() {
    let env = Env::default();
    env.mock_all_auths();
    let (member, _other, members) = two_members(&env);

    let a = new_group(&env, &members, 100, 3_600);
    let b = new_group(&env, &members, 250, 7_200);
    assert_ne!(a.token, b.token);

    a.client.link_member_group(&member, &b.id);
    assert_eq!(a.client.get_member_group_index(&member).len(), 1);

    let obligations = a.client.get_member_obligations(&member);
    assert_eq!(obligations.len(), 2);

    let own = obligations.get(0).unwrap();
    assert_eq!(own.group_id, a.id);
    assert_eq!(own.token, a.token);
    assert_eq!(own.amount_due, 100);
    assert_eq!(own.deadline, 3_600);

    let linked = obligations.get(1).unwrap();
    assert_eq!(linked.group_id, b.id);
    assert_eq!(linked.token, b.token);
    assert_eq!(linked.amount_due, 250);
    assert_eq!(linked.deadline, 7_200);

    // A partial contribution lowers the amount due in that group only.
    a.client.contribute(&member, &a.token, &40);
    let obligations = a.client.get_member_obligations(&member);
    assert_eq!(obligations.get(0).unwrap().amount_due, 60);
    assert_eq!(obligations.get(1).unwrap().amount_due, 250);

    // Paying a group in full removes it from the view.
    b.client.contribute(&member, &b.token, &250);
    let obligations = a.client.get_member_obligations(&member);
    assert_eq!(obligations.len(), 1);
    assert_eq!(obligations.get(0).unwrap().group_id, a.id);
}

#[test]
fn test_exited_and_completed_groups_are_excluded() {
    let env = Env::default();
    env.mock_all_auths();
    let (member, other, members) = two_members(&env);

    let a = new_group(&env, &members, 100, 3_600);
    let b = new_group(&env, &members, 200, 3_600);
    let c = new_group(&env, &members, 300, 3_600);

    a.client.link_member_group(&member, &b.id);
    a.client.link_member_group(&member, &c.id);
    assert_eq!(a.client.get_member_obligations(&member).len(), 3);

    // Member exits group B.
    b.client.request_emergency_exit(&member);
    b.client.approve_exit(&member);
    assert!(!b.client.is_active_group_member(&member));
    assert_eq!(b.client.get_group_obligation(&member), None);

    // Group C is dissolved (completed).
    c.client.contribute(&other, &c.token, &300);
    c.client
        .dissolve_group(&c.admin, &BytesN::from_array(&env, &[7u8; 32]));
    assert_eq!(c.client.get_group_status(), GroupStatus::Dissolved);
    assert_eq!(c.client.get_group_obligation(&member), None);

    let obligations = a.client.get_member_obligations(&member);
    assert_eq!(obligations.len(), 1);
    let own = obligations.get(0).unwrap();
    assert_eq!(own.group_id, a.id);
    assert_eq!(own.amount_due, 100);

    // The index itself is kept; exclusion happens when it is read.
    assert_eq!(a.client.get_member_group_index(&member).len(), 2);
}

#[test]
fn test_member_with_no_outstanding_obligations_returns_empty() {
    let env = Env::default();
    env.mock_all_auths();
    let (member, _other, members) = two_members(&env);
    let a = new_group(&env, &members, 100, 3_600);

    a.client.contribute(&member, &a.token, &100);
    assert_eq!(a.client.get_member_obligations(&member).len(), 0);

    let stranger = Address::generate(&env);
    assert_eq!(a.client.get_member_obligations(&stranger).len(), 0);
}

#[test]
fn test_link_validation_and_unlink() {
    let env = Env::default();
    env.mock_all_auths();
    let (member, _other, members) = two_members(&env);
    let a = new_group(&env, &members, 100, 3_600);
    let b = new_group(&env, &members, 100, 3_600);

    // A group cannot be linked to itself.
    let res = a.client.try_link_member_group(&member, &a.id);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::InvalidLinkedGroup.into());

    // The member must be active in the linked group.
    let mut outsiders = Vec::new(&env);
    outsiders.push_back(Address::generate(&env));
    outsiders.push_back(Address::generate(&env));
    let foreign = new_group(&env, &outsiders, 100, 3_600);
    let res = a.client.try_link_member_group(&member, &foreign.id);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::NotActiveInLinkedGroup.into());

    // Linking twice keeps a single entry.
    a.client.link_member_group(&member, &b.id);
    a.client.link_member_group(&member, &b.id);
    assert_eq!(a.client.get_member_group_index(&member).len(), 1);

    a.client.unlink_member_group(&member, &b.id);
    assert_eq!(a.client.get_member_group_index(&member).len(), 0);
    assert_eq!(a.client.get_member_obligations(&member).len(), 1);
}
