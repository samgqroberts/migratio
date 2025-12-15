//! Cargo subcommand for migratio database migrations.
//!
//! This binary provides the `cargo migratio` command, which generates and runs
//! a migration runner based on user configuration in `Cargo.toml`.

use std::fs;
use std::process::Command;

use cargo_metadata::camino;
use cargo_metadata::MetadataCommand;
use clap::Parser;
use serde::Deserialize;

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
    /// Database type: "sqlite" or "mysql"
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
    let root_package = metadata
        .root_package()
        .ok_or("No root package found. Are you in a Cargo project directory?")?;

    // Find migratio configuration in package.metadata.migratio
    let migratio_metadata = root_package
        .metadata
        .get("migratio")
        .ok_or("No [package.metadata.migratio] found in Cargo.toml. Please add configuration.")?;

    let config: MigratioConfig = serde_json::from_value(migratio_metadata.clone())
        .map_err(|e| format!("Invalid migratio config: {}", e))?;

    // Validate database type
    if config.database != "sqlite" && config.database != "mysql" {
        return Err(format!(
            "Invalid database type '{}'. Must be 'sqlite' or 'mysql'.",
            config.database
        )
        .into());
    }

    // Get the package directory (parent of Cargo.toml)
    let package_dir = root_package
        .manifest_path
        .parent()
        .ok_or("Could not determine package directory")?;

    // Create runner directory
    let runner_dir = metadata.target_directory.join("migratio-runner");
    fs::create_dir_all(&runner_dir)?;
    fs::create_dir_all(runner_dir.join("src"))?;

    // Generate runner Cargo.toml
    let runner_cargo_toml = generate_runner_cargo_toml(
        &root_package.name,
        package_dir,
        &metadata.workspace_root,
        &config,
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

fn generate_runner_cargo_toml(
    package_name: &str,
    package_dir: &camino::Utf8Path,
    workspace_root: &camino::Utf8Path,
    config: &MigratioConfig,
) -> String {
    let feature = match config.database.as_str() {
        "sqlite" => "sqlite",
        "mysql" => "mysql",
        _ => "sqlite",
    };

    // Build features string for the user's crate
    let user_features = if config.features.is_empty() {
        String::new()
    } else {
        format!(", features = {:?}", config.features)
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
migratio-cli = {{ path = "{workspace_root}/migratio-cli", features = ["{feature}"] }}
clap = {{ version = "4", features = ["derive"] }}
"#,
        package_name = package_name,
        package_dir = package_dir,
        workspace_root = workspace_root,
        user_features = user_features,
        feature = feature,
    )
}

fn generate_runner_main(config: &MigratioConfig) -> String {
    let run_fn = match config.database.as_str() {
        "sqlite" => "run_sqlite",
        "mysql" => "run_mysql",
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
