//! Tests for trigger_conditional_release (#318) and waive_release_condition (#318).
//!
//! trigger_conditional_release asks a configured oracle whether its returned price
//! satisfies `price >= expected_value`.  If so it proceeds with a normal release;
//! otherwise it panics with ConditionNotMet.
//!
//! waive_release_condition requires both buyer AND seller to sign (call the function)
//! before the condition is removed; a single signature just records the waiver.
//! Any other caller is rejected.
//!
//! Because there is no public `set_conditional_release` entry-point the condition is
//! injected directly into contract storage via `env.as_contract`.

#![cfg(test)]
use super::*;

use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{
    contract, contractimpl, contracttype,
    testutils::{Address as _, Ledger},
    Address, Env, Symbol, Vec,
};

// ---------------------------------------------------------------------------
//  Minimal on-chain oracle mock
// ---------------------------------------------------------------------------

mod mock_oracle {
    use crate::PriceData;
    use soroban_sdk::{contract, contractimpl, contracttype, Address, Env};

    #[contracttype]
    enum OKey {
        Price,
        Ts,
    }

    #[contract]
    pub struct MockOracle;

    #[contractimpl]
    impl MockOracle {
        pub fn set_price(env: Env, price: i128, timestamp: u64) {
            env.storage().instance().set(&OKey::Price, &price);
            env.storage().instance().set(&OKey::Ts, &timestamp);
        }

        pub fn lastprice(env: Env, _base: Address, _quote: Address) -> Option<PriceData> {
            let price: i128 = env.storage().instance().get(&OKey::Price)?;
            let timestamp: u64 = env.storage().instance().get(&OKey::Ts)?;
            Some(PriceData { price, timestamp })
        }
    }
}

use mock_oracle::MockOracle;

// ---------------------------------------------------------------------------
//  Test helpers
// ---------------------------------------------------------------------------

struct Setup<'a> {
    env: Env,
    client: AhjoorEscrowContractClient<'a>,
    /// The escrow contract's `Address` (needed for `as_contract` injections).
    contract_id: Address,
    admin: Address,
    token_addr: Address,
    token_client: TokenClient<'a>,
    token_admin_client: TokenAdminClient<'a>,
    oracle_addr: Address,
}

fn setup<'a>() -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AhjoorEscrowContract, ());
    let client = AhjoorEscrowContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_addr);
    let token_admin_client = TokenAdminClient::new(&env, &token_addr);

    // Oracle is registered but not yet priced; tests call set_price as needed.
    let oracle_addr = env.register(MockOracle, ());

    client.initialize(&admin);
    client.add_allowed_token(&admin, &token_addr);
    // max_oracle_age = 1 000 seconds — wide enough for all test scenarios.
    client.set_oracle(&admin, &oracle_addr, &1_000u64);

    Setup {
        env,
        client,
        contract_id,
        admin,
        token_addr,
        token_client,
        token_admin_client,
        oracle_addr,
    }
}

/// Mint tokens to `buyer`, create a plain escrow and return its ID.
fn create_escrow<'a>(s: &Setup<'a>, buyer: &Address, seller: &Address, amount: i128) -> u32 {
    s.token_admin_client.mint(buyer, &amount);
    let deadline = s.env.ledger().timestamp() + 10_000;
    s.client.create_escrow(
        buyer,
        seller,
        &Address::generate(&s.env), // arbiter
        &amount,
        &s.token_addr,
        &deadline,
        &None,
        &Vec::new(&s.env),
        &false,
        &0u32,
    )
}

/// Inject a ConditionalRelease condition into contract persistent storage.
/// `expected_value` — the oracle price that must be reached or exceeded.
fn inject_condition(s: &Setup, escrow_id: u32, oracle_contract: &Address, expected_value: i128) {
    let condition = ConditionalRelease {
        oracle_contract: oracle_contract.clone(),
        condition_method: Symbol::new(&s.env, "lastprice"),
        expected_value,
    };
    s.env.as_contract(&s.contract_id, || {
        s.env
            .storage()
            .persistent()
            .set(&DataKey2::ConditionalReleaseCondition(escrow_id), &condition);
    });
}

/// Set the mock oracle price, then advance the ledger timestamp to that same
/// value so the price is never stale (age = 0).
fn set_price(s: &Setup, price: i128, timestamp: u64) {
    use mock_oracle::MockOracleClient;
    let oc = MockOracleClient::new(&s.env, &s.oracle_addr);
    oc.set_price(&price, &timestamp);
    s.env.ledger().set_timestamp(timestamp);
}

// ===========================================================================
//  trigger_conditional_release
// ===========================================================================

/// Happy-path: oracle price equals expected_value → release succeeds.
#[test]
fn test_trigger_conditional_release_succeeds_when_price_meets_threshold() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let escrow_id = create_escrow(&s, &buyer, &seller, 1_000);

    // Condition: price must be >= 500.
    inject_condition(&s, escrow_id, &s.oracle_addr, 500);

    // Set oracle price exactly at the threshold.
    set_price(&s, 500, 100);

    s.client.trigger_conditional_release(&buyer, &escrow_id);

    let escrow = s.client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::Released);
    assert_eq!(s.token_client.balance(&seller), 1_000);
}

/// Oracle price strictly above threshold → release succeeds.
#[test]
fn test_trigger_conditional_release_succeeds_when_price_exceeds_threshold() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let escrow_id = create_escrow(&s, &buyer, &seller, 500);

    inject_condition(&s, escrow_id, &s.oracle_addr, 300);
    set_price(&s, 999, 200);

    s.client.trigger_conditional_release(&buyer, &escrow_id);

    let escrow = s.client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::Released);
    assert_eq!(s.token_client.balance(&seller), 500);
}

/// Oracle price is one unit below the threshold → call must fail with ConditionNotMet.
#[test]
fn test_trigger_conditional_release_rejected_when_price_below_threshold() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let escrow_id = create_escrow(&s, &buyer, &seller, 1_000);

    // Condition: price >= 500; oracle returns 499.
    inject_condition(&s, escrow_id, &s.oracle_addr, 500);
    set_price(&s, 499, 100);

    let result = s.client.try_trigger_conditional_release(&buyer, &escrow_id);
    assert!(result.is_err());

    // Escrow must remain active — funds must not have moved.
    let escrow = s.client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::Active);
    assert_eq!(s.token_client.balance(&seller), 0);
}

/// Calling trigger_conditional_release when no condition has been set panics.
#[test]
fn test_trigger_conditional_release_panics_with_no_condition_set() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let escrow_id = create_escrow(&s, &buyer, &seller, 1_000);

    // No condition injected.
    set_price(&s, 9_999, 100);

    let result = s.client.try_trigger_conditional_release(&buyer, &escrow_id);
    assert!(result.is_err());
}

/// Even once the condition is met, only the buyer or arbiter may trigger the
/// release; a third party is rejected and the escrow stays untouched.
#[test]
fn test_trigger_conditional_release_rejects_third_party() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let third_party = Address::generate(&s.env);
    let escrow_id = create_escrow(&s, &buyer, &seller, 750);

    inject_condition(&s, escrow_id, &s.oracle_addr, 100);
    set_price(&s, 200, 50);

    let result = s
        .client
        .try_trigger_conditional_release(&third_party, &escrow_id);
    assert_eq!(
        result.unwrap_err().unwrap(),
        EscrowError::OnlyBuyerOrArbiterCanReleaseEscrow.into()
    );

    let escrow = s.client.get_escrow(&escrow_id);
    assert_ne!(escrow.status, EscrowStatus::Released);
    assert_eq!(s.token_client.balance(&seller), 0);
}

// ===========================================================================
//  waive_release_condition
// ===========================================================================

/// Both parties signing waives the condition; after the waive the release
/// proceeds normally via the standard release_escrow path.
#[test]
fn test_waive_release_condition_both_parties_removes_condition_and_allows_release() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let escrow_id = create_escrow(&s, &buyer, &seller, 1_000);

    // Condition requires a very high price that the oracle will never return.
    inject_condition(&s, escrow_id, &s.oracle_addr, 1_000_000);

    // Buyer signs — condition still present; just one signature recorded.
    s.client.waive_release_condition(&buyer, &escrow_id);

    // Escrow is still active; condition has not been removed yet.
    let still_has_condition = s.env.as_contract(&s.contract_id, || {
        s.env
            .storage()
            .persistent()
            .has(&DataKey2::ConditionalReleaseCondition(escrow_id))
    });
    assert!(still_has_condition, "condition should still exist after one signature");

    // Seller signs — both have signed; condition is removed.
    s.client.waive_release_condition(&seller, &escrow_id);

    let condition_gone = s.env.as_contract(&s.contract_id, || {
        !s.env
            .storage()
            .persistent()
            .has(&DataKey2::ConditionalReleaseCondition(escrow_id))
    });
    assert!(condition_gone, "condition should be removed after both parties sign");

    // Normal release now succeeds (no oracle check; oracle price irrelevant).
    s.client.release_escrow(&buyer, &escrow_id);
    let escrow = s.client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::Released);
    assert_eq!(s.token_client.balance(&seller), 1_000);
}

/// First signer is seller, second is buyer — order does not matter.
#[test]
fn test_waive_release_condition_seller_then_buyer() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let escrow_id = create_escrow(&s, &buyer, &seller, 600);

    inject_condition(&s, escrow_id, &s.oracle_addr, 999_999);

    s.client.waive_release_condition(&seller, &escrow_id);
    s.client.waive_release_condition(&buyer, &escrow_id);

    let condition_gone = s.env.as_contract(&s.contract_id, || {
        !s.env
            .storage()
            .persistent()
            .has(&DataKey2::ConditionalReleaseCondition(escrow_id))
    });
    assert!(condition_gone);

    s.client.release_escrow(&buyer, &escrow_id);
    assert_eq!(s.token_client.balance(&seller), 600);
}

/// An unauthorized caller (neither buyer nor seller) is rejected.
#[test]
fn test_waive_release_condition_rejects_unauthorized_caller() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let intruder = Address::generate(&s.env);
    let escrow_id = create_escrow(&s, &buyer, &seller, 1_000);

    inject_condition(&s, escrow_id, &s.oracle_addr, 500);

    let result = s.client.try_waive_release_condition(&intruder, &escrow_id);
    assert!(result.is_err());

    // Condition must still be present.
    let still_present = s.env.as_contract(&s.contract_id, || {
        s.env
            .storage()
            .persistent()
            .has(&DataKey2::ConditionalReleaseCondition(escrow_id))
    });
    assert!(still_present);
}

/// Calling waive_release_condition on an escrow that has no condition set panics.
#[test]
fn test_waive_release_condition_panics_with_no_condition_set() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let escrow_id = create_escrow(&s, &buyer, &seller, 1_000);

    // No condition injected.
    let result = s.client.try_waive_release_condition(&buyer, &escrow_id);
    assert!(result.is_err());
}

/// A single party signing is not sufficient — funds remain locked.
#[test]
fn test_waive_release_condition_single_party_signature_is_insufficient() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    let escrow_id = create_escrow(&s, &buyer, &seller, 1_000);

    inject_condition(&s, escrow_id, &s.oracle_addr, 999_999);

    // Only buyer signs.
    s.client.waive_release_condition(&buyer, &escrow_id);

    // Condition still present → trigger_conditional_release must still fail
    // (oracle price is below expected_value).
    set_price(&s, 1, 100);
    let result = s.client.try_trigger_conditional_release(&buyer, &escrow_id);
    assert!(result.is_err());

    let escrow = s.client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::Active);
}
