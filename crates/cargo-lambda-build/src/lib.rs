use cargo_lambda_metadata::{cargo::binary_targets, fs::rename};
use cargo_zigbuild::Build as ZigBuild;
use clap::{Args, ValueHint};
use miette::{IntoDiagnostic, Result, WrapErr};
use object::{read::File as ObjectFile, Architecture, Object};
use sha2::{Digest, Sha256};
use std::{
    fs::{create_dir_all, read, File},
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
};
use strum_macros::EnumString;
use target_arch::TargetArch;
use zip::{write::FileOptions, ZipWriter};

mod toolchain;
mod zig;

#[derive(Args, Clone, Debug)]
#[clap(name = "build")]
pub struct Build {
    /// The format to produce the compile Lambda into, acceptable values are [Binary, Zip]
    #[clap(long, default_value_t = OutputFormat::Binary)]
    output_format: OutputFormat,

    /// Directory where the final lambda binaries will be located
    #[clap(short, long, value_hint = ValueHint::DirPath)]
    lambda_dir: Option<PathBuf>,

    /// Shortcut for --target aarch64-unknown-linux-gnu
    #[clap(long)]
    arm64: bool,

    /// Whether the code that you're building is a Lambda Extension
    #[clap(long)]
    extension: bool,

    #[clap(flatten)]
    build: ZigBuild,
}

pub use cargo_zigbuild::Zig;

pub const TARGET_ARM: &str = "aarch64-unknown-linux-gnu";
pub const TARGET_X86_64: &str = "x86_64-unknown-linux-gnu";

mod target_arch;

#[derive(Clone, Debug, strum_macros::Display, EnumString)]
#[strum(ascii_case_insensitive)]
enum OutputFormat {
    Binary,
    Zip,
}

impl Build {
    pub async fn run(&mut self) -> Result<()> {
        let rustc_meta = rustc_version::version_meta().into_diagnostic()?;
        let host_target = &rustc_meta.host;
        let release_channel = &rustc_meta.channel;

        if self.arm64 && !self.build.target.is_empty() {
            return Err(miette::miette!(
                "invalid options: --arm and --target cannot be specified at the same time"
            ));
        }

        let target_arch = if self.arm64 {
            TargetArch::arm64()
        } else {
            let build_target = self.build.target.get(0);
            match build_target {
                Some(target) => TargetArch::from_str(target)?,
                // No explicit target, but build host same as target host
                None if host_target == TARGET_ARM || host_target == TARGET_X86_64 => {
                    // Set the target explicitly, so it's easier to find the binaries later
                    TargetArch::from_str(host_target)?
                }
                // No explicit target, and build host not compatible with Lambda hosts
                None => TargetArch::x86_64(),
            }
        };
        self.build.target = vec![target_arch.full_zig_string()];
        let rustc_target_without_glibc_version = target_arch.rustc_target_without_glibc_version();
        let profile = match self.build.profile.as_deref() {
            Some("dev" | "test") => "debug",
            Some("release" | "bench") => "release",
            Some(profile) => profile,
            None if self.build.release => "release",
            None => "debug",
        };

        // confirm that target component is included in host toolchain, or add
        // it with `rustup` otherwise.
        toolchain::check_target_component_with_rustc_meta(
            &rustc_target_without_glibc_version,
            host_target,
            release_channel,
        )
        .await?;

        let manifest_path = self
            .build
            .manifest_path
            .as_deref()
            .unwrap_or_else(|| Path::new("Cargo.toml"));
        let binaries = binary_targets(manifest_path)?;

        if !self.build.bin.is_empty() {
            for name in &self.build.bin {
                if !binaries.contains(name) {
                    return Err(miette::miette!(
                        "binary target is missing from this project: {}",
                        name
                    ));
                }
            }
        }

        if !self.build.disable_zig_linker {
            zig::check_installation().await?;
        }

        let mut cmd = self
            .build
            .build_command("build")
            .map_err(|e| miette::miette!("{}", e))?;
        if self.build.release {
            let target_cpu = target_arch.target_cpu();
            cmd.env(
                "RUSTFLAGS",
                format!("-C strip=symbols -C target-cpu={target_cpu}"),
            );
        }

        let mut child = cmd
            .spawn()
            .into_diagnostic()
            .wrap_err("Failed to run cargo build")?;
        let status = child
            .wait()
            .into_diagnostic()
            .wrap_err("Failed to wait on cargo build process")?;
        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }

        let target_dir = Path::new("target");
        let lambda_dir = if let Some(dir) = &self.lambda_dir {
            dir.clone()
        } else {
            target_dir.join("lambda")
        };

        let base = target_dir
            .join(rustc_target_without_glibc_version)
            .join(profile);

        for name in &binaries {
            let binary = base.join(name);
            if binary.exists() {
                let bootstrap_dir = if self.extension {
                    lambda_dir.join("extensions")
                } else {
                    lambda_dir.join(name)
                };
                create_dir_all(&bootstrap_dir).into_diagnostic()?;

                let bin_name = if self.extension {
                    name.as_str()
                } else {
                    "bootstrap"
                };

                match self.output_format {
                    OutputFormat::Binary => {
                        rename(binary, bootstrap_dir.join(bin_name)).into_diagnostic()?;
                    }
                    OutputFormat::Zip => {
                        let parent = if self.extension {
                            Some("extensions")
                        } else {
                            None
                        };
                        zip_binary(bin_name, binary, bootstrap_dir, parent)?;
                    }
                }
            }
        }

        Ok(())
    }
}

pub struct BinaryArchive {
    pub architecture: String,
    pub sha256: String,
    pub path: PathBuf,
}

/// Search for the bootstrap file for a function inside the target directory.
/// If the binary file exists, it creates the zip archive and extracts its architectury by reading the binary.
pub fn find_binary_archive<P: AsRef<Path>>(
    name: &str,
    base_dir: &Option<P>,
    is_extension: bool,
) -> Result<BinaryArchive> {
    let target_dir = Path::new("target");
    let (dir_name, binary_name, parent) = if is_extension {
        ("extensions", name, Some("extensions"))
    } else {
        (name, "bootstrap", None)
    };

    let bootstrap_dir = if let Some(dir) = base_dir {
        dir.as_ref().join(dir_name)
    } else {
        target_dir.join("lambda").join(dir_name)
    };

    let binary_path = bootstrap_dir.join(binary_name);
    if !binary_path.exists() {
        let build_cmd = if is_extension {
            "build --extension"
        } else {
            "build"
        };
        return Err(miette::miette!(
            "binary file for {} not found, use `cargo lambda {}` to create it",
            name,
            build_cmd
        ));
    }

    zip_binary(binary_name, binary_path, bootstrap_dir, parent)
}

/// Create a zip file from a function binary.
/// The binary inside the zip file is always called `bootstrap`.
fn zip_binary<P: AsRef<Path>>(
    name: &str,
    binary_path: P,
    destination_directory: P,
    parent: Option<&str>,
) -> Result<BinaryArchive> {
    let path = binary_path.as_ref();
    let dir = destination_directory.as_ref();
    let zipped = dir.join(format!("{}.zip", name));

    let zipped_binary = File::create(&zipped).into_diagnostic()?;
    let binary_data = read(path).into_diagnostic()?;
    let binary_data = &*binary_data;
    let object = ObjectFile::parse(binary_data).into_diagnostic()?;

    let arch = match object.architecture() {
        Architecture::Aarch64 => "arm64",
        Architecture::X86_64 => "x86_64",
        other => return Err(miette::miette!("invalid binary architecture: {:?}", other)),
    };

    let mut hasher = Sha256::new();
    hasher.update(binary_data);
    let sha256 = format!("{:X}", hasher.finalize());

    let mut zip = ZipWriter::new(zipped_binary);
    let file_name = if let Some(parent) = parent {
        zip.add_directory(parent, FileOptions::default())
            .into_diagnostic()?;
        Path::new(parent).join(name)
    } else {
        PathBuf::from(name)
    };

    zip.start_file(
        file_name.to_str().expect("failed to convert file path"),
        Default::default(),
    )
    .into_diagnostic()?;
    zip.write_all(binary_data).into_diagnostic()?;
    zip.finish().into_diagnostic()?;

    Ok(BinaryArchive {
        architecture: arch.into(),
        path: zipped,
        sha256,
    })
}
