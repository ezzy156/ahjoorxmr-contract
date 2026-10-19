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
fn test_get_default_withdrawal_limits_default_before_set() {
    let (_env, client, _admin) = setup();
    let (window, cap) = client.get_default_withdrawal_limits();
    assert_eq!(window, 86400u64);
    assert_eq!(cap, i128::MAX);
}

#[test]
fn test_get_default_withdrawal_limits_returns_configured_value() {
    let (_env, client, admin) = setup();
    client.set_default_withdrawal_limits(&admin, &3600u64, &500_000i128);
    let (window, cap) = client.get_default_withdrawal_limits();
    assert_eq!(window, 3600);
    assert_eq!(cap, 500_000);
}
