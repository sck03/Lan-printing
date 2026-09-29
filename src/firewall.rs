//! Windows Firewall inspection and a narrowly scoped, UAC-elevated repair.
use super::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use windows::Win32::{NetworkManagement::WindowsFirewall::*, System::Ole::IEnumVARIANT};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkStatus {
    pub port: u16,
    pub address: String,
    pub listening: bool,
    pub state: String,
    pub message: String,
    pub can_repair: bool,
    pub rule_configured: bool,
    pub profiles: Vec<ProfileStatus>,
    pub checked_at: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileStatus {
    name: String,
    firewall_enabled: bool,
    state: String,
    message: String,
}

impl NetworkStatus {
    pub fn demo(port: u16) -> Self {
        Self {
            port,
            address: lan_ip(),
            listening: true,
            state: "demo".into(),
            message: "演示模式不检查或修改真实防火墙；请在正常模式使用端口检查与修复。".into(),
            can_repair: false,
            rule_configured: false,
            profiles: Vec::new(),
            checked_at: now(),
        }
    }
}

struct FirewallApartment;
impl FirewallApartment {
    fn new() -> AppResult<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .map_err(err)?;
        }
        Ok(Self)
    }
}
impl Drop for FirewallApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn executable() -> AppResult<String> {
    Ok(automation_path(
        &std::env::current_exe().map_err(err)?.to_string_lossy(),
    ))
}

fn normalized_program(value: &str) -> String {
    let mut expanded = String::new();
    let mut remaining = value;
    while let Some(start) = remaining.find('%') {
        expanded.push_str(&remaining[..start]);
        let tail = &remaining[start + 1..];
        let Some(end) = tail.find('%') else {
            expanded.push_str(&remaining[start..]);
            return expanded.to_lowercase();
        };
        expanded.push_str(
            &std::env::var(&tail[..end]).unwrap_or_else(|_| format!("%{}%", &tail[..end])),
        );
        remaining = &tail[end + 1..];
    }
    expanded.push_str(remaining);
    automation_path(expanded.trim_matches('"'))
        .replace('/', "\\")
        .to_lowercase()
}

fn any(value: &str) -> bool {
    value.trim().is_empty() || value.trim() == "*"
}

fn port_matches(value: &str, port: u16) -> bool {
    any(value) || value.split(',').any(|part| {
        let part = part.trim();
        if let Some((first, last)) = part.split_once('-') {
            matches!((first.trim().parse::<u16>(), last.trim().parse::<u16>()), (Ok(a), Ok(b)) if a <= port && port <= b)
        } else { part.parse::<u16>() == Ok(port) }
    })
}

// None means an unfamiliar Windows address keyword: never claim it is an allow.
fn local_address_matches(value: &str, address: Ipv4Addr) -> Option<bool> {
    if any(value) {
        return Some(true);
    }
    let mut unknown = false;
    let ip = u32::from(address);
    for item in value.split(',').map(str::trim) {
        let matched = if let Some((network, mask)) = item.split_once('/') {
            let mask = mask
                .parse::<u32>()
                .ok()
                .filter(|n| *n <= 32)
                .map(|n| u32::MAX.checked_shl(32 - n).unwrap_or(0))
                .or_else(|| mask.parse::<Ipv4Addr>().ok().map(u32::from));
            network
                .parse::<Ipv4Addr>()
                .ok()
                .zip(mask)
                .map(|(net, mask)| ip & mask == u32::from(net) & mask)
        } else if let Some((first, last)) = item.split_once('-') {
            first
                .parse::<Ipv4Addr>()
                .ok()
                .zip(last.parse::<Ipv4Addr>().ok())
                .map(|(a, b)| u32::from(a) <= ip && ip <= u32::from(b))
        } else {
            item.parse::<Ipv4Addr>().ok().map(|v| v == address)
        };
        match matched {
            Some(true) => return Some(true),
            Some(false) => {}
            None => unknown = true,
        }
    }
    if unknown { None } else { Some(false) }
}

fn covers_local_subnet(value: &str) -> bool {
    any(value)
        || value.split(',').any(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "localsubnet" | "localsubnet4" | "0.0.0.0/0"
            )
        })
}

#[derive(Default)]
struct RuleCoverage {
    allowed: i32,
    possible_blocks: Vec<(i32, String)>,
}

fn collect_rules(
    policy: &INetFwPolicy2,
    exe: &str,
    port: u16,
    address: Ipv4Addr,
) -> AppResult<RuleCoverage> {
    let mut coverage = RuleCoverage::default();
    unsafe {
        let rules = policy.Rules().map_err(err)?;
        let enumerator: IEnumVARIANT = rules._NewEnum().map_err(err)?.cast().map_err(err)?;
        loop {
            let mut value = Var(VARIANT::default());
            let mut fetched = 0;
            enumerator
                .Next(std::slice::from_mut(&mut value.0), &mut fetched)
                .ok()
                .map_err(err)?;
            if fetched == 0 {
                break;
            }
            let rule: INetFwRule = value.object()?.cast().map_err(err)?;
            if !rule.Enabled().map_err(err)?.as_bool()
                || rule.Direction().map_err(err)? != NET_FW_RULE_DIR_IN
            {
                continue;
            }
            let protocol = rule.Protocol().map_err(err)?;
            if protocol != NET_FW_IP_PROTOCOL_TCP.0 && protocol != NET_FW_IP_PROTOCOL_ANY.0 {
                continue;
            }
            let program = rule.ApplicationName().map_err(err)?.to_string();
            if !any(&program) && normalized_program(&program) != normalized_program(exe) {
                continue;
            }
            // LanPrint runs as a desktop application, never as a Windows service.
            if !rule.ServiceName().map_err(err)?.is_empty() {
                continue;
            }
            if protocol == NET_FW_IP_PROTOCOL_TCP.0
                && !port_matches(&rule.LocalPorts().map_err(err)?.to_string(), port)
            {
                continue;
            }
            let local =
                local_address_matches(&rule.LocalAddresses().map_err(err)?.to_string(), address);
            if local == Some(false) {
                continue;
            }
            let extended: INetFwRule3 = rule.cast().map_err(err)?;
            if !extended.LocalAppPackageId().map_err(err)?.is_empty() {
                continue;
            }
            let profiles = rule.Profiles().map_err(err)?;
            if rule.Action().map_err(err)? == NET_FW_ACTION_BLOCK {
                // Restrictive scopes may affect only some clients: show a possible
                // conflict instead of falsely promising that every LAN PC works.
                coverage
                    .possible_blocks
                    .push((profiles, rule.Name().map_err(err)?.to_string()));
                continue;
            }
            let interfaces = Var(rule.Interfaces().map_err(err)?);
            let no_interface_filter =
                matches!(interfaces.0.Anonymous.Anonymous.vt, VT_EMPTY | VT_NULL);
            if local == Some(true)
                && no_interface_filter
                && rule
                    .InterfaceTypes()
                    .map_err(err)?
                    .to_string()
                    .eq_ignore_ascii_case("All")
                && covers_local_subnet(&rule.RemoteAddresses().map_err(err)?.to_string())
                && (protocol == NET_FW_IP_PROTOCOL_ANY.0
                    || any(&rule.RemotePorts().map_err(err)?.to_string()))
                && extended.LocalUserAuthorizedList().map_err(err)?.is_empty()
                && extended.RemoteUserAuthorizedList().map_err(err)?.is_empty()
                && extended
                    .RemoteMachineAuthorizedList()
                    .map_err(err)?
                    .is_empty()
                && extended.SecureFlags().map_err(err)? == 0
            {
                coverage.allowed |= profiles;
            }
        }
    }
    Ok(coverage)
}

fn profile_status(
    name: &str,
    enabled: bool,
    block_all: bool,
    policy_override: bool,
    default_allow: bool,
    allowed: bool,
    blocks: &[String],
) -> ProfileStatus {
    let (state, message) = if !enabled {
        (
            "disabled",
            "Windows 防火墙当前关闭，未通过它阻止此端口。".into(),
        )
    } else if block_all {
        (
            "blocked",
            "已开启“阻止所有传入连接”，普通放行规则无法生效；请在主机防火墙设置中处理。".into(),
        )
    } else if !blocks.is_empty() {
        (
            "blocked",
            format!(
                "存在可能影响访问的阻止规则：{}。请在主机防火墙高级设置中检查；添加允许规则不能覆盖阻止规则。",
                blocks.join("、")
            ),
        )
    } else if policy_override {
        (
            "managed",
            "本地规则受企业策略覆盖，无法确认放行；请联系网络管理员。".into(),
        )
    } else if default_allow || allowed {
        (
            "allowed",
            "当前网络类型允许本地子网访问此程序的服务端口。".into(),
        )
    } else {
        (
            "needs_rule",
            "未找到覆盖本地子网的有效放行规则，可以点击修复。".into(),
        )
    };
    ProfileStatus {
        name: name.into(),
        firewall_enabled: enabled,
        state: state.into(),
        message,
    }
}

pub fn inspect_network(port: u16) -> AppResult<NetworkStatus> {
    validate_port(port)?;
    let _apartment = FirewallApartment::new()?;
    let address = lan_ip();
    let ip = address.parse::<Ipv4Addr>().map_err(err)?;
    let listening = TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_secs(1),
    )
    .is_ok();
    let exe = executable()?;
    unsafe {
        let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("无法读取 Windows 防火墙，请检查防火墙服务：{e}"))?;
        let active = policy.CurrentProfileTypes().map_err(err)?;
        let modify = policy.LocalPolicyModifyState().map_err(err)?;
        let coverage = collect_rules(&policy, &exe, port, ip)?;
        let mut profiles = Vec::new();
        for (id, name) in [
            (NET_FW_PROFILE2_DOMAIN, "域网络"),
            (NET_FW_PROFILE2_PRIVATE, "专用网络"),
            (NET_FW_PROFILE2_PUBLIC, "公用网络"),
        ] {
            if active & id.0 == 0 {
                continue;
            }
            let blocks = coverage
                .possible_blocks
                .iter()
                .filter(|(mask, _)| mask & id.0 != 0)
                .take(5)
                .map(|(_, name)| name.clone())
                .collect::<Vec<_>>();
            profiles.push(profile_status(
                name,
                policy.get_FirewallEnabled(id).map_err(err)?.as_bool(),
                policy
                    .get_BlockAllInboundTraffic(id)
                    .map_err(err)?
                    .as_bool()
                    || modify == NET_FW_MODIFY_STATE_INBOUND_BLOCKED,
                modify == NET_FW_MODIFY_STATE_GP_OVERRIDE,
                policy.get_DefaultInboundAction(id).map_err(err)? == NET_FW_ACTION_ALLOW,
                coverage.allowed & id.0 != 0,
                &blocks,
            ));
        }
        let rule_configured = active != 0 && coverage.allowed & active == active;
        let (state, message, can_repair) = if !listening {
            (
                "not_listening",
                "未检测到服务端口监听，请重新启动打印站。",
                false,
            )
        } else if ip.is_loopback() || profiles.is_empty() {
            (
                "no_network",
                "未检测到可用的局域网地址，请连接办公室网络后重新检查。",
                false,
            )
        } else if profiles
            .iter()
            .any(|p| p.state == "blocked" || p.state == "managed")
        {
            (
                "blocked",
                "检测到阻止规则或策略限制，请查看下方检查结果。",
                false,
            )
        } else if profiles.iter().any(|p| p.state == "needs_rule") {
            (
                "needs_rule",
                "端口正在监听，但未确认局域网放行，可以点击修复。",
                true,
            )
        } else if profiles.iter().any(|p| p.state == "disabled") {
            (
                "firewall_disabled",
                "端口正在监听；当前部分或全部网络类型的 Windows 防火墙已关闭。",
                !rule_configured && modify == NET_FW_MODIFY_STATE_OK,
            )
        } else {
            (
                "allowed",
                "本机端口和防火墙检查通过，请用另一台电脑打开共享地址验证。",
                false,
            )
        };
        Ok(NetworkStatus {
            port,
            address,
            listening,
            state: state.into(),
            message: message.into(),
            can_repair,
            rule_configured,
            profiles,
            checked_at: now(),
        })
    }
}

fn validate_port(port: u16) -> AppResult<()> {
    if port < 1024 {
        return Err("无效的服务端口。".into());
    }
    Ok(())
}

fn rule_name(exe: &str, port: u16) -> String {
    let identity = hex::encode(Sha256::digest(normalized_program(exe).as_bytes()));
    format!("LanPrint-TCP-{port}-{}", &identity[..16])
}

// Constructing an unattached rule is also used by tests: no firewall mutation.
fn configured_rule(exe: &str, port: u16) -> AppResult<INetFwRule> {
    validate_port(port)?;
    unsafe {
        let rule: INetFwRule =
            CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER).map_err(err)?;
        rule.SetName(&BSTR::from(rule_name(exe, port)))
            .map_err(err)?;
        rule.SetDescription(&BSTR::from(
            "LanPrint: local subnet access to this application and TCP service port only.",
        ))
        .map_err(err)?;
        rule.SetGrouping(&BSTR::from("LanPrint")).map_err(err)?;
        rule.SetApplicationName(&BSTR::from(exe)).map_err(err)?;
        rule.SetProtocol(NET_FW_IP_PROTOCOL_TCP.0).map_err(err)?;
        rule.SetLocalPorts(&BSTR::from(port.to_string()))
            .map_err(err)?;
        rule.SetRemotePorts(&BSTR::from("*")).map_err(err)?;
        rule.SetLocalAddresses(&BSTR::from("*")).map_err(err)?;
        rule.SetRemoteAddresses(&BSTR::from("LocalSubnet"))
            .map_err(err)?;
        rule.SetDirection(NET_FW_RULE_DIR_IN).map_err(err)?;
        rule.SetProfiles(NET_FW_PROFILE2_ALL.0).map_err(err)?;
        rule.SetInterfaceTypes(&BSTR::from("All")).map_err(err)?;
        rule.SetEdgeTraversal(VARIANT_FALSE).map_err(err)?;
        rule.SetAction(NET_FW_ACTION_ALLOW).map_err(err)?;
        rule.SetEnabled(VARIANT_TRUE).map_err(err)?;
        Ok(rule)
    }
}

pub fn repair_network_entry(args: &[String]) -> AppResult<()> {
    let [port] = args else {
        return Err("修复命令只接受一个服务端口。".into());
    };
    let port = port.parse::<u16>().map_err(|_| "无效的服务端口。")?;
    validate_port(port)?;
    let _apartment = FirewallApartment::new()?;
    // Derive the application path from this helper's executable, never from an
    // HTTP body or a user-writable request file consumed with admin rights.
    let exe = executable()?;
    unsafe {
        let policy: INetFwPolicy2 =
            CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER).map_err(err)?;
        if policy.LocalPolicyModifyState().map_err(err)? != NET_FW_MODIFY_STATE_OK {
            return Err("防火墙策略不允许本地修复，请联系主机管理员。".into());
        }
        // Add replaces the same rule identifier, so retries do not accumulate rules.
        policy
            .Rules()
            .map_err(err)?
            .Add(&configured_rule(&exe, port)?)
            .map_err(|e| format!("保存端口放行规则失败：{e}"))?;
    }
    Ok(())
}

pub fn repair_network_elevated(port: u16) -> AppResult<()> {
    validate_port(port)?;
    let _apartment = FirewallApartment::new()?;
    let exe = wide(&executable()?);
    let parameters = wide(&format!("--repair-firewall {port}"));
    unsafe {
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(exe.as_ptr()),
            lpParameters: PCWSTR(parameters.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        ShellExecuteExW(&mut info).map_err(|e| {
            if e.code() == windows::core::HRESULT::from_win32(ERROR_CANCELLED.0) {
                "已取消管理员授权，防火墙设置未修改。".into()
            } else {
                format!("无法启动管理员修复：{e}")
            }
        })?;
        struct Process(HANDLE);
        impl Drop for Process {
            fn drop(&mut self) {
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
        if info.hProcess.is_invalid() {
            return Err("未获得修复进程句柄，请重新检查端口状态。".into());
        }
        let process = Process(info.hProcess);
        if WaitForSingleObject(process.0, 120_000) != WAIT_OBJECT_0 {
            return Err("修复尚未结束，请先完成主机上的管理员提示，再点击重新检查。".into());
        }
        let mut code = 0;
        GetExitCodeProcess(process.0, &mut code).map_err(err)?;
        if code != 0 {
            return Err("端口修复失败，请查看主机错误提示或联系管理员。".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ports_addresses_and_program_identity_are_scoped() {
        assert!(port_matches("80,17800-17900", 17860));
        assert!(!port_matches("17861,80-443", 17860));
        assert!(!port_matches("RPC", 17860));
        assert!(covers_local_subnet("LocalSubnet"));
        assert!(!covers_local_subnet("127.0.0.1"));
        let ip = Ipv4Addr::new(192, 168, 1, 10);
        assert_eq!(local_address_matches("192.168.1.0/24", ip), Some(true));
        assert_eq!(
            local_address_matches("192.168.1.0/255.255.255.0", ip),
            Some(true)
        );
        assert_eq!(
            local_address_matches("127.0.0.1,10.0.0.0/8", ip),
            Some(false)
        );
        assert_eq!(local_address_matches("unknown", ip), None);
        assert_eq!(
            rule_name("C:\\Tools\\LanPrint.exe", 17860),
            rule_name("c:/tools/lanprint.exe", 17860)
        );
        assert_ne!(
            rule_name("C:\\Tools\\LanPrint.exe", 17860),
            rule_name("C:\\Tools\\LanPrint.exe", 17861)
        );
        assert_ne!(
            rule_name("C:\\Tools\\LanPrint.exe", 17860),
            rule_name("C:\\Other\\LanPrint.exe", 17860)
        );
    }
    #[test]
    fn blocks_and_policy_override_take_precedence_over_allow_rules() {
        assert_eq!(
            profile_status("test", true, false, false, false, true, &[]).state,
            "allowed"
        );
        assert_eq!(
            profile_status("test", true, false, false, false, false, &[]).state,
            "needs_rule"
        );
        assert_eq!(
            profile_status("test", true, false, false, true, true, &["block".into()]).state,
            "blocked"
        );
        assert_eq!(
            profile_status("test", true, true, false, true, true, &[]).state,
            "blocked"
        );
        assert_eq!(
            profile_status("test", true, false, true, false, true, &[]).state,
            "managed"
        );
        assert_eq!(
            profile_status("test", false, true, false, false, false, &[]).state,
            "disabled"
        );
    }
    #[test]
    fn repair_rule_is_narrow_without_changing_windows_firewall() {
        let _apartment = FirewallApartment::new().unwrap();
        let exe = executable().unwrap();
        let rule = configured_rule(&exe, 17860).unwrap();
        unsafe {
            assert_eq!(rule.ApplicationName().unwrap().to_string(), exe);
            assert_eq!(rule.LocalPorts().unwrap().to_string(), "17860");
            assert_eq!(
                rule.RemoteAddresses().unwrap().to_string().to_lowercase(),
                "localsubnet"
            );
            assert_eq!(rule.Protocol().unwrap(), NET_FW_IP_PROTOCOL_TCP.0);
            assert_eq!(rule.Profiles().unwrap(), NET_FW_PROFILE2_ALL.0);
            assert_eq!(rule.Direction().unwrap(), NET_FW_RULE_DIR_IN);
            assert_eq!(rule.Action().unwrap(), NET_FW_ACTION_ALLOW);
            assert!(rule.Enabled().unwrap().as_bool());
            assert!(!rule.EdgeTraversal().unwrap().as_bool());
        }
        assert!(repair_network_entry(&[]).is_err());
        assert!(repair_network_entry(&["17860 --extra".into()]).is_err());
        assert!(configured_rule(&exe, 80).is_err());
    }
}
