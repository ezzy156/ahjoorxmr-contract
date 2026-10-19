#![cfg(test)]
//! Coverage for unblock requests (`request_unblock`, `resolve_unblock_request`,
//! `get_unblock_request`) and evidence-window dispute resolution
//! (`set_evidence_window_ledgers`, `close_evidence_window`,
//! `set_max_evidence_submissions`, `resolve_dispute_with_evidence`).
use crate::{AhjoorPaymentsContract, AhjoorPaymentsContractClient, Error, PaymentStatus};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient as TokenAdminClient},
    vec, Address, BytesN, Env, String, Symbol,
};

const START_LEDGER: u32 = 100;
const AMOUNT: i128 = 300;

struct Setup<'a> {
    env: Env,
    client: AhjoorPaymentsContractClient<'a>,
    admin: Address,
    customer: Address,
    merchant: Address,
    token: TokenClient<'a>,
}

fn setup<'a>() -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_sequence_number(START_LEDGER);

    let client = AhjoorPaymentsContractClient::new(&env, &env.register(AhjoorPaymentsContract, ()));
    let admin = Address::generate(&env);
    client.initialize(&admin, &Address::generate(&env), &0);

    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let customer = Address::generate(&env);
    TokenAdminClient::new(&env, &token_addr).mint(&customer, &1_000);

    Setup {
        client,
        admin,
        customer,
        merchant: Address::generate(&env),
        token: TokenClient::new(&env, &token_addr),
        env,
    }
}

fn hash(env: &Env, byte: u8) -> BytesN<32> {
    BytesN::from_array(env, &[byte; 32])
}

fn pay(s: &Setup) -> u32 {
    s.client.create_payment(
        &s.customer,
        &s.merchant,
        &AMOUNT,
        &s.token.address,
        &None,
        &None,
        &None,
    )
}

fn try_pay(s: &Setup) -> bool {
    s.client
        .try_create_payment(
            &s.customer,
            &s.merchant,
            &AMOUNT,
            &s.token.address,
            &None,
            &None,
            &None,
        )
        .is_ok()
}

fn block(s: &Setup) {
    s.client.block_customer(
        &s.merchant,
        &s.customer,
        &Symbol::new(&s.env, "fraud"),
        &hash(&s.env, 9),
    );
}

/// Creates a payment and moves it into the `Disputed` state.
fn disputed_payment(s: &Setup) -> u32 {
    let id = pay(s);
    s.client
        .dispute_payment(&s.customer, &id, &String::from_str(&s.env, "not delivered"));
    id
}

fn submit(s: &Setup, caller: &Address, payment_id: u32, byte: u8) {
    s.client.submit_dispute_evidence(
        caller,
        &payment_id,
        &hash(&s.env, byte),
        &Symbol::new(&s.env, "ipfs"),
    );
}

fn try_submit(s: &Setup, caller: &Address, payment_id: u32, byte: u8) -> Result<(), soroban_sdk::Error> {
    match s.client.try_submit_dispute_evidence(
        caller,
        &payment_id,
        &hash(&s.env, byte),
        &Symbol::new(&s.env, "ipfs"),
    ) {
        Ok(_) => Ok(()),
        Err(Ok(e)) => Err(e),
        Err(Err(_)) => panic!("unexpected invoke error"),
    }
}

// ===========================================================================
//  Unblock requests
// ===========================================================================

#[test]
fn test_blocked_customer_unblock_request_approved() {
    let s = setup();
    block(&s);
    assert!(s.client.is_customer_blocked(&s.merchant, &s.customer));
    let res = s.client.try_create_payment(
        &s.customer,
        &s.merchant,
        &AMOUNT,
        &s.token.address,
        &None,
        &None,
        &None,
    );
    assert_eq!(res.unwrap_err().unwrap(), Error::CustomerBlocked.into());

    s.client
        .request_unblock(&s.customer, &s.merchant, &hash(&s.env, 1));
    // The request id is not returned; it is the next value of the shared counter.
    let request_id = 1;
    let req = s.client.get_unblock_request(&request_id).unwrap();
    assert_eq!(req.request_id, request_id);
    assert_eq!(req.customer, s.customer);
    assert_eq!(req.merchant, s.merchant);
    assert_eq!(req.dispute_hash, hash(&s.env, 1));
    assert_eq!(req.requested_at_ledger, START_LEDGER);
    // No max block age configured: the request expires at the request ledger.
    assert_eq!(req.expires_at_ledger, START_LEDGER);

    s.client.resolve_unblock_request(&s.admin, &request_id, &true);

    assert!(!s.client.is_customer_blocked(&s.merchant, &s.customer));
    assert!(s.client.get_block_entry(&s.merchant, &s.customer).is_none());
    assert!(s.client.get_unblock_request(&request_id).is_none());
    assert!(try_pay(&s));
}

#[test]
fn test_blocked_customer_unblock_request_rejected() {
    let s = setup();
    block(&s);
    s.client
        .request_unblock(&s.customer, &s.merchant, &hash(&s.env, 1));
    assert!(s.client.get_unblock_request(&1).is_some());

    s.client.resolve_unblock_request(&s.admin, &1, &false);

    // The request is consumed but the block stays in place.
    assert!(s.client.get_unblock_request(&1).is_none());
    assert!(s.client.is_customer_blocked(&s.merchant, &s.customer));
    assert!(!try_pay(&s));
}

#[test]
fn test_duplicate_unblock_requests_get_distinct_ids() {
    let s = setup();
    block(&s);
    s.client
        .request_unblock(&s.customer, &s.merchant, &hash(&s.env, 1));
    s.client
        .request_unblock(&s.customer, &s.merchant, &hash(&s.env, 2));

    // Each request is stored independently under its own id.
    let first = s.client.get_unblock_request(&1).unwrap();
    let second = s.client.get_unblock_request(&2).unwrap();
    assert_eq!(first.dispute_hash, hash(&s.env, 1));
    assert_eq!(second.dispute_hash, hash(&s.env, 2));
    assert_eq!(first.customer, second.customer);
    assert_eq!(first.merchant, second.merchant);

    // Rejecting one leaves the other pending and the customer blocked.
    s.client.resolve_unblock_request(&s.admin, &1, &false);
    assert!(s.client.get_unblock_request(&1).is_none());
    assert!(s.client.get_unblock_request(&2).is_some());
    assert!(s.client.is_customer_blocked(&s.merchant, &s.customer));

    // Approving the remaining duplicate unblocks the customer.
    s.client.resolve_unblock_request(&s.admin, &2, &true);
    assert!(s.client.get_unblock_request(&2).is_none());
    assert!(!s.client.is_customer_blocked(&s.merchant, &s.customer));
}

#[test]
fn test_resolving_an_already_resolved_request_fails() {
    let s = setup();
    block(&s);
    s.client
        .request_unblock(&s.customer, &s.merchant, &hash(&s.env, 1));
    s.client.resolve_unblock_request(&s.admin, &1, &true);

    assert!(s.client.try_resolve_unblock_request(&s.admin, &1, &true).is_err());
}

#[test]
#[should_panic(expected = "Request not found")]
fn test_resolve_unknown_unblock_request_panics() {
    let s = setup();
    s.client.resolve_unblock_request(&s.admin, &42, &true);
}

#[test]
#[should_panic(expected = "Only admin")]
fn test_non_admin_cannot_resolve_unblock_request() {
    let s = setup();
    block(&s);
    s.client
        .request_unblock(&s.customer, &s.merchant, &hash(&s.env, 1));
    s.client.resolve_unblock_request(&s.merchant, &1, &true);
}

#[test]
fn test_get_unblock_request_unknown_id_is_none() {
    let s = setup();
    assert!(s.client.get_unblock_request(&7).is_none());
}

// ===========================================================================
//  Evidence window and submission limits
// ===========================================================================

#[test]
fn test_evidence_past_max_submissions_fails() {
    let s = setup();
    s.client.set_max_evidence_submissions(&s.admin, &2);
    let id = disputed_payment(&s);

    submit(&s, &s.customer, id, 1);
    submit(&s, &s.customer, id, 2);
    assert_eq!(
        try_submit(&s, &s.customer, id, 3),
        Err(Error::EvidenceLimitReached.into())
    );

    // The limit is per party: the merchant still has its own allowance.
    submit(&s, &s.merchant, id, 4);
    submit(&s, &s.merchant, id, 5);
    assert_eq!(
        try_submit(&s, &s.merchant, id, 6),
        Err(Error::EvidenceLimitReached.into())
    );

    let record = s.client.get_dispute_evidence(&id);
    assert_eq!(record.customer_evidence.len(), 2);
    assert_eq!(record.merchant_evidence.len(), 2);
}

#[test]
fn test_evidence_after_window_elapses_fails() {
    let s = setup();
    s.client.set_evidence_window_ledgers(&s.admin, &10);
    let id = disputed_payment(&s);

    // The window opens with the first submission.
    submit(&s, &s.customer, id, 1);
    let record = s.client.get_dispute_evidence(&id);
    assert_eq!(record.evidence_window_close_ledger, START_LEDGER + 10);

    s.env.ledger().set_sequence_number(START_LEDGER + 9);
    submit(&s, &s.merchant, id, 2);

    s.env.ledger().set_sequence_number(START_LEDGER + 10);
    assert_eq!(
        try_submit(&s, &s.customer, id, 3),
        Err(Error::EvidenceWindowClosed.into())
    );
    assert_eq!(
        try_submit(&s, &s.merchant, id, 4),
        Err(Error::EvidenceWindowClosed.into())
    );
}

#[test]
fn test_evidence_after_close_evidence_window_fails() {
    let s = setup();
    let id = disputed_payment(&s);
    submit(&s, &s.customer, id, 1);

    s.client.close_evidence_window(&id);
    assert_eq!(
        s.client.get_dispute_evidence(&id).evidence_window_close_ledger,
        START_LEDGER
    );

    assert_eq!(
        try_submit(&s, &s.customer, id, 2),
        Err(Error::EvidenceWindowClosed.into())
    );
    assert_eq!(
        try_submit(&s, &s.merchant, id, 3),
        Err(Error::EvidenceWindowClosed.into())
    );
    assert_eq!(s.client.get_dispute_evidence(&id).customer_evidence.len(), 1);
}

#[test]
#[should_panic(expected = "No evidence record found")]
fn test_close_evidence_window_without_evidence_panics() {
    let s = setup();
    let id = disputed_payment(&s);
    s.client.close_evidence_window(&id);
}

#[test]
fn test_evidence_config_setters_are_admin_only() {
    let s = setup();
    let outsider = Address::generate(&s.env);
    assert!(s
        .client
        .try_set_evidence_window_ledgers(&outsider, &10)
        .is_err());
    assert!(s
        .client
        .try_set_max_evidence_submissions(&outsider, &1)
        .is_err());

    // Defaults still apply: 5 submissions per party.
    let id = disputed_payment(&s);
    for i in 0..5 {
        submit(&s, &s.customer, id, i);
    }
    assert_eq!(
        try_submit(&s, &s.customer, id, 5),
        Err(Error::EvidenceLimitReached.into())
    );
}

#[test]
#[should_panic(expected = "Only customer or merchant can submit evidence")]
fn test_third_party_cannot_submit_evidence() {
    let s = setup();
    let id = disputed_payment(&s);
    submit(&s, &Address::generate(&s.env), id, 1);
}

#[test]
fn test_evidence_on_undisputed_payment_fails() {
    let s = setup();
    let id = pay(&s);
    assert_eq!(
        try_submit(&s, &s.customer, id, 1),
        Err(Error::EvidenceWindowClosed.into())
    );
}

// ===========================================================================
//  resolve_dispute_with_evidence
// ===========================================================================

#[test]
fn test_resolve_dispute_with_evidence_customer_wins_refunds_customer() {
    let s = setup();
    let id = disputed_payment(&s);
    submit(&s, &s.customer, id, 1);
    submit(&s, &s.merchant, id, 2);
    s.client.close_evidence_window(&id);

    assert_eq!(s.token.balance(&s.customer), 1_000 - AMOUNT);
    assert_eq!(s.token.balance(&s.client.address), AMOUNT);

    s.client
        .resolve_dispute_with_evidence(&id, &false, &hash(&s.env, 77));

    assert_eq!(s.token.balance(&s.customer), 1_000);
    assert_eq!(s.token.balance(&s.merchant), 0);
    assert_eq!(s.token.balance(&s.client.address), 0);
    assert_eq!(s.client.get_payment(&id).status, PaymentStatus::Refunded);
    assert_eq!(
        s.client.get_dispute_evidence(&id).resolution_note_hash,
        Some(hash(&s.env, 77))
    );
    assert!(s.client.get_dispute(&id).resolved);
}

#[test]
fn test_resolve_dispute_with_evidence_merchant_wins_pays_merchant() {
    let s = setup();
    let id = disputed_payment(&s);
    submit(&s, &s.merchant, id, 2);

    s.client
        .resolve_dispute_with_evidence(&id, &true, &hash(&s.env, 88));

    // Merchant-favoured resolution completes the payment; funds stay in
    // escrow until the merchant batch is settled.
    assert_eq!(s.client.get_payment(&id).status, PaymentStatus::Completed);
    assert_eq!(s.token.balance(&s.customer), 1_000 - AMOUNT);
    assert_eq!(s.token.balance(&s.client.address), AMOUNT);
    assert!(!s.client.is_settled(&id));
    assert_eq!(
        s.client.get_dispute_evidence(&id).resolution_note_hash,
        Some(hash(&s.env, 88))
    );

    s.client
        .settle_merchant_payments(&s.admin, &s.merchant, &vec![&s.env, id]);

    assert_eq!(s.token.balance(&s.merchant), AMOUNT);
    assert_eq!(s.token.balance(&s.customer), 1_000 - AMOUNT);
    assert_eq!(s.token.balance(&s.client.address), 0);
    assert!(s.client.is_settled(&id));
}

#[test]
fn test_resolve_dispute_with_evidence_cannot_resolve_twice() {
    let s = setup();
    let id = disputed_payment(&s);
    submit(&s, &s.customer, id, 1);
    s.client
        .resolve_dispute_with_evidence(&id, &false, &hash(&s.env, 1));

    let res = s
        .client
        .try_resolve_dispute_with_evidence(&id, &true, &hash(&s.env, 2));
    assert!(res.is_err());
    // The first resolution stands.
    assert_eq!(s.token.balance(&s.customer), 1_000);
    assert_eq!(s.token.balance(&s.merchant), 0);
    assert_eq!(
        s.client.get_dispute_evidence(&id).resolution_note_hash,
        Some(hash(&s.env, 1))
    );
}

#[test]
#[should_panic(expected = "No evidence record found")]
fn test_resolve_dispute_with_evidence_requires_evidence_record() {
    let s = setup();
    let id = disputed_payment(&s);
    s.client
        .resolve_dispute_with_evidence(&id, &false, &hash(&s.env, 1));
}
