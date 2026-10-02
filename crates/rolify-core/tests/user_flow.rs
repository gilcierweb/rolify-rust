//! Full `RolifyUser` provided-method mechanics against the reference
//! `InMemoryStore` - through the public surface only. Lives in `tests/`
//! (integration target) so the same compiled `rolify_core` instance is
//! shared by this crate and `rolify_test` (unit tests inside the lib would
//! see a second `rolify_core` and the trait impls would not unify).
//!
//! Filterable as `cargo test -p rolify-core user::` (both modes).

mod user {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use rolify_core::config::RolifyConfig;
    use rolify_core::error::RolifyError;
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::{ResourceFilter, RoleQuery};
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::{ResourceId, RoleName, RoleRecord, RoleSet};
    use rolify_core::store::{RemovalOutcome, RoleStore, ScopeColumn, Sealed};
    use rolify_test::InMemoryStore;

    #[cfg(not(feature = "is_sync"))]
    use core::future::Future;

    /// Counting store wrapping the reference implementation: proves
    /// `has_any_roles` is ONE store round (never N sequential checks).
    #[derive(Default)]
    struct CountingStore {
        inner: InMemoryStore,
        where_calls: AtomicUsize,
        where_strict_calls: AtomicUsize,
        where_any_calls: AtomicUsize,
    }

    impl CountingStore {
        fn counters(&self) -> (usize, usize, usize) {
            (
                self.where_calls.load(Ordering::Relaxed),
                self.where_strict_calls.load(Ordering::Relaxed),
                self.where_any_calls.load(Ordering::Relaxed),
            )
        }
    }

    impl Sealed for CountingStore {}

    #[maybe_async::maybe_async(AFIT)]
    impl RoleStore for CountingStore {
        type Conn = ();
        type Error = RolifyError;

        fn where_(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            query: &RoleQuery<'_>,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            self.where_calls.fetch_add(1, Ordering::Relaxed);
            self.inner.where_(conn, holder, query)
        }

        fn where_strict(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            query: &RoleQuery<'_>,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            self.where_strict_calls.fetch_add(1, Ordering::Relaxed);
            self.inner.where_strict(conn, holder, query)
        }

        fn where_any(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            queries: &[RoleQuery<'_>],
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            self.where_any_calls.fetch_add(1, Ordering::Relaxed);
            self.inner.where_any(conn, holder, queries)
        }

        fn find_or_create_by(
            &mut self,
            conn: &mut Self::Conn,
            name: &RoleName,
            scope: ResourceRef<'_>,
        ) -> impl Future<Output = Result<RoleRecord, Self::Error>> + Send {
            self.inner.find_or_create_by(conn, name, scope)
        }

        fn add(
            &mut self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            role: &RoleRecord,
        ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
            self.inner.add(conn, holder, role)
        }

        fn remove(
            &mut self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            name: &RoleName,
            target: RemovalTarget<'_>,
            remove_role_if_empty: bool,
        ) -> impl Future<Output = Result<RemovalOutcome, Self::Error>> + Send {
            self.inner
                .remove(conn, holder, name, target, remove_role_if_empty)
        }

        fn exists(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            column: ScopeColumn,
        ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
            self.inner.exists(conn, holder, column)
        }

        fn roles_of(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            self.inner.roles_of(conn, holder)
        }
    }

    /// Test user over the counting store.
    struct TestUser {
        id: i64,
        store: CountingStore,
        conn: (),
        config: RolifyConfig,
    }

    impl TestUser {
        fn new(id: i64, config: RolifyConfig) -> Self {
            Self {
                id,
                store: CountingStore::default(),
                conn: (),
                config,
            }
        }
    }

    impl rolify_core::user::RolifyUser for TestUser {
        type Store = CountingStore;

        fn store(&mut self) -> &mut CountingStore {
            &mut self.store
        }
        fn rolify_config(&self) -> &RolifyConfig {
            &self.config
        }
        fn rolify_id(&self) -> ResourceId {
            ResourceId::from(self.id)
        }
        fn rolify_type() -> &'static str {
            "TestUser"
        }
        fn store_with_conn(&mut self) -> (&mut CountingStore, &mut ()) {
            (&mut self.store, &mut self.conn)
        }
    }

    use rolify_core::user::RolifyUser as _;

    type Probe = Arc<Mutex<Vec<&'static str>>>;

    fn new_probe() -> Probe {
        Arc::new(Mutex::new(Vec::new()))
    }

    fn recorded(probe: &Probe) -> Vec<&'static str> {
        probe.lock().expect("probe lock poisoned").clone()
    }

    fn admin() -> RoleName {
        RoleName::from("admin")
    }
    fn manager() -> RoleName {
        RoleName::from("manager")
    }
    fn ghost() -> RoleName {
        RoleName::from("ghost")
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn add_role_is_idempotent_at_both_levels() {
        let mut user = TestUser::new(1, RolifyConfig::default());
        let forum_seven = ResourceId::from(7_i64);

        let first = user.add_role(&admin(), ResourceRef::Global).await.unwrap();
        let second = user.add_role(&admin(), ResourceRef::Global).await.unwrap();
        let third = user
            .add_role(&manager(), ResourceRef::Instance("Forum", &forum_seven))
            .await
            .unwrap();

        assert_eq!(first, second);
        assert_ne!(first, third);
        assert_eq!(
            user.store().inner.assertion_len(),
            2,
            "each distinct triple exists exactly once"
        );
        let holder = user.rolify_id();
        let roles = {
            let (store, conn) = user.store_with_conn();
            store.roles_of(&mut *conn, &holder).await.unwrap()
        };
        assert_eq!(
            roles.len(),
            2,
            "level-2 link guard blocked the duplicate link"
        );
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn add_role_veto_leaves_store_untouched() {
        let events = new_probe();
        let config = RolifyConfig::builder()
            .before_add({
                let writer = Arc::clone(&events);
                Arc::new(move |_record: &RoleRecord| {
                    writer
                        .lock()
                        .expect("probe lock poisoned")
                        .push("before_add");
                    Err(RolifyError::CallbackVeto {
                        callback: "before_add",
                        reason: "deny".into(),
                    })
                })
            })
            .after_add({
                let writer = Arc::clone(&events);
                Arc::new(move |_record: &RoleRecord| {
                    writer
                        .lock()
                        .expect("probe lock poisoned")
                        .push("after_add");
                })
            })
            .build()
            .unwrap();
        let mut user = TestUser::new(1, config);

        let outcome = user.add_role(&admin(), ResourceRef::Global).await;

        assert!(matches!(
            outcome,
            Err(RolifyError::CallbackVeto {
                callback: "before_add",
                ..
            })
        ));
        assert_eq!(user.store().inner.assertion_len(), 0, "no role row created");
        assert_eq!(user.store().inner.link_count(), 0, "no link created");
        assert_eq!(recorded(&events), vec!["before_add"], "after_add skipped");
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn hooks_fire_around_the_full_add_choreography() {
        let events = new_probe();
        let config = RolifyConfig::builder()
            .before_add({
                let writer = Arc::clone(&events);
                Arc::new(move |_record: &RoleRecord| {
                    writer
                        .lock()
                        .expect("probe lock poisoned")
                        .push("before_add");
                    Ok(())
                })
            })
            .after_add({
                let writer = Arc::clone(&events);
                Arc::new(move |_record: &RoleRecord| {
                    writer
                        .lock()
                        .expect("probe lock poisoned")
                        .push("after_add");
                })
            })
            .build()
            .unwrap();
        let mut user = TestUser::new(1, config);

        user.add_role(&admin(), ResourceRef::Global).await.unwrap();

        assert_eq!(recorded(&events), vec!["before_add", "after_add"]);
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn has_role_routes_strict_only_on_class_or_instance() {
        let mut strict_user =
            TestUser::new(1, RolifyConfig::builder().strict(true).build().unwrap());
        strict_user
            .add_role(&admin(), ResourceRef::Global)
            .await
            .unwrap();

        let strict_class = strict_user
            .has_role(&admin(), ResourceFilter::Class("Forum"))
            .await
            .unwrap();
        let strict_global = strict_user
            .has_role(&admin(), ResourceFilter::Global)
            .await
            .unwrap();
        let strict_any = strict_user
            .has_role(&admin(), ResourceFilter::Any)
            .await
            .unwrap();
        assert!(
            !strict_class,
            "strict class query must not see the global row"
        );
        assert!(strict_global, "strict never engages for Global");
        assert!(strict_any, "strict never engages for Any");

        let mut relaxed_user = TestUser::new(1, RolifyConfig::default());
        relaxed_user
            .add_role(&admin(), ResourceRef::Global)
            .await
            .unwrap();
        let non_strict_class = relaxed_user
            .has_role(&admin(), ResourceFilter::Class("Forum"))
            .await
            .unwrap();
        assert!(
            non_strict_class,
            "non-strict class query sees the global override"
        );
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn cached_role_matches_the_query_path_with_strict_routing() {
        let config = RolifyConfig::builder().strict(true).build().unwrap();
        let mut user = TestUser::new(1, config);
        user.add_role(&admin(), ResourceRef::Global).await.unwrap();
        user.add_role(&manager(), ResourceRef::Class("Forum"))
            .await
            .unwrap();

        let rows = {
            let holder = user.rolify_id();
            let (store, conn) = user.store_with_conn();
            store.roles_of(&mut *conn, &holder).await.unwrap()
        };
        let snapshot = RoleSet::new(&rows);

        let class_query = RoleQuery {
            name: &admin(),
            filter: ResourceFilter::Class("Forum"),
        };
        assert!(!user.has_cached_role(&snapshot, &class_query));
        let any_query = RoleQuery {
            name: &admin(),
            filter: ResourceFilter::Any,
        };
        assert!(user.has_cached_role(&snapshot, &any_query));
        let strict_direct = RoleQuery {
            name: &manager(),
            filter: ResourceFilter::Class("Forum"),
        };
        assert!(user.has_strict_cached_role(&snapshot, &strict_direct));

        let via_store = user
            .has_role(&admin(), ResourceFilter::Class("Forum"))
            .await
            .unwrap();
        assert!(!via_store);
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn all_any_only_has_role_semantics() {
        let mut user = TestUser::new(1, RolifyConfig::default());
        user.add_role(&admin(), ResourceRef::Global).await.unwrap();
        user.add_role(&manager(), ResourceRef::Class("Forum"))
            .await
            .unwrap();

        let all_ok = [
            RoleQuery {
                name: &admin(),
                filter: ResourceFilter::Global,
            },
            RoleQuery {
                name: &manager(),
                filter: ResourceFilter::Class("Forum"),
            },
        ];
        let all_result = user.has_all_roles(&all_ok).await.unwrap();
        assert!(all_result);
        let with_miss = [
            RoleQuery {
                name: &ghost(),
                filter: ResourceFilter::Global,
            },
            RoleQuery {
                name: &manager(),
                filter: ResourceFilter::Class("Forum"),
            },
        ];
        let miss_result = user.has_all_roles(&with_miss).await.unwrap();
        assert!(!miss_result);

        // Measure the has_any_roles call in isolation (prior per-query
        // where_ traffic above is has_all_roles' documented early-exit walk).
        let (where_before, _, where_any_before) = user.store().counters();
        let any_queries = [
            RoleQuery {
                name: &ghost(),
                filter: ResourceFilter::Any,
            },
            RoleQuery {
                name: &manager(),
                filter: ResourceFilter::Class("Forum"),
            },
        ];
        let any_result = user.has_any_roles(&any_queries).await.unwrap();
        assert!(any_result);
        let (where_after, _, where_any_after) = user.store().counters();
        assert_eq!(
            where_any_after - where_any_before,
            1,
            "exactly one OR-folded call"
        );
        assert_eq!(where_after - where_before, 0, "no per-query where_ fan-out");

        let only_result = user
            .only_has_role(&admin(), ResourceFilter::Global)
            .await
            .unwrap();
        assert!(!only_result);
        let mut solo = TestUser::new(2, RolifyConfig::default());
        solo.add_role(&admin(), ResourceRef::Global).await.unwrap();
        let solo_result = solo
            .only_has_role(&admin(), ResourceFilter::Global)
            .await
            .unwrap();
        assert!(solo_result);

        let names = user.roles_name().await.unwrap();
        assert_eq!(names, vec![admin(), manager()]);
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn remove_role_sweeps_and_honors_remove_role_if_empty() {
        let events = new_probe();
        let config = RolifyConfig::builder()
            .before_remove({
                let writer = Arc::clone(&events);
                Arc::new(move |_record: &RoleRecord| {
                    writer
                        .lock()
                        .expect("probe lock poisoned")
                        .push("before_remove");
                    Ok(())
                })
            })
            .after_remove({
                let writer = Arc::clone(&events);
                Arc::new(move |_record: &RoleRecord| {
                    writer
                        .lock()
                        .expect("probe lock poisoned")
                        .push("after_remove");
                })
            })
            .build()
            .unwrap();
        let mut user = TestUser::new(1, config);
        user.add_role(&manager(), ResourceRef::Class("Forum"))
            .await
            .unwrap();
        assert_eq!(user.store().inner.assertion_len(), 1);

        let outcome = user
            .remove_role(&manager(), RemovalTarget::TypeSweep("Forum"))
            .await
            .unwrap();
        assert_eq!(outcome.removed_links, 1);
        assert_eq!(
            outcome.removed_roles.len(),
            1,
            "default removes the emptied row"
        );
        assert_eq!(user.store().inner.assertion_len(), 0);
        assert_eq!(recorded(&events), vec!["before_remove", "after_remove"]);

        let plain_config = RolifyConfig::builder()
            .remove_role_if_empty(false)
            .build()
            .unwrap();
        let mut plain = TestUser::new(2, plain_config);
        plain
            .add_role(&manager(), ResourceRef::Class("Forum"))
            .await
            .unwrap();
        let outcome = plain
            .remove_role(&manager(), RemovalTarget::TypeSweep("Forum"))
            .await
            .unwrap();
        assert!(outcome.removed_roles.is_empty());
        assert_eq!(plain.store().inner.assertion_len(), 1);
    }

    /// `grant`/`revoke` are thin aliases (role.rb:23/85): identical inputs
    /// through the alias and the direct call leave identical stores.
    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn grant_revoke_aliases_delegate_to_add_and_remove() {
        let mut via_alias = TestUser::new(1, RolifyConfig::default());
        let mut via_direct = TestUser::new(2, RolifyConfig::default());
        let forum_seven = ResourceId::from(7_i64);
        let scope = ResourceRef::Instance("Forum", &forum_seven);

        via_alias.grant(&admin(), scope).await.unwrap();
        via_direct.add_role(&admin(), scope).await.unwrap();

        assert_eq!(
            via_alias.store().inner.assertion_len(),
            via_direct.store().inner.assertion_len()
        );
        let alias_holder = via_alias.rolify_id();
        let direct_holder = via_direct.rolify_id();
        let alias_links = via_alias.store().inner.link_count_for(&alias_holder);
        let direct_links = via_direct.store().inner.link_count_for(&direct_holder);
        assert_eq!(alias_links, direct_links);
        let instance_filter = ResourceFilter::Instance("Forum", &forum_seven);
        let alias_has = via_alias
            .has_role(&admin(), instance_filter)
            .await
            .unwrap();
        let direct_has = via_direct
            .has_role(&admin(), instance_filter)
            .await
            .unwrap();
        assert!(alias_has && direct_has);

        let target = RemovalTarget::Exact("Forum", &forum_seven);
        via_alias.revoke(&admin(), target).await.unwrap();
        via_direct.remove_role(&admin(), target).await.unwrap();

        assert_eq!(
            via_alias.store().inner.assertion_len(),
            via_direct.store().inner.assertion_len()
        );
        let alias_gone = via_alias
            .has_role(&admin(), instance_filter)
            .await
            .unwrap();
        let direct_gone = via_direct
            .has_role(&admin(), instance_filter)
            .await
            .unwrap();
        assert!(!alias_gone && !direct_gone);
    }

    /// `has_strict_role` is the direct strict path (role.rb:43-45): no
    /// gate, so a global-only holder fails it while `has_role` still
    /// applies the ladder override.
    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn has_strict_role_is_the_direct_strict_path() {
        let mut user = TestUser::new(1, RolifyConfig::default());
        user.add_role(&admin(), ResourceRef::Global).await.unwrap();

        let ladder_hit = user
            .has_role(&admin(), ResourceFilter::Class("Forum"))
            .await
            .unwrap();
        assert!(ladder_hit, "global row satisfies the class query");
        let strict_miss = user
            .has_strict_role(&admin(), ResourceFilter::Class("Forum"))
            .await
            .unwrap();
        assert!(!strict_miss, "no exact class row exists");

        user.add_role(&admin(), ResourceRef::Class("Forum"))
            .await
            .unwrap();
        let strict_hit = user
            .has_strict_role(&admin(), ResourceFilter::Class("Forum"))
            .await
            .unwrap();
        assert!(strict_hit, "the exact class row matches");
    }
}
