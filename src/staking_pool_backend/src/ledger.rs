use candid::Principal;
use ic_ledger_types::{
    AccountIdentifier, Tokens, DEFAULT_FEE, MAINNET_LEDGER_CANISTER_ID,
    TransferArgs, TransferResult, Subaccount,
};

pub async fn transfer_from_subaccount(
    from_subaccount: [u8; 32],
    to: Principal,
    amount: u64,
) -> Result<u64, String> {
    let transfer_args = TransferArgs {
        memo: ic_ledger_types::Memo(0),
        amount: Tokens::from_e8s(amount.saturating_sub(DEFAULT_FEE.e8s())),
        fee: DEFAULT_FEE,
        from_subaccount: Some(Subaccount(from_subaccount)),
        to: AccountIdentifier::new(&to, &Default::default()),
        created_at_time: None,
    };

    match ic_cdk::call::<(TransferArgs,), (TransferResult,)>(
        MAINNET_LEDGER_CANISTER_ID,
        "transfer",
        (transfer_args,),
    )
    .await
    {
        Ok((result,)) => match result {
            Ok(block_index) => Ok(block_index),
            Err(err) => Err(format!("Transfer failed: {:?}", err)),
        },
        Err((code, msg)) => Err(format!("Call failed: {:?} - {}", code, msg)),
    }
}

pub async fn get_balance(subaccount: [u8; 32]) -> Result<u64, String> {
    let account = AccountIdentifier::new(&ic_cdk::id(), &Subaccount(subaccount));
    
    match ic_cdk::call::<(AccountIdentifier,), (Tokens,)>(
        MAINNET_LEDGER_CANISTER_ID,
        "account_balance",
        (account,),
    )
    .await
    {
        Ok((balance,)) => Ok(balance.e8s()),
        Err((code, msg)) => Err(format!("Balance query failed: {:?} - {}", code, msg)),
    }
}