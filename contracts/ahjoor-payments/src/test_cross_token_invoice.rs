#![cfg(test)]
use crate::multi_token_invoice::*;
use crate::multi_token_invoice_impl::MultiTokenInvoiceImpl;
use crate::AhjoorPaymentsContract;
use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{contract, contractimpl, testutils::Address as _, Address, Env, Map, Vec};

/// Oracle quoting 1 payment token = 2 base tokens (scaled by 1_000_000).
#[contract]
pub struct MockInvoiceOracle;

#[contractimpl]
impl MockInvoiceOracle {
    pub fn get_price(_env: Env, _base: Address, _quote: Address) -> Option<i128> {
        Some(2_000_000)
    }
}

struct Setup {
    env: Env,
    contract_id: Address,
    merchant: Address,
    payer: Address,
    base_token: Address,
    pay_token: Address,
    oracle: Address,
}

fn setup() -> Setup {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AhjoorPaymentsContract, ());
    let oracle = env.register(MockInvoiceOracle, ());

    let token_admin = Address::generate(&env);
    let base_token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let pay_token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let merchant = Address::generate(&env);
    let payer = Address::generate(&env);
    TokenAdminClient::new(&env, &pay_token).mint(&payer, &10_000);

    Setup {
        env,
        contract_id,
        merchant,
        payer,
        base_token,
        pay_token,
        oracle,
    }
}

fn create_invoice(s: &Setup, total: i128, oracle: Option<Address>) -> u32 {
    s.env.as_contract(&s.contract_id, || {
        MultiTokenInvoiceImpl::create_invoice_with_oracle(
            &s.env,
            s.merchant.clone(),
            s.payer.clone(),
            total,
            s.base_token.clone(),
            Vec::from_array(&s.env, [s.base_token.clone(), s.pay_token.clone()]),
            s.base_token.clone(),
            Vec::new(&s.env),
            1_000_000,
            Map::new(&s.env),
            oracle,
        )
    })
}

fn pay_cross_token(s: &Setup, invoice_id: u32, amount: i128) -> InvoicePayment {
    s.env.as_contract(&s.contract_id, || {
        MultiTokenInvoiceImpl::pay_invoice_cross_token(
            &s.env,
            invoice_id,
            s.payer.clone(),
            s.pay_token.clone(),
            amount,
            100,
        )
    })
}

#[test]
fn test_cross_token_payment_fully_pays_and_settles_invoice() {
    let s = setup();
    let invoice_id = create_invoice(&s, 1_000, Some(s.oracle.clone()));

    let payment = pay_cross_token(&s, invoice_id, 500);
    assert_eq!(payment.token, s.pay_token);
    assert_eq!(payment.amount, 500);
    assert_eq!(payment.amount_in_base, 1_000);
    assert_eq!(payment.amount_in_settlement, 1_000);

    let pay_client = TokenClient::new(&s.env, &s.pay_token);
    assert_eq!(pay_client.balance(&s.payer), 9_500);
    assert_eq!(pay_client.balance(&s.contract_id), 500);

    s.env.as_contract(&s.contract_id, || {
        assert_eq!(
            MultiTokenInvoiceImpl::get_invoice_status(&s.env, invoice_id),
            InvoiceStatus::FullyPaid
        );
        assert_eq!(MultiTokenInvoiceImpl::get_invoice_balance(&s.env, invoice_id), 0);
    });

    let batch = s.env.as_contract(&s.contract_id, || {
        MultiTokenInvoiceImpl::settle_invoices(
            &s.env,
            s.merchant.clone(),
            Vec::from_array(&s.env, [invoice_id]),
        )
    });
    assert_eq!(batch.total_settlement_amount, 1_000);
    assert_eq!(batch.status, SettlementStatus::Completed);
    assert_eq!(batch.merchant, s.merchant);

    s.env.as_contract(&s.contract_id, || {
        let stored = MultiTokenInvoiceImpl::get_settlement_batch(&s.env, batch.batch_id).unwrap();
        assert_eq!(stored, batch);
    });
}

#[test]
#[should_panic(expected = "Error(Contract, #12)")]
fn test_cross_token_payment_without_oracle_panics() {
    let s = setup();
    let invoice_id = create_invoice(&s, 1_000, None);

    pay_cross_token(&s, invoice_id, 500);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_settle_partially_paid_cross_token_invoice_panics() {
    let s = setup();
    let invoice_id = create_invoice(&s, 1_000, Some(s.oracle.clone()));

    pay_cross_token(&s, invoice_id, 200);
    s.env.as_contract(&s.contract_id, || {
        assert_eq!(
            MultiTokenInvoiceImpl::get_invoice_status(&s.env, invoice_id),
            InvoiceStatus::PartiallyPaid
        );
        MultiTokenInvoiceImpl::settle_invoices(
            &s.env,
            s.merchant.clone(),
            Vec::from_array(&s.env, [invoice_id]),
        );
    });
}
