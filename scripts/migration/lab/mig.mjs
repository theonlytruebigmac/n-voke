// Phase 0 test 0.10: one real migration, driven by the built endpoint script.
//   node mig.mjs publish                 publish/refresh dist/nvoke-migration.ps1 in both Ecoverse tenants
//   node mig.mjs launch <assetId> <destOrgUnitId> [full|none] [destSide=dst] [launchSide=src] [--expect-ufa] [--review <days>]
//   node mig.mjs action <src|dst> <assetId> <Status|Cleanup>
//   node mig.mjs watch                   log every stage signal until healthy (Ctrl-C to stop)
import { SIDES as LAB, secret } from "./lab-config.mjs";
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const statePath = join(here, "mig-state.json");
const state = existsSync(statePath) ? JSON.parse(readFileSync(statePath, "utf8")) : {};
const save = () => writeFileSync(statePath, JSON.stringify(state, null, 2));
const UA = { "User-Agent": "N-voke-phase0-probe/0.1" };
const ERR = "errors { __typename ... on Error { message } }";

const SIDES = Object.fromEntries(Object.entries(LAB).map(([k, v]) => [k, { host: v.host, org: v.ecoverseOrgId, so: v.serviceOrgId, eco: v.eco, nc: v.nc }]));

async function gql(side, query, variables = {}) {
  for (let attempt = 0; ; attempt++) {
    const r = await fetch("https://api.n-able.com/graphql", { method: "POST", headers: { ...UA, Authorization: `Bearer ${secret(SIDES[side].eco)}`, "Content-Type": "application/json" }, body: JSON.stringify({ query, variables }) });
    const text = await r.text();
    if ((r.status === 429 || r.status === 503) && attempt < 4) { await new Promise((s) => setTimeout(s, 2000 * (attempt + 1))); continue; }
    try { return JSON.parse(text); } catch { return { errors: [{ message: `HTTP ${r.status}: ${text.slice(0, 200)}` }] }; }
  }
}

const sessions = {};
async function rest(side, path, init = {}) {
  const host = SIDES[side].host;
  if (!sessions[side]) {
    const a = await (await fetch(`https://${host}/api/auth/authenticate`, { method: "POST", headers: { ...UA, Authorization: `Bearer ${secret(SIDES[side].nc)}` } })).json();
    sessions[side] = a.tokens.access.token;
  }
  const r = await fetch(`https://${host}${path}`, { ...init, headers: { ...UA, Accept: "application/json", Authorization: `Bearer ${sessions[side]}`, ...(init.headers ?? {}) } });
  const text = await r.text();
  try { return { status: r.status, json: JSON.parse(text) }; } catch { return { status: r.status, json: { text: text.slice(0, 300) } }; }
}

const built = JSON.parse(readFileSync(join(here, "..", "endpoint", "dist", "version.json"), "utf8"));
const content = readFileSync(join(here, "..", "endpoint", "dist", "nvoke-migration.ps1"), "utf8");
const NAME = "N-voke migration (Phase 0 lab)";
const INPUTS = [
  ["Action", "STRING", { stringValue: "Launch" }], ["DestServer", "STRING", { stringValue: "-" }], ["DestCustomerId", "STRING", { stringValue: "-" }],
  ["RegistrationToken", "PASSWORD", { stringValue: "-" }], ["MigrationId", "STRING", { stringValue: "-" }], ["LauncherVersion", "STRING", { stringValue: "-" }],
  ["TaskRepeatMinutes", "INT", { intValue: 15 }], ["TaskExpiryDays", "INT", { intValue: 14 }], ["CleanMode", "STRING", { stringValue: "full" }],
  ["ExpectUfa", "STRING", { stringValue: "false" }], ["ReviewDays", "INT", { intValue: 7 }],
].map(([variable, type, value]) => ({ name: variable, variable, type, value }));
const OUTPUTS = ["Result", "ErrorCode", "Message", "ScriptVersion", "TaskCreated", "StagedAt", "StatusJson", "TaskState", "Cleaned"].map((variable) => ({ name: variable, variable, type: "STRING" }));

function definition(side) {
  return {
    organizationId: SIDES[side].org, name: NAME,
    description: `N-voke agent migration endpoint script ${built.version}. Phase 0 lab build; safe to delete after testing.`,
    language: "POWERSHELL", isWindowsCompatible: true, isLinuxCompatible: false, isDarwinCompatible: false,
    content, timeoutSeconds: 900, inputs: INPUTS, outputs: OUTPUTS,
  };
}

async function runScript(side, assetId, inputs, label, online = true) {
  const scriptId = state.scripts?.[side];
  const configuration = { scriptId, name: `${label} ${new Date().toISOString().slice(11, 19)}`, timeoutSeconds: 900, inputs, ...(online ? { shouldRunWhenOnline: true } : {}) };
  const r = await gql(side, `mutation($input: AssetsScriptRunMutationInput!) { assetsScriptRun(input: $input) { items { id } ${ERR} } }`, { input: { assetIds: [assetId], configuration } });
  const id = r.data?.assetsScriptRun?.items?.[0]?.id;
  console.log(`${side} run "${configuration.name}":`, id ?? JSON.stringify(r));
  return id;
}
const input = (variable, value) => { const d = INPUTS.find((i) => i.variable === variable); return { name: d.name, variable, type: d.type, value: d.type === "INT" ? { intValue: value } : { stringValue: String(value) } }; };

async function taskResult(side, id) {
  const r = await gql(side, `query($id: ID!) { task(id: $id) { status executions { nodes { status errorMessage startedAt ... on ScriptTaskExecution { output { outputs { variable value { stringValue } } stdOutput } } } } } }`, { id });
  if (r.errors) console.error("task query errors:", JSON.stringify(r.errors).slice(0, 300));
  const t = r.data?.task, e = t?.executions?.nodes?.[0];
  const outputs = Object.fromEntries((e?.output?.outputs ?? []).map((o) => [o.variable, o.value?.stringValue]));
  return { status: t?.status, exec: e?.status, startedAt: e?.startedAt, error: e?.errorMessage, outputs };
}

const cmd = process.argv[2];
if (cmd === "publish") {
  state.scripts ??= {};
  for (const side of ["src", "dst"]) {
    const existing = state.scripts[side];
    const r = existing
      ? await gql(side, `mutation($input: ScriptUpdateMutationInput!) { scriptUpdate(input: $input) { item { id } ${ERR} } }`, { input: { scriptId: existing, ...definition(side) } })
      : await gql(side, `mutation($input: ScriptCreateMutationInput!) { scriptCreate(input: $input) { item { id } ${ERR} } }`, { input: definition(side) });
    const id = (r.data?.scriptUpdate ?? r.data?.scriptCreate)?.item?.id;
    console.log(`${side}: ${existing ? "updated" : "created"} ${id ? "ok" : JSON.stringify(r).slice(0, 600)}`);
    if (id) state.scripts[side] = id;
  }
  state.version = built.version;
  save();
} else if (cmd === "launch") {
  const [assetId, destOrgUnit] = [process.argv[3], process.argv[4]];
  const destSide = process.argv[6] ?? "dst"; // server the device moves to
  const launchSide = process.argv[7] ?? "src"; // Ecoverse tenant the device is in now
  const tok = (await rest(destSide, `/api/org-units/${destOrgUnit}/registration-token`)).json.data;
  if (!tok?.registrationToken) throw new Error(`No registration token for destination org unit ${destOrgUnit}`);
  console.log(`destination ${destOrgUnit}: token ${tok.registrationToken.length} chars, expires ${tok.registrationTokenExpiryDate}`);
  const id = await runScript(launchSide, assetId, [
    input("Action", "Launch"), input("DestServer", SIDES[destSide].host), input("DestCustomerId", destOrgUnit),
    input("RegistrationToken", tok.registrationToken), input("MigrationId", `lab/${launchSide}-to-${destSide}`), input("LauncherVersion", built.version),
    input("TaskRepeatMinutes", 15), input("TaskExpiryDays", 14), input("CleanMode", process.argv[5] ?? "full"),
    input("ExpectUfa", process.argv.includes("--expect-ufa") ? "true" : "false"),
    input("ReviewDays", process.argv.includes("--review") ? Number(process.argv[process.argv.indexOf("--review") + 1]) : 7),
  ], "N-voke migration Launch");
  state.launch = { assetId, destOrgUnit, destSide, launchSide, taskId: id, at: new Date().toISOString() };
  save();
} else if (cmd === "action") {
  const [side, assetId, action] = process.argv.slice(3);
  // Ecoverse requires a value for every declared input ("Invalid number of input params." otherwise).
  const id = await runScript(side, assetId, INPUTS.map((d) => d.variable === "Action" ? input("Action", action) : d.variable === "LauncherVersion" ? input("LauncherVersion", built.version) : { name: d.name, variable: d.variable, type: d.type, value: d.value }), `N-voke migration ${action}`);
  if (!id) process.exit(1);
  state.lastAction = { side, assetId, action, id }; save();
  for (let i = 0; i < 90; i++) {
    const r = await taskResult(side, id);
    if (["SUCCEEDED", "FAILED", "CANCELED", "EXPIRED"].includes(r.status)) { console.log(JSON.stringify(r, null, 2)); break; }
    await new Promise((s) => setTimeout(s, 10000));
  }
} else if (cmd === "result") {
  const [side, id] = process.argv.slice(3);
  console.log(JSON.stringify(await taskResult(side, id), null, 2));
} else if (cmd === "watch") {
  const L = state.launch;
  const srcAsset = (await gql("src", `query($o: [ID!]!) { assetSearch(inOrganizations: $o, first: 50) { nodes { id name ncentralDevice { deviceId } systemInfo { serialNumber hostname } networkInterfaces { macAddress } } } }`, { o: [SIDES.src.org] })).data.assetSearch.nodes.find((a) => a.id === L.assetId);
  const serial = srcAsset.systemInfo.serialNumber, srcDeviceId = srcAsset.ncentralDevice.deviceId, host = srcAsset.name;
  const macs = new Set((srcAsset.networkInterfaces ?? []).map((n) => n.macAddress).filter(Boolean));
  console.log(`watching ${host} (source device ${srcDeviceId}, serial ${serial}, MACs ${[...macs].join(", ")})`);
  let last = "";
  for (;;) {
    const sig = {};
    try {
      const launch = await taskResult("src", L.taskId);
      sig.launch = `${launch.status}/${launch.exec ?? "-"} result=${launch.outputs.Result ?? "-"} taskCreated=${launch.outputs.TaskCreated ?? "-"}${launch.error ? " err=" + launch.error : ""}`;
      if (launch.outputs.Result && !state.launchOutputs) { state.launchOutputs = launch.outputs; save(); }
      const s = (await rest("src", `/api/devices/${srcDeviceId}`)).json.data;
      const srcAge = s ? Math.round((Date.now() - Date.parse(s.lastApplianceCheckinTime)) / 60000) : null;
      sig.srcCheckin = s ? (srcAge <= 6 ? "recent" : `stale (${srcAge} min ago)`) : "device gone";
      const se = (await gql("src", `query($o: [ID!]!, $n: String!) { assetSearch(inOrganizations: $o, where: { name: { contains: $n } }, first: 5) { nodes { id agentConnection { status } } } }`, { o: [SIDES.src.org], n: host })).data?.assetSearch?.nodes?.find((a) => a.id === L.assetId);
      sig.srcEcoverse = se?.agentConnection?.status ?? "asset gone";
      const dd = (await rest("dst", `/api/org-units/${SIDES.dst.so}/devices?pageSize=1000`)).json.data ?? [];
      const nd = dd.find((d) => d.longName?.toUpperCase() === host.toUpperCase());
      sig.dstDevice = nd ? `${nd.deviceId} in ${nd.customerName}/${nd.siteName} (org unit ${nd.orgUnitId}), check-in ${Math.round((Date.now() - Date.parse(nd.lastApplianceCheckinTime)) / 60000)} min ago` : "not yet";
      if (nd && !state.newDevice) { state.newDevice = { deviceId: nd.deviceId, orgUnitId: nd.orgUnitId, seenAt: new Date().toISOString() }; save(); }
      const de = (await gql("dst", `query($o: [ID!]!) { assetSearch(inOrganizations: $o, first: 50) { nodes { id name agentConnection { status } ncentralDevice { deviceId } systemInfo { serialNumber } networkInterfaces { macAddress } } } }`, { o: [SIDES.dst.org] })).data?.assetSearch?.nodes ?? [];
      const match = de.find((a) => a.systemInfo?.serialNumber === serial);
      sig.dstEcoverse = match ? `${match.id === L.assetId ? "SAME" : "NEW"} asset ${match.id.slice(0, 8)} ${match.agentConnection?.status} device ${match.ncentralDevice?.deviceId} serial match, MAC match ${(match.networkInterfaces ?? []).some((n) => macs.has(n.macAddress))}` : "not yet";
      if (match && !state.newAsset) { state.newAsset = { id: match.id, seenAt: new Date().toISOString() }; save(); }
    } catch (e) { sig.error = String(e).slice(0, 200); }
    const line = Object.entries(sig).map(([k, v]) => `${k}=${v}`).join(" | ");
    const comparable = line.replace(/\d+ min ago/g, "");
    if (comparable !== last) { const out = `${new Date().toISOString().slice(11, 19)}Z ${line}`; console.log(out); appendFileSync(join(here, "mig-watch.log"), out + "\n"); last = comparable; }
    await new Promise((s) => setTimeout(s, 30000));
  }
} else {
  console.log("usage: publish | launch <assetId> <destOrgUnitId> | action <src|dst> <assetId> <Status|Cleanup> | watch");
}
