mod types;
mod state;
mod utils;
mod ledger;

use candid::Principal;
use ic_cdk_macros::*;
use types::*;
use state::STATE;

#[init]
fn init() {
    ic_cdk::println!("Staking pool canister initialized");
}

#[update]
async fn deposit(args: DepositArgs) -> Result<u64> {
    let caller = ic_cdk::caller();
    
    if args.amount == 0 {
        return Err(StakingError::InvalidAmount);
    }

    // Get user's subaccount
    let subaccount = utils::principal_to_subaccount(&caller);
    
    // Check balance in subaccount
    let balance = ledger::get_balance(subaccount).await
        .map_err(|e| StakingError::TransferFailed(e))?;
    
    if balance < args.amount {
        return Err(StakingError::InsufficientFunds);
    }

    // Add deposit to state
    let deposit_id = STATE.with(|state| {
        state.borrow_mut().add_deposit(caller, args.amount, args.lock_period)
    });

    Ok(deposit_id)
}

#[update]
async fn withdraw(args: WithdrawArgs) -> Result<u64> {
    let caller = ic_cdk::caller();

    // Check if withdrawal is allowed
    STATE.with(|state| {
        state.borrow().can_withdraw(&caller, args.deposit_id)
    })?;

    // Get amount and mark as withdrawn
    let amount = STATE.with(|state| {
        state.borrow_mut().mark_withdrawn(&caller, args.deposit_id)
    })?;

    // Transfer funds back to user
    let user_subaccount = utils::principal_to_subaccount(&caller);
    ledger::transfer_from_subaccount(user_subaccount, caller, amount).await
        .map_err(|e| StakingError::TransferFailed(e))?;

    Ok(amount)
}

#[update]
async fn reward_pool(args: RewardPoolArgs) -> Result<Vec<(Principal, u64)>> {
    // Only the canister controller can call this
    let caller = ic_cdk::caller();
    if caller != ic_cdk::api::controller() {
        return Err(StakingError::Unauthorized);
    }

    // Get reward distribution from pool subaccount
    let pool_subaccount = [0u8; 32]; // Pool uses zero subaccount
    
    // Check pool balance
    let balance = ledger::get_balance(pool_subaccount).await
        .map_err(|e| StakingError::TransferFailed(e))?;
    
    if balance < args.amount {
        return Err(StakingError::InsufficientFunds);
    }

    // Calculate proportional rewards
    let distributions = STATE.with(|state| {
        state.borrow().calculate_proportional_amount(args.amount)
    });

    // Apply rewards to user deposits
    STATE.with(|state| {
        state.borrow_mut().apply_rewards(&distributions);
    });

    Ok(distributions)
}

#[update]
async fn slash_pool(args: SlashPoolArgs) -> Result<Vec<(Principal, u64)>> {
    // Only the canister controller can call this
    let caller = ic_cdk::caller();
    if caller != ic_cdk::api::controller() {
        return Err(StakingError::Unauthorized);
    }

    // Calculate proportional slash amounts
    let distributions = STATE.with(|state| {
        state.borrow().calculate_proportional_amount(args.amount)
    });

    // Apply slash to user deposits
    STATE.with(|state| {
        state.borrow_mut().apply_slash(&distributions);
    });

    // Transfer slashed amount to receiver
    let mut total_slashed = 0u64;
    for (principal, amount) in &distributions {
        let subaccount = utils::principal_to_subaccount(principal);
        
        // Check actual balance available
        let balance = ledger::get_balance(subaccount).await
            .map_err(|e| StakingError::TransferFailed(e))?;
        
        let slash_amount = (*amount).min(balance);
        if slash_amount > 0 {
            ledger::transfer_from_subaccount(subaccount, args.receiver, slash_amount).await
                .map_err(|e| StakingError::TransferFailed(e))?;
            total_slashed += slash_amount;
        }
    }

    Ok(distributions)
}

#[query]
fn get_user_info(principal: Principal) -> Option<UserInfo> {
    STATE.with(|state| {
        state.borrow().users.get(&principal).cloned()
    })
}

#[query]
fn get_deposit_info(principal: Principal, deposit_id: u64) -> Option<Deposit> {
    STATE.with(|state| {
        state.borrow()
            .users
            .get(&principal)
            .and_then(|user| user.deposits.iter().find(|d| d.id == deposit_id).cloned())
    })
}

#[query]
fn get_total_staked() -> u64 {
    STATE.with(|state| state.borrow().total_staked)
}

#[query]
fn get_user_subaccount(principal: Principal) -> [u8; 32] {
    utils::principal_to_subaccount(&principal)
}

// Export candid interface
ic_cdk::export_candid!();