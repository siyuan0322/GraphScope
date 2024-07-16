use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use std::sync::atomic::Ordering::Release;
use std::mem::size_of;

use ::rocksdb::backup::{BackupEngine, BackupEngineOptions, RestoreOptions};
use ::rocksdb::{DBRawIterator, Env, IngestExternalFileOptions, Options, ReadOptions, DB};
use crossbeam_epoch::{self as epoch, Atomic, Guard, Owned, Shared};
use super::ttl::encode_timestamp;
use super::ttl::get_unix_timestamp_sec;
use super::ttl::get_current_timestamp;

use rocksdb::{CompactOptions, WriteBatch};
use super::{StorageIter, StorageRes};
use crate::db::api::*;
use crate::db::storage::{KvPair, RawBytes};

pub struct RocksDB {
    db: Atomic<Arc<DB>>,
    options: HashMap<String, String>,
    is_secondary: bool,
    ttl: u64,
}

pub struct RocksDBBackupEngine {
    db: Arc<DB>,
    backup_engine: BackupEngine,
}

impl RocksDB {
    pub fn open(options: &HashMap<String, String>) -> GraphResult<Self> {
        let opts = init_options(options, false);
        let path = options
            .get("store.data.path")
            .expect("invalid config, missing store.data.path");
        let ttl: u64 = if let Some(ttl) = options.get("store.ttl.sec") {
            ttl.parse().unwrap()
        } else {
            0u64
        };
        let db = DB::open(&opts, path).map_err(|e| {
            let msg = format!("open rocksdb at {} failed: {}", path, e.into_string());
            gen_graph_err!(GraphErrorCode::ExternalStorageError, msg, open, options, path)
        })?;
        let ret = RocksDB { db: Atomic::new(Arc::new(db)), options: options.clone(), is_secondary: false, ttl: ttl };
        Ok(ret)
    }

    pub fn open_as_secondary(options: &HashMap<String, String>) -> GraphResult<Self> {
        let db = RocksDB::open_secondary_helper(options, false).map_err(|e| {
            let msg = format!("open rocksdb at {:?}, error: {:?}", options, e);
            gen_graph_err!(GraphErrorCode::ExternalStorageError, msg, open_as_secondary)
        })?;
        let ttl: u64 = if let Some(ttl) = options.get("store.ttl.sec") {
            ttl.parse().unwrap()
        } else {
            0u64
        };

        let ret = RocksDB { db: Atomic::new(Arc::new(db)), options: options.clone(), is_secondary: true, ttl: ttl };
        Ok(ret)
    }

    pub fn open_secondary_helper(options: &HashMap<String, String>, reopen: bool) -> Result<DB, ::rocksdb::Error> {
        let path = options
            .get("store.data.path")
            .expect("invalid config, missing store.data.path");
        let mut sec_path = options
            .get("store.data.secondary.path")
            .expect("invalid config, missing store.data.secondary.path")
            .clone();
        if reopen {
            while Path::new(&sec_path).exists() {
                sec_path = format!("{}_1", sec_path);
            }
        }
        let opts = init_options(options, true);
        info!("Opening secondary at {}, {}", path, sec_path);
        DB::open_as_secondary(&opts, path, &sec_path)
    }

    fn get_db<'g>(&self, guard: &'g Guard) -> Shared<'g, Arc<DB>> {
        self.db.load(Ordering::Acquire, guard)
    }

    fn replace_db(&self, db: DB) {
        let guard = &epoch::pin();
        let new_db = Arc::new(db);
        let new_db_shared = Owned::new(new_db).into_shared(guard);
        let old_db_shared = self.db.swap(new_db_shared, Release, guard);

        let default = "".to_string();
        let path = self
            .options
            .get("store.data.path")
            .unwrap_or(&default);
        // Use Crossbeam's 'defer' mechanism to safely drop the old Arc
        unsafe {
            // Convert 'Shared' back to 'Arc' for deferred dropping
            // guard.defer_destroy(old_db_shared)
            guard.defer_unchecked(move || {
                info!("Dropped RocksDB {:}", path);
                drop(old_db_shared.into_owned())
            })
        }
        // To force any deferred work to run, we need the epoch to move forward two times.
        epoch::pin().flush();
        epoch::pin().flush();
        info!("RocksDB {:} replaced", path);
    }

    pub fn get(&self, key: &[u8]) -> GraphResult<Option<StorageRes>> {
        let guard = epoch::pin();
        let db_shared = self.get_db(&guard);
        if let Some(db) = unsafe { db_shared.as_ref() } {
            let ret = if self.ttl > 0 {
                let cur_ts = &get_current_timestamp();
                let mut opt = ReadOptions::default();
                opt.set_timestamp(cur_ts);
                db.get_opt(key, &opt)
            } else {
                db.get(key)
            };
            match ret {
                Ok(Some(v)) => Ok(Some(StorageRes::RocksDB(v))),
                Ok(None) => Ok(None),
                Err(e) => {
                    let msg = format!("rocksdb.get failed because {}", e.into_string());
                    let err = gen_graph_err!(GraphErrorCode::ExternalStorageError, msg);
                    Err(err)
                }
            }
        } else {
            let msg = format!("rocksdb.get failed because the acquired db is `None`");
            let err = gen_graph_err!(GraphErrorCode::ExternalStorageError, msg);
            Err(err)
        }
    }

    pub fn put(&self, key: &[u8], val: &[u8]) -> GraphResult<()> {
        if self.is_secondary {
            info!("Cannot put in secondary instance");
            return Ok(());
        }
        let guard = epoch::pin();
        let db_shared = self.get_db(&guard);
        if let Some(db) = unsafe { db_shared.as_ref() } {
            let ret = if self.ttl > 0 {
                let cur_ts = &get_current_timestamp();
                db.put_with_ts(key, cur_ts, val)
            } else {
                db.put(key, val)
            };
            ret.map_err(|e| {
                let msg = format!("rocksdb.put failed because {}", e.into_string());
                gen_graph_err!(GraphErrorCode::ExternalStorageError, msg)
            })
        } else {
            let msg = format!("rocksdb.put failed because the acquired db is `None`");
            let err = gen_graph_err!(GraphErrorCode::ExternalStorageError, msg);
            Err(err)
        }
    }

    pub fn delete(&self, key: &[u8]) -> GraphResult<()> {
        if self.is_secondary {
            info!("Cannot delete in secondary instance");
            return Ok(());
        }
        let guard = epoch::pin();
        let db_shared = self.get_db(&guard);
        if let Some(db) = unsafe { db_shared.as_ref() } {
            let ret = if self.ttl > 0 {
                let cur_ts = &get_current_timestamp();
                db.delete_with_ts(key, cur_ts)
            } else {
                db.delete(key)
            };
            ret.map_err(|e| {
                let msg = format!("rocksdb.delete failed because {}", e.into_string());
                gen_graph_err!(GraphErrorCode::ExternalStorageError, msg)
            })
        } else {
            let msg = format!("rocksdb.delete failed because the acquired db is `None`");
            let err = gen_graph_err!(GraphErrorCode::ExternalStorageError, msg);
            Err(err)
        }
    }

    pub fn scan_prefix(&self, prefix: &[u8]) -> GraphResult<StorageIter> {
        let end = bytes_upper_bound(prefix);
        self.scan_range_impl(prefix, end)
    }

    pub fn scan_from(&self, start: &[u8]) -> GraphResult<StorageIter> {
        self.scan_range_impl(start, None)
    }

    pub fn scan_range(&self, start: &[u8], end: &[u8]) -> GraphResult<StorageIter> {
        self.scan_range_impl(start, Some(end.to_vec()))
    }

    pub fn scan_range_impl(&self, start: &[u8], end: Option<Vec<u8>>) -> GraphResult<StorageIter> {
        let guard = epoch::pin();
        let db_shared = self.get_db(&guard);
        if let Some(db) = unsafe { db_shared.as_ref() } {
            Ok(StorageIter::RocksDB(RocksDBIter::new_range_impl(db.clone(), start, end, self.ttl, guard)))
        } else {
            let msg = format!("rocksdb.new_range failed because the acquired db is `None`");
            let err = gen_graph_err!(GraphErrorCode::ExternalStorageError, msg);
            Err(err)
        }
    }

    pub fn delete_range(&self, start: &[u8], end: &[u8]) -> GraphResult<()> {
        if self.is_secondary {
            info!("Cannot delete_range in secondary instance");
            return Ok(());
        }
        let mut batch = WriteBatch::default();
        let guard = epoch::pin();
        let db_shared = self.get_db(&guard);
        if let Some(db) = unsafe { db_shared.as_ref() } {
            // db.delete_file_in_range(start, end);
            batch.delete_range(start, end);
            db.write(batch).map_err(|e| {
                let msg = format!("rocksdb.delete_range failed because {}", e.into_string());
                gen_graph_err!(GraphErrorCode::ExternalStorageError, msg)
            })?;
            let mut val = false;
            if let Some(conf_str) = self
                .options
                .get("store.rocksdb.disable.auto.compactions")
            {
                val = conf_str.parse::<bool>().unwrap();
            }
            if !val {
                db.compact_range(Option::Some(start), Option::Some(end))
            }
            Ok(())
        } else {
            let msg = format!("rocksdb.delete_range failed because the acquired db is `None`");
            let err = gen_graph_err!(GraphErrorCode::ExternalStorageError, msg);
            Err(err)
        }
    }

    pub fn compact(&self) -> GraphResult<()> {
        info!("begin to compact rocksdb");
        if self.is_secondary {
            info!("Cannot compact in secondary instance");
            return Ok(());
        }
        let guard = epoch::pin();
        let db_shared = self.get_db(&guard);
        let mut opts = CompactOptions::default();
        if self.ttl > 0 {
            let expired_ts = get_unix_timestamp_sec() - self.ttl;
            let ts = &encode_timestamp(expired_ts);
            opts.set_full_history_ts_low(ts);
        }
        if let Some(db) = unsafe { db_shared.as_ref() } {
            db.compact_range_opt(None::<&[u8]>, None::<&[u8]>, &opts);
            info!("compacted rocksdb");
            Ok(())
        } else {
            let msg = format!("rocksdb.compact failed because the acquired db is `None`");
            let err = gen_graph_err!(GraphErrorCode::ExternalStorageError, msg);
            Err(err)
        }
    }

    pub fn load(&self, files: &[&str]) -> GraphResult<()> {
        if self.is_secondary {
            info!("Cannot ingest in secondary instance");
            return Ok(());
        }
        let mut options = IngestExternalFileOptions::default();
        options.set_move_files(true);
        let guard = epoch::pin();
        let db_shared = self.get_db(&guard);
        if let Some(db) = unsafe { db_shared.as_ref() } {
            db.ingest_external_file_opts(&options, files.to_vec())
                .map_err(|e| {
                    let msg = format!("rocksdb.load file {:?} failed because {}", files, e.into_string());
                    gen_graph_err!(GraphErrorCode::ExternalStorageError, msg)
                })
        } else {
            let msg = format!("rocksdb.load failed because the acquired db is `None`");
            let err = gen_graph_err!(GraphErrorCode::ExternalStorageError, msg);
            Err(err)
        }
    }

    pub fn open_backup_engine(&self, backup_path: &str) -> GraphResult<Box<RocksDBBackupEngine>> {
        let backup_opts = BackupEngineOptions::new(backup_path).map_err(|e| {
            let msg = format!(
                "Gen BackupEngineOptions error for path {}, because {}",
                backup_path.to_string(),
                e.into_string()
            );
            gen_graph_err!(GraphErrorCode::ExternalStorageError, msg)
        })?;
        let env = Env::new().map_err(|e| {
            let msg = format!("Gen rocksdb Env failed because {}", e.into_string());
            gen_graph_err!(GraphErrorCode::ExternalStorageError, msg)
        })?;
        let backup_engine = BackupEngine::open(&backup_opts, &env).map_err(|e| {
            let msg = format!(
                "open rocksdb backup engine at {} failed, because {}",
                backup_path.to_string(),
                e.into_string()
            );
            gen_graph_err!(GraphErrorCode::ExternalStorageError, msg)
        })?;
        let guard = epoch::pin();
        let db_shared = self.get_db(&guard);
        if let Some(db) = unsafe { db_shared.as_ref() } {
            let ret = RocksDBBackupEngine { db: db.clone(), backup_engine };
            Ok(Box::from(ret))
        } else {
            let msg = format!("open rocksdb backup engine failed because the acquired db is `None`");
            let err = gen_graph_err!(GraphErrorCode::ExternalStorageError, msg);
            Err(err)
        }
    }

    pub fn new_scan(&self, prefix: &[u8]) -> GraphResult<Box<dyn Iterator<Item = KvPair> + Send>> {
        let guard = epoch::pin();
        let db_shared = self.get_db(&guard);
        if let Some(db) = unsafe { db_shared.as_ref() } {
            Ok(Box::new(Scan::new(db.clone(), prefix, self.ttl, guard)))
        } else {
            let msg = format!("rocksdb.new_scan failed because the acquired db is `None`");
            let err = gen_graph_err!(GraphErrorCode::ExternalStorageError, msg);
            Err(err)
        }
    }

    pub fn try_catch_up_with_primary(&self) -> GraphResult<()> {
        if !self.is_secondary {
            return Ok(());
        }
        let guard = epoch::pin();
        let db_shared = self.get_db(&guard);
        if let Some(db) = unsafe { db_shared.as_ref() } {
            db.try_catch_up_with_primary().map_err(|e| {
                let msg = format!("rocksdb.try_catch_up_with_primary failed because {:?}", e);
                gen_graph_err!(GraphErrorCode::ExternalStorageError, msg)
            })
        } else {
            let msg = format!("rocksdb.try_catch_up_with_primary failed because the acquired db is `None`");
            let err = gen_graph_err!(GraphErrorCode::ExternalStorageError, msg);
            Err(err)
        }
    }

    pub fn reopen(&self, wait_sec: u64) -> GraphResult<()> {
        if !self.is_secondary {
            return Ok(());
        }
        loop {
            std::thread::sleep(Duration::from_secs(wait_sec));
            let db = RocksDB::open_secondary_helper(&self.options, true).map_err(|e| {
                let msg = format!("open rocksdb at {:?}, error: {:?}", self.options, e);
                gen_graph_err!(GraphErrorCode::ExternalStorageError, msg, open_as_secondary)
            })?;
            let ret = db.try_catch_up_with_primary();
            if ret.is_err() {
                error!("New secondary catch up failed: {:?}", ret);
                drop(db);
            } else {
                info!("RocksDB secondary instance reopened");
                self.replace_db(db);
                break;
            }
        }
        Ok(())
    }
}

pub struct Scan<'a> {
    inner_iter: RocksDBIter<'a>,
}

impl<'a> Scan<'a> {
    pub fn new(db: Arc<DB>, prefix: &[u8], ttl: u64,  guard: Guard) -> Self {
        Scan { inner_iter: RocksDBIter::new_prefix(db, prefix, ttl, guard) }
    }
}

impl<'a> Iterator for Scan<'a> {
    type Item = KvPair;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner_iter
            .next()
            .map(|(k, v)| (RawBytes::new(k), RawBytes::new(v)))
    }
}

impl RocksDBBackupEngine {
    /// Optimize this method after a new rust-rocksdb version.
    pub fn create_new_backup(&mut self) -> GraphResult<BackupId> {
        let before = self.get_backup_list();
        self.backup_engine
            .create_new_backup(&self.db)
            .map_err(|e| {
                let msg = format!("create new rocksdb backup failed, because {}", e.into_string());
                gen_graph_err!(GraphErrorCode::ExternalStorageError, msg)
            })?;
        let after = self.get_backup_list();
        if after.len() != before.len() + 1 {
            let msg = "get new created rocksdb backup id failed".to_string();
            return Err(gen_graph_err!(GraphErrorCode::ExternalStorageError, msg));
        }
        let new_backup_id = *after.iter().max().unwrap();
        Ok(new_backup_id)
    }

    /// Do nothing now.
    /// Implement this method after a new rust-rocksdb version.
    #[allow(unused_variables)]
    pub fn delete_backup(&mut self, backup_id: BackupId) -> GraphResult<()> {
        Ok(())
    }

    pub fn restore_from_backup(&mut self, restore_path: &str, backup_id: BackupId) -> GraphResult<()> {
        let mut restore_option = RestoreOptions::default();
        restore_option.set_keep_log_files(false);
        self.backup_engine
            .restore_from_backup(restore_path, restore_path, &restore_option, backup_id as u32)
            .map_err(|e| {
                let msg = format!(
                    "restore from rocksdb backup {} failed, because {}",
                    backup_id,
                    e.into_string()
                );
                gen_graph_err!(GraphErrorCode::ExternalStorageError, msg)
            })?;
        Ok(())
    }

    pub fn verify_backup(&self, backup_id: BackupId) -> GraphResult<()> {
        self.backup_engine
            .verify_backup(backup_id as u32)
            .map_err(|e| {
                let msg =
                    format!("rocksdb backup {} verify failed, because {}", backup_id, e.into_string());
                gen_graph_err!(GraphErrorCode::ExternalStorageError, msg)
            })?;
        Ok(())
    }

    pub fn get_backup_list(&self) -> Vec<BackupId> {
        self.backup_engine
            .get_backup_info()
            .into_iter()
            .map(|info| info.backup_id as BackupId)
            .collect()
    }
}

#[allow(unused_variables)]
fn init_options(options: &HashMap<String, String>, is_secondary: bool) -> Options {
    let mut opts = Options::default();
    if is_secondary {
        opts.set_max_open_files(-1);
    } else {
        opts.create_if_missing(true);
        opts.set_max_background_jobs(6);
        opts.set_write_buffer_size(256 << 20);
        opts.set_max_open_files(-1);
        opts.set_keep_log_file_num(10);
        // https://github.com/facebook/rocksdb/wiki/Basic-Operations#non-sync-writes
        opts.set_use_fsync(true);
        opts.set_level_compaction_dynamic_level_bytes(true);
        opts.set_bytes_per_sync(1048576);

        if let Some(conf_str) = options.get("store.rocksdb.disable.auto.compactions") {
            let val = conf_str.parse().unwrap();
            opts.set_disable_auto_compactions(val);
        }

        if let Some(conf_str) = options.get("store.rocksdb.write.buffer.mb") {
            let size_bytes: usize = conf_str.parse::<usize>().unwrap() * 1024 * 1024;
            opts.set_write_buffer_size(size_bytes);
        }
        if let Some(conf_str) = options.get("store.rocksdb.max.write.buffer.num") {
            opts.set_max_write_buffer_number(conf_str.parse().unwrap());
        } else {
            opts.set_max_write_buffer_number(4);
        }
        if let Some(conf_str) = options.get("store.rocksdb.level0.compaction.trigger") {
            opts.set_level_zero_file_num_compaction_trigger(conf_str.parse().unwrap());
        }
        if let Some(conf_str) = options.get("store.rocksdb.max.level.base.mb") {
            let size_bytes: u64 = conf_str.parse::<u64>().unwrap() * 1024 * 1024;
            opts.set_max_bytes_for_level_base(size_bytes);
        }
        if let Some(conf_str) = options.get("store.rocksdb.background.jobs") {
            let background_jobs = conf_str.parse().unwrap();
            opts.set_max_background_jobs(background_jobs);
        }
        if let Some(conf_str) = options.get("store.rocksdb.paranoid.checks") {
            let check = conf_str.parse().unwrap();
            opts.set_paranoid_checks(check);
        }
    }
    // general configuration
    if let Some(conf_str) = options.get("store.rocksdb.wal.dir") {
        opts.set_wal_dir(Path::new(conf_str));
    }
    if let Some(ttl) = options.get("store.ttl.sec") {
        let ttl: u64 = ttl.parse().unwrap();
        if ttl > 0 {
            let local_compare = move |one: &[u8], two: &[u8]| one.cmp(two);
            opts.set_comparator_with_ts("bytewise_comparator_with_ts", Box::new(local_compare));
            let local_filter = move |level: u32, key: &[u8], value: &[u8]| {
                use rocksdb::CompactionDecision::*;
                const META_TABLE_ID: i64 = i64::min_value();
                let prefix = META_TABLE_ID.to_be_bytes();
                // Always keep the meta entry
                if key.starts_with(&prefix) {
                    info!("not filtering meta entry");
                    return Keep;
                }
                info!("key: {:?}", key);
                let mut is_stale = false;
                if ttl > 0 {  // Data is fresh if TTL is non-positive
                    let cur_time = get_unix_timestamp_sec();
                    let ts = super::ttl::extract_timestamp_from_user_key(key, size_of::<u64>());
                    info!("ts bytes: {:?}", ts);
                    let ts = super::ttl::decode_timestamp(ts);
                    if ts + ttl < cur_time {
                        is_stale = true;
                    }
                    info!("key: {:?}, ts: {:?}, ttl: {:?}, cur_time: {:?}, is_stale {}", key, ts, ttl, cur_time, is_stale);
                }
                if is_stale {
                    Remove
                } else {
                    Keep
                }
            };
            opts.set_compaction_filter("filter_with_ts", local_filter);
        }
    }
    opts
}

pub struct RocksDBIter<'a> {
    _db: Arc<DB>,
    inner: Option<DBRawIterator<'a>>,
    just_seeked: bool,
    _guard: Guard,
}

unsafe impl Send for RocksDBIter<'_> {}

impl<'a> RocksDBIter<'a> {
    fn new_prefix(db: Arc<DB>, prefix: &[u8], ttl: u64, guard: Guard) -> Self {
        let end = bytes_upper_bound(prefix);
        RocksDBIter::new_range_impl(db, prefix, end, ttl, guard)
    }

    fn new_range(db: Arc<DB>, start: &[u8], end: &[u8], ttl: u64, guard: Guard) -> Self {
        RocksDBIter::new_range_impl(db, start, Some(end.to_vec()), ttl, guard)
    }

    fn new_range_impl(db: Arc<DB>, start: &[u8], end: Option<Vec<u8>>, ttl: u64, guard: Guard) -> Self {
        let db_ptr = Arc::into_raw(db.clone()) as *const DB;
        let mut db_iter = Self { _db: db, inner: None, just_seeked: true, _guard: guard };
        let db_ref = unsafe { &*db_ptr };
        let mut option = ReadOptions::default();
        if let Some(end) = end {
            option.set_iterate_upper_bound(end.to_vec());
        }
        if ttl > 0 {
            let ts = &get_current_timestamp();
            option.set_timestamp(ts);
        }
        let mut iter = db_ref.raw_iterator_opt(option);
        iter.seek(start);

        db_iter.inner = Some(iter);

        db_iter
    }

    pub fn next(&mut self) -> Option<(&[u8], &[u8])> {
        if let Some(inner) = &mut self.inner {
            if !inner.valid() {
                return None;
            }

            if self.just_seeked {
                self.just_seeked = false;
            } else {
                inner.next();
            }

            if inner.valid() {
                Some((inner.key().unwrap(), inner.value().unwrap()))
            } else {
                None
            }
        } else {
            None
        }
    }
}

fn bytes_upper_bound(bytes: &[u8]) -> Option<Vec<u8>> {
    for i in (0..bytes.len()).rev() {
        if bytes[i] != u8::MAX {
            let mut ret = bytes.to_vec();
            ret[i] += 1;
            for j in i + 1..bytes.len() {
                ret[j] = 0;
            }
            return Some(ret);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::common::bytes::transform;
    use crate::db::util::fs;

    #[test]
    fn test_rocksdb_iter() {
        let path = "test_rocksdb_iter";
        {
            let mut config = HashMap::new();
            config.insert("store.data.path".to_owned(), path.to_owned());
            let db = RocksDB::open(&config).unwrap();
            let mut ans = Vec::new();
            for i in 1..=10 {
                let key = format!("aaa#{:010}", i);
                db.put(key.as_bytes(), i.to_string().as_bytes())
                    .unwrap();
                ans.push((key, i));
            }
            let mut iter = db.scan_prefix(b"aaa").unwrap();
            for (key, i) in ans {
                let (k, v) = iter.next().unwrap();
                assert_eq!(key, String::from_utf8(k.to_vec()).unwrap());
                assert_eq!(
                    i,
                    String::from_utf8(v.to_vec())
                        .unwrap()
                        .parse::<i32>()
                        .unwrap()
                );
            }
            assert!(iter.next().is_none());

            let mut iter = db.scan_prefix(b"zzz").unwrap();
            assert!(iter.next().is_none());
        }
        fs::rmr(path).unwrap();
    }

    #[test]
    fn test_rocksdb_scan_from() {
        let path = "test_rocksdb_scan_from";
        fs::rmr(path).unwrap();
        {
            let mut config = HashMap::new();
            config.insert("store.data.path".to_owned(), path.to_owned());
            let db = RocksDB::open(&config).unwrap();
            for i in 1..=20 {
                if i % 2 == 0 {
                    let key = format!("aaa#{:010}", i);
                    db.put(key.as_bytes(), transform::i64_to_vec(i).as_slice())
                        .unwrap();
                }
            }

            for i in 1..=20 {
                let key = format!("aaa#{:010}", i);
                let ans = format!("aaa#{:010}", (i + 1) / 2 * 2);
                let mut iter = db.scan_from(key.as_bytes()).unwrap();
                let (k, v) = iter.next().unwrap();
                assert_eq!(k, ans.as_bytes());
                assert_eq!(transform::bytes_to_i64(v).unwrap(), (i + 1) / 2 * 2);
            }
        }
        fs::rmr(path).unwrap();
    }
}
