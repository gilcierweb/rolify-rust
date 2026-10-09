-- Down migration: drop join table first (FK order), then roles.

DROP TABLE IF EXISTS users_roles;
DROP TABLE IF EXISTS roles;