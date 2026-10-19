//! Tests for create_multi_seller_escrow (#317) and delegate_escrow_share (#317).
//!
//! create_multi_seller_escrow splits escrow proceeds among several payees according
//! to BPS shares that must sum to exactly 10 000.  On release each seller receives
//! `(total * bps) / 10_000`; the primary seller (index 0) receives the remainder
//! to absorb integer-division dust.
//!
//! delegate_escrow_share redirects a specific seller's payout to a delegate address.
//! Delegation is recorded before release; on release the delegate receives the share
//! instead of the original seller.

#![cfg(test)]
use super::*;

use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, Vec,
};

// ---------------------------------------------------------------------------
//  Test helpers
// ---------------------------------------------------------------------------

struct Setup<'a> {
    env: Env,
    client: AhjoorEscrowContractClient<'a>,
    admin: Address,
    token_addr: Address,
    token_client: TokenClient<'a>,
    token_admin_client: TokenAdminClient<'a>,
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
    let token_admin_client = TokenAdminClient::new(&env, &token_addr);

    client.initialize(&admin);
    client.add_allowed_token(&admin, &token_addr);

    Setup {
        env,
        client,
        admin,
        token_addr,
        token_client,
        token_admin_client,
    }
}

/// Build a sellers Vec from a slice of `(Address, bps)` tuples.
fn sellers_vec(env: &Env, pairs: &[(Address, u32)]) -> Vec<(Address, u32)> {
    let mut v = Vec::new(env);
    for (addr, bps) in pairs {
        v.push_back((addr.clone(), *bps));
    }
    v
}

/// Create a multi-seller escrow with no collateral requirement (the common case).
fn create_no_collateral<'a>(
    s: &Setup<'a>,
    buyer: &Address,
    sellers: Vec<(Address, u32)>,
    amount: i128,
) -> u32 {
    s.token_admin_client.mint(buyer, &amount);
    let deadline = s.env.ledger().timestamp() + 10_000;
    s.client.create_multi_seller_escrow(
        buyer,
        &sellers,
        &Address::generate(&s.env), // arbiter
        &amount,
        &s.token_addr,
        &deadline,
        &None,
        &0u32,  // required_collateral_bps = 0 → Active immediately
        &0u32,  // collateral_forfeit_bps
        &0u64,  // collateral_deposit_window
    )
}

// ===========================================================================
//  create_multi_seller_escrow — fund distribution
// ===========================================================================

/// Two sellers with equal shares (50 / 50) each receive half the escrow amount.
#[test]
fn test_multi_seller_two_equal_shares_distribute_correctly() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller_a = Address::generate(&s.env);
    let seller_b = Address::generate(&s.env);

    let sellers = sellers_vec(&s.env, &[
        (seller_a.clone(), 5_000),
        (seller_b.clone(), 5_000),
    ]);
    let escrow_id = create_no_collateral(&s, &buyer, sellers, 1_000);

    s.client.release_escrow(&buyer, &escrow_id);

    let escrow = s.client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::Released);
    assert_eq!(s.token_client.balance(&seller_a), 500);
    assert_eq!(s.token_client.balance(&seller_b), 500);
}

/// Three sellers with asymmetric shares (60 / 30 / 10).
#[test]
fn test_multi_seller_three_asymmetric_shares_distribute_correctly() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller_a = Address::generate(&s.env); // 60 % → 600
    let seller_b = Address::generate(&s.env); // 30 % → 300
    let seller_c = Address::generate(&s.env); // 10 % → 100

    let sellers = sellers_vec(&s.env, &[
        (seller_a.clone(), 6_000),
        (seller_b.clone(), 3_000),
        (seller_c.clone(), 1_000),
    ]);
    let escrow_id = create_no_collateral(&s, &buyer, sellers, 1_000);

    s.client.release_escrow(&buyer, &escrow_id);

    let escrow = s.client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::Released);
    assert_eq!(s.token_client.balance(&seller_a), 600);
    assert_eq!(s.token_client.balance(&seller_b), 300);
    assert_eq!(s.token_client.balance(&seller_c), 100);
}

/// The full escrow amount is always distributed: sum of all seller payouts equals
/// the escrow amount (primary seller absorbs integer-division remainder).
#[test]
fn test_multi_seller_total_distributed_equals_escrow_amount() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    // Three sellers; 1/3 each cannot be expressed in whole BPS so the primary
    // seller gets the rounding dust.
    let seller_a = Address::generate(&s.env); // primary
    let seller_b = Address::generate(&s.env); // 3 333 bps
    let seller_c = Address::generate(&s.env); // 3 333 bps; primary gets 3 334

    let sellers = sellers_vec(&s.env, &[
        (seller_a.clone(), 3_334),
        (seller_b.clone(), 3_333),
        (seller_c.clone(), 3_333),
    ]);
    let amount = 1_000i128;
    let escrow_id = create_no_collateral(&s, &buyer, sellers, amount);

    s.client.release_escrow(&buyer, &escrow_id);

    let total_received = s.token_client.balance(&seller_a)
        + s.token_client.balance(&seller_b)
        + s.token_client.balance(&seller_c);
    assert_eq!(total_received, amount, "all funds must be distributed");
}

/// A single-entry sellers Vec (all 10 000 bps) behaves like a single-seller escrow.
#[test]
fn test_multi_seller_single_entry_pays_sole_seller() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);

    let sellers = sellers_vec(&s.env, &[(seller.clone(), 10_000)]);
    let escrow_id = create_no_collateral(&s, &buyer, sellers, 800);

    s.client.release_escrow(&buyer, &escrow_id);

    let escrow = s.client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::Released);
    assert_eq!(s.token_client.balance(&seller), 800);
}

/// BPS shares that do not sum to 10 000 must be rejected.
#[test]
fn test_multi_seller_rejects_shares_not_summing_to_10000() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    s.token_admin_client.mint(&buyer, &1_000);

    let bad_sellers = sellers_vec(&s.env, &[
        (Address::generate(&s.env), 4_000),
        (Address::generate(&s.env), 4_000),
        // Sum = 8 000; missing 2 000 bps.
    ]);
    let deadline = s.env.ledger().timestamp() + 10_000;
    let result = s.client.try_create_multi_seller_escrow(
        &buyer,
        &bad_sellers,
        &Address::generate(&s.env),
        &1_000,
        &s.token_addr,
        &deadline,
        &None,
        &0u32,
        &0u32,
        &0u64,
    );
    assert!(result.is_err());
}

/// An empty sellers Vec must be rejected.
#[test]
fn test_multi_seller_rejects_empty_sellers() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    s.token_admin_client.mint(&buyer, &1_000);

    let empty: Vec<(Address, u32)> = Vec::new(&s.env);
    let deadline = s.env.ledger().timestamp() + 10_000;
    let result = s.client.try_create_multi_seller_escrow(
        &buyer,
        &empty,
        &Address::generate(&s.env),
        &1_000,
        &s.token_addr,
        &deadline,
        &None,
        &0u32,
        &0u32,
        &0u64,
    );
    assert!(result.is_err());
}

/// More than 5 sellers must be rejected.
#[test]
fn test_multi_seller_rejects_more_than_five_sellers() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    s.token_admin_client.mint(&buyer, &1_000);

    // 6 sellers, each 1 000 / 6 rounded — intentionally invalid BPS sum to also
    // test the count guard fires before the BPS guard.  We give valid BPS here so
    // only the count guard fires.
    let six_sellers = sellers_vec(&s.env, &[
        (Address::generate(&s.env), 2_000),
        (Address::generate(&s.env), 2_000),
        (Address::generate(&s.env), 2_000),
        (Address::generate(&s.env), 1_000),
        (Address::generate(&s.env), 1_000),
        (Address::generate(&s.env), 2_000), // 6th seller; sum = 10 000 but count > 5
    ]);
    let deadline = s.env.ledger().timestamp() + 10_000;
    let result = s.client.try_create_multi_seller_escrow(
        &buyer,
        &six_sellers,
        &Address::generate(&s.env),
        &1_000,
        &s.token_addr,
        &deadline,
        &None,
        &0u32,
        &0u32,
        &0u64,
    );
    assert!(result.is_err());
}

// ===========================================================================
//  delegate_escrow_share
// ===========================================================================

/// Delegated share is paid to the delegate, not the original seller.
#[test]
fn test_delegate_escrow_share_redirects_payout_to_delegate() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller_a = Address::generate(&s.env); // will delegate their share
    let seller_b = Address::generate(&s.env);
    let delegate = Address::generate(&s.env);

    let sellers = sellers_vec(&s.env, &[
        (seller_a.clone(), 4_000), // 40 % → 400
        (seller_b.clone(), 6_000), // 60 % → 600
    ]);
    let escrow_id = create_no_collateral(&s, &buyer, sellers, 1_000);

    // seller_a delegates their share to `delegate`.
    s.client.delegate_escrow_share(&seller_a, &escrow_id, &delegate);

    s.client.release_escrow(&buyer, &escrow_id);

    // seller_a receives nothing; delegate receives seller_a's 40 % share.
    assert_eq!(s.token_client.balance(&seller_a), 0, "original seller should receive nothing");
    assert_eq!(s.token_client.balance(&delegate), 400, "delegate must receive the redirected share");
    assert_eq!(s.token_client.balance(&seller_b), 600, "non-delegated seller unaffected");
}

/// Each seller in a multi-seller escrow can independently delegate to different
/// addresses; both delegations are honoured on release.
#[test]
fn test_delegate_escrow_share_multiple_independent_delegations() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller_a = Address::generate(&s.env);
    let seller_b = Address::generate(&s.env);
    let delegate_a = Address::generate(&s.env);
    let delegate_b = Address::generate(&s.env);

    let sellers = sellers_vec(&s.env, &[
        (seller_a.clone(), 3_000), // 300
        (seller_b.clone(), 7_000), // 700
    ]);
    let escrow_id = create_no_collateral(&s, &buyer, sellers, 1_000);

    s.client.delegate_escrow_share(&seller_a, &escrow_id, &delegate_a);
    s.client.delegate_escrow_share(&seller_b, &escrow_id, &delegate_b);

    s.client.release_escrow(&buyer, &escrow_id);

    assert_eq!(s.token_client.balance(&seller_a), 0);
    assert_eq!(s.token_client.balance(&seller_b), 0);
    assert_eq!(s.token_client.balance(&delegate_a), 300);
    assert_eq!(s.token_client.balance(&delegate_b), 700);
}

/// A seller who is not part of the escrow's sellers list must be rejected.
#[test]
fn test_delegate_escrow_share_rejects_non_participant_seller() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller_a = Address::generate(&s.env);
    let outsider = Address::generate(&s.env);
    let delegate = Address::generate(&s.env);

    let sellers = sellers_vec(&s.env, &[(seller_a.clone(), 10_000)]);
    let escrow_id = create_no_collateral(&s, &buyer, sellers, 500);

    // `outsider` is not in the sellers list.
    let result = s.client.try_delegate_escrow_share(&outsider, &escrow_id, &delegate);
    assert!(result.is_err());
}

/// A seller may not delegate to themselves.
#[test]
fn test_delegate_escrow_share_rejects_self_delegation() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller_a = Address::generate(&s.env);
    let seller_b = Address::generate(&s.env);

    let sellers = sellers_vec(&s.env, &[
        (seller_a.clone(), 5_000),
        (seller_b.clone(), 5_000),
    ]);
    let escrow_id = create_no_collateral(&s, &buyer, sellers, 1_000);

    // seller_a tries to delegate to themselves.
    let result = s.client.try_delegate_escrow_share(&seller_a, &escrow_id, &seller_a);
    assert!(result.is_err());
}

/// Delegation after the escrow has been released must be rejected.
#[test]
fn test_delegate_escrow_share_rejects_delegation_after_release() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller_a = Address::generate(&s.env);
    let seller_b = Address::generate(&s.env);
    let delegate = Address::generate(&s.env);

    let sellers = sellers_vec(&s.env, &[
        (seller_a.clone(), 5_000),
        (seller_b.clone(), 5_000),
    ]);
    let escrow_id = create_no_collateral(&s, &buyer, sellers, 1_000);

    // Release first.
    s.client.release_escrow(&buyer, &escrow_id);
    let escrow = s.client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::Released);

    // Attempt delegation on a terminal escrow must fail.
    let result = s.client.try_delegate_escrow_share(&seller_a, &escrow_id, &delegate);
    assert!(result.is_err());
}

/// Without any delegation the full amount goes to the original sellers — regression
/// guard to ensure delegation is genuinely opt-in.
#[test]
fn test_delegate_escrow_share_no_delegation_pays_original_sellers() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller_a = Address::generate(&s.env);
    let seller_b = Address::generate(&s.env);

    let sellers = sellers_vec(&s.env, &[
        (seller_a.clone(), 7_000), // 700
        (seller_b.clone(), 3_000), // 300
    ]);
    let escrow_id = create_no_collateral(&s, &buyer, sellers, 1_000);

    // No delegation called.
    s.client.release_escrow(&buyer, &escrow_id);

    assert_eq!(s.token_client.balance(&seller_a), 700);
    assert_eq!(s.token_client.balance(&seller_b), 300);
}
