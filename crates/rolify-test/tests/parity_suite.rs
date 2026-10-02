//! The D-12 `InMemoryStore` binding: the template Phases 3-5 copy per
//! adapter (one binding line per backend).
//!
//! Bindings:
//!
//! | Binding | Backend | Added by |
//! |---|---|---|
//! | `in_memory_default_user` | `InMemoryBackend<DefaultUser>` (default config) | 02-01 |
//! | `in_memory_strict_user` | `InMemoryBackend<StrictUserClass>` (`rolify strict: true`, active_record.rb:24-26) | 02-02; later strict-dependent modules (02-07 finders strict context) inherit this binding unchanged |
#![cfg(feature = "suite")]

rolify_test::parity_suite!(
    in_memory_default_user,
    rolify_test::backend::InMemoryBackend<rolify_test::fixtures::DefaultUser>
);
rolify_test::parity_suite!(
    in_memory_strict_user,
    rolify_test::backend::InMemoryBackend<rolify_test::fixtures::StrictUserClass>
);
