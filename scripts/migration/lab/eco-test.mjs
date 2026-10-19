// Phase 0 Ecoverse tests 0.1, 0.2, 0.4. Usage:
//   node eco-test.mjs create            publish input-probe.ps1 (or reuse it)
//   node eco-test.mjs run <assetId> <exitCode>
//   node eco-test.mjs poll [taskId…]    (defaults to every task in state.json)
import { SIDES, secret } from "./lab-config.mjs";
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const statePath = join(here, "eco-state.json");
const state = existsSync(statePath) ? JSON.parse(readFileSync(statePath, "utf8")) : { tasks: [] };
const save = () => writeFileSync(statePath, JSON.stringify(state, null, 2));

const ORG = SIDES.src.ecoverseOrgId; // source tenant
const NAME = "N-voke Phase0 input probe";
const key = secret("ecoverse:src");
const ERR = "errors { __typename ... on Error { message } }";

async function gql(query, variables = {}) {
  const r = await fetch("https://api.n-able.com/graphql", {
    method: "POST",
    headers: { Authorization: `Bearer ${key}`, "Content-Type": "application/json", "User-Agent": "N-voke-phase0-probe/0.1" },
    body: JSON.stringify({ query, variables }),
  });
  const text = await r.text();
  try { return { status: r.status, ...JSON.parse(text) }; } catch { return { status: r.status, text }; }
}

const INPUTS = [
  { name: "Test String", variable: "TestString", type: "STRING" },
  { name: "Test Password", variable: "TestPassword", type: "PASSWORD" },
  { name: "Test Quote", variable: "TestQuote", type: "STRING" },
  { name: "Test Int", variable: "TestInt", type: "INT" },
  { name: "Test Bool", variable: "TestBool", type: "BOOLEAN" },
  { name: "Test Exit", variable: "TestExit", type: "INT" },
];
const OUTPUTS = [
  { name: "Out Var", variable: "OutVar", type: "STRING" },
  { name: "Out Global", variable: "OutGlobal", type: "STRING" },
  { name: "Out Env", variable: "OutEnv", type: "STRING" },
  { name: "Out Std", variable: "OutStd", type: "STRING" },
  { name: "Out Bool", variable: "OutBool", type: "BOOLEAN" },
];
const valueFor = (type, v) => ({ STRING: { stringValue: v }, PASSWORD: { stringValue: v }, INT: { intValue: v }, BOOLEAN: { booleanValue: v } })[type];

const cmd = process.argv[2];

if (cmd === "create") {
  const found = await gql(`query($o: [ID!]!, $n: String!) { scriptSearch(inOrganizations: $o, where: { name: { contains: $n } }, first: 5) { nodes { id name inputs { name variable type } } } }`, { o: [ORG], n: "Phase0" });
  const existing = found.data?.scriptSearch?.nodes?.find((s) => s.name === NAME);
  if (existing) { console.log("already exists:", JSON.stringify(existing)); state.scriptId = existing.id; save(); process.exit(0); }
  const input = {
    organizationId: ORG,
    name: NAME,
    description: "Throwaway test from N-voke migration Phase 0. Reports how inputs arrive. Safe to delete.",
    language: "POWERSHELL",
    isWindowsCompatible: true, isLinuxCompatible: false, isDarwinCompatible: false,
    content: readFileSync(join(here, "probes", "input-probe.ps1"), "utf8"),
    timeoutSeconds: 300,
    inputs: INPUTS.map((i) => ({ ...i, value: valueFor(i.type, { STRING: "default", PASSWORD: "default", INT: 0, BOOLEAN: false }[i.type]) })),
    outputs: OUTPUTS,
  };
  for (const sel of ["item { id name }", "script { id name }", "id"]) {
    const r = await gql(`mutation Create($input: ScriptCreateMutationInput!) { scriptCreate(input: $input) { ${sel} ${ERR} } }`, { input });
    console.log(`selection "${sel}":`, JSON.stringify(r).slice(0, 1500));
    const payload = r.data?.scriptCreate;
    const id = payload?.item?.id ?? payload?.script?.id ?? payload?.id;
    if (id) { state.scriptId = id; state.createdPayloadShape = sel; save(); console.log("created", id); break; }
    if (payload) break; // mutation ran but returned errors: stop, don't create twice
  }
} else if (cmd === "update") {
  const content = readFileSync(join(here, "probes", process.argv[3] ?? "input-probe-v3.ps1"), "utf8");
  const full = {
    scriptId: state.scriptId, organizationId: ORG, name: NAME,
    description: "Throwaway test from N-voke migration Phase 0 (v2). Reports how inputs arrive. Safe to delete.",
    language: "POWERSHELL", isWindowsCompatible: true, isLinuxCompatible: false, isDarwinCompatible: false,
    content, timeoutSeconds: 300,
    inputs: INPUTS.map((i) => ({ ...i, value: valueFor(i.type, { STRING: "default", PASSWORD: "default", INT: 0, BOOLEAN: false }[i.type]) })),
    outputs: OUTPUTS,
  };
  for (const input of [full]) {
    const r = await gql(`mutation Update($input: ScriptUpdateMutationInput!) { scriptUpdate(input: $input) { item { id name updatedAt } ${ERR} } }`, { input });
    console.log(`input keys ${Object.keys(input)}:`, JSON.stringify(r).slice(0, 1200));
    if (r.data?.scriptUpdate?.item) break;
  }
} else if (cmd === "run") {
  const [assetId, exitCode, multi] = [process.argv[3], Number(process.argv[4] ?? 0), process.argv[5] === "multi"];
  const online = process.argv.includes("online");
  const values = {
    TestString: "hello world",
    TestPassword: "fake-s3cret-not-real",
    TestQuote: `it's "quoted" $notAVar; & | \`tick %PATH%`,
    TestInt: 42,
    TestBool: multi,
    TestExit: exitCode,
  };
  const input = {
    assetIds: [assetId],
    configuration: {
      scriptId: state.scriptId,
      name: `N-voke Phase0 input probe (exit ${exitCode}${multi ? ", multi-line" : ""}${online ? ", run when online" : ", offline control"})`,
      timeoutSeconds: 300,
      inputs: INPUTS.map((i) => ({ name: i.name, variable: i.variable, type: i.type, value: valueFor(i.type, values[i.variable]) })),
      ...(online ? { shouldRunWhenOnline: true } : {}),
    },
  };
  const r = await gql(`mutation Run($input: AssetsScriptRunMutationInput!) { assetsScriptRun(input: $input) { items { id } ${ERR} } }`, { input });
  console.log(JSON.stringify(r));
  state.tasks = state.tasks.filter((t) => !t.done);
  for (const it of r.data?.assetsScriptRun?.items ?? []) state.tasks.push({ id: it.id, assetId, exitCode, multi, online, at: new Date().toISOString() });
  save();
} else if (cmd === "poll") {
  const ids = process.argv.slice(3).length ? process.argv.slice(3) : state.tasks.map((t) => t.id);
  const fields = `id status createdAt executions { nodes { id status exitCode errorMessage startedAt updatedAt durationMilliseconds
    ... on ScriptTaskExecution { output { runAs stdOutput stdError outputs { name variable type value { stringValue booleanValue intValue floatValue dateTimeValue } } } } } }`;
  for (const id of ids) {
    const r = await gql(`query($id: ID!) { task(id: $id) { ${fields} } }`, { id });
    const t = r.data?.task;
    const ex = t?.executions?.nodes?.[0];
    console.log(`\n### task ${id.slice(0, 16)}… status=${t?.status} exec=${ex?.status} exit=${ex?.exitCode} err=${ex?.errorMessage ?? ""} dur=${ex?.durationMilliseconds}ms runAs=${ex?.output?.runAs}`);
    if (r.errors) console.log("errors:", JSON.stringify(r.errors));
    if (ex?.output) {
      console.log("outputs:", JSON.stringify(ex.output.outputs));
      console.log("stderr:", ex.output.stdError);
      console.log("stdout:\n" + ex.output.stdOutput);
    }
    writeFileSync(join(here, `task-${id.slice(0, 12).replace(/[^a-z0-9]/gi, "_")}.json`), JSON.stringify(r, null, 2));
  }
} else {
  console.log("usage: create | run <assetId> <exitCode> | poll");
}
