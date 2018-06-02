```
$ docker build -t poacpm/phoenix .
```

Gitのクローンの深さ: 1
Cache: Type: Amazon S3


buildspec.ymlなどがリポジトリルートに無いため，
`Buildspec name`で指定する必要がある．
以下，文章．
```
If the buildspec file is in the root of your source directory, you can enter just the name of the file (for example, buildspec.yml). If the buildspec file is in a directory different from the root of your source directory, you must include the path (for example, test/buildspec.yml).
```

環境イメージ: AWS CodeBuild によって管理されたイメージの使用

オペレーティングシステム: Ubuntu

ランタイム: Docker

ランタイムバージョン: aws/codebuild/docker:17.09.0


Add inline policy -> JSON
codebuild-poacpm-service-roleに，ECSへのFullAccess権限を与える．
上記のpoacpm部分は，最初に指定する`project name`のこと
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

Localのkubectlのconfigをs3へアップロードする．
```bash
$ aws s3 cp ~/.kube/config s3://secret.poac.pm/.kube/config
```

set image実行後，rolling updateが行われている様子を見れる．
```bash
$ kubectl get pods
```
