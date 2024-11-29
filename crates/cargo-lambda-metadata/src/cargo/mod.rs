pub use cargo_metadata::{
    Metadata as CargoMetadata, Package as CargoPackage, Target as CargoTarget,
};
use miette::Result;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    fs::{metadata, read_to_string},
    path::{Path, PathBuf},
};
use tracing::{debug, enabled, trace, Level};

use crate::error::MetadataError;

mod build;
pub use build::*;

mod deploy;
pub use deploy::*;

mod profile;
pub use profile::*;

mod watch;
pub use watch::*;

const STRIP_CONFIG: &str = "profile.release.strip=\"symbols\"";
const LTO_CONFIG: &str = "profile.release.lto=\"thin\"";
const CODEGEN_CONFIG: &str = "profile.release.codegen-units=1";
const PANIC_CONFIG: &str = "profile.release.panic=\"abort\"";

#[derive(Debug, Default, Deserialize)]
#[non_exhaustive]
pub struct Metadata {
    #[serde(default)]
    pub lambda: LambdaMetadata,
    #[serde(default)]
    profile: Option<CargoProfile>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[non_exhaustive]
pub struct LambdaMetadata {
    #[serde(flatten)]
    pub package: PackageMetadata,
    #[serde(default)]
    pub bin: HashMap<String, PackageMetadata>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[non_exhaustive]
pub struct PackageMetadata {
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub deploy: Option<DeployConfig>,
    #[serde(default)]
    pub build: BuildConfig,
    #[serde(default)]
    pub watch: Option<WatchConfig>,
}

/// Extract all the binary target names from a Cargo.toml file
pub fn binary_targets<P: AsRef<Path> + Debug>(
    manifest_path: P,
    build_examples: bool,
) -> Result<HashSet<String>, MetadataError> {
    let metadata = load_metadata(manifest_path)?;
    Ok(binary_targets_from_metadata(&metadata, build_examples))
}

pub fn binary_targets_from_metadata(
    metadata: &CargoMetadata,
    build_examples: bool,
) -> HashSet<String> {
    let condition = if build_examples {
        kind_example_filter
    } else {
        kind_bin_filter
    };

    let package_filter: Option<fn(&&CargoPackage) -> bool> = None;
    filter_binary_targets_from_metadata(metadata, condition, package_filter)
}

pub fn kind_bin_filter(target: &CargoTarget) -> bool {
    target.kind.iter().any(|k| k == "bin")
}

// Several targets can have `crate_type` be `bin`, we're only
// interested in the ones which `kind` is `bin` or `example`.
// See https://doc.rust-lang.org/cargo/commands/cargo-metadata.html?highlight=targets%20metadata#json-format
pub fn kind_example_filter(target: &CargoTarget) -> bool {
    target.kind.iter().any(|k| k == "example") && target.crate_types.iter().any(|t| t == "bin")
}

/// Extract all the binary target names from a Cargo.toml file
pub fn filter_binary_targets<P, F, K>(
    manifest_path: P,
    target_filter: F,
    package_filter: Option<K>,
) -> Result<HashSet<String>, MetadataError>
where
    P: AsRef<Path> + Debug,
    F: FnMut(&CargoTarget) -> bool,
    K: FnMut(&&CargoPackage) -> bool,
{
    let metadata = load_metadata(manifest_path)?;
    Ok(filter_binary_targets_from_metadata(
        &metadata,
        target_filter,
        package_filter,
    ))
}

pub fn filter_binary_targets_from_metadata<F, P>(
    metadata: &CargoMetadata,
    target_filter: F,
    package_filter: Option<P>,
) -> HashSet<String>
where
    F: FnMut(&CargoTarget) -> bool,
    P: FnMut(&&CargoPackage) -> bool,
{
    let packages = metadata.packages.iter();
    let targets = if let Some(filter) = package_filter {
        packages
            .filter(filter)
            .flat_map(|p| p.targets.clone())
            .collect::<Vec<_>>()
    } else {
        packages.flat_map(|p| p.targets.clone()).collect::<Vec<_>>()
    };

    targets
        .into_iter()
        .filter(target_filter)
        .map(|target| target.name.clone())
        .collect::<_>()
}

/// Extract target directory information
///
/// This fetches the target directory from `cargo metadata`, resolving the
/// user and project configuration and the environment variables in the right
/// way.
pub fn target_dir<P: AsRef<Path> + Debug>(manifest_path: P) -> Result<PathBuf> {
    let metadata = load_metadata(manifest_path)?;
    Ok(metadata.target_directory.into_std_path_buf())
}

pub fn target_dir_from_metadata(metadata: &CargoMetadata) -> Result<PathBuf> {
    Ok(metadata.target_directory.clone().into_std_path_buf())
}

/// Attempt to read the release profile section in the Cargo manifest.
/// Cargo metadata doesn't expose profile information, so we try
/// to read it from the Cargo.toml file directly.
pub fn cargo_release_profile_config<'a, P: AsRef<Path> + Debug>(
    manifest_path: P,
) -> Result<HashSet<&'a str>, MetadataError> {
    let path = manifest_path.as_ref();
    let file = read_to_string(path)
        .map_err(|e| MetadataError::InvalidManifestFile(path.to_path_buf(), e))?;

    let metadata: Metadata = toml::from_str(&file).map_err(MetadataError::InvalidTomlManifest)?;

    Ok(cargo_release_profile_config_from_metadata(metadata))
}

fn cargo_release_profile_config_from_metadata(metadata: Metadata) -> HashSet<&'static str> {
    let mut config = HashSet::from([STRIP_CONFIG, LTO_CONFIG, CODEGEN_CONFIG, PANIC_CONFIG]);

    let Some(profile) = &metadata.profile else {
        return config;
    };
    let Some(release) = &profile.release else {
        return config;
    };

    if release.strip.is_some() || release.debug_enabled() {
        config.remove(STRIP_CONFIG);
    }
    if release.lto.is_some() {
        config.remove(LTO_CONFIG);
    }
    if release.codegen_units.is_some() {
        config.remove(CODEGEN_CONFIG);
    }
    if release.panic.is_some() {
        config.remove(PANIC_CONFIG);
    }

    config
}

/// Create metadata about the root package in the Cargo manifest, without any dependencies.
#[tracing::instrument(target = "cargo_lambda")]
pub fn load_metadata<P: AsRef<Path> + Debug>(
    manifest_path: P,
) -> Result<CargoMetadata, MetadataError> {
    trace!("loading Cargo metadata");
    let mut metadata_cmd = cargo_metadata::MetadataCommand::new();
    metadata_cmd
        .no_deps()
        .verbose(enabled!(target: "cargo_lambda", Level::TRACE));

    // try to split manifest path and assign current_dir to enable parsing a project-specific
    // cargo config
    let manifest_ref = manifest_path.as_ref();

    match (manifest_ref.parent(), manifest_ref.file_name()) {
        (Some(project), Some(manifest)) if is_project_metadata_ok(project) => {
            metadata_cmd.current_dir(project);
            metadata_cmd.manifest_path(manifest);
        }
        _ => {
            // fall back to using the manifest_path without changing the dir
            // this means there will not be any project-specific config parsing
            metadata_cmd.manifest_path(manifest_ref);
        }
    }

    trace!(metadata = ?metadata_cmd, "loading cargo metadata");
    let meta = metadata_cmd
        .exec()
        .map_err(MetadataError::FailedCmdExecution)?;
    trace!(metadata = ?meta, "loaded cargo metadata");
    Ok(meta)
}

/// Create a HashMap of environment varibales from the package and workspace manifest
/// See the documentation to learn about how we use this metadata:
/// https://www.cargo-lambda.info/commands/watch.html#environment-variables
#[tracing::instrument(target = "cargo_lambda")]
pub fn function_environment_metadata<P: AsRef<Path> + Debug>(
    manifest_path: P,
    name: Option<&str>,
) -> Result<HashMap<String, String>> {
    let metadata = load_metadata(manifest_path)?;
    let ws_metadata: LambdaMetadata =
        serde_json::from_value(metadata.workspace_metadata).unwrap_or_default();

    let mut env = HashMap::new();
    env.extend(ws_metadata.package.env);

    if let Some(name) = name {
        if let Some(res) = ws_metadata.bin.get(name) {
            env.extend(res.env.clone());
        }
    }

    for pkg in &metadata.packages {
        let name = name.unwrap_or(&pkg.name);

        for target in &pkg.targets {
            let target_matches = target.name == name
                && target.kind.iter().any(|kind| kind == "bin")
                && pkg.metadata.is_object();

            debug!(
                name = name,
                target_name = ?target.name,
                target_kind = ?target.kind,
                metadata_object = pkg.metadata.is_object(),
                target_matches = target_matches,
                "searching package metadata"
            );

            if target_matches {
                let package_metadata: Metadata = serde_json::from_value(pkg.metadata.clone())
                    .map_err(MetadataError::InvalidCargoMetadata)?;

                env.extend(package_metadata.lambda.package.env);
                if let Some(res) = package_metadata.lambda.bin.get(name) {
                    env.extend(res.env.clone());
                }
            }
        }
    }

    debug!(env = ?env, "using environment variables from metadata");
    Ok(env)
}

/// Load the main binary in the project.
/// It returns an error if the project includes from than one binary.
/// Use this function when the user didn't provide any funcion name
/// assuming that there is only one binary in the project
pub fn main_binary<P: AsRef<Path> + Debug>(manifest_path: P) -> Result<String, MetadataError> {
    let metadata = load_metadata(manifest_path)?;
    main_binary_from_metadata(&metadata)
}

pub fn main_binary_from_metadata(metadata: &CargoMetadata) -> Result<String, MetadataError> {
    let targets = binary_targets_from_metadata(metadata, false);
    if targets.len() > 1 {
        let mut vec = targets.into_iter().collect::<Vec<_>>();
        vec.sort();
        Err(MetadataError::MultipleBinariesInProject(vec.join(", ")))
    } else if targets.is_empty() {
        Err(MetadataError::MissingBinaryInProject)
    } else {
        targets
            .into_iter()
            .next()
            .ok_or(MetadataError::MissingBinaryInProject)
    }
}

fn is_project_metadata_ok(path: &Path) -> bool {
    path.is_dir() && metadata(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    pub fn fixture(name: &str) -> PathBuf {
        format!("../../tests/fixtures/{name}/Cargo.toml").into()
    }

    #[test]
    fn test_binary_packages() {
        let bins = binary_targets(fixture("single-binary-package"), false).unwrap();
        assert_eq!(1, bins.len());
        assert!(bins.contains("basic-lambda"));
    }

    #[test]
    fn test_binary_packages_with_mutiple_bin_entries() {
        let bins = binary_targets(fixture("multi-binary-package"), false).unwrap();
        assert_eq!(5, bins.len());
        assert!(bins.contains("delete-product"));
        assert!(bins.contains("get-product"));
        assert!(bins.contains("get-products"));
        assert!(bins.contains("put-product"));
        assert!(bins.contains("dynamodb-streams"));
    }

    #[test]
    fn test_binary_packages_with_workspace() {
        let bins = binary_targets(fixture("workspace-package"), false).unwrap();
        assert_eq!(3, bins.len());
        assert!(bins.contains("basic-lambda-1"));
        assert!(bins.contains("basic-lambda-2"));
        assert!(bins.contains("crate-3"));
    }

    #[test]
    fn test_binary_packages_with_mixed_workspace() {
        let bins = binary_targets(fixture("mixed-workspace-package"), false).unwrap();
        assert_eq!(1, bins.len());
        assert!(bins.contains("function-crate"), "{:?}", bins);
    }

    #[test]
    fn test_binary_packages_with_missing_binary_info() {
        let err = binary_targets(fixture("missing-binary-package"), false).unwrap_err();
        assert!(err
            .to_string()
            .contains("a [lib] section, or [[bin]] section must be present"));
    }

    #[test]
    fn test_metadata_packages() {
        let env =
            function_environment_metadata(fixture("single-binary-package"), Some("basic-lambda"))
                .unwrap();

        assert_eq!(env.get("FOO").unwrap(), "BAR");
    }

    #[test]
    fn test_metadata_multi_packages() {
        let env =
            function_environment_metadata(fixture("multi-binary-package"), Some("get-product"))
                .unwrap();

        assert_eq!(env.get("FOO").unwrap(), "BAR");

        let env =
            function_environment_metadata(fixture("multi-binary-package"), Some("delete-product"))
                .unwrap();

        assert_eq!(env.get("BAZ").unwrap(), "QUX");
    }

    #[test]
    fn test_invalid_metadata() {
        let result =
            function_environment_metadata(fixture("missing-binary-package"), Some("get-products"));
        assert!(result.is_err());
    }

    #[test]
    fn test_metadata_workspace_packages() {
        let env =
            function_environment_metadata(fixture("workspace-package"), Some("basic-lambda-1"))
                .unwrap();

        assert_eq!(env.get("FOO").unwrap(), "BAR");

        let env =
            function_environment_metadata(fixture("workspace-package"), Some("basic-lambda-2"))
                .unwrap();

        assert_eq!(env.get("FOO").unwrap(), "BAR");
    }

    #[test]
    fn test_metadata_packages_without_name() {
        let env = function_environment_metadata(fixture("single-binary-package"), None).unwrap();

        assert_eq!(env.get("FOO").unwrap(), "BAR");
    }

    #[test]
    #[ignore = "changing the environment is not reliable"]
    fn test_target_dir_non_set() {
        std::env::remove_var("CARGO_TARGET_DIR");
        let target_dir = target_dir(fixture("single-binary-package")).unwrap();
        assert!(
            target_dir.ends_with("tests/fixtures/single-binary-package/target"),
            "unexpected directory {:?}",
            target_dir
        );
    }

    #[test]
    #[ignore = "changing the environment is not reliable"]
    fn test_target_dir_from_project_config() {
        std::env::remove_var("CARGO_TARGET_DIR");
        let target_dir = target_dir(fixture("target-dir-set-in-project")).unwrap();
        assert!(
            target_dir.ends_with("project_specific_target"),
            "unexpected directory {:?}",
            target_dir
        );
    }

    #[test]
    #[ignore = "changing the environment is not reliable"]
    fn test_target_dir_from_env() {
        std::env::set_var("CARGO_TARGET_DIR", "/tmp/exotic_path");
        let target_dir = target_dir(fixture("single-binary-package")).unwrap();
        assert!(
            target_dir.ends_with("/tmp/exotic_path"),
            "unexpected directory {:?}",
            target_dir
        );
    }

    #[test]
    fn test_main_binary_with_package_name() {
        let manifest_path = fixture("single-binary-package");
        let name = main_binary(manifest_path).unwrap();
        assert_eq!("basic-lambda", name);
    }

    #[test]
    fn test_main_binary_with_binary_name() {
        let manifest_path = fixture("single-binary-different-name");
        let name = main_binary(manifest_path).unwrap();
        assert_eq!("basic-lambda-binary", name);
    }

    #[test]
    fn test_main_binary_multi_binaries() {
        let manifest_path = fixture("multi-binary-package");
        let err = main_binary(manifest_path).unwrap_err();
        assert_eq!(
            "there are more than one binary in the project, please specify a binary name with --binary-name or --binary-path. This is the list of binaries I found: delete-product, dynamodb-streams, get-product, get-products, put-product",
            err.to_string()
        );
    }

    #[test]
    fn test_example_packages() {
        let bins = binary_targets(fixture("examples-package"), true).unwrap();
        assert_eq!(1, bins.len());
        assert!(bins.contains("example-lambda"));
    }

    #[test]
    fn test_release_config() {
        let config = cargo_release_profile_config_from_metadata(Metadata::default());
        assert!(config.contains(STRIP_CONFIG));
        assert!(config.contains(LTO_CONFIG));
        assert!(config.contains(CODEGEN_CONFIG));
        assert!(config.contains(PANIC_CONFIG));
    }
}
