use axum::{
    body::Body,
    extract::Request,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use image::{imageops::FilterType, GenericImageView, ImageEncoder, DynamicImage, ColorType};
use std::io::Cursor;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{error, info, warn};

// 代理配置
const MAX_WIDTH: u32 = 2000;      // 最大宽度限制
const DOWNLOAD_TIMEOUT: u64 = 30; // 下载超时(秒)
const MAX_FILE_SIZE: usize = 10 * 1024 * 1024; // 最大文件大小 10MB

#[tokio::main]
async fn main() {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    // 获取监听地址
    let addr = std::env::var("BIND_ADDRESS")
        .unwrap_or_else(|_| "0.0.0.0:3000".to_string());

    info!("🚀 图片代理服务启动中...");
    info!("📡 监听地址: http://{}", addr);
    info!("🔧 路由格式: /<宽度>/<目标图片URL>");

    // 构建路由 - 使用 fallback 捕获所有请求
    let app = Router::new().fallback(proxy_handler);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("无法绑定地址");

    axum::serve(listener, app)
        .await
        .expect("服务器启动失败");
}

/// 图片代理处理器 - 手动解析路径
async fn proxy_handler(
    req: Request,
) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let headers = req.headers().clone();

    info!("📨 收到请求: {} {}", method, uri.path());
    let path = uri.path();

    // 跳过根路径
    if path == "/" {
        return error_response(
            StatusCode::BAD_REQUEST,
            "请使用格式: /<宽度>/<目标图片URL>",
        );
    }

    // 解析路径: /width/target_url
    let parts: Vec<&str> = path.trim_start_matches('/').splitn(2, '/').collect();

    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "路由格式错误。正确格式: /<宽度>/<目标URL>",
        );
    }

    // 解析宽度参数
    let width_str = parts[0];
    let target_url = parts[1];

    let target_width: u32 = match width_str.parse() {
        Ok(w) => w,
        Err(_) => {
            error!("无效的宽度参数: {}", width_str);
            return error_response(StatusCode::BAD_REQUEST, "无效的宽度参数");
        }
    };

    if target_width == 0 || target_width > MAX_WIDTH {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!("宽度必须在 1-{} 之间", MAX_WIDTH),
        );
    }

    info!("📥 代理请求: 宽度={} URL={}", target_width, target_url);

    // 下载原始图片
    let original_bytes = match download_image(target_url).await {
        Ok(bytes) => bytes,
        Err(status) => return error_response(status, "下载图片失败"),
    };

    // 获取图片尺寸（轻量级操作，不完全解码）
    let (orig_width, orig_height, format) = match get_image_dimensions(&original_bytes) {
        Ok(dims) => dims,
        Err(status) => return error_response(status, "图片格式不支持"),
    };

    // 如果原图宽度 ≤ 目标宽度，直接返回原始数据
    if orig_width <= target_width {
        info!("📐 原始尺寸: {}x{} ≤ 目标宽度 {}, 直接返回原图", orig_width, orig_height, target_width);
        return build_original_response(original_bytes, format);
    }

    // 需要缩放，加载完整图片数据
    let image_data = match load_image(&original_bytes) {
        Ok(data) => data,
        Err(status) => return error_response(status, "图片解析失败"),
    };

    // 计算缩放尺寸
    let new_height = (orig_height as f64 * target_width as f64 / orig_width as f64) as u32;

    info!("📐 原始尺寸: {}x{} -> 输出尺寸: {}x{}", orig_width, orig_height, target_width, new_height);

    // 执行缩放
    let resized = image_data.img.resize(target_width, new_height, FilterType::Lanczos3);

    // 编码图片
    let mut buffer = Vec::new();
    let width = resized.width();
    let height = resized.height();

    // 根据原始颜色类型选择编码方式
    let encode_result = match image_data.original_format {
        image::ImageFormat::Png => {
            // 如果原图有调色板，使用原始调色板
            if image_data.has_palette {
                info!("🎨 使用原始调色板保持颜色");

                let rgb_img = resized.to_rgb8();
                let pixels = rgb_img.as_raw();

                // 使用原始调色板
                let palette_bytes = match &image_data.original_palette {
                    Some(p) => p.clone(),
                    None => {
                        // 如果没有提取到调色板，回退到 RGB8
                        info!("⚠️  无法提取原始调色板，使用 RGB8 格式");
                        let encoder = image::codecs::png::PngEncoder::new_with_quality(
                            std::io::Cursor::new(&mut buffer),
                            image::codecs::png::CompressionType::Best,
                            image::codecs::png::FilterType::Adaptive,
                        );
                        if encoder.write_image(pixels, width, height, image::ExtendedColorType::Rgb8).is_err() {
                            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "PNG编码失败");
                        }
                        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "成功");
                    }
                };

                // 构建调色板颜色到索引的映射
                let palette_colors: Vec<(u8, u8, u8)> = palette_bytes.chunks(3)
                    .map(|c| (c[0], c[1], c[2]))
                    .collect();

                let mut color_to_index: std::collections::HashMap<(u8, u8, u8), u8> = std::collections::HashMap::new();
                for (i, color) in palette_colors.iter().enumerate() {
                    color_to_index.insert(*color, i as u8);
                }

                // 为每个像素找到对应的调色板索引
                let indices: Vec<u8> = pixels.chunks(3)
                    .map(|chunk| {
                        let pixel = (chunk[0], chunk[1], chunk[2]);
                        if let Some(&idx) = color_to_index.get(&pixel) {
                            idx
                        } else {
                            // 找到最接近的颜色
                            let mut best_idx = 0;
                            let mut best_dist = u32::MAX;
                            for (i, &(pr, pg, pb)) in palette_colors.iter().enumerate() {
                                let dr = (pr as i32 - chunk[0] as i32).abs();
                                let dg = (pg as i32 - chunk[1] as i32).abs();
                                let db = (pb as i32 - chunk[2] as i32).abs();
                                let dist = (dr * dr + dg * dg + db * db) as u32;
                                if dist < best_dist {
                                    best_dist = dist;
                                    best_idx = i;
                                }
                            }
                            best_idx as u8
                        }
                    })
                    .collect();

                // PNG 编码
                {
                    let mut cursor = Cursor::new(&mut buffer);
                    let mut encoder = png::Encoder::new(&mut cursor, width, height);
                    encoder.set_color(png::ColorType::Indexed);
                    encoder.set_depth(png::BitDepth::Eight);
                    encoder.set_palette(&palette_bytes);
                    encoder.set_compression(png::Compression::Best);

                    let mut writer = match encoder.write_header() {
                        Ok(w) => w,
                        Err(e) => {
                            error!("PNG编码器初始化失败: {}", e);
                            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "PNG编码失败");
                        }
                    };

                    match writer.write_image_data(&indices) {
                        Ok(_) => {},
                        Err(e) => {
                            error!("PNG数据写入失败: {}", e);
                            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "PNG编码失败");
                        }
                    }
                }

                info!("✅ 使用原始调色板完成，输出大小: {} 字节", buffer.len());
                Ok(())
            } else {
                // 根据颜色类型选择最佳编码方式
                let (pixel_data, color_type) = match image_data.color_type {
                    ColorType::L8 => {
                        // 灰度图
                        let gray_img = resized.to_luma8();
                        (gray_img.as_raw().to_vec(), image::ExtendedColorType::L8)
                    }
                    ColorType::La8 => {
                        // 灰度+透明度
                        let gray_alpha_img = resized.to_luma_alpha8();
                        (gray_alpha_img.as_raw().to_vec(), image::ExtendedColorType::La8)
                    }
                    ColorType::Rgb8 => {
                        // RGB
                        let rgb_img = resized.to_rgb8();
                        (rgb_img.as_raw().to_vec(), image::ExtendedColorType::Rgb8)
                    }
                    ColorType::Rgba8 => {
                        // RGBA
                        let rgba_img = resized.to_rgba8();
                        (rgba_img.as_raw().to_vec(), image::ExtendedColorType::Rgba8)
                    }
                    _ => {
                        // 其他格式转换为 RGB8
                        let rgb_img = resized.to_rgb8();
                        (rgb_img.as_raw().to_vec(), image::ExtendedColorType::Rgb8)
                    }
                };

                let encoder = image::codecs::png::PngEncoder::new_with_quality(
                    std::io::Cursor::new(&mut buffer),
                    image::codecs::png::CompressionType::Best,
                    image::codecs::png::FilterType::Adaptive,
                );
                encoder.write_image(&pixel_data, width, height, color_type)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
        _ => {
            // JPEG 格式（只能使用 RGB8）
            let rgb_img = resized.to_rgb8();
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                std::io::Cursor::new(&mut buffer),
                85,
            );
            encoder.write_image(
                rgb_img.as_raw(),
                width,
                height,
                image::ExtendedColorType::Rgb8,
            ).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
    };

    if encode_result.is_err() {
        error!("图片编码失败");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "图片编码失败");
    }

    info!("✅ 处理完成: 输出大小 {} 字节", buffer.len());

    // 构建响应
    let content_type = match image_data.original_format {
        image::ImageFormat::Png => "image/png",
        _ => "image/jpeg",
    };

    // 设置缓存头
    let mut response = Response::new(Body::from(buffer));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        content_type.parse().unwrap(),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "public, max-age=31536000, immutable".parse().unwrap(),
    );

    // 转发 ETag 如果存在
    if let Some(etag) = headers.get("if-none-match") {
        response.headers_mut().insert(header::ETAG, etag.clone());
    }

    response
}

/// 下载远程图片
async fn download_image(url: &str) -> Result<Vec<u8>, StatusCode> {
    // 验证 URL 格式
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(StatusCode::BAD_REQUEST);
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(DOWNLOAD_TIMEOUT))
        .user_agent("imgproxy/1.0")
        .build()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let response = timeout(
        Duration::from_secs(DOWNLOAD_TIMEOUT),
        client.get(url).send(),
    )
    .await
    .map_err(|_| {
        warn!("下载超时: {}", url);
        StatusCode::GATEWAY_TIMEOUT
    })?
    .map_err(|e| {
        error!("下载失败: {} - {}", url, e);
        StatusCode::BAD_GATEWAY
    })?;

    // 检查响应状态
    if !response.status().is_success() {
        error!("上游返回错误状态: {}", response.status());
        return Err(StatusCode::BAD_GATEWAY);
    }

    // 检查内容长度
    if let Some(len) = response.content_length() {
        if len > MAX_FILE_SIZE as u64 {
            warn!("文件过大: {} 字节", len);
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
    }

    // 限制下载大小
    let bytes = response
        .bytes()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?
        .to_vec();

    if bytes.len() > MAX_FILE_SIZE {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    Ok(bytes)
}

/// 加载图片数据并保留颜色类型信息
struct ImageWithData {
    img: DynamicImage,
    color_type: ColorType,
    original_format: image::ImageFormat,
    has_palette: bool,           // 是否有调色板
    original_palette: Option<Vec<u8>>,  // 原始调色板数据 (RGB格式扁平数组)
}

fn load_image(bytes: &[u8]) -> Result<ImageWithData, StatusCode> {
    // 检测PNG调色板并提取原始调色板
    let (has_palette, original_palette) = if bytes.len() > 8 {
        // PNG 签名: 137 80 78 71 13 10 26 10
        if bytes[0..8] == [137, 80, 78, 71, 13, 10, 26, 10] {
            // 尝试解码PNG头部并提取调色板
            let decoder = png::Decoder::new(Cursor::new(bytes));
            let reader = decoder.read_info().map_err(|_| StatusCode::UNSUPPORTED_MEDIA_TYPE)?;
            let info = reader.info();
            if matches!(info.color_type, png::ColorType::Indexed) {
                // 提取原始调色板 (palette 是扁平的 RGB 字节数组)
                (true, info.palette.as_ref().map(|p| p.to_vec()))
            } else {
                (false, None)
            }
        } else {
            (false, None)
        }
    } else {
        (false, None)
    };

    // 检测图片格式
    let format = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map(|r| r.format().unwrap_or(image::ImageFormat::Jpeg))
        .unwrap_or(image::ImageFormat::Jpeg);

    // 解码图片
    let img = image::load_from_memory(bytes).map_err(|e| {
        error!("图片解析失败: {}", e);
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    })?;

    let color_type = img.color();

    if has_palette {
        info!("🎨 原图颜色类型: Indexed (调色板)");
    } else {
        info!("🎨 原图颜色类型: {:?}", color_type);
    }

    Ok(ImageWithData {
        img,
        color_type,
        original_format: format,
        has_palette,
        original_palette,
    })
}

/// 轻量级获取图片尺寸（不完全解码）
fn get_image_dimensions(bytes: &[u8]) -> Result<(u32, u32, image::ImageFormat), StatusCode> {
    use image::ImageReader;

    // 检测图片格式并获取尺寸
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| StatusCode::UNSUPPORTED_MEDIA_TYPE)?;

    let format = reader.format().ok_or(StatusCode::UNSUPPORTED_MEDIA_TYPE)?;
    let dimensions = reader.into_dimensions().map_err(|_| StatusCode::UNSUPPORTED_MEDIA_TYPE)?;

    Ok((dimensions.0, dimensions.1, format))
}

/// 构建原始图片响应（直接返回原始数据）
fn build_original_response(bytes: Vec<u8>, format: image::ImageFormat) -> Response {
    let content_type = match format {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        _ => "image/jpeg",
    };

    info!("✅ 直接返回原图: {} 字节", bytes.len());

    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        content_type.parse().unwrap(),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "public, max-age=31536000, immutable".parse().unwrap(),
    );

    response
}

/// 构建错误响应
fn error_response(status: StatusCode, message: &str) -> Response {
    let body = serde_json::json!({
        "error": message,
        "code": status.as_u16()
    });
    (status, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}
