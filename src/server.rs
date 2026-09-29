use crate::{
    model::*,
    platform,
    store::{Store, atomic_json},
    worker::{self, Request},
};
use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{ConnectInfo, DefaultBodyLimit, Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::json;
use sha2::Sha256;
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{Mutex, Notify, Semaphore};

struct App {
    machine: Result<String, String>,
    activation: Mutex<()>,
    config: Config,
    root: PathBuf,
    store: Mutex<Store>,
    devices: Mutex<Option<(u64, Devices)>>,
    heavy: Arc<Semaphore>,
    office: Semaphore,
    profile_dialog: Semaphore,
    network_operation: Arc<Semaphore>,
    notify: Notify,
    demo: bool,
}
type Shared = Arc<App>;
struct ApiError(StatusCode, String);
impl From<String> for ApiError {
    fn from(e: String) -> Self {
        Self(StatusCode::BAD_REQUEST, e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}
type ApiResult<T> = Result<T, ApiError>;

pub fn run(args: &[String]) -> AppResult<()> {
    let mut demo = false;
    let mut no_tray = false;
    let mut background = false;
    let mut port = None;
    let mut root = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--demo" => demo = true,
            "--no-tray" => no_tray = true,
            "--background" => background = true,
            "--port" => {
                i += 1;
                port = Some(
                    args.get(i)
                        .ok_or("缺少端口")?
                        .parse::<u16>()
                        .map_err(|_| "无效端口")?,
                );
            }
            "--data-dir" => {
                i += 1;
                root = Some(PathBuf::from(args.get(i).ok_or("缺少数据目录")?));
            }
            "--help" => {
                println!(
                    "LanPrint [--port 17860] [--data-dir PATH] [--no-tray] [--background] [--demo]"
                );
                return Ok(());
            }
            a => return Err(format!("未知参数：{a}")),
        }
        i += 1;
    }
    let root = match root {
        Some(root) => root,
        None => platform::data_directory(demo)?,
    };
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let instance = platform::single_instance(&root)?;
    let path = root.join("config.json");
    let mut config: Config = if path.exists() {
        serde_json::from_slice(&std::fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("config.json 格式错误：{e}"))?
    } else {
        let mut c = Config::default();
        if demo {
            c.port = 17861;
        }
        atomic_json(&path, &c)?;
        c
    };
    if let Some(p) = port {
        config.port = p;
    }
    config.validate()?;
    let url = format!("http://127.0.0.1:{}", config.port);
    // Reopening a desktop shortcut should reveal the running station.
    let Some(_instance) = instance else {
        if !background && !no_tray {
            platform::open(&url);
        }
        return Ok(());
    };
    let store = Store::open(root.clone())?;
    let listener = std::net::TcpListener::bind(("0.0.0.0", config.port))
        .map_err(|e| format!("端口 {} 无法使用：{e}", config.port))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let share_url = format!("http://{}:{}", platform::lan_ip(), config.port);
    let state = Arc::new(App {
        machine: crate::licensing::current_machine(),
        activation: Mutex::new(()),
        config,
        root: root.clone(),
        store: Mutex::new(store),
        devices: Mutex::new(None),
        heavy: Arc::new(Semaphore::new(3)),
        office: Semaphore::new(1),
        profile_dialog: Semaphore::new(1),
        network_operation: Arc::new(Semaphore::new(1)),
        notify: Notify::new(),
        demo,
    });
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let serve = move || -> AppResult<()> {
        let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
        rt.block_on(async move {
            let worker_task = tokio::spawn(queue_loop(state.clone()));
            let cleanup_state = state.clone();
            let cleanup_task = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    if let Err(e) = cleanup_state.store.lock().await.cleanup() {
                        eprintln!("cleanup: {e}");
                    }
                }
            });
            let router = router(state);
            let listener =
                tokio::net::TcpListener::from_std(listener).map_err(|e| e.to_string())?;
            let result = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                tokio::select! { _ = stop_rx => {}, _ = shutdown_signal() => {} }
            })
            .await
            .map_err(|e| e.to_string());
            worker_task.abort();
            cleanup_task.abort();
            result
        })
    };
    #[cfg(windows)]
    {
        if no_tray {
            return serve();
        }
        let handle = std::thread::spawn(serve);
        let startup = crate::startup::Startup::new(&root, port, demo)?;
        platform::tray(&url, &share_url, &root, demo, background, &startup, stop_tx)?;
        handle.join().map_err(|_| "服务线程异常退出".to_string())?
    }
    #[cfg(unix)]
    {
        let _stop_tx = stop_tx;
        println!(
            "LanPrint: {url}\n局域网地址: {share_url}\n数据目录: {}",
            root.display()
        );
        if !no_tray && !background {
            platform::open(&url);
        }
        serve()
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

fn router(state: Shared) -> Router {
    Router::new()
        .route(
            "/",
            get(|State(s): State<Shared>| async move {
                (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    if licensed(&s).is_ok() {
                        include_str!("../web/index.html")
                    } else {
                        include_str!("../web/activation.html")
                    },
                )
            }),
        )
        .route(
            "/app.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../web/app.css"),
                )
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/app.js"),
                )
            }),
        )
        .route("/api/session", get(session))
        .route("/api/share-qr.svg", get(share_qr))
        .route("/api/license", get(license_status).post(activate_license))
        .route(
            "/register",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    include_str!("../web/activation.html"),
                )
            }),
        )
        .route(
            "/activation.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/activation.js"),
                )
            }),
        )
        .route(
            "/profiles.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/profiles.js"),
                )
            }),
        )
        .route("/api/devices", get(devices))
        .route("/api/network", get(network_status))
        .route("/api/network/repair", post(network_repair))
        .route(
            "/network.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/network.js"),
                )
            }),
        )
        .route("/api/profiles", get(list_profiles).post(create_profile))
        .route("/api/profiles/{id}", delete(delete_profile))
        .route("/api/files", get(files).post(upload))
        .route("/api/files/{id}", delete(remove_file))
        .route("/api/files/{id}/download", get(download))
        .route("/api/files/{id}/preview", get(preview))
        .route("/api/jobs", get(jobs))
        .route("/api/print", post(submit_print))
        .route("/api/scan", post(submit_scan))
        .route("/api/jobs/{id}/cancel", post(cancel))
        .layer(DefaultBodyLimit::max(
            (state.config.max_upload_mb as usize + 1) * 1024 * 1024,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), security))
        .with_state(state)
}

fn private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_loopback() || v.is_link_local(),
        IpAddr::V6(v) => {
            v.is_loopback()
                || v.is_unique_local()
                || v.is_unicast_link_local()
                || v.to_ipv4_mapped().is_some_and(|v| private_ip(v.into()))
        }
    }
}
fn sign(config: &Config, value: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(config.secret.as_bytes()).unwrap();
    mac.update(value.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}
fn verify(config: &Config, value: &str, signature: &str) -> bool {
    let Ok(bytes) = hex::decode(signature) else {
        return false;
    };
    let mut mac = Hmac::<Sha256>::new_from_slice(config.secret.as_bytes()).unwrap();
    mac.update(value.as_bytes());
    mac.verify_slice(&bytes).is_ok()
}
fn owner_cookie(headers: &HeaderMap, config: &Config) -> Option<String> {
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    let value = cookies
        .split(';')
        .find_map(|v| v.trim().strip_prefix("lp_session="))?;
    let (id, sig) = value.split_once('.')?;
    if uuid::Uuid::parse_str(id).is_ok() && verify(config, id, sig) {
        Some(id.into())
    } else {
        None
    }
}
async fn security(
    State(state): State<Shared>,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let error = |s: &str| ApiError(StatusCode::FORBIDDEN, s.into()).into_response();
    if req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_none_or(|c| !private_ip(c.0.ip()))
    {
        return error("此服务仅允许内网访问。");
    }
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
        return error("无效的服务地址。");
    };
    let hostname = authority.host();
    let machine = std::env::var("COMPUTERNAME").unwrap_or_default();
    if hostname != "localhost"
        && !hostname.eq_ignore_ascii_case(&machine)
        && !hostname
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(private_ip)
    {
        return error("请使用主机的内网 IP 地址访问。");
    }
    let mut fresh = false;
    let owner = owner_cookie(req.headers(), &state.config).unwrap_or_else(|| {
        fresh = true;
        uuid::Uuid::new_v4().to_string()
    });
    if req.method() != axum::http::Method::GET && req.method() != axum::http::Method::HEAD {
        let token = req
            .headers()
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if fresh || !verify(&state.config, &format!("csrf:{owner}"), token) {
            return error("会话已失效，请刷新页面后重试。");
        }
        if let Some(origin) = req.headers().get(header::ORIGIN)
            && origin.to_str().ok() != Some(format!("http://{host}").as_str())
        {
            return error("不允许跨站提交。");
        }
    }
    req.extensions_mut().insert(owner.clone());
    let mut response = if !registration_route(req.uri().path()) && licensed(&state).is_err() {
        (StatusCode::FORBIDDEN, Json(json!({"error":"打印站尚未注册或授权无效，请在主机完成注册。", "code":"license_required"}))).into_response()
    } else {
        next.run(req).await
    };
    let headers = response.headers_mut();
    if fresh
        && let Ok(value) = format!(
            "lp_session={owner}.{}; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000",
            sign(&state.config, &owner)
        )
        .parse()
    {
        headers.insert(header::SET_COOKIE, value);
    }
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert("referrer-policy", "no-referrer".parse().unwrap());
    headers.insert("content-security-policy", "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' blob:; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'".parse().unwrap());
    response
}
async fn session(
    State(s): State<Shared>,
    Extension(owner): Extension<String>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Json<serde_json::Value> {
    Json(
        json!({"csrf":sign(&s.config,&format!("csrf:{owner}")),"demo":s.demo,"maxUploadMb":s.config.max_upload_mb,
        "retentionMinutes":s.config.retention_minutes,"officeFormats":platform::office_formats(),"localAdmin":addr.ip().is_loopback(),"shareUrl":share_url(s.config.port),
        "platform":std::env::consts::OS,"driverProfiles":cfg!(windows),"firewallRepair":cfg!(windows)}),
    )
}

fn share_url(port: u16) -> String {
    format!("http://{}:{port}", platform::lan_ip())
}

async fn share_qr(State(s): State<Shared>) -> ApiResult<Response> {
    let url = share_url(s.config.port);
    if url.starts_with("http://127.") {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "未获取到局域网地址，请连接办公室网络后刷新。".into(),
        ));
    }
    let code = qrcode::QrCode::new(url.as_bytes()).map_err(|e| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("二维码生成失败：{e}"),
        )
    })?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(148, 148)
        .quiet_zone(true)
        .build();
    Ok((
        [(header::CONTENT_TYPE, "image/svg+xml; charset=utf-8")],
        svg,
    )
        .into_response())
}

fn registration_route(path: &str) -> bool {
    matches!(
        path,
        "/" | "/register" | "/app.css" | "/activation.js" | "/api/session" | "/api/license"
    )
}

fn licensed(s: &App) -> AppResult<lan_print::license_format::License> {
    let machine = s.machine.as_ref().map_err(Clone::clone)?;
    crate::licensing::load(&s.root, machine)
}

async fn license_status(
    State(s): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Json<serde_json::Value> {
    let local = addr.ip().is_loopback();
    let license = licensed(&s);
    Json(json!({
        "registered": license.is_ok(),
        "localAdmin": local,
        "machineCode": if local { s.machine.as_ref().ok() } else { None },
        "customer": if local { license.as_ref().ok().map(|l| l.customer.as_str()) } else { None },
        "message": if !local { "请在打印主机打开本机地址完成注册。" } else {
            match &license { Ok(_) => "已注册 · 本机永久授权", Err(e) => e.as_str() }
        }
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivationInput {
    code: String,
}

async fn activate_license(
    State(s): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ActivationInput>,
) -> ApiResult<Json<serde_json::Value>> {
    require_host(addr)?;
    let _guard = s.activation.lock().await;
    let machine = s.machine.as_ref().map_err(Clone::clone)?;
    let license = crate::licensing::activate(&s.root, machine, &input.code)?;
    Ok(Json(
        json!({"registered": true, "customer": license.customer}),
    ))
}
async fn get_devices(s: &Shared) -> AppResult<Devices> {
    let mut cache = s.devices.lock().await;
    if let Some((at, d)) = &*cache
        && now().saturating_sub(*at) < 30
    {
        return Ok(d.clone());
    }
    let d = worker::call(
        &s.root,
        Request {
            operation: "devices".into(),
            demo: s.demo,
            show_virtual: s.config.show_virtual_printers,
            ..Default::default()
        },
        25,
    )
    .await?
    .devices
    .ok_or("无法读取设备列表")?;
    *cache = Some((now(), d.clone()));
    Ok(d)
}
async fn devices(State(s): State<Shared>) -> ApiResult<Json<Devices>> {
    Ok(Json(get_devices(&s).await?))
}

fn require_host(addr: SocketAddr) -> ApiResult<()> {
    if !addr.ip().is_loopback() {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "请在打印主机打开 http://127.0.0.1 地址进行此管理操作。".into(),
        ));
    }
    Ok(())
}

async fn network_status(
    State(s): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> ApiResult<Json<platform::NetworkStatus>> {
    require_host(addr)?;
    if s.demo {
        return Ok(Json(platform::NetworkStatus::demo(s.config.port)));
    }
    let permit = s
        .network_operation
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "正在检查或修复端口，请等待当前操作完成。".into(),
            )
        })?;
    let port = s.config.port;
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        platform::inspect_network(port)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(Json(result))
}

async fn network_repair(
    State(s): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> ApiResult<Json<platform::NetworkStatus>> {
    require_host(addr)?;
    if s.demo {
        return Err("演示模式不修改真实防火墙。".to_string().into());
    }
    let permit = s
        .network_operation
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "已有端口检查或修复正在进行，请先完成主机上的提示。".into(),
            )
        })?;
    let port = s.config.port;
    // The blocking task owns the permit even when a browser disconnects; a
    // second request cannot start another UAC prompt while repair is running.
    let result = tokio::task::spawn_blocking(move || -> AppResult<platform::NetworkStatus> {
        let _permit = permit;
        let before = platform::inspect_network(port)?;
        if !before.can_repair {
            return Ok(before);
        }
        platform::repair_network_elevated(port)?;
        platform::inspect_network(port)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(Json(result))
}
async fn list_profiles(State(s): State<Shared>) -> ApiResult<Json<Vec<serde_json::Value>>> {
    Ok(Json(crate::profiles::list(&s.root)?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileInput {
    printer: String,
    name: String,
}
async fn create_profile(
    State(s): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ProfileInput>,
) -> ApiResult<Json<serde_json::Value>> {
    require_host(addr)?;
    if s.demo {
        return Err("演示模式不打开真实打印驱动，请在正常模式创建预设。"
            .to_string()
            .into());
    }
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err("请填写 1–80 字的预设名称。".to_string().into());
    }
    let _dialog = s
        .profile_dialog
        .try_acquire()
        .map_err(|_| "已有驱动设置窗口打开，请先完成该窗口。".to_string())?;
    if crate::profiles::list(&s.root)?.len() >= 50 {
        return Err("最多保存 50 个驱动预设，请先删除不再使用的预设。"
            .to_string()
            .into());
    }
    let devices = get_devices(&s).await?;
    if !devices.printers.iter().any(|p| p.id == input.printer) {
        return Err("打印机不存在，请刷新设备列表。".to_string().into());
    }
    let result = worker::call(
        &s.root,
        Request {
            operation: "profile".into(),
            path: input.printer,
            ..Default::default()
        },
        300,
    )
    .await?;
    let mut profile = result.profile.ok_or("驱动未返回预设。".to_string())?;
    profile.name = name.to_string();
    crate::profiles::save(&s.root, &profile)?;
    Ok(Json(
        json!({"id":profile.id,"name":profile.name,"printer":profile.printer}),
    ))
}
async fn delete_profile(
    State(s): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    require_host(addr)?;
    let st = s.store.lock().await;
    if st
        .db
        .jobs
        .iter()
        .any(|j| j.active() && j.print.as_ref().is_some_and(|p| p.profile_id == id))
    {
        return Err("该预设仍被排队或打印中的任务使用，请稍后再删除。"
            .to_string()
            .into());
    }
    crate::profiles::remove(&s.root, &id)?;
    Ok(Json(json!({"ok":true})))
}
fn public_file(f: &StoredFile) -> serde_json::Value {
    json!({"id":f.id,"name":f.name,"bytes":f.bytes,"pages":f.pages,"expiresAt":f.expires_at,"createdAt":f.created_at,"scanned":f.scanned,"extension":f.extension})
}
async fn files(
    State(s): State<Shared>,
    Extension(owner): Extension<String>,
) -> Json<serde_json::Value> {
    let st = s.store.lock().await;
    let mut items: Vec<_> = st
        .db
        .files
        .values()
        .filter(|f| f.owner == owner && (f.expires_at > now() || st.pinned(&f.id)))
        .collect();
    items.sort_by_key(|f| std::cmp::Reverse(f.created_at));
    Json(json!(
        items.into_iter().map(public_file).collect::<Vec<_>>()
    ))
}
async fn upload(
    State(s): State<Shared>,
    Extension(owner): Extension<String>,
    mut multipart: Multipart,
) -> ApiResult<Json<serde_json::Value>> {
    let _permit = s.heavy.clone().try_acquire_owned().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "正在处理其他文件，请稍后重试。".into(),
        )
    })?;
    let mut field = multipart
        .next_field()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("请选择文件。".to_string())?;
    let mut name = field
        .file_name()
        .unwrap_or("file")
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("file")
        .chars()
        .filter(|c| !c.is_control())
        .take(180)
        .collect::<String>();
    let checked_extension = extension(&name);
    let mut ext = checked_extension
        .as_ref()
        .cloned()
        .unwrap_or_else(|_| "upload".into());
    let office = office_extension(&ext);
    let id = uuid::Uuid::new_v4().to_string();
    let mut path = s.root.join("files").join(format!("{id}.{ext}"));
    let pending_file = crate::store::PendingFile(path.clone());
    let converted_path = path.with_extension("pdf");
    let converted_file = office.then(|| crate::store::PendingFile(converted_path.clone()));
    let written = async {
        use tokio::io::AsyncWriteExt;
        let mut out = tokio::fs::File::create(&path)
            .await
            .map_err(|e| e.to_string())?;
        let mut bytes = 0u64;
        while let Some(chunk) = field.chunk().await.map_err(|e| e.to_string())? {
            bytes += chunk.len() as u64;
            if bytes > s.config.max_upload_mb * 1024 * 1024 {
                return Err("文件超过大小限制。".to_string());
            }
            out.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }
        out.flush().await.map_err(|e| e.to_string())?;
        drop(out);
        Ok::<u64, String>(bytes)
    }
    .await;
    drop(field);
    let process = async {
        let mut bytes = written?;
        if bytes == 0 {
            return Err("文件是空的。".into());
        }
        if multipart
            .next_field()
            .await
            .map_err(|e| e.to_string())?
            .is_some()
        {
            return Err("每次请求只能上传一个文件。".into());
        }
        if office {
            if !platform::office_formats().contains(&ext.as_str()) {
                return Err(format!(
                    "主机未检测到可处理 .{ext} 的 Office/WPS 接口，请安装对应办公组件或先导出 PDF。"
                ));
            }
            let _conversion = s
                .office
                .try_acquire()
                .map_err(|_| "正在转换其他 Office/WPS 文档，请稍后重试。".to_string())?;
            worker::call(
                &s.root,
                Request {
                    operation: "office".into(),
                    path: path.to_string_lossy().into(),
                    output: converted_path.to_string_lossy().into(),
                    ..Default::default()
                },
                120,
            )
            .await?;
            path = converted_path;
            ext = "pdf".into();
            name.push_str(".pdf");
            bytes = tokio::fs::metadata(&path)
                .await
                .map_err(|e| e.to_string())?
                .len();
        }
        // Finish consuming the multipart body before reporting an unsupported
        // format, so HTTP keep-alive clients receive the JSON error reliably.
        checked_extension?;
        let info = worker::call(
            &s.root,
            Request {
                operation: "inspect".into(),
                path: path.to_string_lossy().into(),
                ..Default::default()
            },
            30,
        )
        .await?;
        page_range("", info.pages)?;
        let file = StoredFile {
            id: id.clone(),
            owner,
            name,
            extension: ext,
            bytes,
            pages: info.pages,
            created_at: now(),
            expires_at: now() + s.config.retention_minutes * 60,
            scanned: false,
        };
        s.store
            .lock()
            .await
            .insert_file(file.clone(), s.config.max_storage_mb)?;
        Ok(file)
    }
    .await;
    match process {
        Ok(f) => {
            if let Some(converted) = converted_file {
                converted.persist();
            } else {
                pending_file.persist();
            }
            Ok(Json(public_file(&f)))
        }
        Err(e) => Err(e.into()),
    }
}
async fn remove_file(
    State(s): State<Shared>,
    Extension(owner): Extension<String>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut st = s.store.lock().await;
    let f = st.owned(&id, &owner)?;
    if st.pinned(&id) {
        return Err("文件有待处理任务，暂时不能删除。".to_string().into());
    }
    std::fs::remove_file(st.path(&f)).map_err(|e| e.to_string())?;
    st.db.files.remove(&id);
    st.save()?;
    Ok(Json(json!({"ok":true})))
}
async fn touch_file(s: &Shared, owner: &str, id: &str) -> AppResult<(StoredFile, PathBuf)> {
    let mut st = s.store.lock().await;
    let mut f = st.owned(id, owner)?;
    f.expires_at = now() + s.config.retention_minutes * 60;
    st.db.files.insert(id.into(), f.clone());
    st.save()?;
    Ok((f.clone(), st.path(&f)))
}
async fn download(
    State(s): State<Shared>,
    Extension(owner): Extension<String>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let (f, path) = touch_file(&s, &owner, &id).await?;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| e.to_string())?;
    let length = file.metadata().await.map_err(|e| e.to_string())?.len();
    let encoded: String = f
        .name
        .as_bytes()
        .iter()
        .map(|b| format!("%{b:02X}"))
        .collect();
    let mime = match f.extension.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "bmp" => "image/bmp",
        _ => "image/jpeg",
    };
    Ok((
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CONTENT_LENGTH, length.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename=\"document.{}\"; filename*=UTF-8''{encoded}",
                    f.extension
                ),
            ),
        ],
        Body::from_stream(tokio_util::io::ReaderStream::new(file)),
    )
        .into_response())
}
#[derive(Deserialize)]
struct PageQuery {
    #[serde(default)]
    page: u32,
}
async fn preview(
    State(s): State<Shared>,
    Extension(owner): Extension<String>,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> ApiResult<Response> {
    let _permit = s
        .heavy
        .clone()
        .try_acquire_owned()
        .map_err(|_| "正在生成其他预览，请稍后重试。".to_string())?;
    let (f, path) = touch_file(&s, &owner, &id).await?;
    if q.page >= f.pages {
        return Err("页码超出范围。".to_string().into());
    }
    let out = s
        .root
        .join("work")
        .join(format!("{}.jpg", uuid::Uuid::new_v4()));
    let _pending = crate::store::PendingFile(out.clone());
    worker::call(
        &s.root,
        Request {
            operation: "preview".into(),
            path: path.to_string_lossy().into(),
            output: out.to_string_lossy().into(),
            page: q.page,
            ..Default::default()
        },
        30,
    )
    .await?;
    let data = tokio::fs::read(&out).await.map_err(|e| e.to_string())?;
    Ok(([(header::CONTENT_TYPE, "image/jpeg")], data).into_response())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrintInput {
    file_id: String,
    request_id: String,
    options: PrintOptions,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScanInput {
    request_id: String,
    options: ScanOptions,
}
fn request_id(id: &str) -> AppResult<()> {
    uuid::Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| "无效的请求标识。".into())
}
async fn submit_print(
    State(s): State<Shared>,
    Extension(owner): Extension<String>,
    Json(input): Json<PrintInput>,
) -> ApiResult<Json<serde_json::Value>> {
    request_id(&input.request_id)?;
    if let Some(job) = s
        .store
        .lock()
        .await
        .existing_request(&owner, &input.request_id, "print")?
    {
        return Ok(Json(public_job(&job)));
    }
    let devices = get_devices(&s).await?;
    let mut st = s.store.lock().await;
    if let Some(job) = st.existing_request(&owner, &input.request_id, "print")? {
        return Ok(Json(public_job(&job)));
    }
    let f = st.owned(&input.file_id, &owner)?;
    input.options.validate(&devices, f.pages)?;
    if !input.options.profile_id.is_empty() {
        crate::profiles::load(&s.root, &input.options.profile_id, &input.options.printer)?;
    }
    let j = Job {
        id: uuid::Uuid::new_v4().to_string(),
        owner,
        request_id: input.request_id,
        kind: "print".into(),
        name: f.name,
        file_id: Some(f.id),
        print: Some(input.options),
        scan: None,
        status: "queued".into(),
        message: "等待处理".into(),
        created_at: now(),
        finished_at: None,
    };
    let job = st.enqueue(j, s.config.max_pending)?;
    drop(st);
    s.notify.notify_one();
    Ok(Json(public_job(&job)))
}
async fn submit_scan(
    State(s): State<Shared>,
    Extension(owner): Extension<String>,
    Json(input): Json<ScanInput>,
) -> ApiResult<Json<serde_json::Value>> {
    request_id(&input.request_id)?;
    if let Some(job) = s
        .store
        .lock()
        .await
        .existing_request(&owner, &input.request_id, "scan")?
    {
        return Ok(Json(public_job(&job)));
    }
    let devices = get_devices(&s).await?;
    let mut st = s.store.lock().await;
    if let Some(job) = st.existing_request(&owner, &input.request_id, "scan")? {
        return Ok(Json(public_job(&job)));
    }
    input.options.validate(&devices)?;
    st.check_capacity(50 * 1024 * 1024, s.config.max_storage_mb)?;
    let j = Job {
        id: uuid::Uuid::new_v4().to_string(),
        owner,
        request_id: input.request_id,
        kind: "scan".into(),
        name: "扫描文档".into(),
        file_id: None,
        print: None,
        scan: Some(input.options),
        status: "queued".into(),
        message: "等待扫描，请将原稿放入扫描仪".into(),
        created_at: now(),
        finished_at: None,
    };
    let job = st.enqueue(j, s.config.max_pending)?;
    drop(st);
    s.notify.notify_one();
    Ok(Json(public_job(&job)))
}
fn public_job(j: &Job) -> serde_json::Value {
    json!({"id":j.id,"kind":j.kind,"name":j.name,"fileId":j.file_id,"status":j.status,"message":j.message,"createdAt":j.created_at,"finishedAt":j.finished_at})
}
async fn jobs(
    State(s): State<Shared>,
    Extension(owner): Extension<String>,
) -> Json<serde_json::Value> {
    let st = s.store.lock().await;
    Json(json!(
        st.db
            .jobs
            .iter()
            .rev()
            .filter(|j| j.owner == owner)
            .take(100)
            .map(public_job)
            .collect::<Vec<_>>()
    ))
}
async fn cancel(
    State(s): State<Shared>,
    Extension(owner): Extension<String>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    s.store.lock().await.cancel(&id, &owner)?;
    Ok(Json(json!({"ok":true})))
}
async fn queue_loop(s: Shared) {
    loop {
        let job = {
            let mut st = s.store.lock().await;
            let j = st
                .db
                .jobs
                .iter_mut()
                .find(|j| j.status == "queued")
                .map(|j| {
                    j.status = "running".into();
                    j.message = if j.kind == "scan" {
                        "正在扫描，请勿移动原稿"
                    } else {
                        "正在渲染并提交打印"
                    }
                    .into();
                    j.clone()
                });
            if j.is_some() && st.save().is_err() {
                if let Some(job) = &j
                    && let Some(saved) = st.db.jobs.iter_mut().find(|v| v.id == job.id)
                {
                    saved.status = "failed".into();
                    saved.message = "无法保存任务状态，未调用设备；请检查磁盘空间和权限。".into();
                    saved.finished_at = Some(now());
                }
                s.notify.notify_one();
                None
            } else {
                j
            }
        };
        let Some(job) = job else {
            s.notify.notified().await;
            continue;
        };
        let result = execute(&s, &job).await;
        let mut st = s.store.lock().await;
        if let Some(j) = st.db.jobs.iter_mut().find(|j| j.id == job.id) {
            j.finished_at = Some(now());
            match result {
                Ok(file) => {
                    if let Some(f) = file {
                        j.name = f.name;
                        j.file_id = Some(f.id);
                    }
                    j.status = if job.kind == "scan" {
                        "completed"
                    } else if s.demo {
                        "simulated"
                    } else {
                        "submitted"
                    }
                    .into();
                    j.message = if s.demo {
                        "演示任务已完成，未使用真实设备。"
                    } else if job.kind == "scan" {
                        "扫描完成，请及时下载。"
                    } else {
                        "已提交系统打印队列；实际出纸请以打印机为准。"
                    }
                    .into();
                }
                Err(e) => {
                    j.status = "failed".into();
                    j.message = e;
                }
            }
        }
        if let Err(e) = st.save() {
            eprintln!("save job: {e}");
        }
    }
}
async fn execute(s: &Shared, job: &Job) -> AppResult<Option<StoredFile>> {
    licensed(s)?;
    if job.kind == "print" {
        let path = {
            let st = s.store.lock().await;
            let f = st.owned(job.file_id.as_deref().ok_or("文件不存在")?, &job.owner)?;
            st.path(&f)
        };
        worker::call(
            &s.root,
            Request {
                operation: "print".into(),
                path: path.to_string_lossy().into(),
                print: job.print.clone(),
                profile: job
                    .print
                    .as_ref()
                    .filter(|p| !p.profile_id.is_empty())
                    .map(|p| crate::profiles::load(&s.root, &p.profile_id, &p.printer))
                    .transpose()?,
                demo: s.demo,
                ..Default::default()
            },
            180,
        )
        .await?;
        Ok(None)
    } else {
        s.store
            .lock()
            .await
            .check_capacity(50 * 1024 * 1024, s.config.max_storage_mb)?;
        let opts = job.scan.clone().ok_or("扫描设置丢失")?;
        let id = uuid::Uuid::new_v4().to_string();
        let path = s.root.join("files").join(format!("{id}.{}", opts.format));
        let pending = crate::store::PendingFile(path.clone());
        worker::call(
            &s.root,
            Request {
                operation: "scan".into(),
                output: path.to_string_lossy().into(),
                scan: Some(opts.clone()),
                demo: s.demo,
                ..Default::default()
            },
            120,
        )
        .await?;
        let bytes = tokio::fs::metadata(&path)
            .await
            .map_err(|e| e.to_string())?
            .len();
        let f = StoredFile {
            id: id.clone(),
            owner: job.owner.clone(),
            name: format!("扫描-{}.{}", now(), opts.format),
            extension: opts.format,
            bytes,
            pages: 1,
            created_at: now(),
            expires_at: now() + s.config.retention_minutes * 60,
            scanned: true,
        };
        s.store
            .lock()
            .await
            .insert_file(f.clone(), s.config.max_storage_mb)?;
        pending.persist();
        Ok(Some(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_app() -> (Shared, crate::store::PendingDirectory) {
        let root = std::env::temp_dir().join(format!("lanprint-api-{}", uuid::Uuid::new_v4()));
        let store = Store::open(root.clone()).unwrap();
        let cleanup = crate::store::PendingDirectory(root.clone());
        (
            Arc::new(App {
                machine: Ok(lan_print::license_format::machine_code("test")),
                activation: Mutex::new(()),
                root,
                store: Mutex::new(store),
                config: Config::default(),
                devices: Mutex::new(None),
                heavy: Arc::new(Semaphore::new(3)),
                office: Semaphore::new(1),
                profile_dialog: Semaphore::new(1),
                network_operation: Arc::new(Semaphore::new(1)),
                notify: Notify::new(),
                demo: true,
            }),
            cleanup,
        )
    }

    #[test]
    fn network_management_rejects_remote_demo_repair_and_concurrent_requests() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (mut state, _cleanup) = test_app();
            let remote: SocketAddr = "192.168.1.20:1234".parse().unwrap();
            let local: SocketAddr = "127.0.0.1:1234".parse().unwrap();
            assert_eq!(
                network_status(State(state.clone()), ConnectInfo(remote))
                    .await
                    .err()
                    .unwrap()
                    .0,
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                network_repair(State(state.clone()), ConnectInfo(remote))
                    .await
                    .err()
                    .unwrap()
                    .0,
                StatusCode::FORBIDDEN
            );
            let demo = network_status(State(state.clone()), ConnectInfo(local))
                .await
                .map_err(|e| e.1)
                .unwrap();
            assert_eq!(demo.0.state, "demo");
            assert!(!demo.0.can_repair);
            assert!(
                network_repair(State(state.clone()), ConnectInfo(local))
                    .await
                    .is_err()
            );
            Arc::get_mut(&mut state).unwrap().demo = false;
            let _busy = state.network_operation.clone().try_acquire_owned().unwrap();
            assert_eq!(
                network_status(State(state.clone()), ConnectInfo(local))
                    .await
                    .err()
                    .unwrap()
                    .0,
                StatusCode::TOO_MANY_REQUESTS
            );
            assert_eq!(
                network_repair(State(state.clone()), ConnectInfo(local))
                    .await
                    .err()
                    .unwrap()
                    .0,
                StatusCode::TOO_MANY_REQUESTS
            );
        });
    }
    #[test]
    fn retries_return_accepted_jobs_after_files_devices_or_capacity_change() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (state, _cleanup) = test_app();
            for kind in ["print", "scan"] {
                let job = Job {
                    id: uuid::Uuid::new_v4().to_string(),
                    owner: "alice".into(),
                    request_id: uuid::Uuid::new_v4().to_string(),
                    kind: kind.into(),
                    name: "accepted".into(),
                    file_id: Some("deleted-file".into()),
                    print: None,
                    scan: None,
                    status: "completed".into(),
                    message: "done".into(),
                    created_at: now(),
                    finished_at: Some(now()),
                };
                state.store.lock().await.enqueue(job.clone(), 1).unwrap();
                let response = if kind == "print" {
                    submit_print(
                        State(state.clone()),
                        Extension("alice".into()),
                        Json(PrintInput {
                            file_id: "deleted-file".into(),
                            request_id: job.request_id.clone(),
                            options: PrintOptions::default(),
                        }),
                    )
                    .await
                } else {
                    // A retry must also bypass changed storage limits.
                    let mut st = state.store.lock().await;
                    for _ in 0..1000 {
                        let id = uuid::Uuid::new_v4().to_string();
                        st.db.files.insert(
                            id.clone(),
                            StoredFile {
                                id,
                                owner: "alice".into(),
                                name: "full".into(),
                                extension: "pdf".into(),
                                bytes: 1,
                                pages: 1,
                                created_at: now(),
                                expires_at: now() + 60,
                                scanned: true,
                            },
                        );
                    }
                    drop(st);
                    submit_scan(
                        State(state.clone()),
                        Extension("alice".into()),
                        Json(ScanInput {
                            request_id: job.request_id.clone(),
                            options: ScanOptions::default(),
                        }),
                    )
                    .await
                }
                .map_err(|e| e.1)
                .unwrap();
                assert_eq!(response.0["id"], job.id);
            }
            assert_eq!(state.store.lock().await.db.jobs.len(), 2);
            assert!(
                state.devices.lock().await.is_none(),
                "Retries must not invoke device discovery"
            );
        });
    }
    #[test]
    fn driver_preset_management_requires_loopback() {
        assert!(require_host("127.0.0.1:1234".parse().unwrap()).is_ok());
        assert!(require_host("[::1]:1234".parse().unwrap()).is_ok());
        assert!(require_host("192.168.1.10:1234".parse().unwrap()).is_err());
    }
    #[test]
    fn signed_session_rejects_tampering() {
        let c = Config::default();
        let id = uuid::Uuid::new_v4().to_string();
        let sig = sign(&c, &id);
        assert!(verify(&c, &id, &sig));
        assert!(!verify(&c, "changed", &sig));
        assert!(!verify(&c, &id, "bad"));
    }
    #[test]
    fn only_private_networks_are_allowed() {
        for ip in ["127.0.0.1", "192.168.1.1", "10.0.0.1", "172.16.0.1", "::1"] {
            assert!(private_ip(ip.parse().unwrap()));
        }
        for ip in ["8.8.8.8", "172.32.0.1", "1.1.1.1"] {
            assert!(!private_ip(ip.parse().unwrap()));
        }
    }
}
