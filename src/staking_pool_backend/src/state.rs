use crate::types::*;
use candid::Principal;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Default)]
pub struct State {
    pub users: HashMap<Principal, UserInfo>,
    pub total_staked: u64,
    pub next_deposit_id: u64,
    pub pool_balance: u64,
}

thread_local! {
    pub static STATE: RefCell<State> = RefCell::new(State::default());
}

impl State {
    pub fn get_or_create_user(&mut self, principal: Principal) -> &mut UserInfo {
        self.users.entry(principal).or_insert_with(|| {
            UserInfo {
                principal,
                deposits: Vec::new(),
                total_staked: 0,
                subaccount: crate::utils::principal_to_subaccount(&principal),
            }
        })
    }

    pub fn add_deposit(
        &mut self,
        principal: Principal,
        amount: u64,
        lock_period: LockPeriod,
    ) -> u64 {
        let deposit_id = self.next_deposit_id;
        self.next_deposit_id += 1;

        let deposit = Deposit {
            id: deposit_id,
            amount,
            lock_period,
            deposit_time: crate::utils::get_time_nanos(),
            withdrawn: false,
        };

        let user = self.get_or_create_user(principal);
        user.deposits.push(deposit);
        user.total_staked += amount;
        self.total_staked += amount;

        deposit_id
    }

    pub fn can_withdraw(&self, principal: &Principal, deposit_id: u64) -> Result<()> {
        let user = self.users.get(principal)
            .ok_or(StakingError::DepositNotFound)?;

        let deposit = user.deposits.iter()
            .find(|d| d.id == deposit_id)
            .ok_or(StakingError::DepositNotFound)?;

        if deposit.withdrawn {
            return Err(StakingError::AlreadyWithdrawn);
        }

        let current_time = crate::utils::get_time_nanos();
        let unlock_time = deposit.deposit_time + deposit.lock_period.to_seconds() * 1_000_000_000;

        if current_time < unlock_time {
            return Err(StakingError::LockPeriodNotExpired);
        }

        Ok(())
    }

    pub fn mark_withdrawn(&mut self, principal: &Principal, deposit_id: u64) -> Result<u64> {
        let user = self.users.get_mut(principal)
            .ok_or(StakingError::DepositNotFound)?;

        let deposit = user.deposits.iter_mut()
            .find(|d| d.id == deposit_id)
            .ok_or(StakingError::DepositNotFound)?;

        let amount = deposit.amount;
        deposit.withdrawn = true;
        user.total_staked = user.total_staked.saturating_sub(amount);
        self.total_staked = self.total_staked.saturating_sub(amount);

        Ok(amount)
    }

    pub fn calculate_proportional_amount(&self, total_amount: u64) -> Vec<(Principal, u64)> {
        if self.total_staked == 0 {
            return Vec::new();
        }

        self.users
            .iter()
            .filter(|(_, user)| user.total_staked > 0)
            .map(|(principal, user)| {
                let user_share = (user.total_staked as u128 * total_amount as u128) 
                    / self.total_staked as u128;
                (*principal, user_share as u64)
            })
            .collect()
    }

    pub fn apply_rewards(&mut self, distributions: &[(Principal, u64)]) {
        for (principal, amount) in distributions {
            if let Some(user) = self.users.get_mut(principal) {
                // Distribute reward proportionally across all active deposits
                let active_deposits: Vec<&mut Deposit> = user.deposits
                    .iter_mut()
                    .filter(|d| !d.withdrawn)
                    .collect();

                if !active_deposits.is_empty() {
                    let reward_per_deposit = amount / active_deposits.len() as u64;
                    for deposit in active_deposits {
                        deposit.amount += reward_per_deposit;
                    }
                    user.total_staked += amount;
                    self.total_staked += amount;
                }
            }
        }
    }

    pub fn apply_slash(&mut self, distributions: &[(Principal, u64)]) {
        for (principal, amount) in distributions {
            if let Some(user) = self.users.get_mut(principal) {
                // Slash proportionally across all active deposits
                let active_deposits: Vec<&mut Deposit> = user.deposits
                    .iter_mut()
                    .filter(|d| !d.withdrawn)
                    .collect();

                if !active_deposits.is_empty() {
                    let mut remaining_slash = *amount;
                    for deposit in active_deposits {
                        let slash_amount = remaining_slash.min(deposit.amount);
                        deposit.amount = deposit.amount.saturating_sub(slash_amount);
                        remaining_slash = remaining_slash.saturating_sub(slash_amount);
                        
                        if remaining_slash == 0 {
                            break;
                        }
                    }
                    let actual_slash = amount.saturating_sub(remaining_slash);
                    user.total_staked = user.total_staked.saturating_sub(actual_slash);
                    self.total_staked = self.total_staked.saturating_sub(actual_slash);
                }
            }
        }
    }
}