use candid::Principal;
use sha2::{Sha256, Digest};

pub fn principal_to_subaccount(principal: &Principal) -> [u8; 32] {
    let mut subaccount = [0u8; 32];
    let principal_bytes = principal.as_slice();
    
    // Use SHA256 to generate deterministic subaccount
    let mut hasher = Sha256::new();
    hasher.update(b"subaccount:");
    hasher.update(principal_bytes);
    let hash = hasher.finalize();
    
    // Copy first 32 bytes of hash to subaccount
    subaccount.copy_from_slice(&hash[..32]);
    subaccount
}

pub fn get_time_nanos() -> u64 {
    ic_cdk::api::time()
}