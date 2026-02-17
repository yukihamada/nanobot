"""
Lambda上でGitHub → 編集 → コンパイル → デプロイの全パイプラインを実行

このLambda関数自体がコンパイラとして動作し、
他のLambda関数を生成してデプロイします。
"""

import json
import os
import subprocess
import tempfile
import boto3
import shutil
from pathlib import Path

lambda_client = boto3.client('lambda')
iam_client = boto3.client('iam')

def lambda_handler(event, context):
    """
    イベント形式:
    {
        "github_url": "https://github.com/user/repo",
        "edit_instruction": "main関数を修正して...",
        "function_name": "deployed-function",
        "use_ai_edit": true
    }
    """

    github_url = event.get('github_url')
    edit_instruction = event.get('edit_instruction', '')
    function_name = event.get('function_name', 'auto-deployed-function')
    use_ai_edit = event.get('use_ai_edit', True)

    if not github_url:
        return {
            'statusCode': 400,
            'body': json.dumps({'error': 'github_url is required'})
        }

    work_dir = tempfile.mkdtemp()

    try:
        # Step 1: GitHubからクローン
        print(f"Cloning {github_url}...")
        repo_dir = os.path.join(work_dir, 'repo')
        subprocess.run(
            ['git', 'clone', '--depth', '1', github_url, repo_dir],
            check=True,
            capture_output=True
        )

        # Cargo.tomlを確認
        cargo_toml = os.path.join(repo_dir, 'Cargo.toml')
        if not os.path.exists(cargo_toml):
            return {
                'statusCode': 400,
                'body': json.dumps({'error': 'Not a Rust project (Cargo.toml not found)'})
            }

        # Step 2: AI編集（オプション）
        if use_ai_edit and edit_instruction:
            print(f"Requesting AI edit: {edit_instruction}")
            # nanobot APIを呼び出してコード編集
            # TODO: 実装
            print("AI edit: TODO")

        # Step 3: Rustコンパイル
        print("Compiling Rust project...")
        os.chdir(repo_dir)

        # ARM64用にクロスコンパイル
        compile_result = subprocess.run(
            ['cargo', 'build', '--release', '--target', 'aarch64-unknown-linux-musl'],
            capture_output=True,
            text=True
        )

        if compile_result.returncode != 0:
            return {
                'statusCode': 500,
                'body': json.dumps({
                    'error': 'Compilation failed',
                    'stderr': compile_result.stderr
                })
            }

        # プロジェクト名を取得
        project_name = None
        with open(cargo_toml) as f:
            for line in f:
                if line.startswith('name = '):
                    project_name = line.split('"')[1]
                    break

        if not project_name:
            return {
                'statusCode': 500,
                'body': json.dumps({'error': 'Could not determine project name'})
            }

        binary_path = os.path.join(
            repo_dir,
            'target/aarch64-unknown-linux-musl/release',
            project_name
        )

        if not os.path.exists(binary_path):
            return {
                'statusCode': 500,
                'body': json.dumps({'error': f'Binary not found at {binary_path}'})
            }

        # Step 4: Lambda用にパッケージング
        print("Packaging for Lambda...")
        package_dir = os.path.join(work_dir, 'package')
        os.makedirs(package_dir)

        bootstrap_path = os.path.join(package_dir, 'bootstrap')
        shutil.copy2(binary_path, bootstrap_path)
        os.chmod(bootstrap_path, 0o755)

        # ZIP作成
        zip_path = os.path.join(work_dir, 'deployment.zip')
        subprocess.run(
            ['zip', '-j', zip_path, bootstrap_path],
            check=True,
            capture_output=True
        )

        # Step 5: Lambdaにデプロイ
        print(f"Deploying to Lambda function: {function_name}")

        with open(zip_path, 'rb') as f:
            zip_content = f.read()

        # IAMロールの取得/作成
        role_name = 'lambda-rust-execution-role'
        try:
            role = iam_client.get_role(RoleName=role_name)
            role_arn = role['Role']['Arn']
        except:
            # ロールを作成
            trust_policy = {
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "lambda.amazonaws.com"},
                    "Action": "sts:AssumeRole"
                }]
            }

            role = iam_client.create_role(
                RoleName=role_name,
                AssumeRolePolicyDocument=json.dumps(trust_policy)
            )
            role_arn = role['Role']['Arn']

            iam_client.attach_role_policy(
                RoleName=role_name,
                PolicyArn='arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole'
            )

            # ロールの伝播待ち
            import time
            time.sleep(10)

        # Lambda関数の作成/更新
        try:
            lambda_client.update_function_code(
                FunctionName=function_name,
                ZipFile=zip_content
            )
            action = 'updated'
        except lambda_client.exceptions.ResourceNotFoundException:
            lambda_client.create_function(
                FunctionName=function_name,
                Runtime='provided.al2023',
                Role=role_arn,
                Handler='bootstrap',
                Code={'ZipFile': zip_content},
                Architectures=['arm64'],
                Timeout=30,
                MemorySize=512
            )
            action = 'created'

        return {
            'statusCode': 200,
            'body': json.dumps({
                'success': True,
                'action': action,
                'function_name': function_name,
                'binary_size': os.path.getsize(binary_path),
                'zip_size': os.path.getsize(zip_path)
            })
        }

    except Exception as e:
        return {
            'statusCode': 500,
            'body': json.dumps({
                'error': str(e)
            })
        }
    finally:
        # クリーンアップ
        shutil.rmtree(work_dir, ignore_errors=True)
