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
fn test_get_withdrawal_limit_default_before_set() {
    let (_env, client, _admin, merchant) = setup();
    let (window, cap) = client.get_withdrawal_limit(&merchant);
    assert_eq!(window, 0);
    assert_eq!(cap, 0);
}

#[test]
fn test_get_withdrawal_limit_returns_configured_value() {
    let (_env, client, _admin, merchant) = setup();
    client.set_withdrawal_limit(&merchant, &7200u64, &5000i128);
    let (window, cap) = client.get_withdrawal_limit(&merchant);
    assert_eq!(window, 7200);
    assert_eq!(cap, 5000);
}

#[test]
fn test_get_withdrawal_limit_independent_per_merchant() {
    let (env, client, _admin, merchant1) = setup();
    let merchant2 = Address::generate(&env);
    client.approve_merchant(&merchant2);

    client.set_withdrawal_limit(&merchant1, &3600u64, &1000i128);

    let (w1, c1) = client.get_withdrawal_limit(&merchant1);
    let (w2, c2) = client.get_withdrawal_limit(&merchant2);
    assert_eq!((w1, c1), (3600, 1000));
    assert_eq!((w2, c2), (0, 0));
}
