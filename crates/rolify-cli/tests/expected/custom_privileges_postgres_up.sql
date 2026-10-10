-- Decision record (D-10): FK cascade from join to privileges is deliberate; the
-- gem emits no FKs (divergence documented in the parity matrix). Resource
-- cleanup for deleted consumer resources is app-level: one DELETE on privileges by
-- (resource_type, resource_id); cascade sweeps join rows.
--
-- D-01/D-02: sentinel '' strategy: global/class scope rows store empty string
-- in resource_type/resource_id (never SQL NULL) so the UNIQUE triple constraint
-- deduplicates identically on Postgres, MySQL, and SQLite.
--
-- D-05: join table has UNIQUE(user_id, role_id): diverges from the gem's
-- non-unique composite index. This makes add_role race-safe via INSERT with
-- catch-and-ignore of unique violation.
--
-- D-03: VARCHAR sizes: name(255), resource_type(191), resource_id(191).
-- D-12: privileges.id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY.
-- D-13: timestamps NOT NULL DEFAULT CURRENT_TIMESTAMP (updated_at static; privileges never UPDATE).
-- D-08: holder_id_type substitution: BIGINT expands to BIGINT (integer), UUID (uuid), or VARCHAR(191) (string).

CREATE TABLE privileges (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name          VARCHAR(255) NOT NULL,
    resource_type VARCHAR(191) NOT NULL DEFAULT '',
    resource_id   VARCHAR(191) NOT NULL DEFAULT '',
    created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT privileges_triple_unique UNIQUE (name, resource_type, resource_id)
);
CREATE INDEX idx_privileges_resource ON privileges (resource_type, resource_id);
CREATE INDEX idx_privileges_name ON privileges (name);

CREATE TABLE customers_privileges (
    user_id BIGINT NOT NULL,
    role_id BIGINT NOT NULL REFERENCES privileges(id) ON DELETE CASCADE,
    CONSTRAINT customers_privileges_pair_unique UNIQUE (user_id, role_id)
);