use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(name = "rolify-cli", version, about = "Rolify migration generator")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Generate rolify migration files (primary command)
    #[command(alias = "init")]
    Generate(GenerateArgs),
}

#[derive(Parser, Debug)]
pub struct GenerateArgs {
    /// Target backend
    #[arg(long, value_enum, required = true)]
    pub backend: Backend,

    /// Role name (e.g., Role)
    #[arg(value_name = "ROLE_NAME", required = true)]
    pub role_name: String,

    /// Holder name (e.g., User)
    #[arg(value_name = "HOLDER_NAME", required = true)]
    pub holder_name: String,

    /// Output directory (default: current directory)
    #[arg(long, default_value = ".")]
    pub out_dir: String,

    /// Overwrite existing files
    #[arg(long)]
    pub force: bool,

    /// Dry run - print what would be created without writing
    #[arg(long)]
    pub dry_run: bool,

    /// Roles table name (default: roles)
    #[arg(long, default_value = "roles")]
    pub roles_table: String,

    /// Join table name (default: derived from holder plural + _ + `roles_table`)
    #[arg(long)]
    pub join_table: Option<String>,
}

#[derive(ValueEnum, Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    Diesel,
    Sqlx,
    Seaorm,
    Mongodb,
}

impl Backend {
    #[must_use] 
    pub fn as_str(&self) -> &'static str {
        match self {
            Backend::Diesel => "diesel",
            Backend::Sqlx => "sqlx",
            Backend::Seaorm => "seaorm",
            Backend::Mongodb => "mongodb",
        }
    }
}
