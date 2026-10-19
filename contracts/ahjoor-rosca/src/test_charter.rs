#![cfg(test)]
use super::*;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String, Vec,
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
    env.ledger().set_timestamp(100);
    (env, client, admin, members, token_addr)
}

fn charter_hash(env: &Env, byte: u8) -> BytesN<32> {
    BytesN::from_array(env, &[byte; 32])
}

fn uri(env: &Env) -> String {
    String::from_str(env, "ipfs://charter")
}

#[test]
fn test_group_without_charter_behaves_as_today() {
    let (env, client, _admin, members, token) = setup_group();
    let m0 = members.get(0).unwrap();

    assert!(client.get_group_charter().is_none());
    assert!(client.has_acknowledged_charter(&m0));
    let newcomer = Address::generate(&env);
    client.add_member(&newcomer);
    assert!(client.get_group_info().members.contains(&newcomer));

    client.contribute(&m0, &token, &100);
}

#[test]
fn test_set_charter_before_activation_applies_immediately() {
    let (env, client, admin, members, _token) = setup_group();

    let res = client.set_group_charter(&admin, &charter_hash(&env, 1), &uri(&env));
    assert!(res.is_none());

    let charter = client.get_group_charter().unwrap();
    assert_eq!(charter.version, 1);
    assert_eq!(charter.charter_hash, charter_hash(&env, 1));
    assert!(!client.has_acknowledged_charter(&members.get(0).unwrap()));

    // Re-setting before activation bumps the version directly.
    client.set_group_charter(&admin, &charter_hash(&env, 2), &uri(&env));
    assert_eq!(client.get_group_charter().unwrap().version, 2);
}

#[test]
fn test_acknowledge_charter() {
    let (env, client, admin, members, _token) = setup_group();
    let m0 = members.get(0).unwrap();
    client.set_group_charter(&admin, &charter_hash(&env, 1), &uri(&env));

    client.acknowledge_charter(&m0, &1);
    assert!(client.has_acknowledged_charter(&m0));
    assert_eq!(client.get_acknowledged_charter_version(&m0), 1);
}

#[test]
fn test_acknowledge_wrong_version_rejected() {
    let (env, client, admin, members, _token) = setup_group();
    let m0 = members.get(0).unwrap();
    client.set_group_charter(&admin, &charter_hash(&env, 1), &uri(&env));

    let res = client.try_acknowledge_charter(&m0, &2);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError2::CharterVersionMismatch.into()
    );
}

#[test]
fn test_acknowledge_without_charter_rejected() {
    let (_env, client, _admin, members, _token) = setup_group();
    let res = client.try_acknowledge_charter(&members.get(0).unwrap(), &1);
    assert_eq!(res.unwrap_err().unwrap(), ExtError2::CharterNotSet.into());
}

#[test]
fn test_contribute_requires_acknowledgement() {
    let (env, client, admin, members, token) = setup_group();
    let m0 = members.get(0).unwrap();
    client.set_group_charter(&admin, &charter_hash(&env, 1), &uri(&env));

    let res = client.try_contribute(&m0, &token, &100);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError2::CharterNotAcknowledged.into()
    );

    client.acknowledge_charter(&m0, &1);
    client.contribute(&m0, &token, &100);
}

#[test]
fn test_join_with_invite_requires_acknowledgement() {
    let (env, client, admin, _members, _token) = setup_group();
    client.set_group_charter(&admin, &charter_hash(&env, 1), &uri(&env));

    let newcomer = Address::generate(&env);
    client.generate_invite(&newcomer);
    let res = client.try_join_with_invite(&newcomer);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError2::CharterNotAcknowledged.into()
    );

    client.acknowledge_charter(&newcomer, &1);
    client.join_with_invite(&newcomer);
    assert!(client.get_group_info().members.contains(&newcomer));
}

#[test]
fn test_add_member_requires_acknowledgement() {
    let (env, client, admin, _members, _token) = setup_group();
    client.set_group_charter(&admin, &charter_hash(&env, 1), &uri(&env));

    let newcomer = Address::generate(&env);
    let res = client.try_add_member(&newcomer);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError2::CharterNotAcknowledged.into()
    );

    client.acknowledge_charter(&newcomer, &1);
    client.add_member(&newcomer);
    assert!(client.get_group_info().members.contains(&newcomer));
}

#[test]
fn test_join_group_tiered_requires_acknowledgement() {
    let (env, client, admin, members, _token) = setup_group();
    let m0 = members.get(0).unwrap();
    env.as_contract(&client.address, || {
        let mut tiers: Vec<Tier> = Vec::new(&env);
        tiers.push_back(Tier {
            name: soroban_sdk::Symbol::new(&env, "basic"),
            contribution_amount: 100,
            payout_weight: 1,
        });
        env.storage().instance().set(&DataKey3::GroupTiers, &tiers);
    });
    client.set_group_charter(&admin, &charter_hash(&env, 1), &uri(&env));

    let res = client.try_join_group_tiered(&m0, &0);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError2::CharterNotAcknowledged.into()
    );

    client.acknowledge_charter(&m0, &1);
    client.join_group_tiered(&m0, &0);
}

#[test]
fn test_version_bump_after_activation_requires_governance_and_reack() {
    let (env, client, admin, members, token) = setup_group();
    let m0 = members.get(0).unwrap();
    client.set_group_charter(&admin, &charter_hash(&env, 1), &uri(&env));
    for m in members.iter() {
        client.acknowledge_charter(&m, &1);
    }

    // First contribution activates the group.
    client.contribute(&m0, &token, &100);

    // A change after activation opens a governance proposal instead.
    let proposal_id = client
        .set_group_charter(&admin, &charter_hash(&env, 2), &uri(&env))
        .unwrap();
    assert_eq!(client.get_group_charter().unwrap().version, 1);
    let proposal = client.get_proposal(&proposal_id).unwrap();
    assert_eq!(proposal.proposal_type, ProposalType::CharterUpdate);

    for m in members.iter() {
        client.vote_on_proposal(&m, &proposal_id, &true);
    }
    env.ledger()
        .set_timestamp(100 + charter::CHARTER_VOTING_WINDOW_SECONDS + 1);
    client.execute_proposal(&proposal_id);

    let charter = client.get_group_charter().unwrap();
    assert_eq!(charter.version, 2);
    assert_eq!(charter.charter_hash, charter_hash(&env, 2));
    assert!(!client.has_acknowledged_charter(&m0));

    // Start a fresh round so the contribution window is open again.
    client.finalize_round();

    let res = client.try_contribute(&m0, &token, &100);
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError2::CharterNotAcknowledged.into()
    );

    client.acknowledge_charter(&m0, &2);
    client.contribute(&m0, &token, &100);
}
