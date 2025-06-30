use candid::{CandidType, Deserialize, Principal};
use ic_cdk_macros::*;
use std::cell::RefCell;
use std::collections::HashMap;

use std::time::{Duration, SystemTime, UNIX_EPOCH};



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
#[derive(Default)]
struct StakingPool {
    stakes: HashMap<Principal, Vec<Stake>>,
    next_stake_id: u64,
}

thread_local! {
    static STATE: RefCell<StakingPool> = RefCell::new(StakingPool::default());
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