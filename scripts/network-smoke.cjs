// Real read-only firewall inspection; all repair/UAC outcomes are HTTP fixtures.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");
const { startTestServer } = require("./test-server.cjs");
let server, browser;

(async () => {
  server = await startTestServer("network", () => {}, { demo: false });
  const { base } = server;
  const response = await fetch(base + "/api/network");
  const observed = await response.json();
  assert.equal(response.status, 200, JSON.stringify(observed));
  assert.equal(observed.port, Number(new URL(base).port));
  assert.equal(observed.listening, true);
  assert.equal(typeof observed.canRepair, "boolean");
  assert.notEqual(observed.state, "demo");
  console.log("Native read-only inspection:", JSON.stringify(observed));
  // Missing CSRF must fail before any elevated helper can start.
  assert.equal(
    (await fetch(base + "/api/network/repair", { method: "POST" })).status,
    403,
  );

  browser = await chromium.launch({
    headless: true,
    executablePath:
      process.env.EDGE_EXE ||
      "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  });
  const context = await browser.newContext({
    viewport: { width: 1440, height: 1050 },
  });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  // Prevent driver enumeration; this suite only checks the network card.
  await context.route("**/api/devices", (route) =>
    route.fulfill({ json: { printers: [], scanners: [], warnings: [] } }),
  );
  let status = {
    ...observed,
    state: "needs_rule",
    message: "未找到覆盖本地子网的有效放行规则，可以点击修复。",
    canRepair: true,
    ruleConfigured: false,
    profiles: [
      {
        name: "公用网络",
        firewallEnabled: true,
        state: "needs_rule",
        message: "缺少本地子网放行规则。",
      },
    ],
  };
  await context.route("**/api/network", (route) =>
    route.fulfill({ json: status }),
  );
  let repairCount = 0,
    repairError = null,
    releaseRepair;
  let repairGate = new Promise((resolve) => {
    releaseRepair = resolve;
  });
  await context.route("**/api/network/repair", async (route) => {
    repairCount++;
    await repairGate;
    if (repairError)
      return route.fulfill({ status: 400, json: { error: repairError } });
    status = {
      ...status,
      state: "allowed",
      message: "本机端口和防火墙检查通过，请用另一台电脑打开共享地址验证。",
      canRepair: false,
      ruleConfigured: true,
      profiles: [
        {
          name: "公用网络",
          firewallEnabled: true,
          state: "allowed",
          message: "本地子网访问已放行。",
        },
      ],
    };
    await route.fulfill({ json: status });
  });
  const output = path.join(__dirname, "../artifacts");
  fs.mkdirSync(output, { recursive: true });
  await page.goto(base);
  await page.waitForFunction(
    () =>
      !document.querySelector("#network-repair").hidden &&
      !document.querySelector("#network-repair").disabled,
  );
  await page
    .locator("#network-card")
    .screenshot({ path: path.join(output, "network-repair-desktop.png") });
  await page.setViewportSize({ width: 390, height: 844 });
  assert.ok(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  );
  await page
    .locator("#network-card")
    .screenshot({ path: path.join(output, "network-repair-mobile.png") });
  await page.setViewportSize({ width: 1440, height: 1050 });
  await page.locator("#network-repair").click();
  assert.equal(await page.locator("#network-repair").isDisabled(), true);
  assert.equal(await page.locator("#network-check").isDisabled(), true);
  await page.waitForFunction(() =>
    document
      .querySelector("#network-status")
      .textContent.includes("管理员授权"),
  );
  releaseRepair();
  await page.waitForFunction(
    () => document.querySelector("#network-card").dataset.state === "allowed",
  );
  assert.equal(repairCount, 1);
  assert.equal(await page.locator("#network-repair").isHidden(), true);
  assert.ok(
    (await page.locator("#network-port").innerText()).includes(
      "已配置放行规则",
    ),
  );

  // Denying UAC is a visible, recoverable error; it must not look like success.
  status = {
    ...status,
    state: "needs_rule",
    canRepair: true,
    ruleConfigured: false,
  };
  repairError = "已取消管理员授权，防火墙设置未修改。";
  repairGate = Promise.resolve();
  await page.locator("#network-check").click();
  await page.waitForFunction(
    () => !document.querySelector("#network-repair").hidden,
  );
  await page.locator("#network-repair").click();
  await page.waitForFunction(
    () => document.querySelector("#network-card").dataset.state === "error",
  );
  assert.ok(
    (await page.locator("#network-status").innerText()).includes(
      "已取消管理员授权",
    ),
  );
  assert.equal(await page.locator("#network-check").isDisabled(), false);

  status = {
    ...status,
    state: "blocked",
    message: "检测到阻止规则或策略限制，请查看下方检查结果。",
    canRepair: false,
    profiles: [
      {
        name: "公用网络",
        firewallEnabled: true,
        state: "blocked",
        message: "企业策略阻止访问，需要管理员处理。",
      },
    ],
  };
  await page.locator("#network-check").click();
  await page.waitForFunction(
    () => document.querySelector("#network-card").dataset.state === "blocked",
  );
  assert.equal(await page.locator("#network-repair").isHidden(), true);
  await page
    .locator("#network-card")
    .screenshot({ path: path.join(output, "network-desktop.png") });
  await page.setViewportSize({ width: 390, height: 844 });
  assert.ok(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  );
  await page
    .locator("#network-card")
    .screenshot({ path: path.join(output, "network-mobile.png") });

  // Remote clients cannot trigger inspection or UAC through the UI.
  const remote = await browser.newContext();
  await remote.route("**/api/session", async (route) => {
    const session = await (await route.fetch()).json();
    await route.fulfill({ json: { ...session, localAdmin: false } });
  });
  await remote.route("**/api/devices", (route) =>
    route.fulfill({ json: { printers: [], scanners: [], warnings: [] } }),
  );
  let remoteChecks = 0;
  await remote.route("**/api/network", (route) => {
    remoteChecks++;
    return route.fulfill({ json: observed });
  });
  const remotePage = await remote.newPage();
  await remotePage.goto(base);
  await remotePage.waitForFunction(() =>
    document
      .querySelector("#network-status")
      .textContent.includes("当前浏览器已连接"),
  );
  assert.equal(await remotePage.locator("#network-actions").isHidden(), true);
  assert.equal(remoteChecks, 0);
  assert.deepEqual(errors, []);
  fs.writeFileSync(
    path.join(output, "network-report.json"),
    JSON.stringify(
      {
        inspection: observed,
        uiTestsPassed: true,
        repairResponsesSimulated: true,
        firewallModified: false,
      },
      null,
      2,
    ),
  );
  console.log(
    "PASS: native firewall/port inspection, CSRF, repair UI, UAC cancellation, blocked policy, remote UI, desktop/mobile layout; no firewall settings changed.",
  );
})()
  .catch((error) => {
    console.error(error);
    console.error(server?.logs());
    process.exitCode = 1;
  })
  .finally(async () => {
    try {
      await browser?.close();
    } finally {
      await server?.stop();
    }
  })
  .catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
