//! CONF-05 callback choreography tests - the deliberate strengthening over
//! the gem's specs (`shared_examples_for_callbacks.rb` only asserts hooks
//! FIRE via `should_receive`; we additionally lock the veto ordering):
//!
//! * success: `before_add` -> side effect -> `after_add` (remove mirrors)
//! * veto: `before_*` returning `Err` aborts BEFORE the side effect and the
//!   matching `after_*` NEVER runs
//!
//! The gem wiring reference is `rolify/lib/rolify.rb:28` +
//! `role.rb:8-25` (`add_role`/`remove_role` choreography).

use std::sync::{Arc, Mutex};

use super::*;
use crate::resource::Resource;
use crate::role::{ResourceId, RoleRecord};

/// Shared ordering probe: hooks and the stub driver append labels here.
type Probe = Arc<Mutex<Vec<&'static str>>>;

fn new_probe() -> Probe {
    Arc::new(Mutex::new(Vec::new()))
}

fn recorded(probe: &Probe) -> Vec<&'static str> {
    let events = probe.lock().expect("probe lock poisoned");
    events.clone()
}

/// Minimal stub of the 01-04 `RolifyUser::add_role` choreography:
/// `run_before_add` (veto point) -> storage side effect -> `run_after_add`.
fn drive_add(
    config: &RolifyConfig,
    record: &RoleRecord,
    events: &Probe,
) -> Result<(), RolifyError> {
    config.run_before_add(record)?;
    events
        .lock()
        .expect("probe lock poisoned")
        .push("store.add");
    config.run_after_add(record);
    Ok(())
}

/// Same choreography for `remove_role`.
fn drive_remove(
    config: &RolifyConfig,
    record: &RoleRecord,
    events: &Probe,
) -> Result<(), RolifyError> {
    config.run_before_remove(record)?;
    events
        .lock()
        .expect("probe lock poisoned")
        .push("store.remove");
    config.run_after_remove(record);
    Ok(())
}

fn vetoing_hook(callback: &'static str, events: &Probe) -> BeforeHook {
    let writer = Arc::clone(events);
    Arc::new(move |_record: &RoleRecord| {
        writer.lock().expect("probe lock poisoned").push(callback);
        Err(RolifyError::CallbackVeto {
            callback,
            reason: "vetoed in test".into(),
        })
    })
}

fn notify_hook(label: &'static str, events: &Probe) -> BeforeHook {
    let writer = Arc::clone(events);
    Arc::new(move |_record: &RoleRecord| {
        writer.lock().expect("probe lock poisoned").push(label);
        Ok(())
    })
}

fn after_hook(label: &'static str, events: &Probe) -> AfterHook {
    let writer = Arc::clone(events);
    Arc::new(move |_record: &RoleRecord| {
        writer.lock().expect("probe lock poisoned").push(label);
    })
}

#[test]
fn add_hooks_fire_in_order_around_the_side_effect() {
    let events = new_probe();
    let config = RolifyConfig::builder()
        .before_add(notify_hook("before_add", &events))
        .after_add(after_hook("after_add", &events))
        .build()
        .unwrap();
    let record = RoleRecord::global("admin");

    drive_add(&config, &record, &events).unwrap();

    assert_eq!(
        recorded(&events),
        vec!["before_add", "store.add", "after_add"]
    );
}

#[test]
fn before_add_veto_aborts_and_skips_after_add() {
    let events = new_probe();
    let config = RolifyConfig::builder()
        .before_add(vetoing_hook("before_add", &events))
        .after_add(after_hook("after_add", &events))
        .build()
        .unwrap();
    let record = RoleRecord::global("admin");

    let outcome = drive_add(&config, &record, &events);

    assert!(matches!(
        outcome,
        Err(RolifyError::CallbackVeto {
            callback: "before_add",
            ..
        })
    ));
    assert_eq!(
        recorded(&events),
        vec!["before_add"],
        "neither the side effect nor after_add may run after a veto"
    );
}

#[test]
fn remove_hooks_fire_in_order_around_the_side_effect() {
    let events = new_probe();
    let config = RolifyConfig::builder()
        .before_remove(notify_hook("before_remove", &events))
        .after_remove(after_hook("after_remove", &events))
        .build()
        .unwrap();
    let record = RoleRecord::for_class("manager", "Forum");

    drive_remove(&config, &record, &events).unwrap();

    assert_eq!(
        recorded(&events),
        vec!["before_remove", "store.remove", "after_remove"]
    );
}

#[test]
fn before_remove_veto_aborts_and_skips_after_remove() {
    let events = new_probe();
    let config = RolifyConfig::builder()
        .before_remove(vetoing_hook("before_remove", &events))
        .after_remove(after_hook("after_remove", &events))
        .build()
        .unwrap();
    let record = RoleRecord::for_class("manager", "Forum");

    let outcome = drive_remove(&config, &record, &events);

    assert!(matches!(
        outcome,
        Err(RolifyError::CallbackVeto {
            callback: "before_remove",
            ..
        })
    ));
    assert_eq!(recorded(&events), vec!["before_remove"]);
}

#[test]
fn after_hooks_receive_the_record_on_success() {
    let events = new_probe();
    let names_seen = Arc::new(Mutex::new(Vec::new()));
    let names_writer = Arc::clone(&names_seen);
    let config = RolifyConfig::builder()
        .after_add(Arc::new(move |record: &RoleRecord| {
            names_writer
                .lock()
                .expect("probe lock poisoned")
                .push(record.name.as_str().to_owned());
        }))
        .build()
        .unwrap();
    let record = RoleRecord::for_instance("moderator", "Forum", 7_i64);

    drive_add(&config, &record, &events).unwrap();

    assert_eq!(
        names_seen.lock().expect("probe lock poisoned").as_slice(),
        ["moderator"]
    );
}

/// CONF-03: custom entity names (the gem's `customers`/`privileges` corpus,
/// `spec/support/schema.rb`) flow through plain generics - no reflection,
/// no stringly-typed dispatch anywhere in the callback path.
#[test]
fn custom_entity_types_flow_through_generics() {
    struct Customer {
        id: i32,
    }
    impl Resource for Customer {
        fn type_name() -> &'static str {
            "Customer"
        }
        fn resource_id(&self) -> ResourceId {
            ResourceId::from(i64::from(self.id))
        }
    }

    struct Privilege {
        code: String,
    }
    impl Resource for Privilege {
        fn type_name() -> &'static str {
            "Privilege"
        }
        fn resource_id(&self) -> ResourceId {
            ResourceId::from(self.code.clone())
        }
    }

    let events = new_probe();
    let config = RolifyConfig::builder()
        .role_table("privileges")
        .join_table("customers_privileges")
        .before_add(notify_hook("before_add", &events))
        .after_add(after_hook("after_add", &events))
        .build()
        .unwrap();

    let privilege = Privilege {
        code: "VIP-9".into(),
    };
    let record =
        RoleRecord::for_instance("auditor", Privilege::type_name(), privilege.resource_id());
    drive_add(&config, &record, &events).unwrap();

    let customer = Customer { id: 42 };
    let customer_record =
        RoleRecord::for_instance("member", Customer::type_name(), customer.resource_id());
    drive_add(&config, &customer_record, &events).unwrap();

    assert_eq!(
        recorded(&events),
        vec![
            "before_add",
            "store.add",
            "after_add",
            "before_add",
            "store.add",
            "after_add"
        ]
    );
    assert_eq!(config.role_table(), "privileges");
    assert_eq!(config.join_table(), "customers_privileges");
}

/// No global mutable state exists or is needed: two configs built from the
/// same builder entry point are fully independent value objects (the gem's
/// `mattr_accessor` singletons are intentionally not ported).
#[test]
fn configs_are_independent_value_objects() {
    let events_one = new_probe();
    let events_two = new_probe();
    let with_hooks = RolifyConfig::builder()
        .before_add(notify_hook("before_add", &events_one))
        .build()
        .unwrap();
    let plain = RolifyConfig::default();

    let record = RoleRecord::global("admin");
    plain.run_before_add(&record).unwrap();
    with_hooks.run_before_add(&record).unwrap();

    assert_eq!(recorded(&events_one), vec!["before_add"]);
    assert_eq!(recorded(&events_two), Vec::<&'static str>::new());
}
