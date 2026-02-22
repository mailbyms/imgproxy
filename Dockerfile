# 多阶段构建 - 编译阶段
FROM rust:1.88.0-alpine AS builder

# 安装编译依赖
RUN apk add --no-cache musl-dev

# 设置工作目录
WORKDIR /app

# 利用 Docker 缓存层，先编译依赖
# 只有当依赖项发生变化时，这一层才会重构
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -f src/main.rs

# 复制实际源码并编译
COPY src ./src
# 更新文件时间戳确保 cargo 重新编译
RUN touch src/main.rs && \
    cargo build --release && \
    strip target/release/imgproxy

# 运行阶段 - 使用轻量级基础镜像
FROM alpine:3.20

# 安装运行时必须的 CA 证书（用于 HTTPS 请求验证）
RUN apk add --no-cache ca-certificates

# 创建非 root 用户
RUN adduser -D -u 1000 imgproxy

# 设置工作目录
WORKDIR /app

# 从编译阶段复制二进制文件
COPY --from=builder /app/target/release/imgproxy .

# 设置权限并切换用户
RUN chown imgproxy:imgproxy imgproxy
USER imgproxy

# 环境变量配置
ENV BIND_ADDRESS=0.0.0.0:3000
EXPOSE 3000

# 健康检查
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD wget --no-verbose --tries=1 --spider http://localhost:3000/ || exit 1

ENTRYPOINT ["./imgproxy"]
