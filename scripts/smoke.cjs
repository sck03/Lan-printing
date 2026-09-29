// Development-only end-to-end checks. No real printer or scanner is used.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { startTestServer } = require("./test-server.cjs");
const { randomUUID } = require("node:crypto");
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");
const root = path.resolve(__dirname, "..");
const output = path.join(root, "artifacts");
fs.mkdirSync(output, { recursive: true });
// Demo-only preset fixture; no real driver settings or physical printing.
const profileId = randomUUID();
let browser, server;
(async () => {
  server = await startTestServer("smoke", (data) => {
    fs.mkdirSync(path.join(data, "profiles"));
    fs.writeFileSync(
      path.join(data, "profiles", profileId + ".json"),
      JSON.stringify({
        id: profileId,
        name: "照片纸 · 高质量 · 关闭高速",
        printer: "demo-printer",
        driver: "demo-fixture",
        devmode: [],
      }),
    );
  });
  const { base, data } = server;
  browser = await chromium.launch({
    headless: true,
    executablePath:
      process.env.EDGE_EXE ||
      "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  });
  const a = await browser.newContext({
    viewport: { width: 1440, height: 1050 },
  });
  const page = await a.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(base);
  await page.waitForFunction(() =>
    document.querySelector("#connection").textContent.includes("演示模式"),
  );
  const fixture = path.join(output, "sample.png");
  await page.screenshot({ path: fixture });
  await page.locator("#file-input").setInputFiles(fixture);
  await page.waitForFunction(
    () => !document.querySelector("#print-submit").disabled,
  );
  await page.waitForFunction(
    () => document.querySelector("#preview").naturalWidth > 0,
  );
  await page.locator("#driver-profile").selectOption(profileId);
  for (const id of ["paper", "duplex", "print-color", "landscape"])
    assert.equal(await page.locator("#" + id).isDisabled(), true);
  await page.locator("#render-dpi").selectOption("600");
  await page.screenshot({
    path: path.join(output, "desktop.png"),
    fullPage: true,
  });
  await page.locator("#print-submit").click();
  await page.waitForFunction(() =>
    document.querySelector("#jobs").textContent.includes("演示完成"),
  );
  await page.locator("[data-tab=scan]").click();
  await page.locator("#scan-submit").click();
  await page.waitForFunction(() => document.querySelector("#jobs a") !== null);
  const download = await a.request.get(
    base + (await page.locator("#jobs a").first().getAttribute("href")),
  );
  assert.equal(download.status(), 200);
  const pdf = await download.body();
  assert.equal(pdf.subarray(0, 5).toString(), "%PDF-");
  fs.writeFileSync(path.join(output, "scan.pdf"), pdf);
  const session = await (await a.request.get(base + "/api/session")).json();
  const headers = { "x-csrf-token": session.csrf };
  assert.equal(session.localAdmin, true);
  const presets = await (await a.request.get(base + "/api/profiles")).json();
  assert.equal(presets[0].id, profileId);
  assert.equal(presets[0].devmode, undefined);
  const files = await (await a.request.get(base + "/api/files")).json();
  assert.equal(files.length, 2);
  const scan = files.find((f) => f.scanned);
  const previewResponse = await a.request.get(
    `${base}/api/files/${scan.id}/preview?page=0`,
  );
  assert.equal(previewResponse.status(), 200, await previewResponse.text());
  const b = await browser.newContext();
  assert.deepEqual(await (await b.request.get(base + "/api/files")).json(), []);
  assert.equal(
    (await b.request.get(`${base}/api/files/${scan.id}/download`)).status(),
    400,
  );
  assert.equal(
    (await b.request.post(base + "/api/scan", { data: {} })).status(),
    403,
  );
  assert.equal(
    (
      await a.request.post(base + "/api/scan", {
        headers: { ...headers, Origin: "https://evil.example" },
        data: {},
      })
    ).status(),
    403,
  );
  assert.equal(
    (
      await a.request.get(base + "/api/session", {
        headers: { Host: "evil.example" },
      })
    ).status(),
    403,
  );
  const request = {
    request_id: randomUUID(),
    file_id: files.find((f) => !f.scanned).id,
    options: { printer: "demo-printer" },
  };
  const j1 = await (
    await a.request.post(base + "/api/print", { headers, data: request })
  ).json();
  const j2 = await (
    await a.request.post(base + "/api/print", { headers, data: request })
  ).json();
  assert.ok(j1.id);
  assert.equal(j1.id, j2.id);
  const queued = await (
    await a.request.post(base + "/api/scan", {
      headers,
      data: { request_id: randomUUID(), options: { scanner: "demo-scanner" } },
    })
  ).json();
  assert.ok(queued.id);
  assert.equal(
    (
      await a.request.post(`${base}/api/jobs/${queued.id}/cancel`, { headers })
    ).status(),
    200,
  );
  const jobs = await (await a.request.get(base + "/api/jobs")).json();
  assert.equal(jobs.find((j) => j.id === queued.id).status, "cancelled");
  const bad = await a.request.post(base + "/api/files", {
    headers,
    multipart: {
      file: {
        name: "fake.pdf",
        mimeType: "application/pdf",
        buffer: Buffer.from("not a PDF"),
      },
    },
  });
  assert.equal(bad.status(), 400);
  await page.locator("[data-tab=print]").click();
  await page.locator(".file-select").filter({ hasText: "扫描-" }).click();
  await page.waitForFunction(
    () => document.querySelector("#preview").naturalWidth > 0,
  );
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({
    path: path.join(output, "mobile.png"),
    fullPage: true,
  });
  assert.ok(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  );
  assert.deepEqual(errors, []);
  assert.equal(
    (
      await a.request.delete(base + "/api/profiles/" + profileId, { headers })
    ).status(),
    200,
  );
  const missingPreset = await a.request.post(base + "/api/print", {
    headers,
    data: {
      request_id: randomUUID(),
      file_id: files.find((f) => !f.scanned).id,
      options: { printer: "demo-printer", profile_id: profileId },
    },
  });
  assert.equal(missingPreset.status(), 400);
  await page.locator("#refresh-profiles").click();
  await page.waitForFunction(
    () => document.querySelector("#driver-profile").value === "",
  );
  assert.equal(await page.locator("#paper").isDisabled(), false);
  assert.equal(
    Number(download.headers()["content-length"]),
    pdf.length,
    "Streamed download length",
  );
  const conflicting = await a.request.post(base + "/api/scan", {
    headers,
    data: {
      request_id: request.request_id,
      options: { scanner: "demo-scanner" },
    },
  });
  assert.equal(
    conflicting.status(),
    400,
    "A print request ID cannot be reused for scanning",
  );
  for (let i = 0; i < 100; i++) {
    const current = await (await a.request.get(base + "/api/jobs")).json();
    if (current.every((j) => !["queued", "running"].includes(j.status))) break;
    if (i === 99) throw new Error("Queue did not drain");
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert.equal(
    (
      await a.request.delete(`${base}/api/files/${request.file_id}`, {
        headers,
      })
    ).status(),
    200,
  );
  const retried = await a.request.post(base + "/api/print", {
    headers,
    data: request,
  });
  assert.equal(retried.status(), 200);
  assert.equal(
    (await retried.json()).id,
    j1.id,
    "Retry still finds the job after its file is deleted",
  );
  assert.deepEqual(
    fs.readdirSync(path.join(data, "work")),
    [],
    "Finished and failed workers must leave no scratch files",
  );
  const controller = new AbortController();
  const cookie = (await a.cookies())
    .map((c) => `${c.name}=${c.value}`)
    .join("; ");
  const abandoned = fetch(`${base}/api/files/${scan.id}/preview?page=0`, {
    headers: { cookie },
    signal: controller.signal,
  }).then(
    async (response) => {
      await response.arrayBuffer();
      return "completed";
    },
    (error) => error.name,
  );
  let workerStarted = false;
  for (let i = 0; i < 200; i++) {
    if (
      fs
        .readdirSync(path.join(data, "work"), { withFileTypes: true })
        .some((entry) => entry.isDirectory())
    ) {
      workerStarted = true;
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  controller.abort();
  assert.equal(await abandoned, "AbortError");
  assert.ok(
    workerStarted,
    "Cancellation must interrupt a started preview worker",
  );
  for (let i = 0; i < 200; i++) {
    if (fs.readdirSync(path.join(data, "work")).length === 0) break;
    if (i === 199)
      throw new Error("Cancelled preview left scratch files behind");
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  console.log(
    "PASS: upload, image/PDF preview, demo print, streamed PDF download, session isolation, CSRF/Host validation, retries after file deletion, cross-operation request ID rejection, cancellation, corrupt PDF rejection, worker cleanup, driver presets, 600 DPI option, mobile layout; no browser errors.",
  );
})()
  .catch((e) => {
    console.error(e);
    console.error(server?.logs());
    process.exitCode = 1;
  })
  .finally(async () => {
    try {
      if (browser) await browser.close();
    } finally {
      await server?.stop();
    }
  })
  .catch((e) => {
    console.error(e);
    process.exitCode = 1;
  });
