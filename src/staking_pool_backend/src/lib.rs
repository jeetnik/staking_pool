use candid::{CandidType, Deserialize, Principal};
use ic_cdk_macros::*;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
pub enum LockPeriod {
    Days90,
    Days180,
    Days360,
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

ic_cdk::export_candid!();