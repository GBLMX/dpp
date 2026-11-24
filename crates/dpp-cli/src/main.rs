use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use dpp_core::{
    DppError, LevelInfo, LockFile, PackFormat, WorldManifest, WorldPaths, WorldSection,
    adjacent_server_jar, detect_pack_format, find_version_jar, inspect_zip,
    AdapterEvent, AdapterManifest, AdapterSession, AppliedFile, BuildPackOptions, DppError,
    HostLockFile, HostProfile, HostProvidedFile, HostProvidedPack, IndexFile, InstancePaths,
    LevelInfo, LockFile, PackFormat, PackManifest, WorldManifest, WorldPaths, WorldSection,
    adjacent_server_jar, build_pack, detect_host, detect_pack_format, find_version_jar,
    inspect_zip, sha512_file,
};
use dpp_modrinth::{ModrinthClient, default_user_agent};

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
    /// Create a new Beet-compatible datapack project.
    New(NewArgs),
    /// Validate and deterministically package a datapack directory.
    BuildPack(BuildPackArgs),
    /// Show the world manifest and lock status.
    Status,
    /// Report advisory datapack load order; never writes level.dat or runs commands.
    Order(OrderArgs),
    /// Validate manifest, lock, and dpp state directories.
    Check,
    /// Search Modrinth datapacks.
    Search(SearchArgs),
    /// Inspect a local datapack zip against a target format.
    Inspect(InspectArgs),
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

#[derive(Debug, Args)]
struct NewArgs {
    /// Project / pack name; also used as the default directory.
    name: String,

    /// Target directory; defaults to the project name.
    #[arg(long, value_name = "PATH")]
    dir: Option<PathBuf>,

    /// Target Minecraft version.
    #[arg(long, default_value = "26.3", value_name = "VERSION")]
    mc_version: String,

    /// Target data pack format.
    #[arg(long, default_value = "121.0", value_name = "FORMAT")]
    pack_format: String,
}

#[derive(Debug, Args)]
struct BuildPackArgs {
    /// Pack source directory, or a Beet project root containing `src/`.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Output zip path; defaults to <id>-<version>.zip beside the source.
    #[arg(long, value_name = "ZIP")]
    output: Option<PathBuf>,

    /// URL to include in the printed static-source index entry.
    #[arg(long, value_name = "URL")]
    url: Option<String>,

    /// Print only the JSON index entry.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct OrderArgs {
    /// Print the read-only `/datapack enable` suggestions.
    #[arg(long)]
    print_commands: bool,

    /// Print the report as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ApplyArgs {
    /// Print the activation plan without writing applied.toml.
    #[arg(long)]
    dry_run: bool,

    /// Record a manual restart/rejoin instead of the default next-start marker.
    #[arg(long)]
    manual: bool,

    /// Send `/reload` through localhost RCON instead of manual activation.
    #[arg(long)]
    rcon: bool,

    /// Server log to inspect after sending reload.
    #[arg(long, value_name = "PATH")]
    log: Option<PathBuf>,

    /// Remove a staged plan without touching datapacks.
    #[arg(long)]
    abort: bool,

    /// Restore the most recent pre-apply snapshot.
    #[arg(long)]
    rollback: bool,
}

#[derive(Debug, Args)]
struct InstallArgs {
    /// Write a staged plan instead of committing datapacks.
    #[arg(long)]
    stage: bool,
}

#[derive(Debug, Args)]
struct UpdateArgs {
    /// Write a staged plan instead of committing datapacks.
    #[arg(long)]
    stage: bool,
}
#[derive(Debug, Args)]
    /// Write a staged plan instead of committing datapacks.
    #[arg(long)]
    stage: bool,
struct SearchArgs {
    /// Search query.
    query: String,

    /// Filter results by Minecraft version.
    #[arg(long, value_name = "VERSION")]
    mc_version: Option<String>,

    /// Maximum number of results.
    #[arg(long, default_value_t = 10, value_name = "N")]
    limit: u8,
}

#[derive(Debug, Args)]
struct InspectArgs {
    /// Path to a datapack zip.
    path: PathBuf,

    /// Target data pack format, e.g. `121` or `121.0`.
    #[arg(long, value_name = "FORMAT")]
    pack_format: String,
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
    let Cli { world, command } = cli;

    match command {
        Command::New(args) => new_project(args),
        Command::BuildPack(args) => build_pack_command(args),
        Command::Search(args) => search(args),
        Command::Inspect(args) => inspect_pack(args),
        command => {
            let paths = WorldPaths::discover(world.as_deref())?;
            match command {
                Command::Init(args) => init_world(&paths, args),
                Command::Status => show_status(&paths, server_root.as_deref()),
                Command::Order(args) => show_order(&paths, args),
                Command::Check => check_world(&paths),
                Command::Search(_) | Command::Inspect(_) => unreachable!(),
            }
        }
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

fn search(args: SearchArgs) -> Result<()> {
    if args.limit == 0 || args.limit > 100 {
        bail!("--limit must be between 1 and 100");
    }

    let client = ModrinthClient::new(default_user_agent())?;
    let response = client.search_datapacks(&args.query, args.mc_version.as_deref(), args.limit)?;

    if response.hits.is_empty() {
        println!("No results.");
        return Ok(());
    }

    println!(
        "{} hits (showing {})",
        response.total_hits,
        response.hits.len()
    );
    for hit in response.hits {
        println!("{}\t{}", hit.slug, hit.title);
        if !hit.description.is_empty() {
            println!("  {}", hit.description);
        }
    }

    Ok(())
}

fn inspect_pack(args: InspectArgs) -> Result<()> {
    let format = args
        .pack_format
        .parse::<PackFormat>()
        .context("invalid --pack-format")?;
    let inspection = inspect_zip(&args.path, format)?;

    println!("ok: {}", args.path.display());
    println!("pack format: {format}");
    println!("files: {}", inspection.file_count);
    println!("uncompressed: {} bytes", inspection.total_uncompressed_size);

    let features = inspection.meta.feature_flags();
    if !features.is_empty() {
        println!("features: {}", features.join(", "));
    }

    Ok(())
}

fn show_status(paths: &WorldPaths, server_root: Option<&std::path::Path>) -> Result<()> {
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

    match &lock {
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

fn show_order(paths: &WorldPaths, args: OrderArgs) -> Result<()> {
    let report = dpp_ops::order_world(paths)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let names = |names: &[String]| {
        if names.is_empty() {
            "none".to_owned()
        } else {
            names.join(", ")
        }
    };
    let expected: Vec<String> = report
        .expected
        .iter()
        .map(|package| package.install_name.clone())
        .collect();
    println!("Expected: {}", names(&expected));
    match &report.observed {
        Some(observed) => println!("Observed: {}", names(observed)),
        None => println!("Observed: unknown"),
    }
    println!("Order: {}", report.status);
    if !report.missing.is_empty() {
        println!("Missing: {}", report.missing.join(", "));
    }
    if !report.disabled.is_empty() {
        println!("Disabled: {}", report.disabled.join(", "));
    }
    for warning in &report.warnings {
        println!("Warning: {warning}");
    }
    if args.print_commands {
        if report.actions.is_empty() {
            println!("Actions: none");
        } else {
            println!("Actions (run in game/console):");
            for action in &report.actions {
                println!("{action}");
            }
        }
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
