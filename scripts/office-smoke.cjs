// Requires Office/WPS on the test host. Uses demo printing; consumes no paper.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { startTestServer } = require("./test-server.cjs");
const { randomUUID } = require("node:crypto");
const root = path.resolve(__dirname, "..");
let server;
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
(async () => {
  server = await startTestServer("office");
  const { base, data } = server;
  const response = await fetch(base + "/api/session");
  assert.ok(response.ok, "Server startup");
  const session = await response.json();
  const headers = {
    cookie: response.headers.get("set-cookie").split(";")[0],
    "x-csrf-token": session.csrf,
  };
  console.log("Detected formats:", session.officeFormats);
  const report = [];
  for (const [name, pages] of [
    ["中文 文档.docx", 2],
    ["中文 表格.xlsx", 1],
    ["中文 演示.pptx", 2],
  ]) {
    const body = new FormData();
    body.append(
      "file",
      new Blob([fs.readFileSync(path.join(root, "artifacts/office", name))]),
      name,
    );
    const uploaded = await fetch(base + "/api/files", {
      method: "POST",
      headers,
      body,
    });
    const file = await uploaded.json();
    console.log(name, uploaded.status, file);
    assert.equal(uploaded.status, 200, JSON.stringify(file));
    assert.equal(file.extension, "pdf");
    assert.equal(file.pages, pages);
    assert.equal(file.name, name + ".pdf");
    assert.ok(
      !fs.existsSync(path.join(data, "files", file.id + path.extname(name))),
      "Original must be cleaned",
    );
    const preview = await fetch(`${base}/api/files/${file.id}/preview?page=0`, {
      headers,
    });
    assert.equal(preview.status, 200, await preview.clone().text());
    fs.writeFileSync(
      path.join(root, "artifacts/office", name + ".jpg"),
      Buffer.from(await preview.arrayBuffer()),
    );
    const download = await fetch(`${base}/api/files/${file.id}/download`, {
      headers,
    });
    const pdf = Buffer.from(await download.arrayBuffer());
    assert.equal(pdf.subarray(0, 5).toString(), "%PDF-");
    const submitted = await fetch(base + "/api/print", {
      method: "POST",
      headers: { ...headers, "content-type": "application/json" },
      body: JSON.stringify({
        request_id: randomUUID(),
        file_id: file.id,
        options: { printer: "demo-printer", pages: String(pages) },
      }),
    });
    const job = await submitted.json();
    assert.equal(submitted.status, 200, JSON.stringify(job));
    for (let i = 0; i < 50; i++) {
      const jobs = await (await fetch(base + "/api/jobs", { headers })).json();
      const current = jobs.find((item) => item.id === job.id);
      if (current?.status === "simulated") break;
      assert.notEqual(current?.status, "failed");
      if (i === 49) throw new Error("Demo print timed out");
      await delay(250);
    }
    report.push({ name, pages, preview: true, demoPrint: true });
  }
  for (const name of ["bad.docx", "macros.docm"]) {
    const body = new FormData();
    body.append("file", new Blob(["not a document"]), name);
    const bad = await fetch(base + "/api/files", {
      method: "POST",
      headers,
      body,
    });
    const result = await bad.json();
    assert.equal(bad.status, 400, JSON.stringify(result));
    console.log("Rejected:", name, result.error);
  }
  assert.equal(
    fs.readdirSync(path.join(data, "files")).length,
    3,
    "Only the three converted PDFs remain",
  );
  for (let i = 0; i < 10; i++) {
    const body = new FormData();
    body.append(
      "file",
      new Blob([Buffer.alloc(256 * 1024)]),
      "unsupported.docm",
    );
    const rejected = await fetch(base + "/api/files", {
      method: "POST",
      headers,
      body,
    });
    assert.equal(rejected.status, 400);
    assert.ok((await rejected.json()).error);
  }
  assert.equal(
    fs.readdirSync(path.join(data, "files")).length,
    3,
    "Rejected uploads must not leave temporary files",
  );
  assert.deepEqual(
    fs.readdirSync(path.join(data, "work")),
    [],
    "Office workers must remove all scratch files",
  );
  fs.writeFileSync(
    path.join(root, "artifacts/office/report.json"),
    JSON.stringify(report, null, 2),
  );
  console.log(
    "PASS: Office upload, PDF conversion, pagination, previews, download, demo print, cleanup, invalid/macro format rejection.",
  );
})()
  .catch((error) => {
    console.error(error);
    console.error(server?.logs());
    process.exitCode = 1;
  })
  .finally(() => server?.stop())
  .catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
