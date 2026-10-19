#![cfg(test)]
use super::*;
use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, Vec,
};

struct Setup<'a> {
    env: Env,
    client: AhjoorEscrowContractClient<'a>,
    token_addr: Address,
    token_client: TokenClient<'a>,
    token_admin: TokenAdminClient<'a>,
}

fn setup<'a>() -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AhjoorEscrowContract, ());
    let client = AhjoorEscrowContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_addr);
    let token_admin = TokenAdminClient::new(&env, &token_addr);

    client.initialize(&admin);
    client.add_allowed_token(&admin, &token_addr);

    Setup {
        env,
        client,
        token_addr,
        token_client,
        token_admin,
    }
}

fn features(
    env: &Env,
    threshold: Option<i128>,
    parent: Option<u32>,
    bond: Option<i128>,
    deadline_ledgers: Option<u32>,
) -> EscrowFeatureOpts {
    let _ = env;
    EscrowFeatureOpts {
        min_funding_threshold: threshold,
        parent_escrow_id: parent,
        creation_bond: bond,
        funding_deadline_ledgers: deadline_ledgers,
    }
}

// ===========================================================================
//  #800 — min funding threshold / work authorization
// ===========================================================================

#[test]
fn test_no_threshold_work_authorized_immediately() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &1_000);

    let deadline = s.env.ledger().timestamp() + 1_000;
    let id = s.client.create_escrow(
        &buyer,
        &seller,
        &arbiter,
        &250,
        &s.token_addr,
        &deadline,
        &None,
        &Vec::new(&s.env),
        &false,
        &0u32,
    );

    assert!(s.client.get_work_authorized(&id));
}

#[test]
fn test_incremental_deposits_cross_threshold_once() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &2_000);

    let deadline = s.env.ledger().timestamp() + 1_000;
    let id = s.client.create_escrow_with_features(
        &buyer,
        &seller,
        &arbiter,
        &100,
        &s.token_addr,
        &deadline,
        &None,
        &features(&s.env, Some(250), None, None, None),
    );

    assert!(!s.client.get_work_authorized(&id));

    s.client.top_up_escrow(&buyer, &id, &100);
    assert!(!s.client.get_work_authorized(&id));

    s.client.top_up_escrow(&buyer, &id, &50);
    assert!(s.client.get_work_authorized(&id));

    // Further top-ups must not flip authorization again (already true).
    s.client.top_up_escrow(&buyer, &id, &50);
    assert!(s.client.get_work_authorized(&id));
}

#[test]
fn test_threshold_never_met() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &2_000);

    let deadline = s.env.ledger().timestamp() + 1_000;
    let id = s.client.create_escrow_with_features(
        &buyer,
        &seller,
        &arbiter,
        &50,
        &s.token_addr,
        &deadline,
        &None,
        &features(&s.env, Some(500), None, None, None),
    );

    s.client.top_up_escrow(&buyer, &id, &100);
    assert!(!s.client.get_work_authorized(&id));
}

// ===========================================================================
//  #799 — parent/child project bundling
// ===========================================================================

#[test]
fn test_child_escrows_register_under_parent() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &5_000);

    let deadline = s.env.ledger().timestamp() + 1_000;
    let parent = s.client.create_escrow(
        &buyer,
        &seller,
        &arbiter,
        &100,
        &s.token_addr,
        &deadline,
        &None,
        &Vec::new(&s.env),
        &false,
        &0u32,
    );

    let child1 = s.client.create_escrow_with_features(
        &buyer,
        &seller,
        &arbiter,
        &200,
        &s.token_addr,
        &deadline,
        &None,
        &features(&s.env, None, Some(parent), None, None),
    );
    let child2 = s.client.create_escrow_with_features(
        &buyer,
        &seller,
        &arbiter,
        &300,
        &s.token_addr,
        &deadline,
        &None,
        &features(&s.env, None, Some(parent), None, None),
    );

    let children = s.client.get_child_escrows(&parent, &0, &10);
    assert_eq!(children.len(), 2);
    assert_eq!(children.get(0).unwrap(), child1);
    assert_eq!(children.get(1).unwrap(), child2);
    assert_eq!(s.client.get_project_status(&parent), ProjectStatus::InProgress);
}

#[test]
fn test_child_escrows_pagination() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &5_000);

    let deadline = s.env.ledger().timestamp() + 1_000;
    let parent = s.client.create_escrow(
        &buyer,
        &seller,
        &arbiter,
        &100,
        &s.token_addr,
        &deadline,
        &None,
        &Vec::new(&s.env),
        &false,
        &0u32,
    );

    let mut ids = Vec::new(&s.env);
    for _ in 0..3 {
        let child = s.client.create_escrow_with_features(
            &buyer,
            &seller,
            &arbiter,
            &100,
            &s.token_addr,
            &deadline,
            &None,
            &features(&s.env, None, Some(parent), None, None),
        );
        ids.push_back(child);
    }

    let first = s.client.get_child_escrows(&parent, &0, &2);
    assert_eq!(first.len(), 2);
    assert_eq!(first.get(0).unwrap(), ids.get(0).unwrap());
    assert_eq!(first.get(1).unwrap(), ids.get(1).unwrap());

    let second = s.client.get_child_escrows(&parent, &2, &2);
    assert_eq!(second.len(), 1);
    assert_eq!(second.get(0).unwrap(), ids.get(2).unwrap());

    assert_eq!(s.client.get_child_escrows(&parent, &3, &2).len(), 0);
    assert_eq!(s.client.get_child_escrows(&parent, &0, &0).len(), 0);
    assert_eq!(s.client.get_child_escrows(&parent, &1, &u32::MAX).len(), 2);
    assert_eq!(s.client.get_child_escrows(&999, &0, &10).len(), 0);
}

#[test]
fn test_project_status_all_released_and_dispute() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &5_000);

    let deadline = s.env.ledger().timestamp() + 1_000;
    let parent = s.client.create_escrow(
        &buyer,
        &seller,
        &arbiter,
        &50,
        &s.token_addr,
        &deadline,
        &None,
        &Vec::new(&s.env),
        &false,
        &0u32,
    );

    let child1 = s.client.create_escrow_with_features(
        &buyer,
        &seller,
        &arbiter,
        &100,
        &s.token_addr,
        &deadline,
        &None,
        &features(&s.env, None, Some(parent), None, None),
    );
    let child2 = s.client.create_escrow_with_features(
        &buyer,
        &seller,
        &arbiter,
        &100,
        &s.token_addr,
        &deadline,
        &None,
        &features(&s.env, None, Some(parent), None, None),
    );

    s.client.release_escrow(&buyer, &child1);
    assert_eq!(s.client.get_project_status(&parent), ProjectStatus::InProgress);

    s.client.release_escrow(&buyer, &child2);
    assert_eq!(s.client.get_project_status(&parent), ProjectStatus::AllReleased);

    // New disputed child surfaces HasDispute
    let child3 = s.client.create_escrow_with_features(
        &buyer,
        &seller,
        &arbiter,
        &100,
        &s.token_addr,
        &deadline,
        &None,
        &features(&s.env, None, Some(parent), None, None),
    );
    s.client.dispute_escrow(
        &buyer,
        &child3,
        &soroban_sdk::String::from_str(&s.env, "bad"),
        &100,
    );
    assert_eq!(s.client.get_project_status(&parent), ProjectStatus::HasDispute);
}

#[test]
fn test_child_lifecycle_unaffected_by_parent_link() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &2_000);

    let deadline = s.env.ledger().timestamp() + 1_000;
    let parent = s.client.create_escrow(
        &buyer,
        &seller,
        &arbiter,
        &50,
        &s.token_addr,
        &deadline,
        &None,
        &Vec::new(&s.env),
        &false,
        &0u32,
    );
    let child = s.client.create_escrow_with_features(
        &buyer,
        &seller,
        &arbiter,
        &250,
        &s.token_addr,
        &deadline,
        &None,
        &features(&s.env, None, Some(parent), None, None),
    );

    let seller_before = s.token_client.balance(&seller);
    s.client.release_escrow(&buyer, &child);
    assert_eq!(s.client.get_escrow(&child).status, EscrowStatus::Released);
    assert_eq!(s.token_client.balance(&seller), seller_before + 250);
}

// ===========================================================================
//  #797 — creation bond / abandonment
// ===========================================================================

#[test]
fn test_bond_refunded_on_release() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &5_000);

    let deadline = s.env.ledger().timestamp() + 1_000;
    let id = s.client.create_escrow_with_features(
        &buyer,
        &seller,
        &arbiter,
        &500,
        &s.token_addr,
        &deadline,
        &None,
        &features(&s.env, None, None, Some(50), Some(100)),
    );

    // Bond held, principal deferred
    assert_eq!(s.client.get_escrow(&id).amount, 0);
    assert_eq!(s.token_client.balance(&s.client.address), 50);

    s.client.fund_escrow(&buyer, &id, &500);
    assert_eq!(s.client.get_escrow(&id).amount, 500);

    let buyer_before = s.token_client.balance(&buyer);
    s.client.release_escrow(&buyer, &id);
    // Seller gets principal; buyer gets bond back
    assert_eq!(s.token_client.balance(&seller), 500);
    assert_eq!(s.token_client.balance(&buyer), buyer_before + 50);
    assert_eq!(s.client.get_escrow(&id).status, EscrowStatus::Released);
}

#[test]
fn test_bond_refunded_on_mutual_cancellation() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &5_000);

    let deadline = s.env.ledger().timestamp() + 1_000;
    let id = s.client.create_escrow_with_features(
        &buyer,
        &seller,
        &arbiter,
        &400,
        &s.token_addr,
        &deadline,
        &None,
        &features(&s.env, None, None, Some(40), Some(50)),
    );
    s.client.fund_escrow(&buyer, &id, &400);

    let buyer_before = s.token_client.balance(&buyer);
    s.client.request_cancellation(
        &seller,
        &id,
        &soroban_sdk::BytesN::from_array(&s.env, &[1u8; 32]),
    );
    s.client.accept_cancellation(&buyer, &id);

    // Principal + bond returned to buyer
    assert_eq!(s.token_client.balance(&buyer), buyer_before + 400 + 40);
    assert_eq!(s.client.get_escrow(&id).status, EscrowStatus::Refunded);
}

#[test]
fn test_abandonment_bond_forfeited_after_deadline() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    let relayer = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &5_000);

    let deadline = s.env.ledger().timestamp() + 1_000;
    let id = s.client.create_escrow_with_features(
        &buyer,
        &seller,
        &arbiter,
        &400,
        &s.token_addr,
        &deadline,
        &None,
        &features(&s.env, None, None, Some(75), Some(10)),
    );

    let start = s.env.ledger().sequence();
    // Premature claim must fail
    let early = s.client.try_claim_abandonment_bond(&id);
    assert!(early.is_err());

    s.env.ledger().set_sequence_number(start + 10);
    let seller_before = s.token_client.balance(&seller);
    // Anyone may claim
    let _ = relayer;
    s.client.claim_abandonment_bond(&id);

    assert_eq!(s.token_client.balance(&seller), seller_before + 75);
    assert_eq!(s.client.get_escrow(&id).status, EscrowStatus::Abandoned);
}

#[test]
fn test_abandonment_rejected_after_funding() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &5_000);

    let deadline = s.env.ledger().timestamp() + 1_000;
    let id = s.client.create_escrow_with_features(
        &buyer,
        &seller,
        &arbiter,
        &400,
        &s.token_addr,
        &deadline,
        &None,
        &features(&s.env, None, None, Some(25), Some(5)),
    );

    s.client.fund_escrow(&buyer, &id, &100);
    let start = s.env.ledger().sequence();
    s.env.ledger().set_sequence_number(start + 20);

    let result = s.client.try_claim_abandonment_bond(&id);
    assert!(result.is_err());
}
