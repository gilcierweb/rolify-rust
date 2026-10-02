//! The D-12 `InMemoryStore` binding: the template Phases 3-5 copy per
//! adapter (one binding line per backend).
#![cfg(feature = "suite")]

rolify_test::parity_suite!(
    in_memory_default_user,
    rolify_test::backend::InMemoryBackend<rolify_test::fixtures::DefaultUser>
);
