"use strict";
const $ = (id) => document.getElementById(id);
let session,
  devices = { printers: [], scanners: [] },
  files = [],
  selected = null,
  page = 0,
  uploadBusy = false,
  printBusy = false,
  scanBusy = false;
let printAttempt = null,
  scanAttempt = null;
const text = (tag, value, className) => {
  const el = document.createElement(tag);
  el.textContent = value;
  if (className) el.className = className;
  return el;
};
function error(e) {
  $("error").textContent = e.message || String(e);
  $("error").hidden = false;
}
function toast(value) {
  $("toast").textContent = value;
  $("toast").hidden = false;
  setTimeout(() => ($("toast").hidden = true), 4500);
}
async function api(path, options = {}) {
  const headers = { ...options.headers };
  if (options.method && options.method !== "GET")
    headers["x-csrf-token"] = session.csrf;
  if (options.body && !(options.body instanceof FormData)) {
    headers["Content-Type"] = "application/json";
    options.body = JSON.stringify(options.body);
  }
  const r = await fetch(path, { ...options, headers });
  const data = await r
    .json()
    .catch(() => ({ error: `请求失败 (${r.status})` }));
  if (data.code === "license_required") location.replace("/register");
  if (!r.ok) throw new Error(data.error || `请求失败 (${r.status})`);
  return data;
}
function uuid() {
  const b = new Uint8Array(16);
  crypto.getRandomValues(b);
  b[6] = (b[6] & 15) | 64;
  b[8] = (b[8] & 63) | 128;
  const s = [...b].map((v) => v.toString(16).padStart(2, "0")).join("");
  return `${s.slice(0, 8)}-${s.slice(8, 12)}-${s.slice(12, 16)}-${s.slice(16, 20)}-${s.slice(20)}`;
}
function tab(name) {
  document
    .querySelectorAll("[data-tab]")
    .forEach((b) => b.classList.toggle("active", b.dataset.tab === name));
  for (const n of ["print", "scan", "jobs"])
    $(`${n}-panel`).hidden = n !== name;
}
document
  .querySelectorAll("[data-tab]")
  .forEach((b) => (b.onclick = () => tab(b.dataset.tab)));
function options(id, items, placeholder) {
  const el = $(id);
  const prior = el.value;
  el.replaceChildren();
  if (!items.length) {
    el.add(new Option(placeholder, ""));
    return;
  }
  for (const i of items) el.add(new Option(i.name, i.id));
  if (items.some((i) => i.id === prior)) el.value = prior;
  else el.value = (items.find((i) => i.is_default) || items[0]).id;
}
function printerChanged() {
  const p = devices.printers.find((p) => p.id === $("printer").value);
  options(
    "paper",
    (p?.papers || []).map((p) => ({ id: p, name: p })),
    "无可用纸张",
  );
  for (const option of $("duplex").options)
    option.disabled = option.value !== "simplex" && !p?.duplex;
  if (!p?.duplex) $("duplex").value = "simplex";
  $("print-color").options[1].disabled = !p?.color;
  if (!p?.color) $("print-color").value = "false";
  renderProfiles();
  updateSubmit();
}
function updateSubmit() {
  $("print-submit").disabled =
    printBusy ||
    !selected ||
    !$("printer").value ||
    (!$("driver-profile").value && !$("paper").value);
  $("scan-submit").disabled = scanBusy || !$("scanner").value;
}
async function refreshDevices() {
  devices = await api("/api/devices");
  options("printer", devices.printers, "未找到打印机");
  options("scanner", devices.scanners, "未找到 WIA 扫描仪");
  printerChanged();
  const warnings = [...(devices.warnings || [])];
  if (!devices.printers.length)
    warnings.push("未找到可用打印机，请在主机安装驱动并确认设备连接。");
  $("notice").textContent = warnings.join(" ");
  $("notice").hidden = !warnings.length;
}
$("refresh-devices").onclick = () => refreshDevices().catch(error);
$("printer").onchange = printerChanged;
function renderFiles() {
  $("files").replaceChildren();
  for (const f of files) {
    const row = text(
      "div",
      "",
      "file-row" + (selected?.id === f.id ? " selected" : ""),
    );
    const b = text("button", "", "file-select");
    b.append(
      text("strong", f.name),
      text(
        "small",
        `${f.pages} 页 · ${(f.bytes / 1024 / 1024).toFixed(2)} MB${f.scanned ? " · 扫描件" : ""}`,
      ),
    );
    b.onclick = () => selectFile(f);
    const del = text("button", "删除", "subtle");
    del.onclick = async () => {
      try {
        await api(`/api/files/${f.id}`, { method: "DELETE" });
        if (selected?.id === f.id) selectFile(null);
        await refreshFiles();
      } catch (e) {
        error(e);
      }
    };
    row.append(b, del);
    $("files").append(row);
  }
}
async function refreshFiles() {
  const updated = await api("/api/files");
  if (JSON.stringify(files) === JSON.stringify(updated)) return;
  files = updated;
  if (selected && !files.some((f) => f.id === selected.id)) selectFile(null);
  renderFiles();
}
function selectFile(f) {
  selected = f;
  page = 0;
  $("selected-summary").textContent = f
    ? `${f.name} · 共 ${f.pages} 页`
    : "请先选择一个文件";
  $("preview-card").hidden = !f;
  renderFiles();
  updateSubmit();
  if (f) preview();
}
function preview() {
  if (!selected) return;
  $("preview").src = `/api/files/${selected.id}/preview?page=${page}`;
  $("page-label").textContent = ` ${page + 1} / ${selected.pages} `;
  $("prev-page").disabled = page === 0;
  $("next-page").disabled = page + 1 >= selected.pages;
}
$("preview").onerror = () =>
  error(new Error("预览加载失败，请重新选择文件；若文件已过期请重新上传。"));
$("prev-page").onclick = () => {
  if (page > 0) {
    page--;
    preview();
  }
};
$("next-page").onclick = () => {
  if (selected && page + 1 < selected.pages) {
    page++;
    preview();
  }
};
async function upload(file) {
  if (!file || uploadBusy || !session) return;
  if (file.size > session.maxUploadMb * 1024 * 1024) {
    error(new Error(`单文件不能超过 ${session.maxUploadMb} MB`));
    return;
  }
  uploadBusy = true;
  $("file-input").disabled = true;
  $("upload-hint").textContent = /\.(docx?|xlsx?|pptx?|wps|et|dps)$/i.test(
    file.name,
  )
    ? "正在上传并转换文档，最多约 2 分钟…"
    : "正在上传并检查文件…";
  try {
    const body = new FormData();
    body.append("file", file);
    const f = await api("/api/files", { method: "POST", body });
    await refreshFiles();
    selectFile(f);
    toast("文件已准备好，请确认打印设置");
  } catch (e) {
    error(e);
  } finally {
    uploadBusy = false;
    $("file-input").disabled = false;
    $("file-input").value = "";
    $("upload-hint").textContent =
      `支持 PDF、图片、Word、Excel、PPT 及 WPS 文档 · 最大 ${session.maxUploadMb} MB`;
  }
}
$("file-input").onchange = (e) => upload(e.target.files[0]);
for (const name of ["dragenter", "dragover"])
  $("dropzone").addEventListener(name, (e) => {
    e.preventDefault();
    $("dropzone").classList.add("drag");
  });
for (const name of ["dragleave", "drop"])
  $("dropzone").addEventListener(name, (e) => {
    e.preventDefault();
    $("dropzone").classList.remove("drag");
  });
$("dropzone").addEventListener("drop", (e) => upload(e.dataTransfer.files[0]));
function attempt(previous, payload) {
  const key = JSON.stringify(payload);
  return previous?.key === key ? previous : { key, id: uuid() };
}
$("print-form").onsubmit = async (e) => {
  e.preventDefault();
  if (printBusy || !selected) return;
  printBusy = true;
  updateSubmit();
  try {
    const payload = {
      file_id: selected.id,
      options: {
        printer: $("printer").value,
        copies: Number($("copies").value),
        duplex: $("duplex").value,
        paper: $("paper").value,
        landscape: $("landscape").value === "true",
        color: $("print-color").value === "true",
        pages: $("pages").value,
        profile_id: $("driver-profile").value,
        render_dpi: Number($("render-dpi").value),
      },
    };
    printAttempt = attempt(printAttempt, payload);
    await api("/api/print", {
      method: "POST",
      body: { ...payload, request_id: printAttempt.id },
    });
    printAttempt = null;
    toast(session.demo ? "演示打印任务已加入队列" : "打印任务已加入队列");
    tab("jobs");
    await refreshJobs();
  } catch (e) {
    error(e);
  } finally {
    printBusy = false;
    updateSubmit();
  }
};
$("scan-form").onsubmit = async (e) => {
  e.preventDefault();
  if (scanBusy) return;
  scanBusy = true;
  updateSubmit();
  try {
    const payload = {
      options: {
        scanner: $("scanner").value,
        dpi: Number($("dpi").value),
        color: $("scan-color").value === "true",
        format: $("format").value,
        source: $("source").value,
      },
    };
    scanAttempt = attempt(scanAttempt, payload);
    await api("/api/scan", {
      method: "POST",
      body: { ...payload, request_id: scanAttempt.id },
    });
    scanAttempt = null;
    tab("jobs");
    toast("扫描任务已加入队列，请勿移动原稿");
    await refreshJobs();
  } catch (e) {
    error(e);
  } finally {
    scanBusy = false;
    updateSubmit();
  }
};
const statuses = {
  queued: "排队中",
  running: "处理中",
  submitted: "已提交打印机",
  completed: "已完成",
  simulated: "演示完成",
  failed: "失败",
  cancelled: "已取消",
  interrupted: "已中断",
};
let jobsSnapshot = "";
async function refreshJobs() {
  const jobs = await api("/api/jobs");
  const snapshot = JSON.stringify([jobs, files.map((f) => f.id)]);
  if (snapshot === jobsSnapshot) return;
  jobsSnapshot = snapshot;
  $("job-count").textContent =
    jobs.filter((j) => ["queued", "running"].includes(j.status)).length || "";
  $("jobs").replaceChildren();
  if (!jobs.length)
    $("jobs").append(
      text("div", "还没有任务。上传文件或放好原稿，开始第一次使用。", "empty"),
    );
  for (const j of jobs) {
    const row = text("div", "", "job");
    const info = text("div", "", "job-info");
    info.append(
      text("strong", `${j.kind === "scan" ? "扫描" : "打印"} · ${j.name}`),
      text(
        "p",
        `${new Date(j.createdAt * 1000).toLocaleTimeString()} · ${j.message}`,
      ),
    );
    row.append(
      info,
      text("span", statuses[j.status] || j.status, `badge ${j.status}`),
    );
    if (j.status === "queued") {
      const cancel = text("button", "取消", "subtle");
      cancel.onclick = async () => {
        try {
          await api(`/api/jobs/${j.id}/cancel`, { method: "POST" });
          await refreshJobs();
        } catch (e) {
          error(e);
        }
      };
      row.append(cancel);
    }
    if (j.kind === "scan" && j.status === "completed" && j.fileId) {
      if (files.some((f) => f.id === j.fileId)) {
        const a = text("a", "下载文件");
        a.href = `/api/files/${j.fileId}/download`;
        row.append(a);
      } else row.append(text("span", "文件已过期", "muted"));
    }
    $("jobs").append(row);
  }
}
function initializeShareQr() {
  const image = $("share-qr");
  const status = $("share-qr-status");
  image.onload = () => {
    status.hidden = true;
    image.hidden = false;
  };
  image.onerror = () => {
    image.hidden = true;
    status.hidden = false;
    status.textContent = "二维码暂不可用，请复制地址或刷新重试";
  };
  image.src = "/api/share-qr.svg";
}

$("copy-url").onclick = async () => {
  try {
    if (navigator.clipboard && window.isSecureContext)
      await navigator.clipboard.writeText(session.shareUrl);
    else {
      const range = document.createRange();
      range.selectNodeContents($("share-url"));
      const selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      if (!document.execCommand("copy"))
        throw new Error("请选中并复制上方地址");
      selection.removeAllRanges();
    }
    toast("地址已复制");
  } catch (e) {
    error(e);
  }
};
async function poll() {
  try {
    await refreshFiles();
    await refreshJobs();
    $("connection").textContent = session.demo
      ? "● 演示模式"
      : "● 已连接办公室主机";
  } catch (e) {
    $("connection").textContent = "连接中断 · 正在重试";
  }
  setTimeout(poll, 3000);
}
document.addEventListener("DOMContentLoaded", async () => {
  try {
    session = await api("/api/session");
    initializeNetworkCheck();
    $("office-hint").textContent = session.officeFormats?.length
      ? `主机可转换：${session.officeFormats.map((x) => x.toUpperCase()).join("、")}。转换后预览并打印；表格沿用文件内的打印区域和分页。`
      : "主机未检测到办公文档转换工具。Windows 请安装 Office/WPS；macOS、Linux 请安装 LibreOffice，或先导出 PDF。";
    $("share-url").textContent = session.shareUrl;
    initializeShareQr();
    $("retention").textContent = session.retentionMinutes;
    $("upload-hint").textContent =
      `支持 PDF、图片${session.officeFormats?.length ? "、" + session.officeFormats.map((x) => x.toUpperCase()).join("、") : ""} · 最大 ${session.maxUploadMb} MB`;
    await refreshProfiles();
    await refreshDevices();
    await poll();
  } catch (e) {
    $("connection").textContent = "连接失败，请刷新";
    error(e);
  }
});
