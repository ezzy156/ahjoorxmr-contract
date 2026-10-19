#![cfg(test)]
//! Coverage for per-merchant accepted tokens (`set_merchant_accepted_tokens`,
//! `get_merchant_accepted_tokens`), preferred token (`set_preferred_token`,
//! `get_preferred_token`) and swap router config (`set_swap_router`,
//! `get_swap_router`).
use crate::{AhjoorPaymentsContract, AhjoorPaymentsContractClient, Error, ExtError, PaymentStatus};
use ahjoor_token_whitelist::{TokenWhitelistContract, TokenWhitelistContractClient};
use soroban_sdk::{
    testutils::Address as _,
    token::{Client as TokenClient, StellarAssetClient as TokenAdminClient},
    vec, Address, Env, Vec,
};

struct Setup<'a> {
    env: Env,
    admin: Address,
    customer: Address,
    merchant: Address,
    client: AhjoorPaymentsContractClient<'a>,
    whitelist: TokenWhitelistContractClient<'a>,
    token_a: Address,
    token_b: Address,
    /// Funded but never added to the whitelist.
    token_unlisted: Address,
}

fn new_token(env: &Env, holder: &Address) -> Address {
    let addr = env
        .register_stellar_asset_contract_v2(Address::generate(env))
        .address();
    TokenAdminClient::new(env, &addr).mint(holder, &10_000);
    addr
}

/// Payments contract wired to a token whitelist that allows `token_a` and
/// `token_b` (but not `token_unlisted`).
fn setup<'a>() -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let customer = Address::generate(&env);
    let merchant = Address::generate(&env);

    let client = AhjoorPaymentsContractClient::new(&env, &env.register(AhjoorPaymentsContract, ()));
    client.initialize(&admin, &Address::generate(&env), &0);

    let whitelist =
        TokenWhitelistContractClient::new(&env, &env.register(TokenWhitelistContract, ()));
    whitelist.initialize(&admin);
    client.set_token_whitelist_contract(&admin, &whitelist.address);

    let token_a = new_token(&env, &customer);
    let token_b = new_token(&env, &customer);
    let token_unlisted = new_token(&env, &customer);
    whitelist.add_token(&admin, &token_a);
    whitelist.add_token(&admin, &token_b);

    Setup {
        env,
        admin,
        customer,
        merchant,
        client,
        whitelist,
        token_a,
        token_b,
        token_unlisted,
    }
}

fn pay(s: &Setup, merchant: &Address, token: &Address) -> u32 {
    s.client
        .create_payment(&s.customer, merchant, &100, token, &None, &None, &None)
}

// ===========================================================================
//  Merchant accepted tokens
// ===========================================================================

#[test]
fn test_empty_accepted_list_accepts_any_whitelisted_token() {
    let s = setup();
    assert_eq!(s.client.get_merchant_accepted_tokens(&s.merchant).len(), 0);

    let p_a = pay(&s, &s.merchant, &s.token_a);
    let p_b = pay(&s, &s.merchant, &s.token_b);
    assert_eq!(s.client.get_payment(&p_a).token, s.token_a);
    assert_eq!(s.client.get_payment(&p_b).token, s.token_b);

    // Tokens outside the global whitelist are still rejected.
    let res = s.client.try_create_payment(
        &s.customer,
        &s.merchant,
        &100,
        &s.token_unlisted,
        &None,
        &None,
        &None,
    );
    assert_eq!(res.unwrap_err().unwrap(), Error::TokenNotAllowed.into());
}

#[test]
fn test_empty_accepted_list_without_whitelist_accepts_any_token() {
    let env = Env::default();
    env.mock_all_auths();
    let client = AhjoorPaymentsContractClient::new(&env, &env.register(AhjoorPaymentsContract, ()));
    client.initialize(&Address::generate(&env), &Address::generate(&env), &0);

    let customer = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = new_token(&env, &customer);

    assert_eq!(client.get_merchant_accepted_tokens(&merchant).len(), 0);
    let id = client.create_payment(&customer, &merchant, &100, &token, &None, &None, &None);
    assert_eq!(client.get_payment(&id).token, token);
}

#[test]
fn test_payment_in_token_not_accepted_by_merchant_is_rejected() {
    let s = setup();
    s.client
        .set_merchant_accepted_tokens(&s.merchant, &vec![&s.env, s.token_a.clone()]);
    assert_eq!(
        s.client.get_merchant_accepted_tokens(&s.merchant),
        vec![&s.env, s.token_a.clone()]
    );

    // token_b is globally whitelisted but not accepted by this merchant.
    let customer_before = TokenClient::new(&s.env, &s.token_b).balance(&s.customer);
    let res = s.client.try_create_payment(
        &s.customer,
        &s.merchant,
        &100,
        &s.token_b,
        &None,
        &None,
        &None,
    );
    assert_eq!(
        res.unwrap_err().unwrap(),
        ExtError::TokenNotAcceptedByMerchant.into()
    );
    assert_eq!(
        TokenClient::new(&s.env, &s.token_b).balance(&s.customer),
        customer_before
    );

    // The accepted token still works.
    let id = pay(&s, &s.merchant, &s.token_a);
    assert_eq!(s.client.get_payment(&id).token, s.token_a);
}

#[test]
fn test_accepted_tokens_are_per_merchant() {
    let s = setup();
    let other_merchant = Address::generate(&s.env);
    s.client
        .set_merchant_accepted_tokens(&s.merchant, &vec![&s.env, s.token_a.clone()]);

    assert_eq!(s.client.get_merchant_accepted_tokens(&other_merchant).len(), 0);
    let id = pay(&s, &other_merchant, &s.token_b);
    assert_eq!(s.client.get_payment(&id).token, s.token_b);
}

#[test]
fn test_clearing_accepted_tokens_restores_any_whitelisted_token() {
    let s = setup();
    s.client
        .set_merchant_accepted_tokens(&s.merchant, &vec![&s.env, s.token_a.clone()]);
    assert!(s
        .client
        .try_create_payment(&s.customer, &s.merchant, &100, &s.token_b, &None, &None, &None)
        .is_err());

    s.client
        .set_merchant_accepted_tokens(&s.merchant, &Vec::new(&s.env));
    assert_eq!(s.client.get_merchant_accepted_tokens(&s.merchant).len(), 0);

    let id = pay(&s, &s.merchant, &s.token_b);
    assert_eq!(s.client.get_payment(&id).token, s.token_b);
}

#[test]
fn test_accepted_tokens_can_be_replaced() {
    let s = setup();
    s.client
        .set_merchant_accepted_tokens(&s.merchant, &vec![&s.env, s.token_a.clone()]);
    s.client.set_merchant_accepted_tokens(
        &s.merchant,
        &vec![&s.env, s.token_a.clone(), s.token_b.clone()],
    );
    assert_eq!(
        s.client.get_merchant_accepted_tokens(&s.merchant),
        vec![&s.env, s.token_a.clone(), s.token_b.clone()]
    );
    pay(&s, &s.merchant, &s.token_b);

    s.client
        .set_merchant_accepted_tokens(&s.merchant, &vec![&s.env, s.token_b.clone()]);
    assert_eq!(
        s.client
            .try_create_payment(&s.customer, &s.merchant, &100, &s.token_a, &None, &None, &None)
            .unwrap_err()
            .unwrap(),
        ExtError::TokenNotAcceptedByMerchant.into()
    );
}

#[test]
fn test_accepted_tokens_must_be_whitelisted() {
    let s = setup();
    let res = s.client.try_set_merchant_accepted_tokens(
        &s.merchant,
        &vec![&s.env, s.token_a.clone(), s.token_unlisted.clone()],
    );
    assert_eq!(res.unwrap_err().unwrap(), Error::TokenNotAllowed.into());
    assert_eq!(s.client.get_merchant_accepted_tokens(&s.merchant).len(), 0);
}

#[test]
fn test_removing_token_from_whitelist_overrides_merchant_acceptance() {
    let s = setup();
    s.client
        .set_merchant_accepted_tokens(&s.merchant, &vec![&s.env, s.token_a.clone()]);
    s.whitelist.remove_token(&s.admin, &s.token_a);

    let res = s.client.try_create_payment(
        &s.customer,
        &s.merchant,
        &100,
        &s.token_a,
        &None,
        &None,
        &None,
    );
    assert_eq!(res.unwrap_err().unwrap(), Error::TokenNotAllowed.into());
}

// ===========================================================================
//  Preferred token
// ===========================================================================

#[test]
fn test_preferred_token_round_trip() {
    let s = setup();
    assert_eq!(s.client.get_preferred_token(&s.merchant), None);

    s.client.set_preferred_token(&s.merchant, &s.token_a);
    assert_eq!(s.client.get_preferred_token(&s.merchant), Some(s.token_a.clone()));

    s.client.set_preferred_token(&s.merchant, &s.token_b);
    assert_eq!(s.client.get_preferred_token(&s.merchant), Some(s.token_b.clone()));

    // Preferences are per merchant.
    assert_eq!(s.client.get_preferred_token(&Address::generate(&s.env)), None);
}

#[test]
fn test_preferred_token_must_be_whitelisted() {
    let s = setup();
    let res = s
        .client
        .try_set_preferred_token(&s.merchant, &s.token_unlisted);
    assert_eq!(res.unwrap_err().unwrap(), Error::TokenNotAllowed.into());
    assert_eq!(s.client.get_preferred_token(&s.merchant), None);
}

#[test]
fn test_preferred_token_without_router_settles_in_payment_token() {
    let s = setup();
    s.client.set_preferred_token(&s.merchant, &s.token_b);
    assert_eq!(s.client.get_swap_router(), None);

    let id = pay(&s, &s.merchant, &s.token_a);
    s.client.complete_payment(&id);

    assert_eq!(s.client.get_payment(&id).status, PaymentStatus::Completed);
    assert_eq!(TokenClient::new(&s.env, &s.token_a).balance(&s.merchant), 100);
    assert_eq!(TokenClient::new(&s.env, &s.token_b).balance(&s.merchant), 0);
}

// ===========================================================================
//  Swap router
// ===========================================================================

#[test]
fn test_swap_router_admin_round_trip() {
    let s = setup();
    assert_eq!(s.client.get_swap_router(), None);

    let router = Address::generate(&s.env);
    s.client.set_swap_router(&s.admin, &router);
    assert_eq!(s.client.get_swap_router(), Some(router));

    let router2 = Address::generate(&s.env);
    s.client.set_swap_router(&s.admin, &router2);
    assert_eq!(s.client.get_swap_router(), Some(router2));
}

#[test]
#[should_panic(expected = "Only admin can set swap router")]
fn test_non_admin_cannot_set_swap_router() {
    let s = setup();
    s.client
        .set_swap_router(&s.merchant, &Address::generate(&s.env));
}

#[test]
fn test_non_admin_swap_router_attempt_leaves_config_unchanged() {
    let s = setup();
    let router = Address::generate(&s.env);
    s.client.set_swap_router(&s.admin, &router);

    let outsider = Address::generate(&s.env);
    let res = s
        .client
        .try_set_swap_router(&outsider, &Address::generate(&s.env));
    assert!(res.is_err());
    assert_eq!(s.client.get_swap_router(), Some(router));
}
