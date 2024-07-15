use std::time::{SystemTime, UNIX_EPOCH};
use dyn_type::DynType;
use rocksdb::compaction_filter::CompactionFilterFn;

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


pub fn create_ttl_filter<F>(ttl: u64) -> F
    where
        F: CompactionFilterFn + Send + 'static {
    let local_filter = move |level: u32, key: &[u8], value: &[u8]| {
        use rocksdb::CompactionDecision::*;
        const META_TABLE_ID: i64 = i64::min_value();
        let prefix = META_TABLE_ID.to_be_bytes();
        // Always keep the meta entry
        if key.starts_with(prefix) {
            return Keep;
        }
        let is_stale = false;
        if ttl > 0 {  // Data is fresh if TTL is non-positive
            let cur_time = get_unix_timestamp_sec();
            let ts = extract_timestamp_from_user_key(key, size_of::<u64>());
            let ts = decode_timestamp(ts);
            if ts + ttl >= cur_time {
                is_stale = true;
            }
        }
        if is_stale {
            Remove
        }
    };
    local_filter
}
