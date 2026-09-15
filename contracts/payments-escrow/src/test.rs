#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Env,
};

#[allow(dead_code)] // token_admin is only needed to mint the patient's starting balance in `setup`
struct TestCtx<'a> {
    env: Env,
    contract: Address,
    client: PaymentsEscrowClient<'a>,
    token: Address,
    token_admin: token::StellarAssetClient<'a>,
    token_client: token::Client<'a>,
    patient: Address,
    provider: Address,
}

fn setup<'a>() -> TestCtx<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let contract = env.register(PaymentsEscrow, ());
    let client = PaymentsEscrowClient::new(&env, &contract);

    let token_admin_addr = Address::generate(&env);
    let token_contract = env.register_stellar_asset_contract_v2(token_admin_addr.clone());
    let token = token_contract.address();
    let token_admin = token::StellarAssetClient::new(&env, &token);
    let token_client = token::Client::new(&env, &token);

    let patient = Address::generate(&env);
    let provider = Address::generate(&env);
    token_admin.mint(&patient, &1_000_000);

    TestCtx {
        env,
        contract,
        client,
        token,
        token_admin,
        token_client,
        patient,
        provider,
    }
}

#[test]
fn create_then_release_pays_the_provider() {
    let ctx = setup();
    let memo = String::from_str(&ctx.env, "appointment-42");

    let id = ctx.client.create_payment(
        &ctx.patient,
        &ctx.provider,
        &ctx.token,
        &10_000,
        &memo,
        &(ctx.env.ledger().timestamp() + 86_400),
    );
    assert_eq!(id, 0);
    assert_eq!(ctx.token_client.balance(&ctx.patient), 990_000);
    assert_eq!(ctx.token_client.balance(&ctx.contract), 10_000);

    let stored = ctx.client.get_payment(&id);
    assert_eq!(stored.status, PaymentStatus::Pending);
    assert_eq!(stored.amount, 10_000);
    assert_eq!(stored.memo, memo);

    ctx.client.release(&id);

    assert_eq!(ctx.token_client.balance(&ctx.provider), 10_000);
    assert_eq!(ctx.token_client.balance(&ctx.contract), 0);
    assert_eq!(ctx.client.get_payment(&id).status, PaymentStatus::Released);
}

#[test]
fn refund_after_window_returns_funds_to_patient() {
    let ctx = setup();
    let memo = String::from_str(&ctx.env, "appointment-7");
    let deadline = ctx.env.ledger().timestamp() + 3_600;

    let id = ctx.client.create_payment(
        &ctx.patient,
        &ctx.provider,
        &ctx.token,
        &5_000,
        &memo,
        &deadline,
    );

    ctx.env.ledger().set_timestamp(deadline + 1);
    ctx.client.refund(&id);

    assert_eq!(ctx.token_client.balance(&ctx.patient), 1_000_000);
    assert_eq!(ctx.token_client.balance(&ctx.contract), 0);
    assert_eq!(ctx.client.get_payment(&id).status, PaymentStatus::Refunded);
}

#[test]
fn refund_before_window_is_rejected() {
    let ctx = setup();
    let memo = String::from_str(&ctx.env, "appointment-9");
    let deadline = ctx.env.ledger().timestamp() + 3_600;

    let id = ctx.client.create_payment(
        &ctx.patient,
        &ctx.provider,
        &ctx.token,
        &5_000,
        &memo,
        &deadline,
    );

    let result = ctx.client.try_refund(&id);
    assert_eq!(result, Err(Ok(Error::RefundWindowNotOpenYet)));
}

#[test]
fn cannot_release_twice() {
    let ctx = setup();
    let memo = String::from_str(&ctx.env, "appointment-1");
    let id = ctx.client.create_payment(
        &ctx.patient,
        &ctx.provider,
        &ctx.token,
        &1_000,
        &memo,
        &(ctx.env.ledger().timestamp() + 60),
    );

    ctx.client.release(&id);
    let result = ctx.client.try_release(&id);
    assert_eq!(result, Err(Ok(Error::NotPending)));
}

#[test]
fn zero_amount_is_rejected() {
    let ctx = setup();
    let memo = String::from_str(&ctx.env, "appointment-x");
    let result = ctx.client.try_create_payment(
        &ctx.patient,
        &ctx.provider,
        &ctx.token,
        &0,
        &memo,
        &(ctx.env.ledger().timestamp() + 60),
    );
    assert_eq!(result, Err(Ok(Error::AmountNotPositive)));
}

#[test]
fn missing_payment_is_reported() {
    let ctx = setup();
    let result = ctx.client.try_get_payment(&999);
    assert_eq!(result, Err(Ok(Error::PaymentNotFound)));
}
