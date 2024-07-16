use std::time::{SystemTime, UNIX_EPOCH};
use std::convert::TryInto;

pub fn get_unix_timestamp_sec() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
}

pub fn extract_timestamp_from_user_key(user_key: &[u8], ts_sz: usize) -> &[u8] {
    &user_key[user_key.len() - ts_sz..]
}

#[inline]
pub fn encode_timestamp(ts: u64) -> [u8; 8] {
    ts.to_be_bytes()
}

pub fn get_current_timestamp() -> [u8; 8] {
    encode_timestamp(get_unix_timestamp_sec())
}

#[inline]
pub fn decode_timestamp(ptr: &[u8]) -> u64 {
    u64::from_be_bytes(ptr[..8].try_into().unwrap())
}

