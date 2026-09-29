// Decode the rendered QR pixels independently; no physical printing is used.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");
const jsQR = require(process.env.JSQR_MODULE || "jsqr");
const { startTestServer } = require("./test-server.cjs");
let server, browser;

(async () => {
  server = await startTestServer("qr");
  const { base } = server;
  const output = path.join(__dirname, "../artifacts/qr");
  fs.mkdirSync(output, { recursive: true });
  browser = await chromium.launch({
    headless: true,
    executablePath: process.env.EDGE_EXE ||
      "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  });
  const context = await browser.newContext({
    viewport: { width: 1440, height: 1000 },
    permissions: ["clipboard-read", "clipboard-write"],
  });
  const page = await context.newPage();
  const errors = [], externalRequests = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("request", (request) => {
    if (new URL(request.url()).origin !== base) externalRequests.push(request.url());
  });
  await page.goto(base);
  await page.waitForFunction(() => document.querySelector("#share-qr").naturalWidth > 0);
  const url = await page.locator("#share-url").innerText();
  assert.equal(new URL(url).port, new URL(base).port);
  assert.notEqual(new URL(url).hostname, "127.0.0.1");
  const response = await context.request.get(base + "/api/share-qr.svg");
  assert.equal(response.status(), 200);
  assert.match(response.headers()["content-type"], /^image\/svg\+xml/);
  assert.equal(response.headers()["cache-control"], "no-store");
  const widths = [1440, 1024, 768, 720, 390, 320];
  for (const width of widths) {
    await page.setViewportSize({ width, height: 1000 });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
      `Horizontal overflow at ${width}px`);
    const pixels = await page.locator("#share-qr").evaluate((img) => {
      const canvas = document.createElement("canvas");
      canvas.width = img.clientWidth;
      canvas.height = img.clientHeight;
      const ctx = canvas.getContext("2d");
      ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
      return { width: canvas.width, height: canvas.height,
        data: Array.from(ctx.getImageData(0, 0, canvas.width, canvas.height).data) };
    });
    const decoded = jsQR(Uint8ClampedArray.from(pixels.data), pixels.width, pixels.height);
    assert.equal(decoded?.data, url, `QR must decode to the shared URL at ${width}px`);
    if ([1440, 768, 390].includes(width)) {
      await page.screenshot({ path: path.join(output, `${width}.png`) });
    }
  }
  await page.locator("#copy-url").click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), url);
  assert.deepEqual(externalRequests, []);

  const mobile = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true });
  const mobilePage = await mobile.newPage();
  await mobilePage.goto(url);
  await mobilePage.waitForFunction(() => document.querySelector("#printer").options.length > 0);
  assert.equal(await mobilePage.locator("#print-panel").isVisible(), true);
  assert.equal(await mobilePage.locator("#file-input").count(), 1);
  await mobile.close();

  // An image failure preserves copying and the print UI.
  await context.route("**/api/share-qr.svg", (route) => route.fulfill({ status: 503, body: "Unavailable" }));
  await page.reload();
  await page.waitForFunction(() => document.querySelector("#share-qr-status").textContent.includes("暂不可用"));
  assert.equal(await page.locator("#share-qr").isHidden(), true);
  await page.waitForFunction(() => document.querySelector("#printer").options.length > 0);
  await page.locator("#copy-url").click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), url);
  assert.deepEqual(errors, []);
  fs.writeFileSync(path.join(output, "report.json"), JSON.stringify({
    passed: true, sharedUrl: url, decodedAtWidths: widths,
    lanPageOpened: true, copyVerified: true, failureFallbackVerified: true,
    externalRequests, browserErrors: errors,
  }, null, 2));
  console.log("PASS: QR decoding, LAN URL/port, desktop/mobile layout, copying, offline assets and failure fallback.");
})().catch((error) => {
  console.error(error);
  console.error(server?.logs());
  process.exitCode = 1;
}).finally(async () => {
  try { await browser?.close(); } finally { await server?.stop(); }
});
