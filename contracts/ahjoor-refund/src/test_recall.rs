#![cfg(test)]
use super::*;
use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, BytesN, Env,
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

const RESERVE: i128 = 10_000;

/// Seeds the reserve token and deposits `RESERVE` into the merchant's reserve.
fn fund_reserve<'a>(s: &TestSetup<'a>, merchant: &Address) {
    s.env.as_contract(&s.refund_client.address, || {
        s.env
            .storage()
            .instance()
            .set(&crate::DataKey2::ReserveToken, &s.token_addr);
    });
    s.token_admin_client.mint(merchant, &RESERVE);
    s.token_client.approve(
        merchant,
        &s.refund_client.address,
        &RESERVE,
        &(s.env.ledger().sequence() + 1000),
    );
    s.refund_client.deposit_merchant_reserve(merchant, &RESERVE);
}

fn reserve_balance<'a>(s: &TestSetup<'a>, merchant: &Address) -> i128 {
    let key = crate::DataKey2::MerchantReserveBalance(merchant.clone());
    s.env.as_contract(&s.refund_client.address, || {
        s.env.storage().persistent().get(&key).unwrap_or(0)
    })
}

fn recall_hash(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[7u8; 32])
}

struct RecallFixture {
    merchant: Address,
    customer_a: Address,
    customer_b: Address,
    pid_a: u32,
    pid_b: u32,
    recall_id: u32,
    deadline: u64,
}

fn declare<'a>(s: &TestSetup<'a>) -> RecallFixture {
    let merchant = Address::generate(&s.env);
    let customer_a = Address::generate(&s.env);
    let customer_b = Address::generate(&s.env);
    fund_reserve(s, &merchant);

    let pid_a = create_completed_payment(s, &customer_a, &merchant, 1_000);
    let pid_b = create_completed_payment(s, &customer_b, &merchant, 2_000);

    let mut ids = Vec::new(&s.env);
    ids.push_back(pid_a);
    ids.push_back(pid_b);

    let deadline = s.env.ledger().timestamp() + 1_000;
    let recall_id = s.refund_client.declare_recall(
        &merchant,
        &recall_hash(&s.env),
        &ids,
        &5_000u32, // 50%
        &deadline,
    );

    RecallFixture {
        merchant,
        customer_a,
        customer_b,
        pid_a,
        pid_b,
        recall_id,
        deadline,
    }
}

#[test]
fn test_declare_recall_locks_reserve() {
    let s = setup();
    let f = declare(&s);

    let recall = s.refund_client.get_recall(&f.recall_id);
    assert_eq!(recall.merchant, f.merchant);
    assert_eq!(recall.locked_amount, 1_500);
    assert_eq!(recall.claimed_amount, 0);
    assert_eq!(recall.claim_amounts.get(f.pid_a), Some(500));
    assert_eq!(recall.claim_amounts.get(f.pid_b), Some(1_000));
    assert!(!recall.closed);
    assert_eq!(reserve_balance(&s, &f.merchant), RESERVE - 1_500);
}

#[test]
#[should_panic(expected = "InsufficientReserveForRecall")]
fn test_declare_recall_fails_with_insufficient_reserve() {
    let s = setup();
    let merchant = Address::generate(&s.env);
    let customer = Address::generate(&s.env);
    fund_reserve(&s, &merchant);

    let pid = create_completed_payment(&s, &customer, &merchant, RESERVE * 2);
    let mut ids = Vec::new(&s.env);
    ids.push_back(pid);
    s.refund_client.declare_recall(
        &merchant,
        &recall_hash(&s.env),
        &ids,
        &10_000u32,
        &(s.env.ledger().timestamp() + 1_000),
    );
}

#[test]
#[should_panic(expected = "RecallPaymentNotOwnedByMerchant")]
fn test_declare_recall_rejects_foreign_payment() {
    let s = setup();
    let merchant = Address::generate(&s.env);
    let other_merchant = Address::generate(&s.env);
    let customer = Address::generate(&s.env);
    fund_reserve(&s, &merchant);

    let pid = create_completed_payment(&s, &customer, &other_merchant, 100);
    let mut ids = Vec::new(&s.env);
    ids.push_back(pid);
    s.refund_client.declare_recall(
        &merchant,
        &recall_hash(&s.env),
        &ids,
        &5_000u32,
        &(s.env.ledger().timestamp() + 1_000),
    );
}

#[test]
fn test_claim_recall_refund_pays_customer() {
    let s = setup();
    let f = declare(&s);

    let before = s.token_client.balance(&f.customer_a);
    let paid = s
        .refund_client
        .claim_recall_refund(&f.customer_a, &f.recall_id, &f.pid_a);
    assert_eq!(paid, 500);
    assert_eq!(s.token_client.balance(&f.customer_a), before + 500);
    assert!(s
        .refund_client
        .get_recall_claim_status(&f.recall_id, &f.pid_a));
    assert!(!s
        .refund_client
        .get_recall_claim_status(&f.recall_id, &f.pid_b));
    assert_eq!(s.refund_client.get_recall(&f.recall_id).claimed_amount, 500);
}

#[test]
#[should_panic(expected = "RecallAlreadyClaimed")]
fn test_double_claim_rejected() {
    let s = setup();
    let f = declare(&s);

    s.refund_client
        .claim_recall_refund(&f.customer_a, &f.recall_id, &f.pid_a);
    s.refund_client
        .claim_recall_refund(&f.customer_a, &f.recall_id, &f.pid_a);
}

#[test]
#[should_panic(expected = "OnlyPayerCanClaimRecall")]
fn test_only_payer_can_claim() {
    let s = setup();
    let f = declare(&s);

    s.refund_client
        .claim_recall_refund(&f.customer_b, &f.recall_id, &f.pid_a);
}

#[test]
fn test_recall_claim_does_not_affect_abuse_score() {
    let s = setup();
    let f = declare(&s);

    let before = s.refund_client.get_customer_abuse_score(&f.customer_a);
    s.refund_client
        .claim_recall_refund(&f.customer_a, &f.recall_id, &f.pid_a);
    let after = s.refund_client.get_customer_abuse_score(&f.customer_a);
    assert_eq!(before, after);
}

#[test]
#[should_panic(expected = "RecallClaimWindowOpen")]
fn test_close_recall_before_deadline_rejected() {
    let s = setup();
    let f = declare(&s);
    s.refund_client.close_recall(&f.merchant, &f.recall_id);
}

#[test]
fn test_close_recall_releases_unclaimed_reserve() {
    let s = setup();
    let f = declare(&s);

    s.refund_client
        .claim_recall_refund(&f.customer_a, &f.recall_id, &f.pid_a);

    s.env.ledger().with_mut(|li| li.timestamp = f.deadline + 1);
    let released = s.refund_client.close_recall(&f.merchant, &f.recall_id);

    assert_eq!(released, 1_000);
    assert!(s.refund_client.get_recall(&f.recall_id).closed);
    assert_eq!(reserve_balance(&s, &f.merchant), RESERVE - 500);
}

#[test]
#[should_panic(expected = "RecallClaimDeadlinePassed")]
fn test_claim_after_deadline_rejected() {
    let s = setup();
    let f = declare(&s);

    s.env.ledger().with_mut(|li| li.timestamp = f.deadline + 1);
    s.refund_client
        .claim_recall_refund(&f.customer_b, &f.recall_id, &f.pid_b);
}
