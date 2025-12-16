//! Cargo subcommand for migratio database migrations.
//!
//! This binary provides the `cargo migratio` command, which generates and runs
//! a migration runner based on user configuration in `Cargo.toml`.

use std::fs;
use std::process::Command;

use cargo_metadata::camino::{self, Utf8PathBuf};
use cargo_metadata::{Metadata, MetadataCommand, Package};
use clap::Parser;
use serde::Deserialize;

/// Where to get migratio-cli from
enum MigratioCliSource {
    /// Use crates.io with specified version
    CratesIo(String),
    /// Use a local path (when user has path dependency on migratio)
    Path(Utf8PathBuf),
}

#[derive(Parser)]
#[command(name = "cargo")]
#[command(bin_name = "cargo")]
struct Cargo {
    #[command(subcommand)]
    command: CargoCommands,
}

#[derive(clap::Subcommand)]
enum CargoCommands {
    /// Run migratio database migrations
    Migratio(MigratioArgs),
}

#[derive(clap::Args)]
struct MigratioArgs {
    /// All remaining arguments passed to the migration runner
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MigratioConfig {
    /// Path to the function that returns the migrator (e.g., "my_app::migrations::get_migrator")
    migrator_fn: String,
    /// Database type: "sqlite", "mysql", or "postgres"
    database: String,
    /// Environment variable for database URL (default: "DATABASE_URL")
    #[serde(default = "default_database_url_env")]
    database_url_env: String,
    /// Additional features to enable on the user's crate
    #[serde(default)]
    features: Vec<String>,
}

fn default_database_url_env() -> String {
    "DATABASE_URL".to_string()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Cargo {
        command: CargoCommands::Migratio(args),
    } = Cargo::parse();

    // Get project metadata
    let metadata = MetadataCommand::new().exec()?;
    let root_package = match metadata.root_package() {
        Some(pkg) => pkg,
        None => {
            eprintln!("Error: No root package found.");
            eprintln!();
            eprintln!(
                "This command must be run from within a package directory (not a workspace root)."
            );
            std::process::exit(1);
        }
    };

    // Find migratio configuration in package.metadata.migratio
    let migratio_metadata = match root_package.metadata.get("migratio") {
        Some(meta) => meta,
        None => {
            eprintln!("Error: No [package.metadata.migratio] found in Cargo.toml.");
            eprintln!();
            eprintln!("Add configuration to your Cargo.toml:");
            eprintln!("  [package.metadata.migratio]");
            eprintln!("  migrator_fn = \"your_crate::migrations::get_migrator\"");
            eprintln!("  database = \"sqlite\"  # or \"mysql\" or \"postgres\"");
            std::process::exit(1);
        }
    };

    let config: MigratioConfig = serde_json::from_value(migratio_metadata.clone())
        .map_err(|e| format!("Invalid migratio config: {}", e))?;

    // Validate database type
    if config.database != "sqlite" && config.database != "mysql" && config.database != "postgres" {
        return Err(format!(
            "Invalid database type '{}'. Must be 'sqlite', 'mysql', or 'postgres'.",
            config.database
        )
        .into());
    }

    // Get the package directory (parent of Cargo.toml)
    let package_dir = root_package
        .manifest_path
        .parent()
        .ok_or("Could not determine package directory")?;

    // Check if the user's migratio dependency is a path dependency
    // If so, we need to use the local migratio-cli as well to avoid version conflicts
    let migratio_cli_source = find_migratio_cli_source(&metadata, root_package);

    // Create runner directory
    let runner_dir = metadata.target_directory.join("migratio-runner");
    fs::create_dir_all(&runner_dir)?;
    fs::create_dir_all(runner_dir.join("src"))?;

    // Generate runner Cargo.toml
    let runner_cargo_toml = generate_runner_cargo_toml(
        &root_package.name,
        package_dir,
        &config,
        &migratio_cli_source,
    );
    fs::write(runner_dir.join("Cargo.toml"), runner_cargo_toml)?;

    // Generate runner main.rs
    let runner_main = generate_runner_main(&config);
    fs::write(runner_dir.join("src").join("main.rs"), runner_main)?;

    // Build the runner
    eprintln!("Building migration runner...");
    let status = Command::new("cargo")
        .current_dir(&runner_dir)
        .args(["build", "--release"])
        .status()?;

    if !status.success() {
        eprintln!("Failed to build migration runner");
        std::process::exit(1);
    }

    let binary_path = runner_dir
        .join("target")
        .join("release")
        .join("migratio-runner");

    // If no args provided or help requested, show our custom help that includes 'build'
    if args.args.is_empty()
        || args.args.iter().any(|a| a == "--help" || a == "-h")
        || args.args.first().map(|s| s.as_str()) == Some("help")
    {
        print_help();
        return Ok(());
    }

    // Check if this is the "build" command - if so, just print the path and exit
    // The build command is handled here, not by the runner, so no DATABASE_URL needed
    if args.args.first().map(|s| s.as_str()) == Some("build") {
        println!();
        println!("Migration runner built successfully!");
        println!("Binary location: {}", binary_path);
        println!();
        println!("You can copy this binary to your deployment servers and run it directly:");
        println!("  ./migratio-runner upgrade");
        println!("  ./migratio-runner status");
        println!("  ./migratio-runner --help");
        return Ok(());
    }

    // For all other commands, run the runner with forwarded args
    // The runner will check for DATABASE_URL itself
    let status = Command::new(&binary_path).args(&args.args).status()?;

    std::process::exit(status.code().unwrap_or(1));
}

fn print_help() {
    println!("Database migration tool");
    println!();
    println!("Usage: cargo migratio <COMMAND>");
    println!();
    println!("Commands:");
    println!("  status     Show current migration status (requires database)");
    println!("  upgrade    Run pending migrations (requires database)");
    println!("  downgrade  Rollback migrations (requires database)");
    println!("  history    Show migration history (requires database)");
    println!("  preview    Preview pending migrations without running them (requires database)");
    println!("  list       List all migrations defined in the migrator (no database required)");
    println!(
        "  build      Build the migration runner binary for deployment (no database required)"
    );
    println!("  help       Print this message");
    println!();
    println!("Options:");
    println!("  -h, --help  Print help");
}

/// Find the migratio package in the dependency graph and check if it's a path dependency.
/// If it is, return the path to migratio-cli (sibling directory).
/// Otherwise, return the version of migratio the user is using for migratio-cli.
fn find_migratio_cli_source(metadata: &Metadata, root_package: &Package) -> MigratioCliSource {
    // Look for 'migratio' in the resolved packages
    for package in &metadata.packages {
        if package.name == "migratio" {
            // Check if any of the root package's dependencies point to this migratio via path
            for dep in &root_package.dependencies {
                if dep.name == "migratio" {
                    if let Some(path) = &dep.path {
                        // It's a path dependency - migratio-cli should be a sibling
                        let migratio_cli_path = path.parent().unwrap().join("migratio-cli");
                        if migratio_cli_path.exists() {
                            return MigratioCliSource::Path(migratio_cli_path);
                        }
                    }
                }
            }

            // Found migratio in packages - use the same version for migratio-cli
            // This ensures type compatibility between the user's migratio and migratio-cli
            return MigratioCliSource::CratesIo(package.version.to_string());
        }
    }

    // Fallback to crates.io with the same version as this binary
    // (this shouldn't normally happen if the user has migratio as a dependency)
    MigratioCliSource::CratesIo(env!("CARGO_PKG_VERSION").to_string())
}

fn generate_runner_cargo_toml(
    package_name: &str,
    package_dir: &camino::Utf8Path,
    config: &MigratioConfig,
    migratio_cli_source: &MigratioCliSource,
) -> String {
    let feature = match config.database.as_str() {
        "sqlite" => "sqlite",
        "mysql" => "mysql",
        "postgres" => "postgres",
        _ => "sqlite",
    };

    // Build features string for the user's crate
    let user_features = if config.features.is_empty() {
        String::new()
    } else {
        format!(", features = {:?}", config.features)
    };

    let migratio_cli_dep = match migratio_cli_source {
        MigratioCliSource::CratesIo(version) => {
            format!(
                "migratio-cli = {{ version = \"{}\", features = [\"{}\"] }}",
                version, feature
            )
        }
        MigratioCliSource::Path(path) => {
            format!(
                "migratio-cli = {{ path = \"{}\", features = [\"{}\"] }}",
                path, feature
            )
        }
    };

    format!(
        r#"[package]
name = "migratio-runner"
version = "0.0.0"
edition = "2021"
publish = false

# Prevent this from being included in the parent workspace
[workspace]

[[bin]]
name = "migratio-runner"
path = "src/main.rs"

[dependencies]
{package_name} = {{ path = "{package_dir}"{user_features} }}
{migratio_cli_dep}
clap = {{ version = "4", features = ["derive"] }}
"#,
        package_name = package_name,
        package_dir = package_dir,
        user_features = user_features,
        migratio_cli_dep = migratio_cli_dep,
    )
}

fn generate_runner_main(config: &MigratioConfig) -> String {
    let run_fn = match config.database.as_str() {
        "sqlite" => "run_sqlite",
        "mysql" => "run_mysql",
        "postgres" => "run_postgres",
        _ => "run_sqlite",
    };

    format!(
        r#"//! Auto-generated migration runner for migratio.
//! This file is generated by `cargo migratio` and should not be edited manually.

use migratio_cli::{{CliArgs, Commands, {run_fn}}};
use clap::Parser;

fn main() {{
    if let Err(e) = run() {{
        eprintln!("Error: {{}}", e);
        std::process::exit(1);
    }}
}}

fn run() -> Result<(), Box<dyn std::error::Error>> {{
    // Parse args first - this allows --help to work without DATABASE_URL
    let args = CliArgs::parse();

    // Handle commands that don't need a database connection
    if let Commands::List = &args.command {{
        let migrator = {migrator_fn}();
        let migrations = migrator.migrations();
        if migrations.is_empty() {{
            println!("No migrations defined.");
        }} else {{
            println!("Defined migrations ({{}}):", migrations.len());
            for m in migrations {{
                println!("  v{{}}: {{}}", m.version(), m.name());
                if let Some(desc) = m.description() {{
                    println!("      {{}}", desc);
                }}
            }}
        }}
        return Ok(());
    }}

    // Commands that need database access
    let database_url = std::env::var("{database_url_env}")
        .map_err(|_| "Environment variable {database_url_env} not set")?;

    let migrator = {migrator_fn}();

    {run_fn}(migrator, &database_url, args)
}}
"#,
        run_fn = run_fn,
        database_url_env = config.database_url_env,
        migrator_fn = config.migrator_fn,
    )
}
