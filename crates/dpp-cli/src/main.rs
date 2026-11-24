use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use dpp_core::{
    AdapterEvent, AdapterManifest, AdapterSession, AppliedFile, BuildPackOptions, DppError,
    HostLockFile, HostProfile, HostProvidedFile, HostProvidedPack, IndexFile, InstancePaths,
    LevelInfo, LockFile, PackFormat, PackManifest, WorldManifest, WorldPaths, WorldSection,
    adjacent_server_jar, build_pack, detect_host, detect_pack_format, find_version_jar,
    inspect_zip, sha512_file,
};
use dpp_modrinth::{ModrinthClient, default_user_agent};
use dpp_ops::{AddReport, InstallReport};

#[derive(Debug, Parser)]
#[command(name = "dpp", version, about = "Minecraft data pack package manager")]
struct Cli {
    /// World directory; defaults to the current directory.
    #[arg(long, global = true, value_name = "PATH")]
    world: Option<PathBuf>,

    /// Server root for instance/host scope.
    #[arg(long, global = true, value_name = "PATH")]
    server_root: Option<PathBuf>,

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
    /// Add a datapack dependency and install it.
    Add(AddArgs),
    /// Remove a direct dependency and its managed files.
    Remove(RemoveArgs),
    /// Install every package pinned in lock.toml.
    Install(InstallArgs),
    /// Re-solve dependencies and install newer compatible versions.
    Update(UpdateArgs),
    /// Record the committed package layer as applied for the next activation.
    Apply(ApplyArgs),
    /// Save, restore, or list package snapshots.
    Snapshot(SnapshotArgs),
    /// Inspect the server host and record host.lock.
    Host(HostArgs),
    /// Manage instance-level JSON/TOML config patches.
    Config(ConfigArgs),
    /// Validate adapter manifests and protocol events.
    Adapter(AdapterArgs),
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
struct AddArgs {
    /// Modrinth project slug or ID.
    project: String,
}

#[derive(Debug, Args)]
struct HostArgs {
    #[command(subcommand)]
    command: HostCommand,
}

#[derive(Debug, Subcommand)]
enum HostCommand {
    /// Detect the host and show host.lock without writing it.
    Status,
    /// Detect the host, write host.lock, and print the observed profile.
    Check,
}

#[derive(Debug, Args)]
struct SnapshotArgs {
    #[command(subcommand)]
    command: SnapshotCommand,
}

#[derive(Debug, Subcommand)]
enum SnapshotCommand {
    /// Save the current managed package environment.
    Save(SnapshotSaveArgs),
    /// Restore a previously saved environment.
    Restore(SnapshotRestoreArgs),
    /// List saved snapshots.
    List,
}

#[derive(Debug, Args)]
struct SnapshotSaveArgs {
    /// Snapshot name.
    name: String,
}

#[derive(Debug, Args)]
struct SnapshotRestoreArgs {
    /// Snapshot name.
    name: String,
}

#[derive(Debug, Args)]
struct ConfigArgs {
    #[command(subcommand)]
    command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Allow a config target path.
    Allow(ConfigAllowArgs),
    /// Apply a JSON or TOML config patch.
    Apply(ConfigApplyArgs),
    /// List config targets managed by dpp.
    Status,
}

#[derive(Debug, Args)]
struct ConfigAllowArgs {
    /// Relative target path, e.g. config/example.toml.
    target: String,
}

#[derive(Debug, Args)]
struct ConfigApplyArgs {
    /// Path to a .toml or .json patch file.
    patch: PathBuf,
}

#[derive(Debug, Args)]
struct AdapterArgs {
    #[command(subcommand)]
    command: AdapterCommand,
}

#[derive(Debug, Subcommand)]
enum AdapterCommand {
    /// Validate an adapter manifest.
    Validate(AdapterValidateArgs),
    /// Validate one event against an adapter manifest.
    Event(AdapterEventArgs),
}

#[derive(Debug, Args)]
struct AdapterValidateArgs {
    /// Path to dpp-adapter.toml.
    manifest: PathBuf,
}

#[derive(Debug, Args)]
struct AdapterEventArgs {
    /// Path to dpp-adapter.toml.
    manifest: PathBuf,
    /// Path to the event JSON.
    event: PathBuf,
}

#[derive(Debug, Args)]
struct RemoveArgs {
    /// Modrinth project slug or ID.
    project: String,
    /// Write a staged plan instead of committing datapacks.
    #[arg(long)]
    stage: bool,
}

#[derive(Debug, Args)]
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
    let Cli {
        world,
        server_root,
        command,
    } = cli;

    match command {
        Command::Host(args) => host_command(server_root.as_deref(), args),
        Command::Config(args) => config_command(server_root.as_deref(), args),
        Command::Adapter(args) => adapter_command(args),
        Command::New(args) => new_project(args),
        Command::BuildPack(args) => build_pack_command(args),
        Command::Search(args) => search(args),
        Command::Inspect(args) => inspect_pack(args),
        command => {
            let paths = WorldPaths::discover(world.as_deref())?;
            if !matches!(&command, Command::Status | Command::Order(_)) {
                if let Some(message) = dpp_ops::recover_world(&paths)? {
                    eprintln!("{message}");
                }
            }
            match command {
                Command::Init(args) => init_world(&paths, args),
                Command::Status => show_status(&paths, server_root.as_deref()),
                Command::Order(args) => show_order(&paths, args),
                Command::Check => check_world(&paths),
                Command::Add(args) => add_pack(&paths, args, server_root.as_deref()),
                Command::Remove(args) => remove_pack(&paths, args),
                Command::Install(args) => install_lock(&paths, server_root.as_deref(), args.stage),
                Command::Update(args) => update_lock(&paths, server_root.as_deref(), args.stage),
                Command::Apply(args) => apply_command(&paths, args, server_root.as_deref()),
                Command::Snapshot(args) => snapshot_command(&paths, args),
                Command::Host(_)
                | Command::Config(_)
                | Command::Adapter(_)
                | Command::New(_)
                | Command::BuildPack(_)
                | Command::Search(_)
                | Command::Inspect(_) => unreachable!(),
            }
        }
    }
}

fn build_pack_command(args: BuildPackArgs) -> Result<()> {
    let built = build_pack(BuildPackOptions {
        source: args.path,
        output: args.output,
        url: args.url,
    })?;
    let json = serde_json::to_string_pretty(&built.index_entry)?;

    if args.json {
        println!("{json}");
        return Ok(());
    }

    println!("built: {}", built.path.display());
    println!("files: {}", built.files);
    println!("size: {} bytes", built.size);
    println!("sha512: {}", built.sha512);
    println!("index entry:");
    println!("{json}");
    Ok(())
}

fn new_project(args: NewArgs) -> Result<()> {
    if args.name.is_empty()
        || args.name == "."
        || args.name == ".."
        || args.name.contains('/')
        || args.name.contains('\\')
    {
        bail!("invalid project name {:?}", args.name);
    }
    let namespace = sanitize_namespace(&args.name);
    if namespace == "minecraft" {
        bail!("namespace `minecraft` is reserved");
    }
    let directory = args.dir.unwrap_or_else(|| PathBuf::from(&args.name));
    if directory.exists() {
        bail!("target directory already exists: {}", directory.display());
    }
    let pack_format = args
        .pack_format
        .parse::<PackFormat>()
        .context("invalid --pack-format")?;

    let result = (|| -> Result<()> {
        let source = directory.join("src");
        let data = source.join("data").join(&namespace);
        let load_tag = source.join("data/minecraft/tags/function/load.json");
        for path in [
            directory.join("dpp.toml"),
            directory.join("beet.json"),
            source.join("pack.mcmeta"),
            data.join("function/load.mcfunction"),
            data.join("function/hello.mcfunction"),
            load_tag.clone(),
        ] {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
        }

        let meta = format!(
            r#"{{"pack":{{"description":"{}","min_format":[{},{}],"max_format":[{},{}]}}}}"#,
            namespace,
            pack_format.major(),
            pack_format.minor(),
            pack_format.major(),
            pack_format.minor()
        );
        let manifest = format!(
            r#"[package]
id = "local:{namespace}"
version = "0.1.0"
mc_versions = ["{}"]
pack_format = [{}, {}]
description = "{}"
"#,
            args.mc_version,
            pack_format.major(),
            pack_format.minor(),
            namespace
        );
        let beet = format!(
            r#"{{
  "name": "{namespace}",
  "output": "build",
  "data_pack": {{ "load": ["src"] }}
}}
"#
        );
        let load_function = format!(
            "scoreboard objectives add {namespace}.version dummy\nscoreboard players set #protocol {namespace}.version 1\ndata merge storage {namespace}:meta {{version:\"0.1.0\",protocol:\"dpp.events/1\"}}\n"
        );
        let load_json = format!(r#"{{"values":["{namespace}:load"]}}"#);

        fs::write(directory.join("dpp.toml"), manifest)?;
        fs::write(directory.join("beet.json"), beet)?;
        fs::write(source.join("pack.mcmeta"), meta)?;
        fs::write(data.join("function/load.mcfunction"), load_function)?;
        fs::write(
            data.join("function/hello.mcfunction"),
            format!("say hello from {namespace}\n"),
        )?;
        fs::write(load_tag, load_json)?;
        Ok(())
    })();

    if let Err(error) = result {
        let _ = fs::remove_dir_all(&directory);
        return Err(error);
    }

    println!("created datapack project at {}", directory.display());
    println!("  package: local:{namespace}");
    println!("  next: cd {} && beet build", directory.display());
    Ok(())
}

fn sanitize_namespace(name: &str) -> String {
    let mut output = String::with_capacity(name.len());
    for character in name.chars() {
        let character = character.to_ascii_lowercase();
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            output.push(character);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "pack".to_owned()
    } else {
        output
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

fn host_command(server_root: Option<&std::path::Path>, args: HostArgs) -> Result<()> {
    let instance = InstancePaths::discover(server_root)?;
    let observed = detect_host(instance.root())?;
    let provided = HostProvidedFile::load_optional(&instance.host_provided())?;
    match args.command {
        HostCommand::Status => {
            println!("Instance: {}", instance.root().display());
            println!("Observed host:");
            print_host(&observed);
            print_provided("Host-provided (observed)", &provided.packs);
            match HostLockFile::load_optional(&instance.host_lock())? {
                Some(lock) => {
                    println!("host.lock:");
                    print_host(&lock.host);
                    print_provided("Host-provided (locked)", &lock.provided_packs);
                }
                None => println!("host.lock: absent (run `dpp host check`)"),
            }
        }
        HostCommand::Check => {
            instance.ensure_state_dirs()?;
            HostLockFile {
                host: observed.clone(),
                provided_packs: provided.packs.clone(),
            }
            .write(&instance.host_lock())?;
            println!("ok: {}", instance.root().display());
            print_host(&observed);
            print_provided("Host-provided", &provided.packs);
            println!("host.lock: {}", instance.host_lock().display());
        }
    }
    Ok(())
}

fn print_provided(label: &str, packs: &[HostProvidedPack]) {
    if packs.is_empty() {
        return;
    }
    println!("{label}:");
    for pack in packs {
        let version = pack.version.as_deref().unwrap_or("unknown");
        let enabled = match pack.enabled {
            Some(true) => "enabled",
            Some(false) => "disabled",
            None => "unknown",
        };
        println!(
            "  {} [{}] version={} ({enabled})",
            pack.id, pack.plugin, version
        );
    }
}

fn print_host(profile: &HostProfile) {
    println!("  kind: {}", profile.kind);
    if let Some(version) = profile.version.as_deref() {
        println!("  version: {version}");
    }
    if !profile.capabilities.is_empty() {
        println!("  capabilities: {}", profile.capabilities.join(", "));
    }
    for adapter in &profile.adapters {
        println!("  adapter: {}", adapter.name);
    }
}

fn adapter_command(args: AdapterArgs) -> Result<()> {
    match args.command {
        AdapterCommand::Validate(args) => {
            let manifest = AdapterManifest::load(&args.manifest)?;
            println!(
                "adapter: {} {} ({})",
                manifest.adapter.id, manifest.adapter.version, manifest.adapter.protocol
            );
            println!("capabilities: {}", manifest.adapter.capabilities.join(", "));
            if !manifest.adapter.events.is_empty() {
                println!("events: {}", manifest.adapter.events.join(", "));
            }
        }
        AdapterCommand::Event(args) => {
            let manifest = AdapterManifest::load(&args.manifest)?;
            let text = std::fs::read_to_string(&args.event)?;
            let event = AdapterEvent::from_json_str(&text)?;
            AdapterSession::default().accept(&manifest, &event)?;
            println!("ok: event {} id {}", event.event, event.id);
        }
    }
    Ok(())
}

fn config_command(server_root: Option<&std::path::Path>, args: ConfigArgs) -> Result<()> {
    let instance = InstancePaths::discover(server_root)?;
    match args.command {
        ConfigCommand::Allow(args) => {
            dpp_ops::allow_target(&instance, &args.target)?;
            println!("allowed config target: {}", args.target);
        }
        ConfigCommand::Apply(args) => {
            let patch = dpp_ops::ConfigPatch::load(&args.patch)?;
            let report = dpp_ops::apply_patch(&instance, &patch)?;
            println!("applied config patch to {}", report.target);
            if let Some(backup) = report.backup {
                println!("  backup: {}", backup.display());
            }
            println!("  sha512: {}", report.hash);
        }
        ConfigCommand::Status => {
            let targets = dpp_ops::target_lock_summary(&instance)?;
            if targets.is_empty() {
                println!("no managed config targets");
                return Ok(());
            }
            for (target, hash) in targets {
                println!("{target}\t{hash}");
            }
        }
    }
    Ok(())
}

fn add_pack(
    paths: &WorldPaths,
    args: AddArgs,
    server_root: Option<&std::path::Path>,
) -> Result<()> {
    let (mc_version, target) = target_for_world(paths)?;
    let host = load_host_profile(server_root)?;
    let client = ModrinthClient::new(default_user_agent())?;
    let report: AddReport = dpp_ops::add_package(
        paths,
        &args.project,
        &mc_version,
        target,
        &client,
        host.as_ref(),
    )?;
    print_install_report(&report.install);
    Ok(())
}

fn remove_pack(paths: &WorldPaths, args: RemoveArgs) -> Result<()> {
    let report = dpp_ops::remove_package(paths, &args.project, args.stage)?;
    if let Some(plan) = &report.staged_plan {
        println!("staged plan: {}", plan.display());
        return Ok(());
    }
    match report.install_name {
        Some(install_name) => println!("removed {} ({install_name})", report.id),
        None => println!("removed {}", report.id),
    }
    Ok(())
}

fn install_lock(
    paths: &WorldPaths,
    server_root: Option<&std::path::Path>,
    stage: bool,
) -> Result<()> {
    let (mc_version, target) = target_for_world(paths)?;
    let host = load_host_profile(server_root)?;
    let client = ModrinthClient::new(default_user_agent())?;
    let report: InstallReport =
        dpp_ops::install_world(paths, &mc_version, target, &client, host.as_ref(), stage)?;
    print_install_report(&report);
    Ok(())
}

fn update_lock(
    paths: &WorldPaths,
    server_root: Option<&std::path::Path>,
    stage: bool,
) -> Result<()> {
    let (mc_version, target) = target_for_world(paths)?;
    let host = load_host_profile(server_root)?;
    let client = ModrinthClient::new(default_user_agent())?;
    let report = dpp_ops::update_world(paths, &mc_version, target, &client, host.as_ref(), stage)?;
    if let Some(plan) = &report.staged_plan {
        println!("staged plan: {}", plan.display());
        return Ok(());
    }

    for package in &report.installed {
        println!(
            "updated {} {} -> {}",
            package.id,
            package.version,
            package.path.display()
        );
    }
    for package in &report.skipped {
        println!("ok {package}");
    }
    for package in &report.removed {
        println!("removed {package}");
    }
    if report.installed.is_empty() && report.skipped.is_empty() && report.removed.is_empty() {
        println!("already up to date");
    }
    Ok(())
}

fn apply_command(
    paths: &WorldPaths,
    args: ApplyArgs,
    server_root: Option<&std::path::Path>,
) -> Result<()> {
    let report = dpp_ops::apply_world(
        paths,
        dpp_ops::ApplyOptions {
            dry_run: args.dry_run,
            manual: args.manual,
            rcon: args.rcon,
            server_root: server_root.map(std::path::Path::to_path_buf),
            log: args.log,
            abort: args.abort,
            rollback: args.rollback,
        },
    )?;

    if report.dry_run {
        println!("would apply {}", report.lock_hash);
    } else if report.method == "abort" {
        println!("aborted staged plan");
    } else if report.method == "rollback" {
        println!("rolled back to {}", report.lock_hash);
    } else {
        println!("applied {}", report.lock_hash);
    }
    if report.staged {
        println!("  committed staged plan");
    }
    println!("  method: {}", report.method);
    println!("  verification: {}", report.verification);
    println!("  applied_at: {}", report.applied_at);
    match report.method.as_str() {
        "rcon" if report.dry_run => println!("  would send reload through RCON"),
        "rcon" => println!("  reload sent through RCON"),
        "manual" => {
            println!("  restart or rejoin the world to activate the committed package layer")
        }
        "abort" | "rollback" => {}
        _ => println!("  effective on the next world start; no reload was sent"),
    }
    for warning in &report.warnings {
        println!("warning: {warning}");
    }
    Ok(())
}

fn load_host_profile(server_root: Option<&std::path::Path>) -> Result<Option<HostProfile>> {
    let Some(root) = server_root else {
        return Ok(None);
    };
    let instance = InstancePaths::discover(Some(root))?;
    Ok(Some(detect_host(instance.root())?))
}

fn snapshot_command(paths: &WorldPaths, args: SnapshotArgs) -> Result<()> {
    match args.command {
        SnapshotCommand::Save(args) => {
            let info = dpp_ops::save_snapshot(paths, &args.name)?;
            println!(
                "saved snapshot {} ({} packages)",
                info.name,
                info.packages.len()
            );
            println!("  path: {}", info.path.display());
        }
        SnapshotCommand::Restore(args) => {
            let info = dpp_ops::restore_snapshot(paths, &args.name)?;
            println!(
                "restored snapshot {} ({} packages)",
                info.name,
                info.packages.len()
            );
            println!("  path: {}", info.path.display());
        }
        SnapshotCommand::List => {
            let snapshots = dpp_ops::list_snapshots(paths)?;
            if snapshots.is_empty() {
                println!("no snapshots");
                return Ok(());
            }
            for info in snapshots {
                println!(
                    "{}\t{}\t{} packages",
                    info.name,
                    info.created_at,
                    info.packages.len()
                );
            }
        }
    }
    Ok(())
}

fn print_install_report(report: &InstallReport) {
    for package in &report.installed {
        println!(
            "installed {} {} -> {}",
            package.id,
            package.version,
            package.path.display()
        );
    }
    for package in &report.skipped {
        println!("ok {package}");
    }
    if let Some(plan) = &report.staged_plan {
        println!("staged plan: {}", plan.display());
    } else if report.installed.is_empty() && report.skipped.is_empty() {
        println!("nothing to install");
    }
}

fn target_for_world(paths: &WorldPaths) -> Result<(String, PackFormat)> {
    paths.require_level_dat()?;
    let level = LevelInfo::load(&paths.level_dat())?;
    let mc_version = level.mc_version.clone();
    match detect_world_format(paths, &level, &mc_version) {
        Ok(target) => Ok((mc_version, target)),
        Err(error) if paths.manifest().exists() => {
            let manifest = WorldManifest::load(&paths.manifest())?;
            if let Some(target) = manifest.world.pack_format {
                let mc_version = manifest
                    .world
                    .mc_version
                    .unwrap_or_else(|| mc_version.clone());
                return Ok((mc_version, target));
            }
            Err(error)
        }
        Err(error) => Err(error),
    }
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

    match PackManifest::load_from_zip(&args.path)? {
        Some(manifest) => {
            println!("pack: {} {}", manifest.package.id, manifest.package.version);
            if !manifest.dependencies.is_empty() {
                println!(
                    "dependencies: {}",
                    manifest
                        .dependencies
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            if !manifest.conflicts.is_empty() {
                println!(
                    "conflicts: {}",
                    manifest
                        .conflicts
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            if !manifest.host.requires.is_empty() {
                println!("host requires: {}", manifest.host.requires.join(", "));
            }
            if !manifest.host.forbids.is_empty() {
                println!("host forbids: {}", manifest.host.forbids.join(", "));
            }
        }
        None => println!("pack manifest: legacy (no dpp.toml)"),
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
    match dpp_ops::journal_status(paths)? {
        Some(journal) => println!(
            "Journal: {} transaction {} (recovery pending; run a mutating command or dpp check)",
            journal.phase, journal.transaction
        ),
        None => println!("Journal: none"),
    }
    match dpp_ops::staged_plan_status(paths)? {
        Some(plan) => println!(
            "Staged: {} plan ({} packages, {} removed)",
            plan.transaction, plan.packages, plan.removed
        ),
        None => println!("Staged: none"),
    }
    match AppliedFile::load_optional(&paths.applied())? {
        Some(applied) => {
            println!(
                "Applied: {} via {}, {} at {}",
                applied.lock_hash, applied.method, applied.verification, applied.applied_at
            );
            let current = if paths.lock().is_file() {
                Some(format!("sha512:{}", sha512_file(&paths.lock())?))
            } else {
                None
            };
            if current.as_deref() != Some(applied.lock_hash.as_str()) {
                println!("Warning: applied marker does not match the current lock");
            }
        }
        None => println!("Applied: none"),
    }
    if let Some(root) = server_root {
        let instance = InstancePaths::discover(Some(root))?;
        let provided_packs = if let Some(lock) = HostLockFile::load_optional(&instance.host_lock())?
        {
            lock.provided_packs
        } else {
            HostProvidedFile::load_optional(&instance.host_provided())?.packs
        };
        print_provided("Host-provided:", &provided_packs);
        let managed = IndexFile::load_optional(&paths.index())?.unwrap_or_default();
        for provided in &provided_packs {
            let index_collision = managed.packages.iter().any(|package| {
                package.install_name == provided.id
                    || package.package_id.as_deref() == Some(provided.id.as_str())
            });
            let lock_collision = lock.as_ref().is_some_and(|lock| {
                lock.packages.iter().any(|package| {
                    package.install_name == provided.id
                        || package.package_id.as_deref() == Some(provided.id.as_str())
                })
            });
            if index_collision || lock_collision {
                println!(
                    "Warning: host-provided {} collides with a dpp-managed package; not auto-removing either",
                    provided.id
                );
            }
        }
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

    if paths.lock().is_file() {
        let report = dpp_ops::order_world(paths)?;
        if report.status == "drift" {
            eprintln!("warning: load order differs from lock; run `dpp order --print-commands`");
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
