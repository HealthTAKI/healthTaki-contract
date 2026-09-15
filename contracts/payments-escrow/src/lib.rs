//! HealthTaki payments-escrow contract.
//!
//! Wallet-to-wallet Stellar payments (what the HealthTaki frontend already
//! does via Freighter + Horizon) don't need a smart contract. This contract
//! exists for the one thing plain payments can't do: let a patient pay for a
//! medical service *up front* while giving both sides recourse if the visit
//! doesn't happen as agreed.
//!
//! Flow:
//! 1. Patient calls `create_payment`, locking `amount` of `token` in the
//!    contract for a specific `provider`.
//! 2. Provider calls `release` after delivering the service, moving the
//!    escrowed funds to themselves.
//! 3. If the provider hasn't released by `refundable_after` (a ledger
//!    timestamp agreed at creation time, e.g. "24h after the appointment
//!    slot"), the patient can call `refund` to reclaim their funds.
//!
//! There is no admin key and no dispute arbiter: the contract only enforces
//! the timing rule above. Anything more (partial releases, arbitration) is a
//! deliberate non-goal for this first version — see the README.
#![no_std]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, token, Address, Env, String,
};

/// Ledger count roughly corresponding to one day, assuming ~5s per ledger.
/// Used only to size how far storage TTLs are extended; it has no bearing on
/// contract logic, which relies solely on `env.ledger().timestamp()`.
const DAY_IN_LEDGERS: u32 = 17_280;
const PAYMENT_TTL_THRESHOLD: u32 = DAY_IN_LEDGERS;
const PAYMENT_TTL_EXTEND_TO: u32 = DAY_IN_LEDGERS * 30;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PaymentStatus {
    Pending,
    Released,
    Refunded,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Payment {
    pub patient: Address,
    pub provider: Address,
    pub token: Address,
    pub amount: i128,
    /// Free-form reference (e.g. an off-chain invoice/appointment id) so a
    /// backend indexer can correlate this escrow with its own records.
    pub memo: String,
    pub created_at: u64,
    /// Ledger timestamp (seconds) after which the patient may reclaim the
    /// funds if the provider has not released them.
    pub refundable_after: u64,
    pub status: PaymentStatus,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    NextId,
    Payment(u64),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AmountNotPositive = 1,
    RefundNotBeforeCreation = 2,
    PaymentNotFound = 3,
    NotPending = 4,
    RefundWindowNotOpenYet = 5,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaymentCreated {
    #[topic]
    pub id: u64,
    pub patient: Address,
    pub provider: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaymentReleased {
    #[topic]
    pub id: u64,
    pub provider: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaymentRefunded {
    #[topic]
    pub id: u64,
    pub patient: Address,
}

#[contract]
pub struct PaymentsEscrow;

#[contractimpl]
impl PaymentsEscrow {
    /// Lock `amount` of `token` (a SEP-41 token contract address, e.g. the
    /// native XLM Stellar Asset Contract or a stablecoin) transferred from
    /// `patient`, held for `provider` until `release` or `refund`. Returns
    /// the new payment's id.
    pub fn create_payment(
        env: Env,
        patient: Address,
        provider: Address,
        token: Address,
        amount: i128,
        memo: String,
        refundable_after: u64,
    ) -> Result<u64, Error> {
        patient.require_auth();

        if amount <= 0 {
            return Err(Error::AmountNotPositive);
        }
        if refundable_after < env.ledger().timestamp() {
            return Err(Error::RefundNotBeforeCreation);
        }

        token::Client::new(&env, &token).transfer(
            &patient,
            &env.current_contract_address(),
            &amount,
        );

        let id = Self::next_id(&env);
        let payment = Payment {
            patient: patient.clone(),
            provider: provider.clone(),
            token,
            amount,
            memo,
            created_at: env.ledger().timestamp(),
            refundable_after,
            status: PaymentStatus::Pending,
        };
        Self::save(&env, id, &payment);

        PaymentCreated {
            id,
            patient,
            provider,
            amount,
        }
        .publish(&env);

        Ok(id)
    }

    /// Called by the provider once the service has been delivered. Moves the
    /// escrowed funds to the provider and marks the payment released.
    pub fn release(env: Env, id: u64) -> Result<(), Error> {
        let mut payment = Self::load(&env, id)?;
        payment.provider.require_auth();

        if payment.status != PaymentStatus::Pending {
            return Err(Error::NotPending);
        }

        token::Client::new(&env, &payment.token).transfer(
            &env.current_contract_address(),
            &payment.provider,
            &payment.amount,
        );

        payment.status = PaymentStatus::Released;
        let provider = payment.provider.clone();
        Self::save(&env, id, &payment);

        PaymentReleased { id, provider }.publish(&env);
        Ok(())
    }

    /// Called by the patient to reclaim escrowed funds once `refundable_after`
    /// has passed without the provider releasing them (e.g. a missed or
    /// cancelled appointment).
    pub fn refund(env: Env, id: u64) -> Result<(), Error> {
        let mut payment = Self::load(&env, id)?;
        payment.patient.require_auth();

        if payment.status != PaymentStatus::Pending {
            return Err(Error::NotPending);
        }
        if env.ledger().timestamp() < payment.refundable_after {
            return Err(Error::RefundWindowNotOpenYet);
        }

        token::Client::new(&env, &payment.token).transfer(
            &env.current_contract_address(),
            &payment.patient,
            &payment.amount,
        );

        payment.status = PaymentStatus::Refunded;
        let patient = payment.patient.clone();
        Self::save(&env, id, &payment);

        PaymentRefunded { id, patient }.publish(&env);
        Ok(())
    }

    /// Read-only lookup of a payment by id.
    pub fn get_payment(env: Env, id: u64) -> Result<Payment, Error> {
        Self::load(&env, id)
    }

    fn load(env: &Env, id: u64) -> Result<Payment, Error> {
        let key = DataKey::Payment(id);
        let payment = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(Error::PaymentNotFound)?;
        env.storage()
            .persistent()
            .extend_ttl(&key, PAYMENT_TTL_THRESHOLD, PAYMENT_TTL_EXTEND_TO);
        Ok(payment)
    }

    fn save(env: &Env, id: u64, payment: &Payment) {
        let key = DataKey::Payment(id);
        env.storage().persistent().set(&key, payment);
        env.storage()
            .persistent()
            .extend_ttl(&key, PAYMENT_TTL_THRESHOLD, PAYMENT_TTL_EXTEND_TO);
    }

    fn next_id(env: &Env) -> u64 {
        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(0);
        env.storage().instance().set(&DataKey::NextId, &(id + 1));
        env.storage()
            .instance()
            .extend_ttl(PAYMENT_TTL_THRESHOLD, PAYMENT_TTL_EXTEND_TO);
        id
    }
}

mod test;
