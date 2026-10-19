#![cfg(test)]
use super::*;
use soroban_sdk::{testutils::Address as _, Address, Env};

fn setup<'a>() -> (Env, AhjoorPaymentsContractClient<'a>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(AhjoorPaymentsContract, ());
    let client = AhjoorPaymentsContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin, &admin, &0u32);
    client.set_min_collateral(&0i128);
    (env, client, admin)
}

#[test]
fn test_get_payment_expiry_bounds_default_before_set() {
    let (_env, client, _admin) = setup();
    let (min, max) = client.get_payment_expiry_bounds();
    assert_eq!(min, 60u64);
    assert_eq!(max, 30 * 24 * 60 * 60u64);
}

#[test]
fn test_get_payment_expiry_bounds_returns_configured_value() {
    let (_env, client, _admin) = setup();
    client.set_payment_expiry_bounds(&300u64, &7200u64);
    let (min, max) = client.get_payment_expiry_bounds();
    assert_eq!(min, 300);
    assert_eq!(max, 7200);
}
