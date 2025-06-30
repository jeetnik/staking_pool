use candid::{CandidType, Deserialize, Principal};
use ic_cdk::api::management_canister::main::raw_rand;
use ic_cdk::{call, caller, id, trap};
use ic_cdk_macros::*;
use ic_ledger_types::{
    account_balance, transfer, AccountBalanceArgs, AccountIdentifier, BlockIndex, Memo, Subaccount,
    Tokens, TransferArgs, TransferError, DEFAULT_SUBACCOUNT, MAINNET_LEDGER_CANISTER_ID,
};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// Constants
const ICP_LEDGER_CANISTER_ID: Principal = MAINNET_LEDGER_CANISTER_ID;
const TRANSFER_FEE: u64 = 10_000; // 0.0001 ICP
const MIN_DEPOSIT: u64 = 100_000; // 0.001 ICP
const MAX_DEPOSIT: u64 = 100_000_000_000; // 1000 ICP
const REWARD_SUBACCOUNT: [u8; 32] = [1u8; 32]; // Fixed subaccount for rewards

// Lock periods in seconds
const LOCK_90_DAYS: u64 = 90 * 24 * 60 * 60;
const LOCK_180_DAYS: u64 = 180 * 24 * 60 * 60;
const LOCK_360_DAYS: u64 = 360 * 24 * 60 * 60;

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
pub enum LockPeriod {
    Days90,
    Days180,
    Days360,
}

impl LockPeriod {
    fn to_seconds(&self) -> u64 {
        match self {
            LockPeriod::Days90 => LOCK_90_DAYS,
            LockPeriod::Days180 => LOCK_180_DAYS,
            LockPeriod::Days360 => LOCK_360_DAYS,
        }
    }

    fn multiplier(&self) -> f64 {
        match self {
            LockPeriod::Days90 => 1.0,
            LockPeriod::Days180 => 1.5,
            LockPeriod::Days360 => 2.0,
        }
    }
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Stake {
    pub id: u64,
    pub amount: u64,
    pub lock_period: LockPeriod,
    pub deposit_time: u64,
    pub unlock_time: u64,
    pub subaccount: Subaccount,
    pub is_active: bool,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct StakingInfo {
    pub total_staked: u64,
    pub active_stakes: Vec<Stake>,
    pub total_rewards_earned: u64,
    pub pending_rewards: u64,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct DepositArgs {
    pub amount: u64,
    pub lock_period: LockPeriod,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct PoolStats {
    pub total_staked: u64,
    pub total_rewards_distributed: u64,
    pub total_slashed: u64,
    pub total_stakers: usize,
    pub active_stakes_count: usize,
}

#[derive(CandidType, Deserialize, Debug, Clone)]
pub enum StakingError {
    InsufficientFunds,
    InvalidAmount,
    TransferFailed(String),
    StakeNotFound,
    StakeStillLocked,
    StakeAlreadyWithdrawn,
    Unauthorized,
    InvalidLockPeriod,
    DepositTimeout,
    SystemError(String),
    InvalidReceiver,
}

type Result<T> = std::result::Result<T, StakingError>;

// Global state
thread_local! {
    static STATE: RefCell<StakingPool> = RefCell::new(StakingPool::default());
}

#[derive(Default)]
struct StakingPool {
    stakes: HashMap<Principal, Vec<Stake>>,
    user_rewards: HashMap<Principal, u64>,
    total_pool_balance: u64,
    total_rewards_distributed: u64,
    total_slashed: u64,
    next_stake_id: u64,
    reward_pool_balance: u64,
}

impl StakingPool {
    fn add_stake(&mut self, user: Principal, mut stake: Stake) {
        stake.id = self.next_stake_id;
        self.next_stake_id += 1;
        
        self.stakes.entry(user).or_default().push(stake.clone());
        self.total_pool_balance += stake.amount;
    }

    fn get_user_stakes(&self, user: &Principal) -> Vec<Stake> {
        self.stakes.get(user).cloned().unwrap_or_default()
    }

    fn get_active_user_stakes(&self, user: &Principal) -> Vec<Stake> {
        self.get_user_stakes(user)
            .into_iter()
            .filter(|stake| stake.is_active)
            .collect()
    }

    fn find_stake_by_id(&self, user: &Principal, stake_id: u64) -> Option<(usize, Stake)> {
        self.stakes.get(user)?
            .iter()
            .enumerate()
            .find(|(_, stake)| stake.id == stake_id)
            .map(|(idx, stake)| (idx, stake.clone()))
    }

    fn update_stake(&mut self, user: Principal, stake_index: usize, updater: impl FnOnce(&mut Stake)) -> bool {
        if let Some(user_stakes) = self.stakes.get_mut(&user) {
            if let Some(stake) = user_stakes.get_mut(stake_index) {
                updater(stake);
                return true;
            }
        }
        false
    }

    fn get_total_staked_amount(&self) -> u64 {
        self.stakes
            .values()
            .flat_map(|stakes| stakes.iter())
            .filter(|stake| stake.is_active)
            .map(|stake| stake.amount)
            .sum()
    }

    fn get_total_weighted_stake(&self) -> f64 {
        self.stakes
            .values()
            .flat_map(|stakes| stakes.iter())
            .filter(|stake| stake.is_active)
            .map(|stake| stake.amount as f64 * stake.lock_period.multiplier())
            .sum()
    }

    fn get_all_active_stakes(&self) -> Vec<(Principal, Stake)> {
        self.stakes
            .iter()
            .flat_map(|(user, stakes)| {
                stakes.iter()
                    .filter(|stake| stake.is_active)
                    .map(move |stake| (*user, stake.clone()))
            })
            .collect()
    }

    fn get_active_stakes_count(&self) -> usize {
        self.stakes
            .values()
            .flat_map(|stakes| stakes.iter())
            .filter(|stake| stake.is_active)
            .count()
    }

    fn add_user_reward(&mut self, user: Principal, amount: u64) {
        *self.user_rewards.entry(user).or_insert(0) += amount;
    }

    fn get_user_rewards(&self, user: &Principal) -> u64 {
        self.user_rewards.get(user).copied().unwrap_or(0)
    }
}

// Helper functions
fn get_current_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs()
}

async fn get_random_nonce() -> u64 {
    match raw_rand().await {
        Ok((random_bytes,)) => {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&random_bytes[..8]);
            u64::from_be_bytes(bytes)
        }
        Err(_) => get_current_time() + (caller().as_slice()[0] as u64),
    }
}

fn generate_subaccount(user: Principal, nonce: u64) -> Subaccount {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    
    user.hash(&mut hasher);
    nonce.hash(&mut hasher);
    
    let hash = hasher.finish();
    let mut subaccount = [0u8; 32];
    subaccount[..8].copy_from_slice(&hash.to_be_bytes());
    subaccount[8..16].copy_from_slice(&nonce.to_be_bytes());
    subaccount
}

fn get_account_identifier(subaccount: Subaccount) -> AccountIdentifier {
    AccountIdentifier::new(&id(), &subaccount)
}

async fn get_balance(subaccount: Subaccount) -> u64 {
    let account = get_account_identifier(subaccount);
    let balance_args = AccountBalanceArgs { account };
    
    match call(ICP_LEDGER_CANISTER_ID, "account_balance", (balance_args,)).await {
        Ok((tokens,)) => tokens.e8s(),
        Err(_) => 0,
    }
}

async fn transfer_icp(
    from_subaccount: Option<Subaccount>,
    to: AccountIdentifier,
    amount: u64,
    memo: Memo,
) -> std::result::Result<BlockIndex, TransferError> {
    if amount <= TRANSFER_FEE {
        return Err(TransferError::InsufficientFunds { balance: Tokens::from_e8s(amount) });
    }

    let transfer_args = TransferArgs {
        memo,
        amount: Tokens::from_e8s(amount),
        fee: Tokens::from_e8s(TRANSFER_FEE),
        from_subaccount,
        to,
        created_at_time: None,
    };

    call(ICP_LEDGER_CANISTER_ID, "transfer", (transfer_args,))
        .await
        .map_err(|_| TransferError::TxTooOld { allowed_window_nanos: 0 })
        .and_then(|result: (std::result::Result<BlockIndex, TransferError>,)| result.0)
}

fn validate_deposit_args(args: &DepositArgs) -> Result<()> {
    if args.amount < MIN_DEPOSIT {
        return Err(StakingError::InvalidAmount);
    }
    if args.amount > MAX_DEPOSIT {
        return Err(StakingError::InvalidAmount);
    }
    Ok(())
}

// Public methods
#[update]
async fn deposit(args: DepositArgs) -> Result<(String, u64)> {
    let user = caller();
    let current_time = get_current_time();
    
    // Validate input
    validate_deposit_args(&args)?;

    // Generate unique subaccount for this deposit
    let nonce = get_random_nonce().await;
    let subaccount = generate_subaccount(user, nonce);
    let account_id = get_account_identifier(subaccount);

    let unlock_time = current_time + args.lock_period.to_seconds();
    
    let stake = Stake {
        id: 0, // Will be set in add_stake
        amount: args.amount,
        lock_period: args.lock_period,
        deposit_time: current_time,
        unlock_time,
        subaccount,
        is_active: true,
    };

    let stake_id = STATE.with(|state| {
        let mut state_ref = state.borrow_mut();
        let stake_id = state_ref.next_stake_id;
        state_ref.add_stake(user, stake);
        stake_id
    });

    Ok((
        format!(
            "Stake created with ID: {}. Please transfer {} e8s to account: {}. Unlock time: {}",
            stake_id, args.amount, account_id, unlock_time
        ),
        stake_id
    ))
}

#[update]
async fn confirm_deposit(stake_id: u64) -> Result<String> {
    let user = caller();
    
    let (stake_index, stake) = STATE.with(|state| {
        let state_ref = state.borrow();
        state_ref.find_stake_by_id(&user, stake_id)
            .ok_or(StakingError::StakeNotFound)
    })?;

    // Check if already confirmed
    if !stake.is_active {
        return Err(StakingError::StakeAlreadyWithdrawn);
    }

    // Check balance in subaccount
    let balance = get_balance(stake.subaccount).await;
    if balance < stake.amount {
        return Err(StakingError::InsufficientFunds);
    }

    Ok(format!(
        "Deposit confirmed for stake ID: {}. Amount: {} e8s locked until timestamp: {}",
        stake_id, stake.amount, stake.unlock_time
    ))
}

#[update]
async fn withdraw(stake_id: u64) -> Result<String> {
    let user = caller();
    let current_time = get_current_time();

    let (stake_index, stake) = STATE.with(|state| {
        let state_ref = state.borrow();
        state_ref.find_stake_by_id(&user, stake_id)
            .ok_or(StakingError::StakeNotFound)
    })?;

    if !stake.is_active {
        return Err(StakingError::StakeAlreadyWithdrawn);
    }

    if current_time < stake.unlock_time {
        return Err(StakingError::StakeStillLocked);
    }

    // Check balance in subaccount
    let balance = get_balance(stake.subaccount).await;
    if balance < stake.amount {
        return Err(StakingError::InsufficientFunds);
    }

    // Transfer funds back to user
    let user_account = AccountIdentifier::new(&user, &DEFAULT_SUBACCOUNT);
    let transfer_amount = stake.amount.saturating_sub(TRANSFER_FEE);
    
    match transfer_icp(
        Some(stake.subaccount),
        user_account,
        transfer_amount,
        Memo(0),
    ).await {
        Ok(block_index) => {
            // Mark stake as inactive
            STATE.with(|state| {
                let mut state_ref = state.borrow_mut();
                state_ref.update_stake(user, stake_index, |s| {
                    s.is_active = false;
                });
                state_ref.total_pool_balance -= stake.amount;
            });
            
            Ok(format!(
                "Successfully withdrew {} e8s from stake ID: {}. Transaction block: {}",
                transfer_amount, stake_id, block_index
            ))
        }
        Err(e) => Err(StakingError::TransferFailed(format!("{:?}", e))),
    }
}

#[update]
async fn reward_pool(amount: u64) -> Result<String> {
    if amount == 0 {
        return Err(StakingError::InvalidAmount);
    }

    // Check reward pool balance
    let reward_balance = get_balance(REWARD_SUBACCOUNT).await;
    if reward_balance < amount + TRANSFER_FEE {
        return Err(StakingError::InsufficientFunds);
    }

    let total_weighted_stake = STATE.with(|state| state.borrow().get_total_weighted_stake());
    
    if total_weighted_stake == 0.0 {
        return Err(StakingError::InvalidAmount);
    }

    let all_stakes = STATE.with(|state| state.borrow().get_all_active_stakes());
    let mut total_distributed = 0u64;
    let mut successful_transfers = 0usize;

    // Calculate and distribute rewards proportionally based on weighted stakes
    for (user_principal, stake) in all_stakes {
        let weighted_stake = stake.amount as f64 * stake.lock_period.multiplier();
        let user_reward = ((weighted_stake / total_weighted_stake) * amount as f64) as u64;
        
        if user_reward > TRANSFER_FEE {
            let user_account = AccountIdentifier::new(&user_principal, &DEFAULT_SUBACCOUNT);
            let transfer_amount = user_reward.saturating_sub(TRANSFER_FEE);
            
            match transfer_icp(
                Some(REWARD_SUBACCOUNT),
                user_account,
                transfer_amount,
                Memo(1), // Memo 1 for rewards
            ).await {
                Ok(_) => {
                    total_distributed += user_reward;
                    successful_transfers += 1;
                    
                    // Track user rewards
                    STATE.with(|state| {
                        state.borrow_mut().add_user_reward(user_principal, transfer_amount);
                    });
                }
                Err(_) => continue, // Skip failed transfers
            }
        }
    }

    STATE.with(|state| {
        state.borrow_mut().total_rewards_distributed += total_distributed;
    });

    Ok(format!(
        "Distributed {} e8s in rewards to {} stakers out of {} total stake positions",
        total_distributed, successful_transfers, all_stakes.len()
    ))
}

#[update]
async fn slash_pool(amount: u64, receiver: Principal) -> Result<String> {
    if amount == 0 {
        return Err(StakingError::InvalidAmount);
    }

    if receiver == Principal::anonymous() {
        return Err(StakingError::InvalidReceiver);
    }

    let total_staked = STATE.with(|state| state.borrow().get_total_staked_amount());
    
    if total_staked == 0 {
        return Err(StakingError::InvalidAmount);
    }

    let all_stakes = STATE.with(|state| state.borrow().get_all_active_stakes());
    let mut total_slashed = 0u64;
    let mut successful_slashes = 0usize;

    // Calculate slash amount proportionally and reduce stake amounts
    for (user_principal, stake) in &all_stakes {
        let slash_amount = (stake.amount * amount) / total_staked;
        let actual_slash = slash_amount.min(stake.amount);
        
        if actual_slash > 0 {
            STATE.with(|state| {
                let mut state_ref = state.borrow_mut();
                if let Some(user_stakes) = state_ref.stakes.get_mut(user_principal) {
                    if let Some(user_stake) = user_stakes.iter_mut().find(|s| s.id == stake.id) {
                        user_stake.amount -= actual_slash;
                        total_slashed += actual_slash;
                        state_ref.total_pool_balance -= actual_slash;
                        successful_slashes += 1;
                        
                        // If stake becomes too small, mark as inactive
                        if user_stake.amount < MIN_DEPOSIT {
                            user_stake.is_active = false;
                        }
                    }
                }
            });
        }
    }

    // Transfer slashed amount to receiver
    if total_slashed > TRANSFER_FEE {
        let receiver_account = AccountIdentifier::new(&receiver, &DEFAULT_SUBACCOUNT);
        let transfer_amount = total_slashed.saturating_sub(TRANSFER_FEE);
        
        match transfer_icp(
            None, // Transfer from canister's default account
            receiver_account,
            transfer_amount,
            Memo(2), // Memo 2 for slashing
        ).await {
            Ok(block_index) => {
                STATE.with(|state| {
                    state.borrow_mut().total_slashed += total_slashed;
                });
                
                Ok(format!(
                    "Slashed {} e8s from {} stakes and sent {} e8s to receiver. Transaction block: {}",
                    total_slashed, successful_slashes, transfer_amount, block_index
                ))
            }
            Err(e) => Err(StakingError::TransferFailed(format!("{:?}", e))),
        }
    } else {
        Err(StakingError::InvalidAmount)
    }
}

// Query methods
#[query]
fn get_staking_info(user: Principal) -> StakingInfo {
    STATE.with(|state| {
        let state_ref = state.borrow();
        let stakes = state_ref.get_active_user_stakes(&user);
        let total_staked = stakes.iter().map(|s| s.amount).sum();
        let total_rewards_earned = state_ref.get_user_rewards(&user);
        
        StakingInfo {
            total_staked,
            active_stakes: stakes,
            total_rewards_earned,
            pending_rewards: 0, // Could be calculated based on pending reward pool
        }
    })
}

#[query]
fn get_pool_stats() -> PoolStats {
    STATE.with(|state| {
        let state_ref = state.borrow();
        PoolStats {
            total_staked: state_ref.get_total_staked_amount(),
            total_rewards_distributed: state_ref.total_rewards_distributed,
            total_slashed: state_ref.total_slashed,
            total_stakers: state_ref.stakes.len(),
            active_stakes_count: state_ref.get_active_stakes_count(),
        }
    })
}

#[query]
fn get_account_identifier_for_deposit(user: Principal, nonce: u64) -> String {
    let subaccount = generate_subaccount(user, nonce);
    let account_id = get_account_identifier(subaccount);
    account_id.to_string()
}

#[query]
fn get_reward_pool_account() -> String {
    let account_id = get_account_identifier(REWARD_SUBACCOUNT);
    account_id.to_string()
}

#[query]
fn get_stake_by_id(user: Principal, stake_id: u64) -> Option<Stake> {
    STATE.with(|state| {
        let state_ref = state.borrow();
        state_ref.find_stake_by_id(&user, stake_id)
            .map(|(_, stake)| stake)
    })
}

#[query]
fn get_time_until_unlock(user: Principal, stake_id: u64) -> Option<u64> {
    STATE.with(|state| {
        let state_ref = state.borrow();
        let current_time = get_current_time();
        
        state_ref.find_stake_by_id(&user, stake_id)
            .map(|(_, stake)| {
                if current_time >= stake.unlock_time {
                    0
                } else {
                    stake.unlock_time - current_time
                }
            })
    })
}

// Export candid interface
ic_cdk::export_candid!();