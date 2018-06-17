# CodeBuild

## Configure your project
Specify settings for your build project.

* Project name: `poacpm`


## Source: What to build

* Source provider: `GitHub`
* Repository: `Use a repository in my account`
* Choose a repository: `poacpm/poacpm`
* Git clone depth: `1`
* Webhook: `true`
* Build Badge: `true`


## Environment: How to build

* Environment image: `Use an image managed by AWS CodeBuild`
* Operating system: `Ubuntu`
* Runtime: `Docker`
* Runtime version: `aws/codebuild/docker:17.09.0`
* Privileged: `already true`
* Build specification: `Use the buildspec.yml in the source code root directory`
* Buildspec name: [`.codebuild/buildspec.yml`](buildspec.yml)
* Certificate: `Do not install any certificate`


## Artifacts: Where to put the artifacts from this build project

* Type: `No artifacts`


## Cache

* Type: `Amazon S3`
* Bucket: `secret.poac.pm`
* Path prefix: `.codebuild/`
* Lifecycle: `false`


## Service role

default


## VPC

* VPC: `No VPC`


# IAM
## Roles
### codebuild-poacpm-service-role
```json:AmazonECR_FullAccess
{
    "Version": "2012-10-17",
    "Statement": [
        {
            "Sid": "VisualEditor0",
            "Effect": "Allow",
            "Action": [
                "ecr:CreateRepository",
                "ecr:GetAuthorizationToken"
            ],
            "Resource": "*"
        },
        {
            "Sid": "VisualEditor1",
            "Effect": "Allow",
            "Action": "ecr:*",
            "Resource": "arn:aws:ecr:*:*:repository/*"
        }
    ]
}
```
```json:AmazonS3_ReadOnly
{
    "Version": "2012-10-17",
    "Statement": [
        {
            "Sid": "VisualEditor0",
            "Effect": "Allow",
            "Action": [
                "s3:GetObjectVersionTorrent",
                "s3:GetObjectAcl",
                "s3:GetObject",
                "s3:GetObjectTorrent",
                "s3:GetObjectVersionTagging",
                "s3:GetObjectVersionAcl",
                "s3:GetObjectTagging",
                "s3:GetObjectVersionForReplication",
                "s3:GetObjectVersion",
                "s3:ListMultipartUploadParts"
            ],
            "Resource": "arn:aws:s3:::*/*"
        },
        {
            "Sid": "VisualEditor1",
            "Effect": "Allow",
            "Action": [
                "s3:ListBucketByTags",
                "s3:GetLifecycleConfiguration",
                "s3:ListBucketMultipartUploads",
                "s3:GetBucketTagging",
                "s3:GetInventoryConfiguration",
                "s3:GetBucketWebsite",
                "s3:ListBucketVersions",
                "s3:GetBucketLogging",
                "s3:GetAccelerateConfiguration",
                "s3:GetBucketVersioning",
                "s3:GetBucketAcl",
                "s3:GetBucketNotification",
                "s3:GetBucketPolicy",
                "s3:GetReplicationConfiguration",
                "s3:GetBucketRequestPayment",
                "s3:GetBucketCORS",
                "s3:GetAnalyticsConfiguration",
                "s3:GetMetricsConfiguration",
                "s3:GetBucketLocation",
                "s3:GetIpConfiguration"
            ],
            "Resource": "arn:aws:s3:::*"
        }
    ]
}
```

## References
https://docs.aws.amazon.com/codebuild/latest/userguide/build-env-ref-env-vars.html

