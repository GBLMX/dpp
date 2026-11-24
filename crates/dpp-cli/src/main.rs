use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use dpp_core::{
    DppError, LevelInfo, LockFile, PackFormat, WorldManifest, WorldPaths, WorldSection,
    adjacent_server_jar, detect_pack_format, find_version_jar,
};

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
    /// Override the Minecraft version read from level.dat.
    #[arg(long, value_name = "VERSION")]
    mc_version: Option<String>,

    /// Override the data pack format, e.g. `121` or `121.0`.
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

    let level = LevelInfo::load(&paths.level_dat())?;
    let mc_version = args.mc_version.unwrap_or_else(|| level.mc_version.clone());

    let pack_format = match args.pack_format.as_deref() {
        Some(raw) => raw.parse::<PackFormat>().context("invalid --pack-format")?,
        None => detect_world_format(paths, &level, &mc_version)?,
    };

    println!(
        "detected Minecraft {} (snapshot: {}, data version: {})",
        level.mc_version,
        level.snapshot,
        level
            .data_version
            .map_or_else(|| "unknown".to_owned(), |value| value.to_string())
    );
    println!("using data pack format {pack_format}");

    let manifest = WorldManifest {
        world: WorldSection {
            mc_version: Some(mc_version),
            pack_format: Some(pack_format),
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

    let level = LevelInfo::load(&paths.level_dat())?;
    let manifest = WorldManifest::load(&paths.manifest())?;
    let lock = LockFile::load_optional(&paths.lock())?;

    println!("World: {}", paths.root().display());
    println!(
        "Minecraft: {} (snapshot: {})",
        level.mc_version, level.snapshot
    );
    if let Some(data_version) = level.data_version {
        println!("Data version: {data_version}");
    }
    println!(
        "Manifest: schema {}, {} direct dependencies",
        manifest.schema,
        manifest.dependencies.len()
    );

    if let Some(target) = manifest.world.mc_version.as_deref() {
        if target != level.mc_version {
            println!(
                "Warning: manifest targets Minecraft {target}, but world is {}",
                level.mc_version
            );
        }
    }

    if let Some(pack_format) = manifest.world.pack_format {
        println!("Target pack format: {pack_format}");
    }

    match detect_world_format(paths, &level, &level.mc_version) {
        Ok(detected) => {
            println!("Detected pack format: {detected}");
            if let Some(target) = manifest.world.pack_format {
                if target != detected {
                    println!(
                        "Warning: manifest targets pack format {target}, but world is {detected}"
                    );
                }
            }
        }
        Err(error) => println!("Detected pack format: unknown ({error})"),
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

    let level = LevelInfo::load(&paths.level_dat())?;
    let manifest = WorldManifest::load(&paths.manifest())?;
    paths.require_state_dirs()?;

    if let Some(target) = manifest.world.mc_version.as_deref() {
        if target != level.mc_version {
            return Err(DppError::ManifestWorldMcVersionMismatch {
                manifest: target.to_owned(),
                world: level.mc_version.clone(),
            }
            .into());
        }
    }

    if let Some(target) = manifest.world.pack_format {
        match detect_world_format(paths, &level, &level.mc_version) {
            Ok(world_format) => {
                if target != world_format {
                    return Err(DppError::ManifestWorldPackFormatMismatch {
                        manifest: target,
                        world: world_format,
                    }
                    .into());
                }
            }
            Err(error) => eprintln!("warning: could not verify pack format: {error}"),
        }
    }

    if let Some(lock) = LockFile::load_optional(&paths.lock())? {
        lock.validate()?;

        if let (Some(lock_format), Some(manifest_format)) =
            (lock.target_pack_format(), manifest.world.pack_format)
        {
            if lock_format != manifest_format {
                return Err(DppError::PackFormatMismatch {
                    lock: lock_format,
                    manifest: manifest_format,
                }
                .into());
            }
        }
    }

    println!("ok: {}", paths.root().display());
    Ok(())
}

fn detect_world_format(
    paths: &WorldPaths,
    level: &LevelInfo,
    mc_version: &str,
) -> Result<PackFormat> {
    let snapshot = level.snapshot && level.mc_version == mc_version;
    let jar = locate_jar(paths, mc_version);
    Ok(detect_pack_format(mc_version, snapshot, jar.as_deref())?)
}

fn locate_jar(paths: &WorldPaths, mc_version: &str) -> Option<PathBuf> {
    find_version_jar(mc_version).or_else(|| adjacent_server_jar(paths.root()))
}
