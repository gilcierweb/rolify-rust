-- Decision record (D-10): FK cascade from join to roles is deliberate; the
-- gem emits no FKs (divergence documented in the parity matrix). Resource
-- cleanup for deleted consumer resources is app-level: one DELETE on roles by
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
-- RESEARCH Pitfall 8: MySQL string columns declare CHARACTER SET utf8mb4
-- COLLATE utf8mb4_bin for byte-exact role-name and id comparison. The
-- charset/collate clauses follow the data type (MySQL grammar position);
-- the server rejects them after NOT NULL/DEFAULT (error 1064, verified
-- against mysql:8.4).
-- D-12: roles.id BIGINT AUTO_INCREMENT PRIMARY KEY.
-- D-13: timestamps NOT NULL DEFAULT CURRENT_TIMESTAMP (updated_at static; roles never UPDATE).

CREATE TABLE roles (
    id            BIGINT AUTO_INCREMENT PRIMARY KEY,
    name          VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
    resource_type VARCHAR(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL DEFAULT '',
    resource_id   VARCHAR(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL DEFAULT '',
    created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT roles_triple_unique UNIQUE (name, resource_type, resource_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
CREATE INDEX idx_roles_resource ON roles (resource_type, resource_id);
CREATE INDEX idx_roles_name ON roles (name);

CREATE TABLE users_roles (
    user_id BIGINT NOT NULL,
    role_id BIGINT NOT NULL,
    CONSTRAINT users_roles_pair_unique UNIQUE (user_id, role_id),
    CONSTRAINT users_roles_role_id_fk FOREIGN KEY (role_id) REFERENCES roles(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;