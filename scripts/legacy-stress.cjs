// Maintainer-only integration checks; issuer keys stay in the private directory.
// Uses a fresh data directory and simulated devices, never physical printing.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const http = require("node:http");
const net = require("node:net");
const { randomUUID, createHash } = require("node:crypto");
const { execFileSync } = require("node:child_process");
const { setTimeout: delay } = require("node:timers/promises");
const { startTestServer } = require("./test-server.cjs");
let server;

function pdfFixture(file, bytes) {
  const fd = fs.openSync(file, "wx");
  try {
    fs.ftruncateSync(fd, bytes);
    fs.writeSync(fd, "%PDF-1.4\n", 0, "ascii");
    let position = bytes - 2048;
    const offsets = [0];
    const objects = [
      "<< /Type /Catalog /Pages 2 0 R >>",
      "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
      "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] >>",
    ];
    for (let i = 0; i < objects.length; i++) {
      offsets.push(position);
      const object = `${i + 1} 0 obj\n${objects[i]}\nendobj\n`;
      fs.writeSync(fd, object, position, "ascii");
      position += object.length;
    }
    const xref = position;
    const table = `xref\n0 4\n0000000000 65535 f \n${offsets.slice(1).map((offset) => `${String(offset).padStart(10, "0")} 00000 n \n`).join("")}trailer\n<< /Size 4 /Root 1 0 R >>\n`;
    fs.writeSync(fd, table, position, "ascii");
    const end = `startxref\n${xref}\n%%EOF\n`;
    assert.ok(position + table.length < bytes - end.length);
    fs.writeSync(fd, end, bytes - end.length, "ascii");
  } finally { fs.closeSync(fd); }
}

function streamUpload(base, cookie, csrf, file, chunked = false) {
  return new Promise((resolve, reject) => {
    const boundary = "LanPrint-" + randomUUID();
    const prefix = Buffer.from(`--${boundary}\r\nContent-Disposition: form-data; name="file"; filename="大文件.pdf"\r\nContent-Type: application/pdf\r\n\r\n`);
    const suffix = Buffer.from(`\r\n--${boundary}--\r\n`);
    const req = http.request(base + "/api/files", {
      method: "POST",
      headers: {
        Cookie: cookie, "x-csrf-token": csrf, "Content-Type": "multipart/form-data; boundary=" + boundary,
        ...(chunked ? { "Transfer-Encoding": "chunked" } : { "Content-Length": prefix.length + fs.statSync(file).size + suffix.length }),
      },
    }, (res) => {
      let text = "";
      res.setEncoding("utf8"); res.on("data", (part) => { text += part; });
      res.on("end", () => { try { resolve({ status: res.statusCode, data: JSON.parse(text) }); } catch (error) { reject(error); } });
    });
    req.on("error", reject);
    req.setTimeout(120000, () => req.destroy(new Error("Upload timed out")));
    req.write(prefix);
    const input = fs.createReadStream(file);
    input.on("error", reject);
    input.pipe(req, { end: false }); input.on("end", () => req.end(suffix));
  });
}

async function raw(base, request) {
  return new Promise((resolve, reject) => {
    const url = new URL(base);
    const socket = net.connect(Number(url.port), "127.0.0.1");
    let text = "";
    socket.setTimeout(5000, () => socket.destroy(new Error("Raw request timed out")));
    socket.on("connect", () => socket.end(request));
    socket.on("data", (data) => { text += data; });
    socket.on("error", reject); socket.on("end", () => resolve(text));
  });
}

(async () => {
  server = await startTestServer("legacy-stress");
  const { base, data } = server;
  const first = await fetch(base + "/api/session");
  const cookie = first.headers.get("set-cookie").split(";")[0];
  const session = await first.json();
  assert.ok(session.legacy); assert.equal(session.maxUploadMb, 100);
  const headers = { Cookie: cookie, "x-csrf-token": session.csrf, "Content-Type": "application/json" };
  const large = path.join(data, "large.pdf"), small = path.join(data, "small.pdf");
  pdfFixture(large, 100 * 1024 * 1024); pdfFixture(small, 4096);
  const uploaded = await streamUpload(base, cookie, session.csrf, large);
  assert.equal(uploaded.status, 200, JSON.stringify(uploaded.data));
  assert.equal(uploaded.data.bytes, 100 * 1024 * 1024); assert.equal(uploaded.data.pages, 1);
  const download = await fetch(`${base}/api/files/${uploaded.data.id}/download`, { headers: { Cookie: cookie } });
  assert.equal(download.status, 200);
  assert.equal(Number(download.headers.get("content-length")), uploaded.data.bytes);
  const hash = createHash("sha256"); let length = 0;
  for await (const chunk of download.body) { length += chunk.length; hash.update(chunk); }
  const expected = createHash("sha256");
  for await (const chunk of fs.createReadStream(large)) expected.update(chunk);
  assert.equal(length, uploaded.data.bytes); assert.equal(hash.digest("hex"), expected.digest("hex"));
  const preview = await fetch(`${base}/api/files/${uploaded.data.id}/preview?page=0`, { headers: { Cookie: cookie } });
  assert.equal(preview.status, 200); await preview.arrayBuffer();
  const chunked = await streamUpload(base, cookie, session.csrf, small, true);
  assert.equal(chunked.status, 200, JSON.stringify(chunked.data));
  const oversized = path.join(data, "oversized.pdf"); pdfFixture(oversized, 100 * 1024 * 1024 + 1);
  const tooBig = await streamUpload(base, cookie, session.csrf, oversized);
  assert.equal(tooBig.status, 413);
  for (const format of ["jpg", "png"]) {
    const submitted = await fetch(base + "/api/scan", { method: "POST", headers, body: JSON.stringify({ request_id: randomUUID(), options: { scanner: "demo-scanner", format, color: false, dpi: 300 } }) });
    assert.equal(submitted.status, 200);
    const job = await submitted.json();
    let finished;
    for (let i = 0; i < 100; i++) {
      const jobs = await (await fetch(base + "/api/jobs", { headers: { Cookie: cookie } })).json();
      finished = jobs.find((item) => item.id === job.id);
      if (!["queued", "running"].includes(finished.status)) break;
      await delay(100);
    }
    assert.equal(finished.status, "completed", finished.message);
    const result = await fetch(`${base}/api/files/${finished.fileId}/download`, { headers: { Cookie: cookie } });
    assert.equal(result.headers.get("content-type"), format === "png" ? "image/png" : "image/jpeg"); await result.arrayBuffer();
  }
  const authority = new URL(base).host;
  const duplicate = await raw(base, `GET /api/session HTTP/1.1\r\nHost: ${authority}\r\nHost: ${authority}\r\n\r\n`);
  assert.match(duplicate, /^HTTP\/1.1 400/);
  const conflicting = await raw(base, `POST /api/license HTTP/1.1\r\nHost: ${authority}\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n`);
  assert.match(conflicting, /^HTTP\/1.1 400/);
  const abandoned = net.connect(Number(new URL(base).port), "127.0.0.1");
  await new Promise((resolve) => abandoned.once("connect", resolve));
  abandoned.write(`POST /api/files HTTP/1.1\r\nHost: ${authority}\r\nCookie: ${cookie}\r\nx-csrf-token: ${session.csrf}\r\nContent-Length: 100000\r\nContent-Type: multipart/form-data; boundary=abandoned\r\n\r\npartial`);
  abandoned.destroy(); await delay(250);
  assert.equal((await fetch(base + "/api/session")).status, 200, "Disconnect must not terminate the host");
  let scratchClean = false;
  for (let i = 0; i < 100; i++) {
    if (!fs.readdirSync(path.join(data, "work")).length) { scratchClean = true; break; }
    await delay(50);
  }
  assert.ok(scratchClean, "Disconnected/failed operations must clean scratch files");
  const powershell = path.join(process.env.SystemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
  const memory = JSON.parse(execFileSync(powershell, ["-NoProfile", "-Command", `(Get-Process -Id ${server.pid} | Select-Object WorkingSet64, PeakWorkingSet64) | ConvertTo-Json -Compress`], { windowsHide: true, encoding: "utf8" }));
  const output = path.resolve(__dirname, "../artifacts/legacy"); fs.mkdirSync(output, { recursive: true });
  fs.writeFileSync(path.join(output, "stress-report.json"), JSON.stringify({ passed: true, uploadBytes: uploaded.data.bytes, memory, checks: ["100 MB upload/download/hash/preview", "100 MB + 1 byte rejected", "chunked upload", "JPG/PNG grayscale scan", "duplicate headers", "conflicting HTTP framing", "aborted upload survival/cleanup"] }, null, 2));
  console.log("PASS: exact 100 MB upload/download/preview, oversize rejection, chunked framing, scans, disconnect survival and cleanup; host peak working set", Math.ceil(memory.PeakWorkingSet64 / 1024 / 1024), "MB");
})().catch((error) => { console.error(error); console.error(server?.logs()); process.exitCode = 1; }).finally(async () => { await server?.stop(); });
