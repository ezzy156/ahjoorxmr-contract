#![cfg(test)]
use super::*;
use crate::errors::{ExtError2, ExtError3};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger},
    token, Address, BytesN, Env, IntoVal, Symbol, Vec,
};

const CONTRIBUTION: i128 = 100;
const MEMBERS: u32 = 3;
const ROUND_SECONDS: u64 = 3_600;
const START_BALANCE: i128 = 1_000;

struct Setup<'a> {
    env: Env,
    client: AhjoorContractClient<'a>,
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
            max_members: Some(10),
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
        members,
    }
}

fn reason(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[7u8; 32])
}

fn has_event(env: &Env, topic: &str) -> bool {
    let expected = (Symbol::new(env, topic),).into_val(env);
    env.events().all().iter().any(|e| e.1 == expected)
}

/// An outsider who has been banned.
fn banned_outsider(s: &Setup) -> Address {
    let outsider = Address::generate(&s.env);
    s.client.ban_member(&0, &outsider, &reason(&s.env));
    outsider
}

#[test]
fn test_ban_and_unban_emit_events() {
    let s = setup();
    let outsider = Address::generate(&s.env);
    assert!(!s.client.is_banned(&0, &outsider));

    s.client.ban_member(&0, &outsider, &reason(&s.env));
    assert!(has_event(&s.env, "member_banned"));
    assert!(s.client.is_banned(&0, &outsider));

    let res = s.client.try_ban_member(&0, &outsider, &reason(&s.env));
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::AlreadyBanned.into());

    s.client.unban_member(&0, &outsider);
    assert!(has_event(&s.env, "member_unbanned"));
    assert!(!s.client.is_banned(&0, &outsider));

    let res = s.client.try_unban_member(&0, &outsider);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::NotBanned.into());
}

#[test]
fn test_active_member_must_be_removed_first() {
    let s = setup();
    let m0 = s.members.get(0).unwrap();

    let res = s.client.try_ban_member(&0, &m0, &reason(&s.env));
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError3::MemberStillActive.into()
    );

    s.client.remove_member(&m0);
    s.client.ban_member(&0, &m0, &reason(&s.env));
    assert!(s.client.is_banned(&0, &m0));

    // The removed member cannot be re-added.
    let res = s.client.try_add_member(&m0);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::MemberBanned.into());
}

#[test]
fn test_add_member_rejects_banned() {
    let s = setup();
    let outsider = banned_outsider(&s);
    let res = s.client.try_add_member(&outsider);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::MemberBanned.into());

    s.client.unban_member(&0, &outsider);
    s.client.add_member(&outsider);
    assert!(s.client.get_group_info().members.contains(&outsider));
}

#[test]
fn test_invite_paths_reject_banned() {
    let s = setup();
    let outsider = banned_outsider(&s);
    let res = s.client.try_generate_invite(&outsider);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::MemberBanned.into());

    // An invite issued before the ban is revoked by it.
    let invitee = Address::generate(&s.env);
    s.client.generate_invite(&invitee);
    s.client.ban_member(&0, &invitee, &reason(&s.env));
    assert!(s.client.try_join_with_invite(&invitee).is_err());
    assert!(!s.client.get_group_info().members.contains(&invitee));
}

#[test]
fn test_waitlist_rejects_banned_and_ban_clears_entry() {
    let s = setup();
    let outsider = banned_outsider(&s);
    let res = s.client.try_join_waitlist(&outsider);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::MemberBanned.into());

    let waiting = Address::generate(&s.env);
    s.client.join_waitlist(&waiting);
    assert_eq!(s.client.get_waitlist().len(), 1);
    s.client.ban_member(&0, &waiting, &reason(&s.env));
    assert_eq!(s.client.get_waitlist().len(), 0);
}

#[test]
fn test_successor_paths_reject_banned() {
    let s = setup();
    let m0 = s.members.get(0).unwrap();
    let outsider = banned_outsider(&s);
    let res = s.client.try_designate_successor(&m0, &outsider);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::MemberBanned.into());

    // A designation made before the ban cannot be accepted.
    let successor = Address::generate(&s.env);
    s.client.designate_successor(&m0, &successor);
    s.client.ban_member(&0, &successor, &reason(&s.env));
    let res = s.client.try_accept_succession(&successor, &m0);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::MemberBanned.into());
    // Unrelated validation still runs first for non-banned successors.
    let res = s.client.try_designate_successor(&m0, &m0);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError2::InvalidSuccessor.into()
    );
}

#[test]
fn test_governance_vote_bans_address() {
    let s = setup();
    let outsider = Address::generate(&s.env);
    let creator = s.members.get(0).unwrap();
    s.env.ledger().set_timestamp(1_000);

    s.client.create_proposal(
        &creator,
        &ProposalType::MemberBan,
        &soroban_sdk::String::from_str(&s.env, "ban"),
        &outsider,
        &100,
        &None,
    );
    for m in s.members.iter() {
        s.client.vote_on_proposal(&m, &0, &true);
    }
    s.env.ledger().set_timestamp(1_200);
    s.client.execute_proposal(&0);

    assert!(s.client.is_banned(&0, &outsider));
    let res = s.client.try_join_waitlist(&outsider);
    assert_eq!(res.unwrap_err().unwrap(), ExtError3::MemberBanned.into());
}
