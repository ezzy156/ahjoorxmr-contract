#![cfg(test)]
use super::*;
use soroban_sdk::{testutils::Address as _, Address, Env};

fn setup<'a>() -> (Env, AhjoorPaymentsContractClient<'a>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(AhjoorPaymentsContract, ());
    let client = AhjoorPaymentsContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);
    client.initialize(&admin, &admin, &0u32);
    client.set_min_collateral(&0i128);
    client.approve_merchant(&merchant);
    (env, client, admin, merchant)
}

#[test]
fn test_get_max_invoices_per_period_default_before_set() {
    let (_env, client, _admin, merchant) = setup();
    let (period_ledgers, max_count) = client.get_max_invoices_per_period(&merchant);
    assert_eq!(period_ledgers, 0);
    assert_eq!(max_count, 0);
}

#[test]
fn test_get_max_invoices_per_period_returns_configured_value() {
    let (_env, client, admin, merchant) = setup();
    client.set_max_invoices_per_period(&admin, &merchant, &100u32, &10u32);
    let (period_ledgers, max_count) = client.get_max_invoices_per_period(&merchant);
    assert_eq!(period_ledgers, 100);
    assert_eq!(max_count, 10);
}

#[test]
fn test_get_max_invoices_per_period_independent_per_merchant() {
    let (env, client, admin, merchant1) = setup();
    let merchant2 = Address::generate(&env);
    client.approve_merchant(&merchant2);

    client.set_max_invoices_per_period(&admin, &merchant1, &50u32, &5u32);

    let (pl1, mc1) = client.get_max_invoices_per_period(&merchant1);
    let (pl2, mc2) = client.get_max_invoices_per_period(&merchant2);
    assert_eq!((pl1, mc1), (50, 5));
    assert_eq!((pl2, mc2), (0, 0));
}
