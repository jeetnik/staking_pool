use candid::{CandidType, Deserialize, Principal};
use ic_cdk::{caller, trap,call};
use ic_cdk_macros::*;
use std::cell::RefCell;
use std::collections::HashMap;
use ic_ledger_types::{
    AccountIdentifier, Subaccount, DEFAULT_SUBACCOUNT, MAINNET_LEDGER_CANISTER_ID,account_balance, AccountBalanceArgs, Tokens,
    transfer, TransferArgs, TransferError, BlockIndex, Memo};

use ic_cdk::api::management_canister::main::raw_rand;
use ic_cdk::id;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};


use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ICP_LEDGER_CANISTER_ID: Principal = MAINNET_LEDGER_CANISTER_ID;

const MIN_DEPOSIT: u64 = 100_000; // 0.001 ICP
const MAX_DEPOSIT: u64 = 100_000_000_000; // 1000 ICP
const TRANSFER_FEE: u64 = 10_000; // 0.0001 ICP
const REWARD_SUBACCOUNT: [u8; 32] = [1u8; 32];
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
}

fn get_current_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs()
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

impl LockPeriod {
    fn multiplier(&self) -> f64 {
        match self {
            LockPeriod::Days90 => 1.0,
            LockPeriod::Days180 => 1.5,
            LockPeriod::Days360 => 2.0,
        }
    }
}

struct StakingPool {
    stakes: HashMap<Principal, Vec<Stake>>,
    user_rewards: HashMap<Principal, u64>,
    total_pool_balance: u64,
    total_rewards_distributed: u64,
    next_stake_id: u64,
    reward_pool_balance: u64,
}

impl StakingPool {
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

    fn add_user_reward(&mut self, user: Principal, amount: u64) {
        *self.user_rewards.entry(user).or_insert(0) += amount;
    }
}



async fn transfer_icp(
    from_subaccount: Option<Subaccount>,
    to: AccountIdentifier,
    amount: u64,
    memo: Memo,
) -> std::result::Result<BlockIndex, TransferError> {
    if amount <= TRANSFER_FEE {
        return Err(TransferError::InsufficientFunds { 
            balance: Tokens::from_e8s(amount) 
        });
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

impl StakingPool {
    fn update_stake(&mut self, user: Principal, stake_index: usize, updater: impl FnOnce(&mut Stake)) -> bool {
        if let Some(user_stakes) = self.stakes.get_mut(&user) {
            if let Some(stake) = user_stakes.get_mut(stake_index) {
                updater(stake);
                return true;
            }
        }
        false
    }
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Stake {
    pub id: u64,
    pub amount: u64,
    pub lock_period: LockPeriod,
    pub deposit_time: u64,
    pub unlock_time: u64,
    pub is_active: bool,
}
#[derive(CandidType, Deserialize, Debug)]
pub struct DepositArgs {
    pub amount: u64,
    pub lock_period: LockPeriod,
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

type Result<T> = std::result::Result<T, StakingError>;
#[derive(CandidType, Deserialize, Debug, Clone)]
pub enum StakingError {
    InvalidAmount,
    StakeNotFound,
    StakeStillLocked,
    StakeAlreadyWithdrawn,
}
#[derive(Default)]
struct StakingPool {
    stakes: HashMap<Principal, Vec<Stake>>,
    next_stake_id: u64,
}impl StakingPool {
    fn add_stake(&mut self, user: Principal, mut stake: Stake) {
        stake.id = self.next_stake_id;
        self.next_stake_id += 1;
        self.stakes.entry(user).or_default().push(stake);
    }
}



fn validate_deposit_args(args: &DepositArgs) -> Result<()> {
    if args.amount < MIN_DEPOSIT || args.amount > MAX_DEPOSIT {
        return Err(StakingError::InvalidAmount);
    }
    Ok(())
}
thread_local! {
    static STATE: RefCell<StakingPool> = RefCell::new(StakingPool::default());
}

async fn get_balance(subaccount: Subaccount) -> u64 {
    let account = get_account_identifier(subaccount);
    let balance_args = AccountBalanceArgs { account };
    
    match call(ICP_LEDGER_CANISTER_ID, "account_balance", (balance_args,)).await {
        Ok((tokens,)): std::result::Result<(Tokens,), _> => tokens.e8s(),
        Err(_) => 0,
    }
}


impl StakingPool {
    fn find_stake_by_id(&self, user: &Principal, stake_id: u64) -> Option<(usize, Stake)> {
        self.stakes.get(user)?
            .iter()
            .enumerate()
            .find(|(_, stake)| stake.id == stake_id)
            .map(|(idx, stake)| (idx, stake.clone()))
    }
}

#[update]
async fn confirm_deposit(stake_id: u64) -> Result<String> {
    let user = caller();
    
    let (_, stake) = STATE.with(|state| {
        let state_ref = state.borrow();
        state_ref.find_stake_by_id(&user, stake_id)
            .ok_or(StakingError::StakeNotFound)
    })?;

    let balance = get_balance(stake.subaccount).await;
    if balance < stake.amount {
        return Err(StakingError::InsufficientFunds);
    }

    Ok(format!(
        "Deposit confirmed for stake ID: {}. Amount: {} e8s",
        stake_id, stake.amount
    ))
}

// Add to Stake struct
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Stake {
    pub id: u64,
    pub amount: u64,
    pub lock_period: LockPeriod,
    pub deposit_time: u64,
    pub unlock_time: u64,
    pub subaccount: Subaccount, // New field
    pub is_active: bool,
}fn generate_subaccount(user: Principal, nonce: u64) -> Subaccount {
    let mut hasher = DefaultHasher::new();
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

#[update]
async fn deposit(args: DepositArgs) -> Result<(String, u64)> {
    let user = caller();
    let current_time = get_current_time();
    
    validate_deposit_args(&args)?;

    let nonce = get_random_nonce().await;
    let subaccount = generate_subaccount(user, nonce);
    let account_id = get_account_identifier(subaccount);
    let unlock_time = current_time + args.lock_period.to_seconds();
    
    let stake = Stake {
        id: 0,
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
            "Stake created with ID: {}. Transfer {} e8s to: {}",
            stake_id, args.amount, account_id
        ),
        stake_id
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

    let balance = get_balance(stake.subaccount).await;
    if balance < stake.amount {
        return Err(StakingError::InsufficientFunds);
    }

    let user_account = AccountIdentifier::new(&user, &DEFAULT_SUBACCOUNT);
    let transfer_amount = stake.amount.saturating_sub(TRANSFER_FEE);
    
    match transfer_icp(
        Some(stake.subaccount),
        user_account,
        transfer_amount,
        Memo(0),
    ).await {
        Ok(block_index) => {
            STATE.with(|state| {
                let mut state_ref = state.borrow_mut();
                state_ref.update_stake(user, stake_index, |s| {
                    s.is_active = false;
                });
            });
            
            Ok(format!(
                "Successfully withdrew {} e8s from stake ID: {}. Block: {}",
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
                Memo(1),
            ).await {
                Ok(_) => {
                    total_distributed += user_reward;
                    successful_transfers += 1;
                    
                    STATE.with(|state| {
                        state.borrow_mut().add_user_reward(user_principal, transfer_amount);
                    });
                }
                Err(_) => continue,
            }
        }
    }

    STATE.with(|state| {
        state.borrow_mut().total_rewards_distributed += total_distributed;
    });

    Ok(format!(
        "Distributed {} e8s to {} stakers",
        total_distributed, successful_transfers
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

    // Proportionally reduce stake amounts
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
        
        match transfer_icp(None, receiver_account, transfer_amount, Memo(2)).await {
            Ok(block_index) => {
                STATE.with(|state| {
                    state.borrow_mut().total_slashed += total_slashed;
                });
                
                Ok(format!(
                    "Slashed {} e8s from {} stakes. Block: {}",
                    total_slashed, successful_slashes, block_index
                ))
            }
            Err(e) => Err(StakingError::TransferFailed(format!("{:?}", e))),
        }
    } else {
        Err(StakingError::InvalidAmount)
    }
}

#[update]
async fn create_stake(args: DepositArgs) -> Result<String> {
    let user = caller();
    let current_time = get_current_time();
    
    validate_deposit_args(&args)?;
    
    let unlock_time = current_time + args.lock_period.to_seconds();
    
    let stake = Stake {
        id: 0, // Will be set in add_stake
        amount: args.amount,
        lock_period: args.lock_period,
        deposit_time: current_time,
        unlock_time,
        is_active: true,
    };

    let stake_id = STATE.with(|state| {
        let mut state_ref = state.borrow_mut();
        let stake_id = state_ref.next_stake_id;
        state_ref.add_stake(user, stake);
        stake_id
    });

    Ok(format!("Stake created with ID: {}", stake_id))
}

#[query]
fn get_stakes(user: Principal) -> Vec<Stake> {
    STATE.with(|state| {
        state.borrow().stakes.get(&user).cloned().unwrap_or_default()
    })
}


#[query]
fn get_time_until_unlock(user: Principal, stake_id: u64) -> Option<u64> {
    STATE.with(|state| {
        let state_ref = state.borrow();
        let current_time = get_current_time();
        
        if let Some(stakes) = state_ref.stakes.get(&user) {
            if let Some(stake) = stakes.iter().find(|s| s.id == stake_id) {
                if current_time >= stake.unlock_time {
                    return Some(0);
                } else {
                    return Some(stake.unlock_time - current_time);
                }
            }
        }
        None
    })
}
#[query]
fn get_reward_pool_account() -> String {
    let account_id = get_account_identifier(REWARD_SUBACCOUNT);
    account_id.to_string()
}


#[query]
fn get_account_identifier_for_deposit(user: Principal, nonce: u64) -> String {
    let subaccount = generate_subaccount(user, nonce);
    let account_id = get_account_identifier(subaccount);
    account_id.to_string()
}
ic_cdk::export_candid!();