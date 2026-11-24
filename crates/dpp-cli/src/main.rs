use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use dpp_core::{DppError, LockFile, PackFormat, WorldManifest, WorldPaths, WorldSection};
#[derive(Debug, Parser)]
#[command(name = "dpp", version, about = "Minecraft data pack package manager")]
struct Cli {
    /// World directory; defaults to the current directory.
    #[arg(long, global = true, value_name = "PATH")]
    world: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize dpp state in a Minecraft world.
    Init(InitArgs),
    /// Show the world manifest and lock status.
    Status,
    /// Validate manifest, lock, and dpp state directories.
    Check,
}

#[derive(Debug, Args)]
struct InitArgs {
    /// Minecraft version to record in dpp.toml.
    #[arg(long, value_name = "VERSION")]
    mc_version: Option<String>,

    /// Data pack format to record, e.g. `121` or `121.0`.
    #[arg(long, value_name = "FORMAT")]
    pack_format: Option<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let paths = WorldPaths::discover(cli.world.as_deref())?;

    match cli.command {
        Command::Init(args) => init_world(&paths, args),
        Command::Status => show_status(&paths),
        Command::Check => check_world(&paths),
    }
}

fn init_world(paths: &WorldPaths, args: InitArgs) -> Result<()> {
    paths.require_level_dat()?;

    if paths.manifest().exists() {
        return Err(DppError::ManifestExists(paths.manifest()).into());
    }

    let pack_format = args
        .pack_format
        .as_deref()
        .map(str::parse::<PackFormat>)
        .transpose()
        .context("invalid --pack-format")?;

    let manifest = WorldManifest {
        world: WorldSection {
            mc_version: args.mc_version,
            pack_format,
            order: None,
        },
        ..WorldManifest::default()
    };

    paths.ensure_state_dirs()?;
    manifest.write_new(&paths.manifest())?;

    println!("initialized dpp world at {}", paths.root().display());
    println!("  manifest: {}", paths.manifest().display());
    println!("  state:    {}", paths.state_dir().display());
    Ok(())
}

fn show_status(paths: &WorldPaths) -> Result<()> {
    paths.require_level_dat()?;

    let manifest = WorldManifest::load(&paths.manifest())?;
    let lock = LockFile::load_optional(&paths.lock())?;

    println!("World: {}", paths.root().display());
    println!(
        "Manifest: schema {}, {} direct dependencies",
        manifest.schema,
        manifest.dependencies.len()
    );

    if let Some(pack_format) = manifest.world.pack_format {
        println!("Target pack format: {pack_format}");
    }

    match lock {
        Some(lock) => {
            println!("Lock: {} packages", lock.packages.len());
            if let Some(pack_format) = lock.target_pack_format() {
                println!("Lock target pack format: {pack_format}");
            }
        }
        None => println!("Lock: absent"),
    }

    if paths.state_dir().is_dir() {
        println!("State: {}", paths.state_dir().display());
    } else {
        println!("State: missing (run `dpp init`)");
    }

    Ok(())
}

fn check_world(paths: &WorldPaths) -> Result<()> {
    paths.require_level_dat()?;

    let manifest = WorldManifest::load(&paths.manifest())?;
    paths.require_state_dirs()?;

    if let Some(lock) = LockFile::load_optional(&paths.lock())? {
        lock.validate()?;

        if let (Some(lock_format), Some(manifest_format)) =
            (lock.target_pack_format(), manifest.world.pack_format)
            && lock_format != manifest_format
        {
            return Err(DppError::PackFormatMismatch {
                lock: lock_format,
                manifest: manifest_format,
            }
            .into());
        }
    }

    println!("ok: {}", paths.root().display());
    Ok(())
}
