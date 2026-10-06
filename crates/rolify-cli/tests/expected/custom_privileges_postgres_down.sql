-- Down migration: drop join table first (FK order), then privileges.

DROP TABLE IF EXISTS customers_privileges;
DROP TABLE IF EXISTS privileges;