use candid::{CandidType, Deserialize, Principal};
use serde::Serialize;

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub enum LockPeriod {
    Days90,
    Days180,
    Days360,
}

impl LockPeriod {
    pub fn to_seconds(&self) -> u64 {
        match self {
            LockPeriod::Days90 => 90 * 24 * 60 * 60,
            LockPeriod::Days180 => 180 * 24 * 60 * 60,
            LockPeriod::Days360 => 360 * 24 * 60 * 60,
        }
    }
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct Deposit {
    pub id: u64,
    pub amount: u64,
    pub lock_period: LockPeriod,
    pub deposit_time: u64,
    pub withdrawn: bool,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct UserInfo {
    pub principal: Principal,
    pub deposits: Vec<Deposit>,
    pub total_staked: u64,
    pub subaccount: [u8; 32],
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct DepositArgs {
    pub amount: u64,
    pub lock_period: LockPeriod,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct WithdrawArgs {
    pub deposit_id: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct RewardPoolArgs {
    pub amount: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct SlashPoolArgs {
    pub amount: u64,
    pub receiver: Principal,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum StakingError {
    InsufficientFunds,
    DepositNotFound,
    LockPeriodNotExpired,
    AlreadyWithdrawn,
    TransferFailed(String),
    Unauthorized,
    InvalidAmount,
}

pub type Result<T> = std::result::Result<T, StakingError>;