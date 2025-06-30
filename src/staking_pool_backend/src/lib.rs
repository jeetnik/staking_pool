use candid::{CandidType, Deserialize, Principal};
use ic_cdk::{caller, trap};
use ic_cdk_macros::*;
use std::cell::RefCell;
use std::collections::HashMap;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MIN_DEPOSIT: u64 = 100_000; // 0.001 ICP
const MAX_DEPOSIT: u64 = 100_000_000_000; // 1000 ICP


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
ic_cdk::export_candid!();