mod zip_ext;

use std::{
    fs::{self},
    path::{Path, PathBuf},
    process::{self, Command},
};

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use fs_extra::{dir, file};
use time::{OffsetDateTime, UtcOffset};
use zip::{CompressionMethod, DateTime, write::FileOptions};
use zip_ext::zip_create_from_directory_with_options;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Check the build of device_faker
    Check {
        /// Build in release mode (default: false)
        #[clap(short, long, default_value = "false")]
        release: bool,

        /// Print detailed output (default: false)
        #[clap(short, long, default_value = "false")]
        verbose: bool,
    },

    /// Build device_faker and device_faker_cli
    Build {
        /// Build in release mode (default: false)
        #[clap(short, long, default_value = "false")]
        release: bool,

        /// Print detailed output (default: false)
        #[clap(short, long, default_value = "false")]
        verbose: bool,
    },

    /// Clean build artifacts
    Clean,

    /// Format source code
    Format {
        /// Print detailed output (default: false)
        #[clap(short, long, default_value = "false")]
        verbose: bool,
    },

    /// Run the Clippy linter
    Lint {
        /// Automatically fix lint issues (default: false)
        #[clap(short, long, default_value = "false")]
        fix: bool,
    },

    /// Update project dependencies
    Update,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let Some(command) = cli.command else {
        eprintln!("No command specified. Use --help to see available commands.");
        process::exit(1);
    };

    match command {
        Commands::Check { release, verbose } => {
            check(release, verbose)?;
        }
        Commands::Build { release, verbose } => {
            build(release, verbose)?;
        }
        Commands::Clean => {
            clean()?;
        }
        Commands::Format { verbose } => {
            format(verbose)?;
        }
        Commands::Lint { fix } => {
            lint(fix)?;
        }
        Commands::Update => {
            update()?;
        }
    }

    Ok(())
}

fn build(release: bool, verbose: bool) -> Result<()> {
    let temp_dir = temp_dir(release);

    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir)?;

    let mut cargo = cargo_ndk();
    let args = vec!["build", "--target", "aarch64-linux-android"];
    cargo.args(args);
    if release {
        cargo.arg("--release");
    }

    if verbose {
        cargo.arg("--verbose");
    }

    run_command(&mut cargo, "build device_faker native library")?;
    run_command(
        cargo.current_dir("device_faker_cli/"),
        "build device_faker_cli",
    )?;

    // Build WebUI first so that module/webroot/ exists before copying
    build_webui()?;

    let module_dir = module_dir();
    dir::copy(
        &module_dir,
        &temp_dir,
        &dir::CopyOptions::new().overwrite(true).content_only(true),
    )
    .context("copy module template into output temp directory")?;
    fs::remove_file(temp_dir.join(".gitignore")).context("remove temp .gitignore")?;

    let zygisk_bin = bin_path(release);
    ensure!(
        zygisk_bin.exists(),
        "native library was not produced at {}; check the earlier cargo-ndk build output",
        zygisk_bin.display()
    );
    file::copy(
        &zygisk_bin,
        temp_dir.join("zygisk/arm64-v8a.so"),
        &file::CopyOptions::new().overwrite(true),
    )
    .with_context(|| format!("copy {} into module zygisk output", zygisk_bin.display()))?;

    let cli_bin = cli_bin_path(release);
    ensure!(
        cli_bin.exists(),
        "CLI binary was not produced at {}; check the earlier cargo-ndk build output",
        cli_bin.display()
    );
    file::copy(
        &cli_bin,
        temp_dir.join("bin/device_faker_cli"),
        &file::CopyOptions::new().overwrite(true),
    )
    .with_context(|| format!("copy {} into module bin output", cli_bin.display()))?;

    let build_type = if release { "release" } else { "debug" };
    let package_path = Path::new("output").join(format!("device_faker-({build_type}).zip"));

    let local_offset = UtcOffset::from_hms(8, 0, 0).expect("UTC+8 offset should always be valid");

    zip_create_from_directory_with_options(&package_path, &temp_dir, |entry_path| {
        let mut options: FileOptions<'_, ()> = FileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(9));

        if let Some(modified_time) = zip_entry_modified_time(entry_path, local_offset) {
            options = options.last_modified_time(modified_time);
        }

        options
    })
    .with_context(|| format!("create package zip at {}", package_path.display()))?;

    println!("device_faker built successfully: {:?}", package_path);

    Ok(())
}

fn zip_entry_modified_time(entry_path: &Path, local_offset: UtcOffset) -> Option<DateTime> {
    let modified = fs::metadata(entry_path).ok()?.modified().ok()?;
    let modified = OffsetDateTime::from(modified).to_offset(local_offset);

    DateTime::try_from(modified).ok()
}

fn check(release: bool, verbose: bool) -> Result<()> {
    let mut cargo = cargo_ndk();
    cargo.args([
        "check",
        "--target",
        "aarch64-linux-android",
        "-Z",
        "build-std",
        "-Z",
        "trim-paths",
    ]);
    cargo.env("RUSTFLAGS", "-C default-linker-libraries");

    if release {
        cargo.arg("--release");
    }

    if verbose {
        cargo.arg("--verbose");
    }

    run_command(&mut cargo, "check device_faker native library")?;

    Ok(())
}

fn clean() -> Result<()> {
    let temp_dir = temp_dir(false);
    let _ = fs::remove_dir_all(&temp_dir);

    run_command(Command::new("cargo").arg("clean"), "clean cargo artifacts")?;

    Ok(())
}

fn format(verbose: bool) -> Result<()> {
    let mut command = Command::new("cargo");
    command.args(["fmt", "--all"]);
    if verbose {
        command.arg("--verbose");
    }
    run_command(&mut command, "format workspace")?;

    Ok(())
}

fn lint(fix: bool) -> Result<()> {
    let command_builder = |fix: bool| {
        let mut command = cargo_ndk();
        command.arg("clippy");
        if fix {
            command.args(["--fix", "--allow-dirty", "--allow-staged", "--all"]);
        }
        command.args(["--target", "aarch64-linux-android"]);
        command
    };

    let mut debug_command = command_builder(fix);
    run_command(&mut debug_command, "run clippy for debug build")?;
    let mut release_command = command_builder(fix);
    release_command.arg("--release");
    run_command(&mut release_command, "run clippy for release build")?;

    Ok(())
}

fn update() -> Result<()> {
    run_command(
        Command::new("cargo").args(["update", "--recursive"]),
        "update workspace dependencies",
    )?;
    run_command(
        Command::new("cargo")
            .current_dir("xtask")
            .args(["update", "--recursive"]),
        "update xtask dependencies",
    )?;

    Ok(())
}

fn module_dir() -> PathBuf {
    Path::new("module").to_path_buf()
}

fn temp_dir(release: bool) -> PathBuf {
    Path::new("output")
        .join(".temp")
        .join(if release { "release" } else { "debug" })
}

fn bin_path(release: bool) -> PathBuf {
    Path::new("target")
        .join("aarch64-linux-android")
        .join(if release { "release" } else { "debug" })
        .join("libzygisk.so")
}

fn cli_bin_path(release: bool) -> PathBuf {
    Path::new("device_faker_cli/target")
        .join("aarch64-linux-android")
        .join(if release { "release" } else { "debug" })
        .join("device_faker_cli")
}

fn cargo_ndk() -> Command {
    let mut command = Command::new("cargo");
    command
        .args(["+nightly", "ndk", "--platform", "31", "-t", "arm64-v8a"])
        .env("RUSTFLAGS", "-C default-linker-libraries");
    command
}

fn build_webui() -> Result<()> {
    let npm_cmd = if cfg!(windows) { "npm.cmd" } else { "npm" };
    let npm = || {
        let mut command = Command::new(npm_cmd);
        command.current_dir("webui");
        command
    };

    let mut install = npm();
    install.arg("install");
    run_command(&mut install, "install WebUI dependencies")?;

    let mut build = npm();
    build.args(["run", "build"]);
    run_command(&mut build, "build WebUI")?;

    Ok(())
}

fn run_command(command: &mut Command, description: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("failed to start {description}"))?;
    ensure!(status.success(), "{description} failed with status {status}");
    Ok(())
}
