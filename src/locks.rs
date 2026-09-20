//! Lock-recovery policy: one owner for the poisoned-lock rule, usable from
//! every layer (the api layer stays platform-agnostic, so these live
//! outside platform).

/// Recover the guarded data from a poisoned mutex and keep using it: under
/// the single-threaded message loop, poisoning implies the data is intact.
pub fn borrow<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// RwLock twins of borrow: same poisoned-lock recovery, one owner for the
/// policy — hand-rolled copies beside these helpers would diverge on any
/// future policy change.
pub fn borrow_read<T>(m: &std::sync::RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|e| e.into_inner())
}

pub fn borrow_write<T>(m: &std::sync::RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(|e| e.into_inner())
}
