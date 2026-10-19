#![cfg(test)]
use super::*;
use soroban_sdk::{testutils::Address as _, Address, Env};

use ahjoor_payments::AhjoorPaymentsContract;

fn setup<'a>() -> (Env, AhjoorRefundContractClient<'a>, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let payment_id = env.register(AhjoorPaymentsContract, ());
    let refund_id = env.register(AhjoorRefundContract, ());
    let refund_client = AhjoorRefundContractClient::new(&env, &refund_id);

    let admin = Address::generate(&env);
    refund_client.initialize(&admin, &payment_id, &86_400u64, &None);

    (env, refund_client, admin)
}

#[test]
fn test_add_and_remove_merchant_from_auto_approve_whitelist() {
    let (env, client, admin) = setup();
    let merchant_a = Address::generate(&env);
    let merchant_b = Address::generate(&env);

    assert_eq!(client.get_auto_approved_merchants(&0, &u32::MAX).len(), 0);

    client.add_to_auto_approve(&admin, &merchant_a);
    client.add_to_auto_approve(&admin, &merchant_b);
    let list = client.get_auto_approved_merchants(&0, &u32::MAX);
    assert_eq!(list.len(), 2);
    assert!(list.contains(&merchant_a));
    assert!(list.contains(&merchant_b));

    client.remove_from_auto_approve(&admin, &merchant_a);
    let list = client.get_auto_approved_merchants(&0, &u32::MAX);
    assert_eq!(list.len(), 1);
    assert!(!list.contains(&merchant_a));
    assert!(list.contains(&merchant_b));
}

#[test]
#[should_panic(expected = "Merchant already whitelisted")]
fn test_add_duplicate_merchant_to_auto_approve_panics() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);

    client.add_to_auto_approve(&admin, &merchant);
    client.add_to_auto_approve(&admin, &merchant);
}

#[test]
#[should_panic(expected = "Merchant not in whitelist")]
fn test_remove_non_whitelisted_merchant_panics() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);

    client.remove_from_auto_approve(&admin, &merchant);
}
