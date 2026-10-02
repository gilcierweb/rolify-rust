//! Config hook callbacks: port of `shared_examples_for_callbacks.rb`
//! (l.13-63) plus the CONF-05 Result-veto strengthening.
//!
//! ## Porting stance
//!
//! The gem spec only mocks `should_receive(:role_callback)` per hook slot.
//! The Rust port pins REAL hook invocation through the public add/remove
//! path with a recording sink, plus the CONF-05 strengthening (`before_*`
//! returning `Err` aborts the operation AND skips the paired `after_*`).
//! D-19 entry for the 02-09 harvest.

use std::cell::RefCell;
use std::sync::Arc;

use rolify_core::config::RolifyConfig;
use rolify_core::error::RolifyError;
use rolify_core::kernel::RemovalTarget;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{RoleName, RoleRecord};
use rolify_core::user::RolifyUser;

use crate::backend::{InMemoryBackend, TestBackend};
use crate::fixtures::UserClass;

thread_local! {
    /// Per-thread hook invocation log: cargo's default harness runs
    /// wrappers in parallel on separate threads, so per-thread ownership
    /// prevents any interleaving between sibling cases. Cleared at every
    /// case start.
    static HOOK_LOG: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

/// Record one hook firing in the thread-local log.
fn push_hook(tag: &'static str) {
    HOOK_LOG.with(|log| log.borrow_mut().push(tag));
}

/// Clear the thread-local log at case start.
fn clear_hook_log() {
    HOOK_LOG.with(|log| log.borrow_mut().clear());
}

/// Snapshot the thread-local log for assertions.
fn hook_log_snapshot() -> Vec<&'static str> {
    HOOK_LOG.with(|log| log.borrow().clone())
}

/// All four hooks installed, each pushing its tag
/// (`shared_examples_for_callbacks.rb:13-63` behavior, real hooks).
struct AllHooksUserClass;

impl UserClass for AllHooksUserClass {
    fn rolify_type() -> &'static str {
        "User"
    }

    /// # Panics
    ///
    /// Never in practice: the default table names always validate.
    fn config() -> RolifyConfig {
        RolifyConfig::builder()
            .before_add(Arc::new(|_: &RoleRecord| {
                push_hook("before_add");
                Ok(())
            }))
            .after_add(Arc::new(|_: &RoleRecord| push_hook("after_add")))
            .before_remove(Arc::new(|_: &RoleRecord| {
                push_hook("before_remove");
                Ok(())
            }))
            .after_remove(Arc::new(|_: &RoleRecord| push_hook("after_remove")))
            .build()
            .expect("the all-hooks configuration always passes validation")
    }
}

/// Only `after_add` installed (l.25-35 behavior in isolation).
struct AfterAddOnlyUserClass;

impl UserClass for AfterAddOnlyUserClass {
    fn rolify_type() -> &'static str {
        "User"
    }

    /// # Panics
    ///
    /// Never in practice: the default table names always validate.
    fn config() -> RolifyConfig {
        RolifyConfig::builder()
            .after_add(Arc::new(|_: &RoleRecord| push_hook("after_add")))
            .build()
            .expect("the after-add-only configuration always passes validation")
    }
}

/// Only `after_remove` installed (l.51-63 behavior in isolation).
struct AfterRemoveOnlyUserClass;

impl UserClass for AfterRemoveOnlyUserClass {
    fn rolify_type() -> &'static str {
        "User"
    }

    /// # Panics
    ///
    /// Never in practice: the default table names always validate.
    fn config() -> RolifyConfig {
        RolifyConfig::builder()
            .after_remove(Arc::new(|_: &RoleRecord| push_hook("after_remove")))
            .build()
            .expect("the after-remove-only configuration always passes validation")
    }
}

/// `before_add` vetoes (CONF-05 strengthening); `after_add` must never run.
struct VetoBeforeAddUserClass;

impl UserClass for VetoBeforeAddUserClass {
    fn rolify_type() -> &'static str {
        "User"
    }

    /// # Panics
    ///
    /// Never in practice: the default table names always validate.
    fn config() -> RolifyConfig {
        RolifyConfig::builder()
            .before_add(Arc::new(|_: &RoleRecord| {
                push_hook("before_add");
                Err(RolifyError::CallbackVeto {
                    callback: "before_add",
                    reason: "vetoed by the suite".into(),
                })
            }))
            .after_add(Arc::new(|_: &RoleRecord| push_hook("after_add")))
            .build()
            .expect("the veto-before-add configuration always passes validation")
    }
}

/// `before_remove` vetoes (CONF-05 strengthening); `after_remove` must
/// never run.
struct VetoBeforeRemoveUserClass;

impl UserClass for VetoBeforeRemoveUserClass {
    fn rolify_type() -> &'static str {
        "User"
    }

    /// # Panics
    ///
    /// Never in practice: the default table names always validate.
    fn config() -> RolifyConfig {
        RolifyConfig::builder()
            .before_remove(Arc::new(|_: &RoleRecord| {
                push_hook("before_remove");
                Err(RolifyError::CallbackVeto {
                    callback: "before_remove",
                    reason: "vetoed by the suite".into(),
                })
            }))
            .after_remove(Arc::new(|_: &RoleRecord| push_hook("after_remove")))
            .build()
            .expect("the veto-before-remove configuration always passes validation")
    }
}

/// `before_add` fires around the add
/// (`shared_examples_for_callbacks.rb:13-23`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn before_add_hook_fires() -> Result<(), RolifyError> {
    clear_hook_log();
    let mut backend: InMemoryBackend<AllHooksUserClass> = InMemoryBackend::build().await?;
    let subject = backend.subject("admin");
    subject
        .add_role(&RoleName::from("admin"), ResourceRef::Global)
        .await?;
    assert_eq!(hook_log_snapshot(), vec!["before_add", "after_add"]);
    Ok(())
}

/// `after_add` fires after a successful add even when installed alone
/// (`shared_examples_for_callbacks.rb:25-35`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn after_add_hook_fires() -> Result<(), RolifyError> {
    clear_hook_log();
    let mut backend: InMemoryBackend<AfterAddOnlyUserClass> = InMemoryBackend::build().await?;
    let subject = backend.subject("admin");
    subject
        .add_role(&RoleName::from("admin"), ResourceRef::Global)
        .await?;
    assert_eq!(hook_log_snapshot(), vec!["after_add"]);
    Ok(())
}

/// `before_remove` fires around the remove
/// (`shared_examples_for_callbacks.rb:37-49`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn before_remove_hook_fires() -> Result<(), RolifyError> {
    clear_hook_log();
    let mut backend: InMemoryBackend<AllHooksUserClass> = InMemoryBackend::build().await?;
    let subject = backend.subject("admin");
    subject
        .add_role(&RoleName::from("admin"), ResourceRef::Global)
        .await?;
    clear_hook_log();
    subject
        .remove_role(&RoleName::from("admin"), RemovalTarget::NameOnly)
        .await?;
    assert_eq!(hook_log_snapshot(), vec!["before_remove", "after_remove"]);
    Ok(())
}

/// `after_remove` fires after a successful remove even when installed alone
/// (`shared_examples_for_callbacks.rb:51-63`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn after_remove_hook_fires() -> Result<(), RolifyError> {
    clear_hook_log();
    let mut backend: InMemoryBackend<AfterRemoveOnlyUserClass> = InMemoryBackend::build().await?;
    let subject = backend.subject("admin");
    subject
        .add_role(&RoleName::from("admin"), ResourceRef::Global)
        .await?;
    clear_hook_log();
    subject
        .remove_role(&RoleName::from("admin"), RemovalTarget::NameOnly)
        .await?;
    assert_eq!(hook_log_snapshot(), vec!["after_remove"]);
    Ok(())
}

/// A vetoing `before_add` aborts the add and skips `after_add` (CONF-05
/// strengthening).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn before_add_veto_aborts_add_and_skips_after() -> Result<(), RolifyError> {
    clear_hook_log();
    let mut backend: InMemoryBackend<VetoBeforeAddUserClass> = InMemoryBackend::build().await?;
    let subject = backend.subject("admin");
    let name = RoleName::from("admin");
    let before = subject.roles_name().await?.len();
    let outcome = subject.add_role(&name, ResourceRef::Global).await;
    assert!(matches!(
        outcome,
        Err(RolifyError::CallbackVeto {
            callback: "before_add",
            ..
        })
    ));
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after);
    assert_eq!(hook_log_snapshot(), vec!["before_add"]);
    Ok(())
}

/// A vetoing `before_remove` aborts the remove and skips `after_remove`
/// (CONF-05 strengthening). The role is provisioned via `grant_to`
/// (store-direct, hook-free by design) because the veto config owns this
/// backend.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn before_remove_veto_aborts_remove_and_skips_after() -> Result<(), RolifyError> {
    clear_hook_log();
    let mut backend: InMemoryBackend<VetoBeforeRemoveUserClass> = InMemoryBackend::build().await?;
    let name = RoleName::from("admin");
    backend
        .grant_to("admin", &name, ResourceRef::Global)
        .await?;
    let subject = backend.subject("admin");
    let before = subject.roles_name().await?.len();
    let outcome = subject.remove_role(&name, RemovalTarget::NameOnly).await;
    assert!(matches!(
        outcome,
        Err(RolifyError::CallbackVeto {
            callback: "before_remove",
            ..
        })
    ));
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after);
    assert_eq!(hook_log_snapshot(), vec!["before_remove"]);
    Ok(())
}

/// One add-then-remove lifecycle pins the complete four-tag order, which
/// the mock-based gem spec cannot express (l.13-63 composite).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn full_lifecycle_order_single_case() -> Result<(), RolifyError> {
    clear_hook_log();
    let mut backend: InMemoryBackend<AllHooksUserClass> = InMemoryBackend::build().await?;
    let subject = backend.subject("admin");
    let name = RoleName::from("admin");
    subject.add_role(&name, ResourceRef::Global).await?;
    subject.remove_role(&name, RemovalTarget::NameOnly).await?;
    assert_eq!(
        hook_log_snapshot(),
        vec!["before_add", "after_add", "before_remove", "after_remove"]
    );
    Ok(())
}

/// Expand the 7 `callbacks` wrappers for one backend. Wrapper names carry
/// the `callbacks_` prefix so the 12 binding modules never collide. The
/// `$backend` type is intentionally unused: hook behavior is
/// class-config-driven and backend-independent, so the concrete stacks
/// prevent identical-across-bindings noise while keeping the frozen
/// 12-module expansion contract.
#[macro_export]
macro_rules! parity_callbacks_cases {
    ($backend:ty) => {
        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn callbacks_before_add_hook_fires() {
            $crate::suite::callbacks::before_add_hook_fires()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn callbacks_after_add_hook_fires() {
            $crate::suite::callbacks::after_add_hook_fires()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn callbacks_before_remove_hook_fires() {
            $crate::suite::callbacks::before_remove_hook_fires()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn callbacks_after_remove_hook_fires() {
            $crate::suite::callbacks::after_remove_hook_fires()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn callbacks_before_add_veto_aborts_add_and_skips_after() {
            $crate::suite::callbacks::before_add_veto_aborts_add_and_skips_after()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn callbacks_before_remove_veto_aborts_remove_and_skips_after() {
            $crate::suite::callbacks::before_remove_veto_aborts_remove_and_skips_after()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn callbacks_full_lifecycle_order_single_case() {
            $crate::suite::callbacks::full_lifecycle_order_single_case()
                .await
                .unwrap();
        }
    };
}
