#![cfg(test)]
use super::*;
use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, String,
};

struct Setup<'a> {
    env: Env,
    client: AhjoorEscrowContractClient<'a>,
    admin: Address,
    contract_id: Address,
    token_addr: Address,
    token_client: TokenClient<'a>,
    token_admin: TokenAdminClient<'a>,
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
    let token_admin = TokenAdminClient::new(&env, &token_addr);

    client.initialize(&admin);
    client.add_allowed_token(&admin, &token_addr);

    Setup {
        env,
        client,
        admin,
        contract_id,
        token_addr,
        token_client,
        token_admin,
    }
}

fn config(s: &Setup, arbiter: &Address, deadline_duration: u64) -> EscrowTemplateConfig {
    EscrowTemplateConfig {
        arbiter: arbiter.clone(),
        token: s.token_addr.clone(),
        deadline_duration,
    }
}

// ===========================================================================
//  create_escrow_template
// ===========================================================================

#[test]
fn test_create_template_stores_config() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);

    let id = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 3_600));
    assert_eq!(id, 0);

    let template = s.client.get_escrow_template(&id);
    assert_eq!(template.id, 0);
    assert_eq!(template.creator, creator);
    assert_eq!(template.config.arbiter, arbiter);
    assert_eq!(template.config.token, s.token_addr);
    assert_eq!(template.config.deadline_duration, 3_600);
    assert!(template.active);
}

#[test]
fn test_create_template_ids_increment() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);

    let a = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 100));
    let b = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 200));
    let c = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 300));
    assert_eq!((a, b, c), (0, 1, 2));
    assert_eq!(s.client.get_escrow_template(&b).config.deadline_duration, 200);
}

#[test]
#[should_panic(expected = "Error(Contract, #43)")]
fn test_create_template_rejects_non_allowlisted_token() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let other_token = s
        .env
        .register_stellar_asset_contract_v2(s.admin.clone())
        .address();

    let cfg = EscrowTemplateConfig {
        arbiter: Address::generate(&s.env),
        token: other_token,
        deadline_duration: 100,
    };
    s.client.create_escrow_template(&creator, &cfg);
}

#[test]
#[should_panic(expected = "Error(Contract, #44)")]
fn test_create_template_rejects_zero_deadline_duration() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    s.client.create_escrow_template(&creator, &config(&s, &arbiter, 0));
}

#[test]
#[should_panic(expected = "Template not found")]
fn test_get_missing_template_panics() {
    let s = setup();
    s.client.get_escrow_template(&42);
}

// ===========================================================================
//  create_escrow_from_template
// ===========================================================================

#[test]
fn test_create_escrow_from_template_uses_template_config() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &1_000);

    s.env.ledger().with_mut(|l| l.timestamp = 10_000);
    let template_id = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 5_000));

    let escrow_id = s
        .client
        .create_escrow_from_template(&buyer, &seller, &template_id, &400);

    let escrow = s.client.get_escrow(&escrow_id);
    assert_eq!(escrow.buyer, buyer);
    assert_eq!(escrow.seller, seller);
    assert_eq!(escrow.arbiter, arbiter);
    assert_eq!(escrow.token, s.token_addr);
    assert_eq!(escrow.amount, 400);
    assert_eq!(escrow.original_amount, 400);
    assert_eq!(escrow.status, EscrowStatus::Active);
    assert_eq!(escrow.created_at, 10_000);
    assert_eq!(escrow.deadline, 15_000);

    assert_eq!(s.token_client.balance(&buyer), 600);
    assert_eq!(s.token_client.balance(&s.contract_id), 400);
}

#[test]
fn test_template_reusable_by_any_caller() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    let template_id = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 1_000));

    let buyer_a = Address::generate(&s.env);
    let buyer_b = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    s.token_admin.mint(&buyer_a, &500);
    s.token_admin.mint(&buyer_b, &500);

    let e1 = s
        .client
        .create_escrow_from_template(&buyer_a, &seller, &template_id, &100);
    let e2 = s
        .client
        .create_escrow_from_template(&buyer_b, &seller, &template_id, &200);

    assert_ne!(e1, e2);
    assert_eq!(s.client.get_escrow(&e1).buyer, buyer_a);
    assert_eq!(s.client.get_escrow(&e2).buyer, buyer_b);
    assert_eq!(s.client.get_escrow(&e2).amount, 200);
}

#[test]
#[should_panic(expected = "Error(Contract, #23)")]
fn test_create_escrow_from_template_rejects_zero_amount() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &1_000);

    let template_id = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 1_000));
    s.client
        .create_escrow_from_template(&buyer, &seller, &template_id, &0);
}

#[test]
#[should_panic(expected = "Template not found")]
fn test_create_escrow_from_missing_template_panics() {
    let s = setup();
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &1_000);
    s.client.create_escrow_from_template(&buyer, &seller, &7, &100);
}

#[test]
#[should_panic(expected = "Error(Contract, #45)")]
fn test_create_escrow_from_deactivated_template_panics() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &1_000);

    let template_id = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 1_000));
    s.client.deactivate_escrow_template(&creator, &template_id);
    s.client
        .create_escrow_from_template(&buyer, &seller, &template_id, &100);
}

#[test]
fn test_create_escrow_from_template_blocked_when_paused() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &1_000);

    let template_id = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 1_000));
    s.client
        .pause_contract(&s.admin, &String::from_str(&s.env, "maintenance"));

    let result = s
        .client
        .try_create_escrow_from_template(&buyer, &seller, &template_id, &100);
    assert!(result.is_err());
    assert_eq!(s.token_client.balance(&buyer), 1_000);
}

// ===========================================================================
//  update_escrow_template
// ===========================================================================

#[test]
fn test_update_template_changes_config_for_new_escrows() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter_a = Address::generate(&s.env);
    let arbiter_b = Address::generate(&s.env);
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &1_000);

    let template_id = s.client.create_escrow_template(&creator, &config(&s, &arbiter_a, 1_000));
    let before = s
        .client
        .create_escrow_from_template(&buyer, &seller, &template_id, &100);

    s.client
        .update_escrow_template(&creator, &template_id, &config(&s, &arbiter_b, 9_000));

    let template = s.client.get_escrow_template(&template_id);
    assert_eq!(template.config.arbiter, arbiter_b);
    assert_eq!(template.config.deadline_duration, 9_000);
    assert!(template.active);

    let after = s
        .client
        .create_escrow_from_template(&buyer, &seller, &template_id, &100);

    // Existing escrows keep the config they were created with.
    assert_eq!(s.client.get_escrow(&before).arbiter, arbiter_a);
    assert_eq!(s.client.get_escrow(&after).arbiter, arbiter_b);
    assert_eq!(
        s.client.get_escrow(&after).deadline,
        s.env.ledger().timestamp() + 9_000
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #49)")]
fn test_update_template_rejects_non_creator() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let stranger = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);

    let template_id = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 1_000));
    s.client
        .update_escrow_template(&stranger, &template_id, &config(&s, &arbiter, 2_000));
}

#[test]
#[should_panic(expected = "Error(Contract, #45)")]
fn test_update_deactivated_template_panics() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);

    let template_id = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 1_000));
    s.client.deactivate_escrow_template(&creator, &template_id);
    s.client
        .update_escrow_template(&creator, &template_id, &config(&s, &arbiter, 2_000));
}

#[test]
#[should_panic(expected = "Error(Contract, #43)")]
fn test_update_template_rejects_non_allowlisted_token() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    let other_token = s
        .env
        .register_stellar_asset_contract_v2(s.admin.clone())
        .address();

    let template_id = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 1_000));
    let cfg = EscrowTemplateConfig {
        arbiter,
        token: other_token,
        deadline_duration: 1_000,
    };
    s.client.update_escrow_template(&creator, &template_id, &cfg);
}

#[test]
#[should_panic(expected = "Error(Contract, #44)")]
fn test_update_template_rejects_zero_deadline_duration() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);

    let template_id = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 1_000));
    s.client
        .update_escrow_template(&creator, &template_id, &config(&s, &arbiter, 0));
}

#[test]
#[should_panic(expected = "Template not found")]
fn test_update_missing_template_panics() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    s.client
        .update_escrow_template(&creator, &3, &config(&s, &arbiter, 1_000));
}

// ===========================================================================
//  deactivate_escrow_template
// ===========================================================================

#[test]
fn test_deactivate_template_marks_inactive() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);

    let template_id = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 1_000));
    s.client.deactivate_escrow_template(&creator, &template_id);

    let template = s.client.get_escrow_template(&template_id);
    assert!(!template.active);
    // Config is retained after deactivation.
    assert_eq!(template.config.arbiter, arbiter);
}

#[test]
fn test_deactivate_does_not_affect_existing_escrows() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &1_000);

    let template_id = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 1_000));
    let escrow_id = s
        .client
        .create_escrow_from_template(&buyer, &seller, &template_id, &250);
    s.client.deactivate_escrow_template(&creator, &template_id);

    let escrow = s.client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::Active);
    assert_eq!(escrow.amount, 250);
}

#[test]
#[should_panic(expected = "Error(Contract, #50)")]
fn test_deactivate_template_rejects_non_creator() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let stranger = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);

    let template_id = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 1_000));
    s.client.deactivate_escrow_template(&stranger, &template_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn test_deactivate_template_twice_panics() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);

    let template_id = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 1_000));
    s.client.deactivate_escrow_template(&creator, &template_id);
    s.client.deactivate_escrow_template(&creator, &template_id);
}

#[test]
#[should_panic(expected = "Template not found")]
fn test_deactivate_missing_template_panics() {
    let s = setup();
    let creator = Address::generate(&s.env);
    s.client.deactivate_escrow_template(&creator, &11);
}

#[test]
fn test_deactivating_one_template_leaves_others_active() {
    let s = setup();
    let creator = Address::generate(&s.env);
    let arbiter = Address::generate(&s.env);
    let buyer = Address::generate(&s.env);
    let seller = Address::generate(&s.env);
    s.token_admin.mint(&buyer, &1_000);

    let a = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 1_000));
    let b = s.client.create_escrow_template(&creator, &config(&s, &arbiter, 2_000));
    s.client.deactivate_escrow_template(&creator, &a);

    assert!(!s.client.get_escrow_template(&a).active);
    assert!(s.client.get_escrow_template(&b).active);

    let escrow_id = s
        .client
        .create_escrow_from_template(&buyer, &seller, &b, &100);
    assert_eq!(s.client.get_escrow(&escrow_id).amount, 100);
}
