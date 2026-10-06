fn main() {
    println!("cargo:rerun-if-changed=templates/postgres/up.sql");
    println!("cargo:rerun-if-changed=templates/postgres/down.sql");
    println!("cargo:rerun-if-changed=templates/mysql/up.sql");
    println!("cargo:rerun-if-changed=templates/mysql/down.sql");
    println!("cargo:rerun-if-changed=templates/sqlite/up.sql");
    println!("cargo:rerun-if-changed=templates/sqlite/down.sql");
}
