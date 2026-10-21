// Phase 0 REST-path tests 0.6–0.8. Usage:
//   node rest-test.mjs run <itemId> <deviceId> [--exit N] [--mode exit|setshouldexit|none] [--password] [--long N] [--name suffix]
//   node rest-test.mjs poll [taskId…]        (defaults to every task in rest-state.json)
import { SIDES, secret } from "./lab-config.mjs";
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const statePath = join(here, "rest-state.json");
const state = existsSync(statePath) ? JSON.parse(readFileSync(statePath, "utf8")) : { tasks: [] };
const save = () => writeFileSync(statePath, JSON.stringify(state, null, 2));

const HOST = SIDES.src.host;
const UA = { "User-Agent": "N-voke-phase0-probe/0.1", Accept: "application/json" };
const FAKE_TOKEN = "fake-reg-token-0000-not-real";
const jwt = secret("ncentral:src");
const auth = await (await fetch(`https://${HOST}/api/auth/authenticate`, { method: "POST", headers: { ...UA, Authorization: `Bearer ${jwt}` } })).json();
const H = { ...UA, Authorization: `Bearer ${auth.tokens.access.token}` };
const call = async (method, path, body) => {
  const r = await fetch(`https://${HOST}${path}`, { method, headers: { ...H, ...(body ? { "Content-Type": "application/json" } : {}) }, body: body ? JSON.stringify(body) : undefined });
  const text = await r.text();
  let json; try { json = JSON.parse(text); } catch { json = { text: text.slice(0, 2000) }; }
  return { status: r.status, json };
};
const arg = (flag, dflt) => { const i = process.argv.indexOf(flag); return i > 0 ? process.argv[i + 1] : dflt; };

const cmd = process.argv[2];
if (cmd === "run") {
  const itemId = Number(process.argv[3]), deviceId = Number(process.argv[4]);
  const exitCode = Number(arg("--exit", 0)), mode = arg("--mode", "exit"), long = Number(arg("--long", 0));
  const asPassword = process.argv.includes("--password");
  const device = (await call("GET", `/api/devices/${deviceId}`)).json.data;
  const commandLine = [
    "-Action Launch",
    `-DestServer ${SIDES.dst.host}`,
    `-RegistrationToken ${FAKE_TOKEN}`,
    `-TestQuote "it's quoted `+"`"+`$notAVar; & | %PATH%"`,
    "-TestInt 42",
    "-TestSwitch",
    `-TestExit ${exitCode}`,
    `-ExitMode ${mode}`,
    ...(long ? [`-LongOutputChars ${long}`] : []),
    ...(process.argv.includes("--stderr") ? ["-WriteStderr"] : []),
  ].join(" ");
  const label = `exit ${exitCode} via ${mode}${asPassword ? ", password param" : ""}${long ? `, ${long} chars` : ""}${process.argv.includes("--stderr") ? ", with stderr" : ""}${arg("--name") ? ", " + arg("--name") : ""}`;
  const body = {
    name: `N-voke Phase0 REST probe (${label}) ${new Date().toISOString().slice(11, 19)}`,
    itemId,
    taskType: "Script",
    customerId: Number(arg("--customer", device.customerId)),
    deviceId,
    credential: { type: "LocalSystem" },
    parameters: [{ name: "CommandLine", value: commandLine, type: asPassword ? "password" : "string", description: "Command line" }],
  };
  console.log("device:", device.longName, "customerId", device.customerId, "siteId", device.siteId);
  console.log("commandLine:", commandLine.replace(FAKE_TOKEN, "<FAKE_TOKEN>"));
  const r = await call("POST", "/api/scheduled-tasks/direct", body);
  console.log(`POST /api/scheduled-tasks/direct -> HTTP ${r.status}`, JSON.stringify(r.json).slice(0, 1500));
  const taskId = r.json?.data?.taskId;
  if (taskId) { state.tasks.push({ taskId, deviceId, label, at: new Date().toISOString() }); save(); }
} else if (cmd === "poll") {
  const ids = process.argv.slice(3).length ? process.argv.slice(3).map(Number) : state.tasks.map((t) => t.taskId);
  for (const id of ids) {
    const [task, status, details] = await Promise.all([call("GET", `/api/scheduled-tasks/${id}`), call("GET", `/api/scheduled-tasks/${id}/status`), call("GET", `/api/scheduled-tasks/${id}/status/details`)]);
    const t = state.tasks.find((x) => x.taskId === id);
    console.log(`\n### task ${id} (${t?.label ?? "?"}) task HTTP ${task.status} status HTTP ${status.status} details HTTP ${details.status}`);
    console.log("status:", JSON.stringify(status.json.data ?? status.json));
    const d = [].concat(details.json.data ?? details.json);
    for (const x of d) {
      console.log(`device ${x.deviceId} ${x.deviceName}: status=${x.status} message=${JSON.stringify(x.message)} outputFileName=${x.outputFileName} outputLength=${x.output?.length}`);
      if (x.output) console.log("output:\n" + x.output.replaceAll(FAKE_TOKEN, "<FAKE_TOKEN>"));
    }
    writeFileSync(join(here, `rest-task-${id}.json`), JSON.stringify({ task: task.json, status: status.json, details: details.json }, null, 2).replaceAll(FAKE_TOKEN, "<FAKE_TOKEN>"));
  }
} else {
  console.log("usage: run <itemId> <deviceId> [...] | poll [taskId…]");
}
