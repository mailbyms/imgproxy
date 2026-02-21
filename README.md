# imgproxy - 轻量级图片代理服务

`imgproxy` 是一个使用 Rust 编写的轻量级、高性能图片处理代理服务。它可以按需下载远程图片、进行等比例缩放并添加适当的缓存头。

## 🌟 特性

- **即时缩放**: 通过简单的 URL 路径参数，即可对远程图片进行等比例缩放。
- **调色板优化**: 特别针对 PNG 格式进行了优化，能够提取并保留原图的调色板（Palette），即使在缩放后也能最大程度保持原图的颜色表现。
- **等比例缩放**: 仅会在原图宽度大于目标宽度时进行缩小操作，避免放大导致的模糊。
- **智能缓存**: 自动添加 `Cache-Control` 和 `ETag` 响应头，提升客户端加载速度。
- **安全可靠**: 
  - 设置了最大文件大小限制（默认 10MB）。
  - 下载请求具有毫秒级超时控制。
  - 限制最大宽度数值，防止服务器资源过载。
- **错误处理**: 清晰的 JSON 格式错误响应。

## 🚀 路由格式

```text
/<宽度>/<目标图片URL>
```

- **宽度**: 目标图片的显示宽度（1 - 2000）。会自动根据原图比例计算高度。
- **目标图片URL**: 需要代理处理的原始图片地址（需包含 http:// 或 https://）。

## ⚙️ 配置项

你可以通过环境变量来配置服务：

| 环境变量 | 描述 | 默认值 |
| :--- | :--- | :--- |
| `BIND_ADDRESS` | 服务监听地址和端口 | `0.0.0.0:3000` |
| `RUST_LOG` | 日志级别 (info, debug, error 等) | `info` |

## 🛠️ 技术栈

- **Web 框架**: [Axum](https://github.com/tokio-rs/axum)
- **运行时**: [Tokio](https://tokio.rs/)
- **网络请求**: [Reqwest](https://github.com/seanmonstar/reqwest)
- **图片处理**: [image-rs](https://github.com/image-rs/image) & [png-rs](https://github.com/image-rs/image-png)
- **日志系统**: [Tracing](https://github.com/tokio-rs/tracing)

## 🏗️ 安装与运行

确保你已安装 [Rust](https://www.rust-lang.org/tools/install) 工具链。

1. **克隆并编译**:
   ```bash
   cargo build --release
   ```

2. **直接运行**:
   ```bash
   cargo run
   ```

3. **测试示例**:
   服务运行后，你可以通过浏览器或 curl 访问：
   ```bash
   http://localhost:3000/600/https://example.com/sample.png
   ```

## 📝 开发限制

- 最大原始图片大小: 10MB
- 最大允许宽度: 2000px
- 下载超时: 30s
