// Shared lifecycle for isolated integration-test servers.
const fs = require("node:fs");
const path = require("node:path");
const os = require("node:os");
const net = require("node:net");
const { spawn, execFileSync } = require("node:child_process");
const { setTimeout: delay } = require("node:timers/promises");

function issueTestLicense(machine, customer = "自动化测试") {
  return execFileSync(
    process.env.LANPRINT_GENERATOR || path.join(__dirname, "../target/debug/license-generator.exe"),
    ["issue", "--private-key", process.env.LANPRINT_SIGNING_KEY || path.join(__dirname, "../admin-tools/signing-key.hex"), "--machine", machine, "--customer", customer],
    { encoding: "utf8", windowsHide: true },
  ).trim();
}

async function startTestServer(prefix, setup = () => {}, { demo = true, activate = true } = {}) {
  const tempRoot = fs.realpathSync(os.tmpdir());
  const data = fs.mkdtempSync(path.join(tempRoot, `LanPrint-${prefix}-`));
  let child,
    exited,
    stopped = false,
    log = "",
    spawnError;
  const stop = async () => {
    if (stopped) return;
    stopped = true;
    if (child?.pid && child.exitCode === null && child.signalCode === null) {
      // End only this test's process tree, including in-flight device workers.
      if (process.platform === "win32") {
        await new Promise((resolve, reject) => {
          const kill = spawn(
            "taskkill",
            ["/PID", String(child.pid), "/T", "/F"],
            { windowsHide: true, stdio: "ignore" },
          );
          kill.once("error", reject);
          kill.once("exit", resolve);
        });
      } else child.kill();
      await exited;
    }
    if (process.env.KEEP_TEST_DATA === "1") console.log("Test data:", data);
    else {
      const resolved = fs.realpathSync(data);
      if (
        path.dirname(resolved) !== tempRoot ||
        !path.basename(resolved).startsWith(`LanPrint-${prefix}-`)
      ) {
        throw new Error(
          `Refusing to clean unexpected test directory: ${resolved}`,
        );
      }
      fs.rmSync(resolved, {
        recursive: true,
        force: true,
        maxRetries: 20,
        retryDelay: 100,
      });
    }
  };
  try {
    setup(data);
    const socket = net.createServer();
    await new Promise((resolve, reject) => {
      socket.once("error", reject);
      socket.listen(0, "127.0.0.1", resolve);
    });
    const port = socket.address().port;
    await new Promise((resolve) => socket.close(resolve));
    const base = `http://127.0.0.1:${port}`;
    const exe =
      process.env.LANPRINT_EXE ||
      path.join(__dirname, "../target/debug/lan-print.exe");
    child = spawn(
      exe,
      [
        ...(demo ? ["--demo"] : []),
        "--no-tray",
        "--port",
        String(port),
        "--data-dir",
        data,
      ],
      { windowsHide: true, stdio: ["ignore", "pipe", "pipe"] },
    );
    child.on("error", (error) => {
      spawnError = error;
    });
    exited = new Promise((resolve) => {
      child.once("exit", resolve);
      child.once("error", resolve);
    });
    child.stdout.on("data", (bytes) => {
      log += bytes;
    });
    child.stderr.on("data", (bytes) => {
      log += bytes;
    });
    for (let i = 0; i < 80; i++) {
      if (spawnError) throw spawnError;
      if (child.exitCode !== null || child.signalCode !== null)
        throw new Error(`Server exited: ${log}`);
      try {
        const response = await fetch(base + "/api/session", {
          signal: AbortSignal.timeout(1000),
        });
        const session = await response.json();
        if (response.ok) {
          if (activate) {
            const status = await (await fetch(base + "/api/license")).json();
            if (!status.registered) {
              const code = issueTestLicense(status.machineCode);
              const result = await fetch(base + "/api/license", {
                method: "POST",
                headers: { "Content-Type": "application/json", "x-csrf-token": session.csrf, Cookie: response.headers.get("set-cookie").split(";")[0] },
                body: JSON.stringify({ code }),
              });
              if (!result.ok) throw new Error(`Test activation failed: ${await result.text()}`);
            }
          }
          return { base, data, stop, logs: () => log };
        }
      } catch (error) {
        if (error.message?.startsWith("Test activation failed:") || error.code === "ENOENT" || error.status) throw error;
      }
      await delay(250);
    }
    throw new Error(`Server startup timed out: ${log}`);
  } catch (error) {
    await stop();
    throw error;
  }
}

module.exports = { startTestServer, issueTestLicense };
