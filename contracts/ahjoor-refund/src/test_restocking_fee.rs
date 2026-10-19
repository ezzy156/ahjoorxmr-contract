#![cfg(test)]
use super::*;
use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{
    testutils::Address as _,
    Address, Env, String,
};

use ahjoor_payments::{AhjoorPaymentsContract, AhjoorPaymentsContractClient};

struct TestSetup<'a> {
    env: Env,
    refund_client: AhjoorRefundContractClient<'a>,
    payment_client: AhjoorPaymentsContractClient<'a>,
    admin: Address,
    token_addr: Address,
    token_client: TokenClient<'a>,
    token_admin_client: TokenAdminClient<'a>,
}

fn setup<'a>() -> TestSetup<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let payment_id = env.register(AhjoorPaymentsContract, ());
    let payment_client = AhjoorPaymentsContractClient::new(&env, &payment_id);

    let refund_id = env.register(AhjoorRefundContract, ());
    let refund_client = AhjoorRefundContractClient::new(&env, &refund_id);

    let admin = Address::generate(&env);
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_addr);
    let token_admin_client = TokenAdminClient::new(&env, &token_addr);

    payment_client.initialize(&admin, &admin, &0u32);
    refund_client.initialize(&admin, &payment_id, &86_400u64, &None);

    TestSetup {
        env,
        refund_client,
        payment_client,
        admin,
        token_addr,
        token_client,
        token_admin_client,
    }
}

fn create_completed_payment<'a>(
    s: &TestSetup<'a>,
    customer: &Address,
    merchant: &Address,
    amount: i128,
) -> u32 {
    s.token_admin_client.mint(customer, &(amount * 2));
    let pid =
        s.payment_client
            .create_payment(customer, merchant, &amount, &s.token_addr, &None, &None, &None);
    s.payment_client.complete_payment(&pid);
    pid
}

fn request<'a>(s: &TestSetup<'a>, customer: &Address, pid: u32, amount: i128, reason_code: u32) -> u32 {
    s.token_admin_client.mint(customer, &amount);
    s.refund_client.request_refund(
        customer,
        &pid,
        &amount,
        &String::from_str(&s.env, "Returned item"),
        &reason_code,
    )
}

#[test]
fn test_default_restocking_fee_deducted_on_approval() {
    let s = setup();
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);

    s.refund_client.set_restocking_fee(&merchant, &None, &1_000u32); // 10%

    let pid = create_completed_payment(&s, &customer, &merchant, 1_000);
    let refund_id = request(&s, &customer, pid, 1_000, 0);

    let merchant_before = s.token_client.balance(&merchant);
    s.refund_client.approve_refund(&s.admin, &refund_id);

    let refund = s.refund_client.get_refund(&refund_id);
    assert_eq!(refund.restocking_fee, 100);
    assert_eq!(s.token_client.balance(&merchant), merchant_before + 100);

    let customer_before = s.token_client.balance(&customer);
    s.refund_client.process_refund(&s.admin, &refund_id);
    assert_eq!(s.token_client.balance(&customer), customer_before + 900);
}

#[test]
fn test_reason_specific_fee_overrides_default() {
    let s = setup();
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);

    s.refund_client.set_restocking_fee(&merchant, &None, &1_000u32);
    s.refund_client.set_restocking_fee(&merchant, &Some(2u32), &500u32);

    assert_eq!(s.refund_client.get_restocking_fee_bps(&merchant, &2u32), 500);
    assert_eq!(s.refund_client.get_restocking_fee_bps(&merchant, &1u32), 1_000);

    let pid = create_completed_payment(&s, &customer, &merchant, 1_000);
    let refund_id = request(&s, &customer, pid, 1_000, 2);
    s.refund_client.approve_refund(&s.admin, &refund_id);

    assert_eq!(s.refund_client.get_refund(&refund_id).restocking_fee, 50);
}

#[test]
#[should_panic(expected = "RestockingFeeExceedsCap")]
fn test_fee_above_default_cap_rejected() {
    let s = setup();
    let merchant = Address::generate(&s.env);
    s.refund_client.set_restocking_fee(&merchant, &None, &2_001u32);
}

#[test]
fn test_admin_cap_is_configurable() {
    let s = setup();
    let merchant = Address::generate(&s.env);

    assert_eq!(s.refund_client.get_max_restocking_fee_bps(), 2_000);
    s.refund_client.set_max_restocking_fee_bps(&s.admin, &500u32);
    assert_eq!(s.refund_client.get_max_restocking_fee_bps(), 500);

    assert!(s
        .refund_client
        .try_set_restocking_fee(&merchant, &None, &600u32)
        .is_err());
    s.refund_client.set_restocking_fee(&merchant, &None, &500u32);
}

#[test]
fn test_no_fee_configured_means_full_refund() {
    let s = setup();
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);

    let pid = create_completed_payment(&s, &customer, &merchant, 1_000);
    let refund_id = request(&s, &customer, pid, 1_000, 0);
    s.refund_client.approve_refund(&s.admin, &refund_id);

    assert_eq!(s.refund_client.get_refund(&refund_id).restocking_fee, 0);
}

#[test]
fn test_batch_approve_applies_restocking_fee() {
    let s = setup();
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);

    s.refund_client.set_restocking_fee(&merchant, &None, &1_000u32);
    let pid = create_completed_payment(&s, &customer, &merchant, 1_000);
    let refund_id = request(&s, &customer, pid, 1_000, 0);

    let mut ids = Vec::new(&s.env);
    ids.push_back(refund_id);
    s.refund_client.batch_approve_refunds(&s.admin, &ids);

    assert_eq!(s.refund_client.get_refund(&refund_id).restocking_fee, 100);
}

#[test]
fn test_appeal_outcome_exempt_from_restocking_fee() {
    let s = setup();
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);

    s.refund_client.set_restocking_fee(&merchant, &None, &1_000u32);
    let pid = create_completed_payment(&s, &customer, &merchant, 1_000);
    let refund_id = request(&s, &customer, pid, 1_000, 0);

    s.refund_client.reject_refund(
        &s.admin,
        &refund_id,
        &String::from_str(&s.env, "Not eligible"),
    );
    s.refund_client.appeal_refund(&customer, &refund_id);

    let customer_before = s.token_client.balance(&customer);
    s.refund_client.resolve_appeal(&s.admin, &refund_id, &true);

    let refund = s.refund_client.get_refund(&refund_id);
    assert_eq!(refund.restocking_fee, 0);
    assert_eq!(s.token_client.balance(&customer), customer_before + 1_000);
}

#[test]
fn test_export_refund_policy_includes_restocking_fees() {
    let s = setup();
    let merchant = Address::generate(&s.env);

    s.refund_client.set_restocking_fee(&merchant, &None, &1_000u32);
    s.refund_client.set_restocking_fee(&merchant, &Some(3u32), &250u32);

    let policy = s.refund_client.export_refund_policy(&merchant);
    assert_eq!(policy.default_restocking_fee_bps, 1_000);
    assert_eq!(policy.restocking_fee_by_reason.get(3u32), Some(250));
    assert_eq!(policy.restocking_fee_by_reason.get(1u32), None);
}
