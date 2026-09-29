"use strict";
let driverProfiles = [],
  profileBusy = false;
async function refreshProfiles() {
  if (!session.driverProfiles) {
    driverProfiles = [];
    $("driver-profiles").hidden = true;
    renderProfiles();
    return;
  }
  driverProfiles = await api("/api/profiles");
  renderProfiles();
}
function renderProfiles() {
  const select = $("driver-profile"),
    previous = select.value;
  select.replaceChildren(new Option("通用设置（使用上方参数）", ""));
  for (const profile of driverProfiles.filter(
    (p) => p.printer === $("printer").value,
  )) {
    select.add(new Option(profile.name, profile.id));
  }
  if ([...select.options].some((o) => o.value === previous))
    select.value = previous;
  profileChanged();
}
function profileChanged() {
  const active = Boolean($("driver-profile").value);
  for (const id of ["paper", "duplex", "print-color", "landscape"])
    $(id).disabled = active;
  $("profile-hint").textContent = active
    ? "纸张尺寸、纸张类型、方向、质量、色彩、双面和高速等选项均使用此驱动预设；上方通用参数不覆盖预设。份数和页码仍可调整。"
    : "照片纸类型、质量和“高速”等原厂选项，可在主机创建预设后使用。";
  $("delete-profile").disabled = !active || profileBusy;
  $("profile-admin").hidden = !session?.driverProfiles || !session?.localAdmin || session?.demo;
  updateSubmit();
}
$("driver-profile").onchange = () => {
  profileChanged();
};
$("refresh-profiles").onclick = () => refreshProfiles().catch(error);
$("create-profile").onclick = async () => {
  if (profileBusy) return;
  const name = $("profile-name").value.trim();
  if (!name || !$("printer").value) {
    error(new Error("请先选择打印机并填写预设名称。"));
    return;
  }
  profileBusy = true;
  $("create-profile").disabled = true;
  $("create-profile").textContent = "请在主机驱动窗口设置并点击“确定”…";
  try {
    const printer = $("printer").value;
    const profile = await api("/api/profiles", {
      method: "POST",
      body: { printer, name },
    });
    await refreshProfiles();
    if ($("printer").value === printer) {
      $("driver-profile").value = profile.id;
      profileChanged();
    }
    $("profile-name").value = "";
    toast("驱动预设已保存，局域网内的打印页面均可选用");
  } catch (e) {
    error(e);
  } finally {
    profileBusy = false;
    $("create-profile").disabled = false;
    $("create-profile").textContent = "打开原厂设置并保存预设";
    profileChanged();
  }
};
$("delete-profile").onclick = async () => {
  const id = $("driver-profile").value;
  if (!id || profileBusy) return;
  if (!confirm("删除此驱动预设？已提交的打印不会被取消。")) return;
  try {
    await api(`/api/profiles/${id}`, { method: "DELETE" });
    await refreshProfiles();
  } catch (e) {
    error(e);
  }
};
