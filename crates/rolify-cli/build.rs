fn main() {
    println!("cargo:rerun-if-changed=templates/postgres/up.sql");
    println!("cargo:rerun-if-changed=templates/postgres/down.sql");
    println!("cargo:rerun-if-changed=templates/mysql/up.sql");
    println!("cargo:rerun-if-changed=templates/mysql/down.sql");
    println!("cargo:rerun-if-changed=templates/sqlite/up.sql");
    println!("cargo:rerun-if-changed=templates/sqlite/down.sql");
    println!("cargo:rerun-if-changed=templates/seaorm_migration.rs.txt");
    println!("cargo:rerun-if-changed=templates/mongo_docs.rs.txt");
    println!("cargo:rerun-if-changed=templates/scaffolding/role_stub.rs.txt");
    println!("cargo:rerun-if-changed=templates/scaffolding/holder_stub.rs.txt");
    println!("cargo:rerun-if-changed=templates/scaffolding/config_example.rs.txt");
    println!("cargo:rerun-if-changed=templates/scaffolding/README_diesel.md.txt");
    println!("cargo:rerun-if-changed=templates/scaffolding/README_sqlx.md.txt");
    println!("cargo:rerun-if-changed=templates/scaffolding/README_seaorm.md.txt");
    println!("cargo:rerun-if-changed=templates/scaffolding/README_mongodb.md.txt");
}
