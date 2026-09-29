(() => {
  const $ = (id) => document.getElementById(id);
  let csrf = "";
  const error = (message) => {
    $("activation-error").textContent = message;
    $("activation-error").hidden = !message;
  };
  async function request(url, options = {}) {
    const response = await fetch(url, { ...options, cache: "no-store" });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || "请求失败，请稍后重试。");
    return result;
  }
  async function refresh() {
    error("");
    try {
      const session = await request("/api/session");
      csrf = session.csrf;
      const state = await request("/api/license");
      if (state.registered && location.pathname !== "/register") {
        location.replace("/");
        return;
      }
      $("activation-badge").textContent = state.registered ? "已注册" : "等待注册";
      $("activation-status").textContent = state.message;
      $("activation-local").hidden = !state.localAdmin || state.registered;
      $("activation-remote").hidden = state.localAdmin || state.registered;
      $("activation-success").hidden = !state.registered;
      $("licensed-customer").textContent = state.customer ? `授权给：${state.customer} · 永久有效` : "此打印站已注册。";
      $("machine-code").value = state.machineCode || "无法读取机器码";
      $("activate-submit").disabled = !state.machineCode;
      $("copy-machine").disabled = !state.machineCode;
    } catch (e) { error(e.message); }
  }
  $("copy-machine").addEventListener("click", async () => {
    const field = $("machine-code");
    field.focus();
    field.select();
    try {
      if (navigator.clipboard?.writeText) await navigator.clipboard.writeText(field.value);
      else if (!document.execCommand("copy")) throw new Error("copy");
      $("activation-status").textContent = "机器码已复制，请发送给软件提供方。";
    } catch { $("activation-status").textContent = "机器码已选中，请按 Ctrl+C 复制。"; }
  });
  $("activation-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    error("");
    const button = $("activate-submit");
    button.disabled = true;
    button.textContent = "正在验证注册码…";
    try {
      // Refresh the signed session when another browser tab has changed cookies.
      csrf = (await request("/api/session")).csrf;
      await request("/api/license", {
        method: "POST",
        headers: { "Content-Type": "application/json", "x-csrf-token": csrf },
        body: JSON.stringify({ code: $("registration-code").value.trim() }),
      });
      location.replace("/");
    } catch (e) {
      error(e.message);
      button.disabled = false;
      button.textContent = "注册并进入打印站 →";
    }
  });
  $("refresh-activation").addEventListener("click", refresh);
  refresh();
})();
