"use strict";
let networkResult = null;
let networkBusy = false;

function updateNetworkButtons() {
  $("network-check").disabled = networkBusy;
  $("network-repair").hidden = !networkResult?.canRepair;
  $("network-repair").disabled = networkBusy || !networkResult?.canRepair;
}

function renderNetwork(result) {
  networkResult = result;
  $("network-card").dataset.state = result.state;
  $("network-status").textContent = result.message;
  $("network-port").textContent =
    `TCP ${result.port} · 主机地址 ${result.address} · ${result.listening ? "服务正在监听" : "未检测到监听"}${result.ruleConfigured ? " · 已配置放行规则" : ""}`;
  $("network-details").replaceChildren();
  for (const profile of result.profiles || []) {
    $("network-details").append(
      text("li", `${profile.name}：${profile.message}`),
    );
  }
  updateNetworkButtons();
}

function networkError(e) {
  networkResult = null;
  $("network-card").dataset.state = "error";
  $("network-status").textContent = e.message || String(e);
  $("network-port").textContent = "请点击重新检查获取最新状态。";
  $("network-details").replaceChildren();
}

async function checkNetwork() {
  if (networkBusy || !session?.localAdmin) return;
  networkBusy = true;
  updateNetworkButtons();
  $("network-status").textContent = "正在检查主机服务端口与网络状态…";
  try {
    renderNetwork(await api("/api/network"));
  } catch (e) {
    networkError(e);
  } finally {
    networkBusy = false;
    updateNetworkButtons();
  }
}

$("network-check").onclick = checkNetwork;
$("network-repair").onclick = async () => {
  if (networkBusy || !session?.localAdmin || !networkResult?.canRepair) return;
  networkBusy = true;
  updateNetworkButtons();
  $("network-status").textContent =
    "请在主机上确认 Windows 管理员授权；修复后会自动重新检查…";
  try {
    const result = await api("/api/network/repair", { method: "POST" });
    renderNetwork(result);
    if (
      result.ruleConfigured &&
      ["allowed", "firewall_disabled"].includes(result.state)
    ) {
      toast("端口放行规则已配置，请从另一台电脑验证访问");
    }
  } catch (e) {
    networkError(e);
  } finally {
    networkBusy = false;
    updateNetworkButtons();
  }
};

function initializeNetworkCheck() {
  $("network-card").hidden = false;
  if (!session.firewallRepair) {
    $("network-hint").textContent = "请在系统防火墙中允许打印站的局域网连接，并从另一台电脑打开共享地址验证。";
  }
  if (!session.localAdmin) {
    $("network-actions").hidden = true;
    $("network-status").textContent = "当前浏览器已连接到打印主机。";
    $("network-hint").textContent =
      "如需检查或修复其他电脑的访问权限，请在打印主机打开 http://127.0.0.1 地址，再使用端口检查与修复。";
    return;
  }
  checkNetwork();
}
