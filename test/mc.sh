#!/bin/bash
source .env
# 配置 MinIO 连接
mc alias set myminio http://host.docker.internal:9000 root 01234567

# 创建 bucket
mc mb myminio/${BUCKET_NAME}

# 创建 service account
mc admin user svcacct add myminio root \
  --access-key "${S3_ACCESS_KEY_ID}" \
  --secret-key "${S3_SECRET_ACCESS_KEY}"

# 设置 bucket 策略（可选，如果需要公开访问）
# mc anonymous set download myminio/plasma-bucket

echo "Setup completed!"
