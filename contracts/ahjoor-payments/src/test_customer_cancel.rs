#![cfg(test)]
use super::*;
use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env,
};

struct Setup<'a> {
    env: Env,
    client: AhjoorPaymentsContractClient<'a>,
    token_addr: Address,
    token_client: TokenClient<'a>,
    tac: TokenAdminClient<'a>,
}

fn setup<'a>() -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(AhjoorPaymentsContract, ());
    let client = AhjoorPaymentsContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_addr);
    let tac = TokenAdminClient::new(&env, &token_addr);
    client.initialize(&admin, &admin, &0u32);
    client.set_min_collateral(&0i128);
    Setup {
        env,
        client,
        token_addr,
        token_client,
        tac,
    }
}

fn approve(s: &Setup<'_>, customer: &Address, amount: i128) {
    s.token_client.approve(
        customer,
        &s.client.address,
        &amount,
        &(s.env.ledger().sequence() + 1000),
    );
}

#[test]
fn test_customer_cancel_authorized_payment() {
    let s = setup();
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);
    s.tac.mint(&customer, &1_000);
    approve(&s, &customer, 1_000);

    s.env.ledger().set_sequence_number(100);
    let pid = s
        .client
        .authorize_payment(&merchant, &customer, &s.token_addr, &300, &200u64);

    s.client.cancel_payment(&customer, &pid);

    let payment = s.client.get_payment(&pid);
    assert_eq!(payment.status, PaymentStatus::CustomerCancelled);
    assert_eq!(s.token_client.balance(&customer), 1_000);
    assert_eq!(s.token_client.balance(&s.client.address), 0);
}

#[test]
fn test_cancel_rejected_after_capture() {
    let s = setup();
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);
    s.tac.mint(&customer, &1_000);
    approve(&s, &customer, 1_000);

    s.env.ledger().set_sequence_number(100);
    let pid = s
        .client
        .authorize_payment(&merchant, &customer, &s.token_addr, &300, &200u64);
    s.client.capture_payment(&merchant, &pid);

    let result = s.client.try_cancel_payment(&customer, &pid);
    assert_eq!(
        result.unwrap_err().unwrap(),
        ExtError::NotAuthorizedForCancel.into()
    );
}

#[test]
fn test_cancel_rejected_by_non_owner() {
    let s = setup();
    let customer = Address::generate(&s.env);
    let other = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);
    s.tac.mint(&customer, &1_000);
    approve(&s, &customer, 1_000);

    s.env.ledger().set_sequence_number(100);
    let pid = s
        .client
        .authorize_payment(&merchant, &customer, &s.token_addr, &300, &200u64);

    let result = s.client.try_cancel_payment(&other, &pid);
    assert_eq!(
        result.unwrap_err().unwrap(),
        ExtError::NotPaymentCustomer.into()
    );
}

#[test]
fn test_cancel_rejected_past_capture_deadline() {
    let s = setup();
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);
    s.tac.mint(&customer, &1_000);
    approve(&s, &customer, 1_000);

    s.env.ledger().set_sequence_number(100);
    let pid = s
        .client
        .authorize_payment(&merchant, &customer, &s.token_addr, &300, &200u64);

    s.env.ledger().set_sequence_number(200);
    let result = s.client.try_cancel_payment(&customer, &pid);
    assert_eq!(
        result.unwrap_err().unwrap(),
        ExtError::CancelPastCaptureDeadline.into()
    );
}

#[test]
fn test_capture_rejected_after_customer_cancel() {
    let s = setup();
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);
    s.tac.mint(&customer, &1_000);
    approve(&s, &customer, 1_000);

    s.env.ledger().set_sequence_number(100);
    let pid = s
        .client
        .authorize_payment(&merchant, &customer, &s.token_addr, &300, &200u64);
    s.client.cancel_payment(&customer, &pid);

    let result = s.client.try_capture_payment(&merchant, &pid);
    assert!(result.is_err());
}

#[test]
fn test_get_customer_refunds_pagination_and_cap() {
    let s = setup();
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);
    s.tac.mint(&customer, &10_000);
    approve(&s, &customer, 10_000);

    s.env.ledger().set_sequence_number(100);
    let mut ids = soroban_sdk::Vec::new(&s.env);
    for _ in 0..3 {
        let pid = s
            .client
            .authorize_payment(&merchant, &customer, &s.token_addr, &300, &200u64);
        s.client.cancel_payment(&customer, &pid);
        ids.push_back(pid);
    }

    // Index includes withdrawn (customer-cancelled) requests.
    let page = s.client.get_customer_refunds(&customer, &0u32, &10u32);
    assert_eq!(page.len(), 3);
    assert_eq!(page.get(0).unwrap(), ids.get(0).unwrap());
    assert_eq!(page.get(2).unwrap(), ids.get(2).unwrap());

    // Pagination window.
    let page = s.client.get_customer_refunds(&customer, &1u32, &1u32);
    assert_eq!(page.len(), 1);
    assert_eq!(page.get(0).unwrap(), ids.get(1).unwrap());

    // Page size is capped.
    let capped = s.client.get_customer_refunds(&customer, &0u32, &u32::MAX);
    assert!(capped.len() <= 50);
}
