use std::{cell::RefCell, collections::VecDeque};

use app_lib::settings_transaction::{save_with_rollback, SettingsStore};
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
enum Operation {
    Get,
    Set(Value),
    Delete,
    Save,
}

struct FakeStore {
    cached: RefCell<Option<Value>>,
    pending_auto_save: RefCell<bool>,
    save_results: RefCell<VecDeque<Result<(), &'static str>>>,
    operations: RefCell<Vec<Operation>>,
}

impl FakeStore {
    fn new(previous: Option<Value>, save_results: Vec<Result<(), &'static str>>) -> Self {
        Self {
            cached: RefCell::new(previous),
            pending_auto_save: RefCell::new(false),
            save_results: RefCell::new(save_results.into()),
            operations: RefCell::new(Vec::new()),
        }
    }
}

impl SettingsStore for FakeStore {
    type Error = &'static str;

    fn get(&self, _key: &str) -> Option<Value> {
        self.operations.borrow_mut().push(Operation::Get);
        self.cached.borrow().clone()
    }

    fn set(&self, _key: &str, value: Value) {
        self.operations
            .borrow_mut()
            .push(Operation::Set(value.clone()));
        *self.cached.borrow_mut() = Some(value);
        *self.pending_auto_save.borrow_mut() = true;
    }

    fn delete(&self, _key: &str) {
        self.operations.borrow_mut().push(Operation::Delete);
        *self.cached.borrow_mut() = None;
        *self.pending_auto_save.borrow_mut() = true;
    }

    fn save(&self) -> Result<(), Self::Error> {
        self.operations.borrow_mut().push(Operation::Save);
        // Matches tauri-plugin-store 2.4.4: explicit save cancels a pending
        // debounced auto-save before attempting the disk write.
        *self.pending_auto_save.borrow_mut() = false;
        self.save_results.borrow_mut().pop_front().unwrap_or(Ok(()))
    }
}

#[test]
fn failed_save_restores_the_previous_cache_and_cancels_rejected_auto_save() {
    let previous = json!({"vad_enabled": true});
    let next = json!({"vad_enabled": false});
    let store = FakeStore::new(Some(previous.clone()), vec![Err("write failed"), Ok(())]);

    let error = save_with_rollback(&store, "settings", next.clone()).unwrap_err();

    assert_eq!(error.original, "write failed");
    assert_eq!(error.rollback_error, None);
    assert_eq!(*store.cached.borrow(), Some(previous.clone()));
    assert!(!*store.pending_auto_save.borrow());
    assert_eq!(
        *store.operations.borrow(),
        vec![
            Operation::Get,
            Operation::Set(next),
            Operation::Save,
            Operation::Set(previous),
            Operation::Save,
        ]
    );
}

#[test]
fn failed_first_save_restores_an_absent_value_even_if_disk_rollback_fails() {
    let next = json!({"vad_enabled": false});
    let store = FakeStore::new(None, vec![Err("write failed"), Err("rollback failed")]);

    let error = save_with_rollback(&store, "settings", next.clone()).unwrap_err();

    assert_eq!(error.original, "write failed");
    assert_eq!(error.rollback_error, Some("rollback failed"));
    assert_eq!(*store.cached.borrow(), None);
    assert!(!*store.pending_auto_save.borrow());
    assert_eq!(
        *store.operations.borrow(),
        vec![
            Operation::Get,
            Operation::Set(next),
            Operation::Save,
            Operation::Delete,
            Operation::Save,
        ]
    );
}

#[test]
fn successful_save_keeps_the_new_cache_without_a_rollback_write() {
    let previous = json!({"vad_enabled": true});
    let next = json!({"vad_enabled": false});
    let store = FakeStore::new(Some(previous), vec![Ok(())]);

    save_with_rollback(&store, "settings", next.clone()).unwrap();

    assert_eq!(*store.cached.borrow(), Some(next.clone()));
    assert!(!*store.pending_auto_save.borrow());
    assert_eq!(
        *store.operations.borrow(),
        vec![Operation::Get, Operation::Set(next), Operation::Save]
    );
}
