# healthTaki-contract

A [Soroban](https://developers.stellar.org/docs/build/smart-contracts/overview) smart contract, written in Rust, for HealthTaki — Stellar payments for medical personnel.

## Why a smart contract at all

The rest of HealthTaki is deliberately non-custodial: the frontend pays providers wallet-to-wallet via Freighter, and the backend only proves wallet ownership for login — neither one ever touches funds. Plain payments already cover "patient sends provider some XLM/USDC."

What plain payments *can't* do is give either side recourse before a payment is final. This contract adds exactly one capability on top of the existing flow: **pay-and-hold**. A patient locks funds for a specific provider up front (e.g. at booking); the provider claims them once the service is delivered; if the provider never claims by an agreed deadline, the patient can reclaim the funds themselves. No admin key, no arbiter — the contract only ever moves funds according to those two authorizations.

## Contract: `payments-escrow`

Source: [`contracts/payments-escrow`](contracts/payments-escrow).

### State

```rust
pub struct Payment {
    pub patient: Address,
    pub provider: Address,
    pub token: Address,        // any SEP-41 token contract, e.g. native XLM's SAC or a stablecoin
    pub amount: i128,
    pub memo: String,          // free-form reference, e.g. an off-chain invoice/appointment id
    pub created_at: u64,
    pub refundable_after: u64, // ledger timestamp (seconds)
    pub status: PaymentStatus, // Pending | Released | Refunded
}
```

### Functions

| Function | Caller | Effect |
|---|---|---|
| `create_payment(patient, provider, token, amount, memo, refundable_after) -> u64` | patient | Transfers `amount` of `token` from `patient` into the contract, records a new `Pending` payment, returns its id. |
| `release(id)` | provider | Requires `status == Pending`. Transfers the held funds to the provider, marks `Released`. |
| `refund(id)` | patient | Requires `status == Pending` and `ledger().timestamp() >= refundable_after`. Transfers the held funds back to the patient, marks `Refunded`. |
| `get_payment(id) -> Payment` | anyone | Read-only lookup. |

Each state-changing call emits a topic-indexed event (`PaymentCreated`, `PaymentReleased`, `PaymentRefunded`) so an off-chain indexer (see the backend's "payment indexing" next step) can follow escrow activity without polling every provider address on Horizon.

Errors are a typed `Error` enum (`AmountNotPositive`, `RefundNotBeforeCreation`, `PaymentNotFound`, `NotPending`, `RefundWindowNotOpenYet`) returned as `Result`s rather than panics, so callers get a stable error code instead of a trap.

### What it deliberately does not do

- No partial releases or splitting a payment across multiple providers.
- No dispute arbitration beyond the time-based refund window — a provider and patient who disagree before the deadline have to resolve it off-chain (or the patient just waits for the window to open).
- No on-chain provider registry — provider identity/verification stays in the backend (`/providers/me`), which is a better fit for mutable profile data than contract storage.

These are natural follow-ups if the product needs them, not oversights.

## Building and testing

Requires Rust with the `wasm32v1-none` target:

```bash
rustup target add wasm32v1-none
```

```bash
make test    # cargo test — unit tests run against a real, in-process token contract
make build   # cargo build --target wasm32v1-none --release -> target/wasm32v1-none/release/healthtaki_payments_escrow.wasm
```

Tests in [`contracts/payments-escrow/src/test.rs`](contracts/payments-escrow/src/test.rs) deploy a Stellar Asset Contract as the test token, exercise the full create → release and create → refund paths, and check every error branch.

## Deploying (testnet)

Using the [Stellar CLI](https://developers.stellar.org/docs/tools/cli/stellar-cli):

```bash
stellar contract build
stellar contract deploy \
  --wasm target/wasm32v1-none/release/healthtaki_payments_escrow.wasm \
  --source <your-identity> \
  --network testnet
```

The deployed contract id is what the frontend would call with `create_payment` / `release` / `refund` via `@stellar/stellar-sdk`'s `Contract`/`assembleTransaction` helpers and Freighter's `signTransaction`, the same signing path it already uses for classic payments.

## Relation to the rest of HealthTaki

This repo is the contract only. It pairs with:

- **Frontend** (Next.js) — wallet connect, balances, and payment history today; would add "create/release/refund escrow" UI to use this contract.
- **Backend** (Rust/Axum) — "Sign in with Stellar" identity and provider profiles; a natural extension is a worker that watches this contract's events to reconcile invoices instead of polling Horizon per-address.
