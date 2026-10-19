#![cfg(test)]
use super::*;
use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{testutils::Address as _, Address, Env, String, Vec};

struct Setup<'a> {
    env: Env,
    client: AhjoorPaymentsContractClient<'a>,
    admin: Address,
    fee_recipient: Address,
    token_addr: Address,
    token_client: TokenClient<'a>,
    tac: TokenAdminClient<'a>,
}

/// Protocol fee of 3% (300 bps).
fn setup<'a>() -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(AhjoorPaymentsContract, ());
    let client = AhjoorPaymentsContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let fee_recipient = Address::generate(&env);
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_addr);
    let tac = TokenAdminClient::new(&env, &token_addr);
    client.initialize(&admin, &fee_recipient, &300u32);
    client.set_min_collateral(&0i128);
    Setup {
        env,
        client,
        admin,
        fee_recipient,
        token_addr,
        token_client,
        tac,
    }
}

fn reference(env: &Env) -> String {
    String::from_str(env, "order-1")
}

fn payees(env: &Env, shares: &[u32]) -> (Vec<(Address, u32)>, Vec<Address>) {
    let mut list = Vec::new(env);
    let mut addrs = Vec::new(env);
    for bps in shares.iter() {
        let a = Address::generate(env);
        list.push_back((a.clone(), *bps));
        addrs.push_back(a);
    }
    (list, addrs)
}

fn funded_customer(s: &Setup<'_>, amount: i128) -> Address {
    let customer = Address::generate(&s.env);
    s.tac.mint(&customer, &amount);
    customer
}

#[test]
fn test_two_payee_split_payment() {
    let s = setup();
    let customer = funded_customer(&s, 1_000);
    let (list, addrs) = payees(&s.env, &[7_000, 3_000]);

    let pid = s
        .client
        .create_split_payment(&customer, &list, &1_000, &s.token_addr, &reference(&s.env));
    assert_eq!(s.token_client.balance(&s.client.address), 1_000);

    let stored = s.client.get_split_payees(&pid);
    assert_eq!(stored.len(), 2);
    assert_eq!(stored.get(0).unwrap().gross, 700);
    assert_eq!(stored.get(1).unwrap().gross, 300);

    s.client.complete_payment(&pid);

    // 700 - 3% = 679, 300 - 3% = 291, fees 21 + 9 = 30.
    assert_eq!(s.token_client.balance(&addrs.get(0).unwrap()), 679);
    assert_eq!(s.token_client.balance(&addrs.get(1).unwrap()), 291);
    assert_eq!(s.token_client.balance(&s.fee_recipient), 30);
    assert_eq!(s.token_client.balance(&s.client.address), 0);

    let stored = s.client.get_split_payees(&pid);
    assert_eq!(stored.get(0).unwrap().paid, 679);
    assert_eq!(stored.get(0).unwrap().fee, 21);
    assert_eq!(stored.get(1).unwrap().paid, 291);
    assert_eq!(stored.get(1).unwrap().fee, 9);
    assert_eq!(s.client.get_payment(&pid).status, PaymentStatus::Completed);
}

#[test]
fn test_rounding_dust_goes_to_first_payee_and_nothing_leaks() {
    let s = setup();
    let customer = funded_customer(&s, 101);
    let (list, addrs) = payees(&s.env, &[3_333, 3_333, 3_334]);

    let pid = s
        .client
        .create_split_payment(&customer, &list, &101, &s.token_addr, &reference(&s.env));

    // 101 * 3333 / 10000 = 33 for each payee; 2 units of dust to payee 0.
    let stored = s.client.get_split_payees(&pid);
    assert_eq!(stored.get(0).unwrap().gross, 35);
    assert_eq!(stored.get(1).unwrap().gross, 33);
    assert_eq!(stored.get(2).unwrap().gross, 33);

    s.client.complete_payment(&pid);

    let mut total: i128 = s.token_client.balance(&s.fee_recipient);
    for a in addrs.iter() {
        total += s.token_client.balance(&a);
    }
    assert_eq!(total, 101);
    assert_eq!(s.token_client.balance(&s.client.address), 0);
    assert_eq!(s.token_client.balance(&addrs.get(0).unwrap()), 34);
}

#[test]
fn test_max_payee_split_payment() {
    let s = setup();
    let customer = funded_customer(&s, 10_000);
    assert_eq!(s.client.get_max_split_payees(), 10);
    let (list, addrs) = payees(&s.env, &[1_000; 10]);

    let pid = s
        .client
        .create_split_payment(&customer, &list, &10_000, &s.token_addr, &reference(&s.env));
    s.client.complete_payment(&pid);

    let mut total: i128 = s.token_client.balance(&s.fee_recipient);
    for a in addrs.iter() {
        assert_eq!(s.token_client.balance(&a), 970);
        total += s.token_client.balance(&a);
    }
    assert_eq!(total, 10_000);
}

#[test]
#[should_panic(expected = "Too many split payees")]
fn test_rejects_more_than_max_payees() {
    let s = setup();
    s.client.set_max_split_payees(&s.admin, &3);
    let customer = funded_customer(&s, 1_000);
    let (list, _) = payees(&s.env, &[2_500, 2_500, 2_500, 2_500]);
    s.client
        .create_split_payment(&customer, &list, &1_000, &s.token_addr, &reference(&s.env));
}

#[test]
#[should_panic(expected = "split payees must sum to 10000 bps")]
fn test_rejects_bps_below_total() {
    let s = setup();
    let customer = funded_customer(&s, 1_000);
    let (list, _) = payees(&s.env, &[5_000, 4_000]);
    s.client
        .create_split_payment(&customer, &list, &1_000, &s.token_addr, &reference(&s.env));
}

#[test]
#[should_panic(expected = "split payees must sum to 10000 bps")]
fn test_rejects_bps_above_total() {
    let s = setup();
    let customer = funded_customer(&s, 1_000);
    let (list, _) = payees(&s.env, &[6_000, 5_000]);
    s.client
        .create_split_payment(&customer, &list, &1_000, &s.token_addr, &reference(&s.env));
}

#[test]
#[should_panic(expected = "Merchant not approved")]
fn test_rejects_unapproved_payee() {
    let s = setup();
    s.client.set_merchant_open_mode(&false);
    let customer = funded_customer(&s, 1_000);
    let (list, addrs) = payees(&s.env, &[5_000, 5_000]);
    s.client.approve_merchant(&addrs.get(0).unwrap());
    s.client
        .create_split_payment(&customer, &list, &1_000, &s.token_addr, &reference(&s.env));
}

#[test]
fn test_accepts_all_approved_payees() {
    let s = setup();
    s.client.set_merchant_open_mode(&false);
    let customer = funded_customer(&s, 1_000);
    let (list, addrs) = payees(&s.env, &[5_000, 5_000]);
    for a in addrs.iter() {
        s.client.approve_merchant(&a);
    }
    let pid = s
        .client
        .create_split_payment(&customer, &list, &1_000, &s.token_addr, &reference(&s.env));
    assert_eq!(s.client.get_split_payees(&pid).len(), 2);
}

#[test]
#[should_panic(expected = "Duplicate split payee")]
fn test_rejects_duplicate_payee() {
    let s = setup();
    let customer = funded_customer(&s, 1_000);
    let payee = Address::generate(&s.env);
    let mut list = Vec::new(&s.env);
    list.push_back((payee.clone(), 5_000u32));
    list.push_back((payee, 5_000u32));
    s.client
        .create_split_payment(&customer, &list, &1_000, &s.token_addr, &reference(&s.env));
}

#[test]
fn test_partial_refund_reverses_each_payee_proportionally() {
    let s = setup();
    let customer = funded_customer(&s, 1_000);
    let (list, addrs) = payees(&s.env, &[7_000, 3_000]);
    let pid = s
        .client
        .create_split_payment(&customer, &list, &1_000, &s.token_addr, &reference(&s.env));

    s.client.partial_refund(&pid, &100);
    assert_eq!(s.token_client.balance(&customer), 100);

    let stored = s.client.get_split_payees(&pid);
    assert_eq!(stored.get(0).unwrap().refunded, 70);
    assert_eq!(stored.get(1).unwrap().refunded, 30);

    s.client.complete_payment(&pid);

    // Remaining 630 / 270, fee 3% (18 / 8).
    assert_eq!(s.token_client.balance(&addrs.get(0).unwrap()), 612);
    assert_eq!(s.token_client.balance(&addrs.get(1).unwrap()), 262);
    assert_eq!(s.token_client.balance(&s.fee_recipient), 26);
    assert_eq!(s.token_client.balance(&s.client.address), 0);
}

#[test]
fn test_customer_favoured_dispute_refunds_every_payee_share() {
    let s = setup();
    let customer = funded_customer(&s, 1_000);
    let (list, addrs) = payees(&s.env, &[6_000, 4_000]);
    let pid = s
        .client
        .create_split_payment(&customer, &list, &1_000, &s.token_addr, &reference(&s.env));

    s.client.partial_refund(&pid, &50);
    s.client
        .dispute_payment(&customer, &pid, &String::from_str(&s.env, "not delivered"));
    s.client.resolve_dispute(&pid, &false);

    assert_eq!(s.token_client.balance(&customer), 1_000);
    assert_eq!(s.token_client.balance(&s.client.address), 0);
    for a in addrs.iter() {
        assert_eq!(s.token_client.balance(&a), 0);
    }
    let stored = s.client.get_split_payees(&pid);
    for p in stored.iter() {
        assert_eq!(p.refunded, p.gross);
    }
    assert_eq!(s.client.get_payment(&pid).status, PaymentStatus::Refunded);
}

#[test]
fn test_merchant_favoured_dispute_pays_every_payee() {
    let s = setup();
    let customer = funded_customer(&s, 1_000);
    let (list, addrs) = payees(&s.env, &[6_000, 4_000]);
    let pid = s
        .client
        .create_split_payment(&customer, &list, &1_000, &s.token_addr, &reference(&s.env));

    s.client
        .dispute_payment(&customer, &pid, &String::from_str(&s.env, "late"));
    s.client.resolve_dispute(&pid, &true);

    assert_eq!(s.token_client.balance(&addrs.get(0).unwrap()), 582);
    assert_eq!(s.token_client.balance(&addrs.get(1).unwrap()), 388);
    assert_eq!(s.token_client.balance(&s.fee_recipient), 30);
    assert_eq!(s.token_client.balance(&s.client.address), 0);
    assert!(s.client.is_settled(&pid));
}

#[test]
fn test_get_split_payees_empty_for_regular_payment() {
    let s = setup();
    let customer = funded_customer(&s, 1_000);
    let merchant = Address::generate(&s.env);
    let pid = s.client.create_payment(
        &customer,
        &merchant,
        &500,
        &s.token_addr,
        &None,
        &None,
        &None,
    );
    assert_eq!(s.client.get_split_payees(&pid).len(), 0);
}
