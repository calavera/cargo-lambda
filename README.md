# cargo-lambda

[![crates.io][crate-image]][crate-link]
[![Build Status][build-image]][build-link]

cargo-lambda is a [Cargo](https://doc.rust-lang.org/cargo/) subcommand to help you work with AWS Lambda.

The [build](#build) subcommand compiles AWS Lambda functions natively and produces artifacts which you can then upload to AWS Lambda or use with other echosystem tools, like [SAM Cli](https://github.com/aws/aws-sam-cli) or the [AWS CDK](https://github.com/aws/aws-cdk).

The [start](#start) subcommand boots a development server that emulates interations with the AWS Lambda control plane. This subcommand also reloads your Rust code as you work on it.

The [invoke](#invoke) subcommand sends request to the control plane emulator to test and debug interactions with your Lambda functions.

## Installation

Install this subcommand on your host machine with Cargo itself:

```
cargo install cargo-lambda
```

## Usage

### Build

Within a Rust project that includes a `Cargo.toml` file, run the `cargo lambda build` command to natively compile your Lambda functions in the project.
The resulting artifacts such as binaries or zips, will be placed in the `target/lambda` directory.
This is an example of the output produced by this command:

```
❯ tree target/lambda
target/lambda
├── delete-product
│   └── bootstrap
├── dynamodb-streams
│   └── bootstrap
├── get-product
│   └── bootstrap
├── get-products
│   └── bootstrap
└── put-product
    └── bootstrap

5 directories, 5 files
```

#### Build - Output Format

By default, cargo-lambda produces a binary artifact for each Lambda functions in the project.
However, you can configure cargo-lambda to produce a ready to upload zip artifact.

The `--output-format` paramters controls the output format, the two current options are `zip` and `binary` with `binary` being the default.

Example usage to create a zip.

```
cargo lambda build --output-format zip
```

#### Build - Architectures

By default, cargo-lambda compiles the code for Linux X86-64 architectures, you can compile for Linux ARM architectures by providing the right target:

```
cargo lambda build --target aarch64-unknown-linux-gnu
```

#### Build - Compilation Profiles

By default, cargo-lambda compiles the code in `debug` mode. If you want to change the profile to compile in `release` mode, you can provide the right flag.

```
cargo lambda build --release
```

When you compile your code in release mode, cargo-lambda will strip the binaries from all debug symbols to reduce the binary size.

#### Build - How does it work?

cargo-lambda uses [Zig](https://ziglang.org) and [cargo-zigbuild](https://crates.io/crates/cargo-zigbuild)
to compile the code for the right architecture. If Zig is not installed in your host machine, the first time that your run cargo-lambda, it will guide you through some installation options. If you run cargo-lambda in a non-interactive shell, the build process will fail until you install that dependency.

### Start

The start subcommand emulates the AWS Lambda control plane API. Run this command at the root of a Rust workspace and cargo-lambda will use cargo-watch to hot compile changes in your Lambda functions.

:warning: This command works best with the **[Lambda Runtime version 0.5.1](https://crates.io/crates/lambda_runtime/0.5.1)**. Previous versions of the rumtime are likely to crash with serialization errors.

```
cargo lambda start
```

The function is not compiled until the first time that you try to execute. See the [invoke](#invoke) command to learn how to execute a function. Cargo will run the command `cargo run --bin FUNCTION_NAME` to try to compile the function. `FUNCTION_NAME` can be either the name of the package if the package has only one binary, or the binary name in the `[[bin]]` section if the package includes more than one binary.

#### Start - Environment variables

If you need to set environment variables for your function to run, you can specify them in the metadata section of your Cargo.toml file.

Use the section `package.metadata.lambda.env` to set global variables that will applied to all functions in your package:

```toml
[package]
name = "basic-lambda"

[package.metadata.lambda.env]
RUST_LOG = "debug"
MY_CUSTOM_ENV_VARIABLE = "custom value"
```

If you have more than one function in the same package, and you want to set specific variables for each one of them, you can use a section named after each one of the binaries in your package, `package.metadata.lambda.bin.BINARY_NAME`:

```toml
[package]
name = "lambda-project"

[package.metadata.lambda.env]
RUST_LOG = "debug"

[[package.metadata.lambda.bin.get-product]]
GET_PRODUCT_ENV_VARIABLE = "custom value"

[[package.metadata.lambda.bin.add-product]]
ADD_PRODUCT_ENV_VARIABLE = "custom value"

[[bin]]
name = "get-product"
path = "src/bin/get-product.rs"

[[bin]]
name = "add-product"
path = "src/bin/add-product.rs"
```

### Invoke

The invoke subcomand helps you send requests to the control plane emulator. To use this subcommand, you have to provide the name of the Lambda function that you want to invoke, and the payload that you want to send. When the control plane emulator receives the request, it will compile your Lambda function and handle your request.

#### Invoke - Ascii data

The `--data-ascii` flag allows you to send a payload directly from the command line:

```
cargo lambda invoke basic-lambda --data-ascii '{"command": "hi"}'
```

#### Invoke - File data

The `--data-file` flag allows you to read the payload from a file in your file system:

```
cargo lambda invoke basic-lambda --data-file examples/my-payload.json
```

#### Invoke - Example data

The `--data-example` flag allows you to fetch an example payload from the [aws-lambda-events repository](https://github.com/LegNeato/aws-lambda-events/), and use it as your request payload. For example, if you want to use the [example-apigw-request.json](https://github.com/LegNeato/aws-lambda-events/blob/master/aws_lambda_events/src/generated/fixtures/example-apigw-request.json) payload, you have to pass the name `apigw-request` into this flag:

```
cargo lambda invoke http-lambda --data-example apigw-request
```

After the first download, these examples are cached in your home directory, under `.cargo/lambda/invoke-fixtures`.

[//]: # (badges)

[crate-image]: https://img.shields.io/crates/v/cargo-lambda.svg
[crate-link]: https://crates.io/crates/cargo-lambda
[build-image]: https://github.com/calavera/cargo-lambda/workflows/Build/badge.svg
[build-link]: https://github.com/calavera/cargo-lambda/actions?query=workflow%3ACI+branch%3Amain
