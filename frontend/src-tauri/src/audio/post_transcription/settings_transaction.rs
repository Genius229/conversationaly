//! Cache-and-disk transaction for settings stores with debounced auto-save.

use serde_json::Value;

pub trait SettingsStore {
    type Error;

    fn get(&self, key: &str) -> Option<Value>;
    fn set(&self, key: &str, value: Value);
    fn delete(&self, key: &str);
    fn save(&self) -> Result<(), Self::Error>;
}

#[derive(Debug)]
pub struct SaveWithRollbackError<E> {
    pub original: E,
    pub rollback_error: Option<E>,
}

/// Persist one value without leaving a rejected choice in the native cache.
///
/// Both explicit saves are required for tauri-plugin-store 2.4.x: `set` and
/// `delete` schedule a debounced auto-save, while `save` first cancels that
/// pending task. The caller serializes this whole transaction with reads and
/// other writes so a rollback cannot clobber a later successful update.
pub fn save_with_rollback<S: SettingsStore + ?Sized>(
    store: &S,
    key: &str,
    next: Value,
) -> Result<(), SaveWithRollbackError<S::Error>> {
    let previous = store.get(key);
    store.set(key, next);
    if let Err(original) = store.save() {
        match previous {
            Some(value) => store.set(key, value),
            None => store.delete(key),
        }
        let rollback_error = store.save().err();
        return Err(SaveWithRollbackError {
            original,
            rollback_error,
        });
    }
    Ok(())
}
