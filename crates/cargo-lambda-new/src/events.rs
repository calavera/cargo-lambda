pub(crate) const WELL_KNOWN_EVENTS: [&str; 50] = [
    "activemq::ActiveMqEvent",
    "autoscaling::AutoScalingEvent",
    "bedrock_agent_runtime::AgentEvent",
    "chime_bot::ChimeBotEvent",
    "cloudformation::CloudFormationCustomResourceRequest",
    "cloudformation::CloudFormationCustomResourceResponse",
    "cloudformation::provider::CloudFormationCustomResourceRequest",
    "cloudformation::provider::CloudFormationCustomResourceResponse",
    "cloudwatch_alarms::CloudWatchAlarm",
    "cloudwatch_events::CloudWatchEvent",
    "cloudwatch_logs::CloudwatchLogsEvent",
    "cloudwatch_logs::CloudwatchLogsLogEvent",
    "codebuild::CodeBuildEvent",
    "code_commit::CodeCommitEvent",
    "codedeploy::CodeDeployDeploymentEvent",
    "codedeploy::CodeDeployEvent",
    "codedeploy::CodeDeployInstanceEvent",
    "codedeploy::CodeDeployLifecycleEvent",
    "codepipeline_cloudwatch::CodePipelineCloudWatchEvent",
    "codepipeline_cloudwatch::CodePipelineDeploymentEvent",
    "codepipeline_cloudwatch::CodePipelineEvent",
    "codepipeline_cloudwatch::CodePipelineInstanceEvent",
    "codepipeline_job::CodePipelineJobEvent",
    "cognito::CognitoEvent",
    "cognito::CognitoEventUserPoolsPreTokenGenV2",
    "config::ConfigEvent",
    "connect::ConnectEvent",
    "documentdb::DocumentDbEvent",
    "dynamodb::Event",
    "ecr_scan::EcrScanEvent",
    "eventbridge::EventBridgeEvent",
    "firehose::KinesisFirehoseEvent",
    "iot_1_click::IoTOneClickDeviceEvent",
    "iot_1_click::IoTOneClickEvent",
    "iot_button::IoTButtonEvent",
    "kafka::KafkaEvent",
    "kinesis_analytics::KinesisAnalyticsOutputDeliveryEvent",
    "kinesis::KinesisEvent",
    "lex::LexEvent",
    "rabbitmq::RabbitMqEvent",
    "s3_batch_job::S3BatchJobEvent",
    "s3::S3Event",
    "secretsmanager::SecretsManagerSecretRotationEvent",
    "serde_json::Value", // this type is a special case not included in the events crate
    "ses::SimpleEmailEvent",
    "sns::CloudWatchAlarmPayload",
    "sns::SnsEvent",
    "sqs::SqsEvent",
    "sqs::SqsApiEvent",
    "sqs::SqsApiEventObj",
];
