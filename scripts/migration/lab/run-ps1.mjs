// Publishes a read-only diagnostic PowerShell script to an Ecoverse tenant, runs it on one asset and
// prints its "Report" output (a JSON string). Usage: node run-ps1.mjs <src|dst> <assetId> probes/<file>.ps1
import { readFileSync } from "node:fs";
import { basename } from "node:path";
import { SIDES, secret } from "./lab-config.mjs";

const [side, assetId, file] = process.argv.slice(2);
if (!SIDES[side] || !assetId || !file) throw new Error("usage: node run-ps1.mjs <src|dst> <assetId> probes/<file>.ps1");
const key = secret(SIDES[side].eco);
const ERR = "errors { __typename ... on Error { message } }";
const gql = async (query, variables) =>
  (await fetch("https://api.n-able.com/graphql", { method: "POST", headers: { "User-Agent": "N-voke-lab/0.1", Authorization: `Bearer ${key}`, "Content-Type": "application/json" }, body: JSON.stringify({ query, variables }) })).json();

const name = `N-voke lab: ${basename(file, ".ps1")}`;
const found = await gql(`query($o: [ID!]!, $n: String!) { scriptSearch(inOrganizations: $o, where: { name: { contains: $n } }, first: 5) { nodes { id name } } }`, { o: [SIDES[side].ecoverseOrgId], n: name });
let scriptId = found.data?.scriptSearch?.nodes?.find((s) => s.name === name)?.id;
if (!scriptId) {
  const c = await gql(`mutation($input: ScriptCreateMutationInput!) { scriptCreate(input: $input) { item { id } ${ERR} } }`, {
    input: { organizationId: SIDES[side].ecoverseOrgId, name, description: "N-voke lab diagnostic (read-only). Safe to delete.", language: "POWERSHELL", isWindowsCompatible: true, isLinuxCompatible: false, isDarwinCompatible: false, content: readFileSync(file, "utf8"), timeoutSeconds: 900, inputs: [], outputs: [{ name: "Report", variable: "Report", type: "STRING" }] },
  });
  scriptId = c.data?.scriptCreate?.item?.id;
  if (!scriptId) throw new Error(JSON.stringify(c));
}
const r = await gql(`mutation($input: AssetsScriptRunMutationInput!) { assetsScriptRun(input: $input) { items { id } ${ERR} } }`, { input: { assetIds: [assetId], configuration: { scriptId, name, timeoutSeconds: 900, shouldRunWhenOnline: true } } });
const id = r.data?.assetsScriptRun?.items?.[0]?.id;
if (!id) throw new Error(JSON.stringify(r));
console.error(`task ${id}`);
for (;;) {
  await new Promise((s) => setTimeout(s, 15000));
  const t = await gql(`query($id: ID!) { task(id: $id) { status executions { nodes { errorMessage ... on ScriptTaskExecution { output { outputs { variable value { stringValue } } } } } } } }`, { id });
  const task = t.data?.task;
  if (!["SUCCEEDED", "FAILED", "CANCELED", "EXPIRED"].includes(task?.status)) { console.error(new Date().toISOString().slice(11, 19), task?.status); continue; }
  const e = task.executions.nodes[0];
  const report = e?.output?.outputs?.find((o) => o.variable === "Report")?.value?.stringValue;
  console.error(task.status, e?.errorMessage ?? "");
  try { console.log(JSON.stringify(JSON.parse(report), null, 1)); } catch { console.log(report); }
  break;
}
