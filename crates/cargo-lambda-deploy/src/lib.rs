use aws_smithy_types::retry::{RetryConfig, RetryMode};
use cargo_lambda_build::{create_binary_archive, zip_binary, BinaryArchive, BinaryData};
use cargo_lambda_interactive::progress::Progress;
use cargo_lambda_metadata::cargo::{function_deploy_metadata, main_binary, DeployConfig};
use cargo_lambda_remote::{
    aws_sdk_lambda::types::{Architecture, Runtime},
    RemoteConfig,
};
use clap::{Args, ValueHint};
use functions::load_deploy_environment;
use miette::{IntoDiagnostic, Result, WrapErr};
use serde::Serialize;
use serde_json::ser::to_string_pretty;
use std::{path::PathBuf, time::Duration};
use strum_macros::{Display, EnumString};

mod extensions;
mod functions;
mod roles;

#[derive(Clone, Debug, Display, EnumString)]
#[strum(ascii_case_insensitive)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Serialize)]
struct DryOutput {
    kind: String,
    name: String,
    path: PathBuf,
    arch: String,
    runtimes: Vec<String>,
    tags: Option<String>,
    bucket: Option<String>,
    include: Option<Vec<PathBuf>>,
}

impl std::fmt::Display for DryOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "🔍 deployment for {} `{}`:", self.kind, self.name)?;
        writeln!(f, "🏠 binary located at {}", self.path.display())?;
        writeln!(f, "🔗 architecture {}", self.arch)?;

        if let Some(tags) = &self.tags {
            writeln!(f, "🏷️ tagged with {}", tags.replace(',', ", "))?;
        }

        if let Some(bucket) = &self.bucket {
            writeln!(f, "🪣 stored on S3 bucket `{}`", bucket)?;
        }

        if let Some(paths) = &self.include {
            writeln!(f, "🗃️ extra files included:")?;
            for file in paths {
                writeln!(f, "- {}", file.display())?;
            }
        }

        write!(f, "👟 running on {}", self.runtimes.join(", "))?;
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum DeployResult {
    Extension(extensions::DeployOutput),
    Function(functions::DeployOutput),
    Dry(DryOutput),
}

impl std::fmt::Display for DeployResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeployResult::Extension(o) => o.fmt(f),
            DeployResult::Function(o) => o.fmt(f),
            DeployResult::Dry(o) => o.fmt(f),
        }
    }
}

#[derive(Args, Clone, Debug)]
#[command(
    name = "deploy",
    after_help = "Full command documentation: https://www.cargo-lambda.info/commands/deploy.html"
)]
pub struct Deploy {
    #[command(flatten)]
    remote_config: RemoteConfig,

    #[command(flatten)]
    function_config: functions::FunctionDeployConfig,

    /// Directory where the lambda binaries are located
    #[arg(short, long, value_hint = ValueHint::DirPath)]
    lambda_dir: Option<PathBuf>,

    /// Path to Cargo.toml
    #[arg(long, value_name = "PATH", default_value = "Cargo.toml")]
    pub manifest_path: PathBuf,

    /// Name of the binary to deploy if it doesn't match the name that you want to deploy it with
    #[arg(long, conflicts_with = "binary_path")]
    pub binary_name: Option<String>,

    /// Local path of the binary to deploy if it doesn't match the target path generated by cargo-lambda-build
    #[arg(long, conflicts_with = "binary_name")]
    pub binary_path: Option<PathBuf>,

    /// S3 bucket to upload the code to
    #[arg(long)]
    pub s3_bucket: Option<String>,

    /// Whether the code that you're deploying is a Lambda Extension
    #[arg(long)]
    extension: bool,

    /// Whether an extension is internal or external
    #[arg(long, requires = "extension")]
    internal: bool,

    /// Comma separated list with compatible runtimes for the Lambda Extension (--compatible_runtimes=provided.al2,nodejs16.x)
    /// List of allowed runtimes can be found in the AWS documentation: https://docs.aws.amazon.com/lambda/latest/dg/API_CreateFunction.html#SSS-CreateFunction-request-Runtime
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "provided.al2,provided.al2023",
        requires = "extension"
    )]
    compatible_runtimes: Vec<String>,

    /// Format to render the output (text, or json)
    #[arg(short, long, default_value_t = OutputFormat::Text)]
    output_format: OutputFormat,

    /// Option to add one or many tags, allows multiple repetitions (--tag organization=aws --tag team=lambda)
    /// This option overrides any values set with the --tags flag.
    #[arg(long, conflicts_with = "tags")]
    tag: Option<Vec<String>>,

    /// Comma separated list of tags to apply to the function or extension (--tags organization=aws,team=lambda)
    /// This option overrides any values set with the --tag flag.
    #[arg(long, value_delimiter = ',', conflicts_with = "tag")]
    tags: Option<Vec<String>>,

    /// Option to add one or more files and directories to include in the zip file to upload.
    #[arg(short, long)]
    include: Option<Vec<PathBuf>>,

    /// Perform all the operations to locate and package the binary to deploy, but don't do the final deploy.
    #[arg(long, alias = "dry-run")]
    dry: bool,

    /// Name of the function or extension to deploy
    #[arg(value_name = "NAME")]
    name: Option<String>,
}

impl Deploy {
    #[tracing::instrument(skip(self), target = "cargo_lambda")]
    pub async fn run(&self) -> Result<()> {
        tracing::trace!(options = ?self, "deploying project");

        if self.function_config.enable_function_url && self.function_config.disable_function_url {
            return Err(miette::miette!("invalid options: --enable-function-url and --disable-function-url cannot be set together"));
        }

        let progress = Progress::start("loading binary data");
        let (name, archive) = match self.load_archive() {
            Ok(arc) => arc,
            Err(err) => {
                progress.finish_and_clear();
                return Err(err);
            }
        };

        let retry = RetryConfig::standard()
            .with_retry_mode(RetryMode::Adaptive)
            .with_max_attempts(3)
            .with_initial_backoff(Duration::from_secs(5));

        let sdk_config = self.remote_config.sdk_config(Some(retry)).await;
        let architecture = Architecture::from(archive.architecture.as_str());
        let compatible_runtimes = self
            .compatible_runtimes
            .iter()
            .map(|runtime| Runtime::from(runtime.as_str()))
            .collect::<Vec<_>>();

        let mut tags = self.tags.clone();
        if tags.is_none() {
            tags.clone_from(&self.tag);
        }

        let result = if self.dry {
            self.dry_output(&name, &archive, &tags)
        } else if self.extension {
            extensions::deploy(
                &name,
                &self.manifest_path,
                &sdk_config,
                &archive,
                architecture,
                compatible_runtimes,
                &self.s3_bucket,
                &tags,
                &progress,
            )
            .await
        } else {
            let binary_name = self.binary_name_or_default(&name);
            functions::deploy(
                &name,
                &binary_name,
                &self.manifest_path,
                &self.function_config,
                &self.remote_config,
                &sdk_config,
                &self.s3_bucket,
                &tags,
                &archive,
                architecture,
                &progress,
            )
            .await
        };

        progress.finish_and_clear();
        let output = result?;

        match &self.output_format {
            OutputFormat::Text => println!("{output}"),
            OutputFormat::Json => {
                let text = to_string_pretty(&output)
                    .into_diagnostic()
                    .wrap_err("failed to serialize output into json")?;
                println!("{text}")
            }
        }

        Ok(())
    }

    fn load_archive(&self) -> Result<(String, BinaryArchive)> {
        match &self.binary_path {
            Some(bp) if bp.is_dir() => Err(miette::miette!("invalid file {:?}", bp)),
            Some(bp) => {
                let name = match &self.name {
                    Some(name) => name.clone(),
                    None => bp
                        .file_name()
                        .and_then(|s| s.to_str())
                        .map(String::from)
                        .ok_or_else(|| miette::miette!("invalid binary path {:?}", bp))?,
                };

                let destination = bp
                    .parent()
                    .ok_or_else(|| miette::miette!("invalid binary path {:?}", bp))?;

                let data = BinaryData::new(&name, self.extension, self.internal);
                let arc = zip_binary(bp, destination, &data, self.include.clone())?;
                Ok((name, arc))
            }
            None => {
                let name = match (&self.name, &self.binary_name) {
                    (Some(name), _) => name.clone(),
                    (None, Some(bn)) => bn.clone(),
                    (None, None) => main_binary(&self.manifest_path).into_diagnostic()?,
                };
                let binary_name = self.binary_name_or_default(&name);
                let data = BinaryData::new(&binary_name, self.extension, self.internal);

                let arc = create_binary_archive(
                    &self.manifest_path,
                    &self.lambda_dir,
                    &data,
                    self.include.clone(),
                )?;
                Ok((name, arc))
            }
        }
    }

    fn dry_output(
        &self,
        name: &str,
        archive: &BinaryArchive,
        tags: &Option<Vec<String>>,
    ) -> Result<DeployResult> {
        let (kind, name, runtimes, meta) = if self.extension {
            let deploy_metadata = function_deploy_metadata(
                &self.manifest_path,
                name,
                tags,
                &self.s3_bucket,
                DeployConfig::default(),
            )
            .into_diagnostic()?;
            (
                "extension",
                name.to_owned(),
                self.compatible_runtimes.clone(),
                deploy_metadata,
            )
        } else {
            let binary_name = self.binary_name_or_default(name);
            let (_, deploy_metadata) = load_deploy_environment(
                &self.manifest_path,
                &binary_name,
                &self.function_config,
                tags,
                &self.s3_bucket,
            )?;
            (
                "function",
                binary_name,
                vec![self.function_config.runtime.clone()],
                deploy_metadata,
            )
        };

        Ok(DeployResult::Dry(DryOutput {
            kind: kind.to_string(),
            path: archive.path.clone(),
            arch: archive.architecture.clone(),
            bucket: meta.s3_bucket.clone(),
            tags: meta.s3_tags(),
            include: meta.include.clone(),
            name,
            runtimes,
        }))
    }

    fn binary_name_or_default(&self, name: &str) -> String {
        self.binary_name.clone().unwrap_or_else(|| name.to_string())
    }
}
