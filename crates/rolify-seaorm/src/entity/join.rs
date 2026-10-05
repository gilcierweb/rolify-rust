//! Flat static entity for the `users_roles` join table.
//!
//! Link rows connect a holder (`user_id`, stringified holder PK — the gem's
//! `Team#team_code` string-PK precedent keeps ids as text) to a role
//! (`role_id`, FK `roles.id` with `ON DELETE CASCADE`). The canonical
//! schema carries no surrogate key: the composite `UNIQUE(user_id,
//! role_id)` pair is the identity, modeled here as a composite primary
//! key so `Entity::find()` and friends stay available.

use sea_orm::entity::prelude::*;

/// Join row: holder id <-> role id, unique per pair.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "users_roles")]
pub struct Model {
    /// Stringified holder primary key (`VARCHAR(191) NOT NULL`).
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: String,
    /// Role row id; `BIGINT NOT NULL REFERENCES roles(id) ON DELETE CASCADE`.
    #[sea_orm(primary_key, auto_increment = false)]
    pub role_id: i64,
}

impl ActiveModelBehavior for ActiveModel {}
