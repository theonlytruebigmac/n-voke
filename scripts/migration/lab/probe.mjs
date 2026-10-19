// Phase 0 read-only probe. Makes no changes on any server.
// Secrets come from the macOS Keychain (see lab-config.mjs); outputs are redacted.
// Usage: node probe.mjs [src|dst|all]
import { mkdirSync, writeFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

import { SIDES as LAB, secret as labSecret } from "./lab-config.mjs";

const SIDES = Object.fromEntries(Object.entries(LAB).map(([k, v]) => [k, { host: v.host, soName: v.serviceOrgName, user: v.apiUser }]));
const GQL_URL = "https://api.n-able.com/graphql";
const UA = "N-voke-phase0-probe/0.1 (+https://github.com/theonlytruebigmac/n-voke)";
const MAX_DEVICE_DETAIL = 3;

const outDir = join(dirname(fileURLToPath(import.meta.url)), `out-${new Date().toISOString().replace(/[:.]/g, "-")}`);
mkdirSync(outDir, { recursive: true });

const secret = (account) => labSecret(account, { optional: true });

// Keys whose names end in token/jwt/password/secret hold secrets; expiry dates etc. don't end that way.
const SECRET_KEY = /(token|jwt|password|secret|activationkey)$/i;
function redact(value, key = "") {
  if (Array.isArray(value)) return value.map((v) => redact(v));
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, redact(v, k)]));
  if (typeof value === "string" && SECRET_KEY.test(key) && value.length > 0) return `<redacted len=${value.length}>`;
  return value;
}
const redactText = (text, secrets) => secrets.filter(Boolean).reduce((t, s) => t.split(s).join("<redacted>"), text);

const log = [];
function record(side, name, result) {
  const entry = { side, name, ...result };
  log.push(entry);
  const mark = result.ok ? "ok " : "ERR";
  console.log(`[${side}] ${mark} ${name}${result.status ? ` (HTTP ${result.status})` : ""}${result.note ? ` — ${result.note}` : ""}`);
  writeFileSync(join(outDir, `${side}-${name.replace(/[^a-z0-9]+/gi, "_")}.json`), JSON.stringify(redact(entry), null, 2));
}

async function http(url, { method = "GET", headers = {}, body } = {}) {
  for (let attempt = 0; ; attempt++) {
    const started = Date.now();
    const res = await fetch(url, { method, headers: { "User-Agent": UA, Accept: "application/json", ...headers }, body });
    const text = await res.text();
    if (res.status === 429 && attempt < 4) {
      await new Promise((r) => setTimeout(r, (Number(res.headers.get("retry-after")) || 2 ** attempt) * 1000));
      continue;
    }
    let json;
    try { json = JSON.parse(text); } catch { json = undefined; }
    return { status: res.status, ms: Date.now() - started, json, text: json ? undefined : text.slice(0, 4000) };
  }
}

// ---------------------------------------------------------------- N-central REST
function shapeOf(items) {
  const keys = new Set();
  for (const it of items ?? []) Object.keys(it ?? {}).forEach((k) => keys.add(k));
  return [...keys].sort();
}
const idOf = (o) => o?.orgUnitId ?? o?.soId ?? o?.customerId ?? o?.siteId ?? o?.id;
const nameOf = (o) => o?.orgUnitName ?? o?.soName ?? o?.customerName ?? o?.siteName ?? o?.name;

async function probeRest(side, { host, soName }, jwt) {
  const base = `https://${host}`;
  if (!jwt) return record(side, "rest.auth", { ok: false, note: `no Keychain item ncentral:${side}` });
  const auth = await http(`${base}/api/auth/authenticate`, { method: "POST", headers: { Authorization: `Bearer ${jwt}` } });
  const access = auth.json?.tokens?.access?.token;
  record(side, "rest.auth", { ok: !!access, status: auth.status, body: auth.json ?? auth.text, note: access ? `access expires in ${auth.json.tokens.access.expirySeconds}s, refresh in ${auth.json.tokens.refresh?.expirySeconds}s` : "no access token" });
  if (!access) return;
  const H = { Authorization: `Bearer ${access}` };
  const get = async (name, path) => {
    const r = await http(`${base}${path}`, { headers: H });
    const data = r.json?.data;
    record(side, name, { ok: r.status < 400, status: r.status, ms: r.ms, path, count: Array.isArray(data) ? data.length : undefined, totalItems: r.json?.totalItems, keys: Array.isArray(data) ? shapeOf(data) : undefined, body: r.json ?? r.text });
    return r;
  };

  await get("rest.server-info", "/api/server-info");
  const auth2 = await http(`${base}/api/server-info/extra/authenticated`, { method: "POST", headers: H });
  record(side, "rest.server-info-extra", { ok: auth2.status < 400, status: auth2.status, body: auth2.json ?? auth2.text });

  const sos = (await get("rest.service-orgs", "/api/service-orgs?pageSize=1000")).json?.data ?? [];
  const so = sos.find((s) => nameOf(s)?.toLowerCase() === soName.toLowerCase());
  if (!so) return record(side, "rest.find-so", { ok: false, note: `"${soName}" not in [${sos.map(nameOf).join(", ")}]` });
  const soId = idOf(so);
  record(side, "rest.find-so", { ok: true, note: `${soName} = ${soId}` });

  await get("rest.customers", "/api/customers?pageSize=1000");
  await get("rest.sites", "/api/sites?pageSize=1000");
  const children = (await get("rest.so-children", `/api/org-units/${soId}/children?pageSize=1000`)).json?.data ?? [];

  // Registration tokens: SO, every child, and grandchildren (sites) if children isn't recursive.
  const units = [{ ...so, _type: "so" }, ...children];
  for (const c of children.slice(0, 20)) {
    const sub = (await http(`${base}/api/org-units/${idOf(c)}/children?pageSize=1000`, { headers: H })).json?.data ?? [];
    for (const s of sub) if (!units.some((u) => idOf(u) === idOf(s))) units.push(s);
  }
  const tokenSummary = [];
  for (const u of units) {
    const id = idOf(u);
    const type = (u.orgUnitType ?? u._type ?? "").toString().toLowerCase();
    const paths = [`/api/org-units/${id}/registration-token`];
    if (type.includes("customer")) paths.push(`/api/customers/${id}/registration-token`);
    if (type.includes("site")) paths.push(`/api/sites/${id}/registration-token`);
    for (const p of paths) {
      const r = await http(`${base}${p}`, { headers: H });
      const d = r.json?.data ?? r.json;
      tokenSummary.push({ id, name: nameOf(u), type: u.orgUnitType ?? u._type, path: p, status: r.status, keys: d && typeof d === "object" ? Object.keys(d) : undefined, body: redact(r.json ?? r.text) });
    }
  }
  record(side, "rest.registration-tokens", { ok: true, units: units.map((u) => ({ id: idOf(u), name: nameOf(u), type: u.orgUnitType ?? u._type, parentId: u.parentId })), tokens: tokenSummary });

  const devices = (await get("rest.so-devices", `/api/org-units/${soId}/devices?pageSize=1000`)).json?.data ?? [];
  await get("rest.devices-page1", "/api/devices?pageSize=5");
  for (const d of devices.slice(0, MAX_DEVICE_DETAIL)) {
    const id = d.deviceId ?? d.id;
    await get(`rest.device-${id}`, `/api/devices/${id}`);
    await get(`rest.device-${id}-assets`, `/api/devices/${id}/assets`);
    await get(`rest.device-${id}-custom-properties`, `/api/devices/${id}/custom-properties`);
    await get(`rest.device-${id}-scheduled-tasks`, `/api/devices/${id}/scheduled-tasks`);
  }
  return { soId };
}

// ---------------------------------------------------------------- N-central SOAP (reads only)
function soapEnvelope(op, inner) {
  return `<?xml version="1.0" encoding="UTF-8"?><soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/" xmlns:ei2="http://ei2.nobj.nable.com/"><soapenv:Header/><soapenv:Body><ei2:${op}>${inner}</ei2:${op}></soapenv:Body></soapenv:Envelope>`;
}
const xmlEscape = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

async function probeSoap(side, { host, user }, jwt, soId) {
  const url = `https://${host}/dms2/services2/ServerEI2`;
  const call = async (name, op, inner) => {
    const r = await http(url, { method: "POST", headers: { "Content-Type": "text/xml; charset=utf-8", SOAPAction: '""', Accept: "text/xml" }, body: soapEnvelope(op, inner) });
    const raw = r.text ?? JSON.stringify(r.json);
    // customerList returns registration tokens as key/value pairs; mask the value after any *registrationtoken key.
    const masked = redactText(raw, [jwt]).replace(/(registrationtoken<\/key>\s*<value>)([^<]+)/gi, (_, k, v) => `${k}<redacted len=${v.length}>`);
    const ok = r.status === 200 && !/Fault>/.test(raw);
    record(side, name, { ok, status: r.status, ms: r.ms, xml: masked.slice(0, 20000) });
    return ok;
  };
  await call("soap.versionInfoGet", "versionInfoGet", "");
  if (!jwt) return;
  let creds = `<ei2:username></ei2:username><ei2:password>${xmlEscape(jwt)}</ei2:password>`;
  if (!(await call("soap.customerList", "customerList", `${creds}<ei2:settings/>`))) {
    creds = `<ei2:username>${xmlEscape(user)}</ei2:username><ei2:password>${xmlEscape(jwt)}</ei2:password>`;
    await call("soap.customerList-with-username", "customerList", `${creds}<ei2:settings/>`);
  }
  if (soId) await call("soap.repositoryItemList", "repositoryItemList", `${creds}<ei2:customerId>${soId}</ei2:customerId><ei2:includeAncestorItems>true</ei2:includeAncestorItems>`);
}

// ---------------------------------------------------------------- Ecoverse GraphQL (reads only)
async function gql(key, query, variables = {}) {
  const r = await http(GQL_URL, { method: "POST", headers: { Authorization: `Bearer ${key}`, "Content-Type": "application/json" }, body: JSON.stringify({ query, variables }) });
  return { status: r.status, ms: r.ms, data: r.json?.data, errors: r.json?.errors, text: r.text };
}

async function probeEcoverse(side, { soName }, key) {
  if (!key) return record(side, "gql.auth", { ok: false, note: `no Keychain item ecoverse:${side}` });
  const me = await gql(key, "{ me { id } }");
  record(side, "gql.me", { ok: !!me.data?.me, ...me });

  const orgs = [];
  for (let after = null; ;) {
    const r = await gql(key, "query($after: String) { organizationSearch(first: 100, after: $after) { totalCount pageInfo { hasNextPage endCursor } nodes { id name __typename } } }", { after });
    const page = r.data?.organizationSearch;
    if (!page) { record(side, "gql.orgs", { ok: false, ...r }); return; }
    orgs.push(...page.nodes);
    if (!page.pageInfo.hasNextPage) break;
    after = page.pageInfo.endCursor;
  }
  const so = orgs.find((o) => o.name.toLowerCase() === soName.toLowerCase()) ?? orgs.find((o) => /n-voke/i.test(o.name));
  record(side, "gql.orgs", { ok: !!so, note: so ? `${so.name} = ${so.id} (${so.__typename}); ${orgs.length} orgs visible` : `no N-voke org among ${orgs.length}`, orgs });
  if (!so) return;

  const base = "id name customer { name } site { name } agentConnection { status statusChangedAt } operatingSystem { name }";
  const assets = await gql(key, `query($o: [ID!]!) { assetSearch(inOrganizations: $o, first: 100) { totalCount nodes { ${base} ncentralDevice { deviceId server { id origin } } } } }`, { o: [so.id] });
  record(side, "gql.assets", { ok: !!assets.data, ...assets, note: assets.data ? `${assets.data.assetSearch?.totalCount} assets` : undefined });

  // Introspection is off, so try candidate identity fields one at a time; error text often names the right field.
  const candidates = {
    "systemInfo": "systemInfo { hostname serialNumber macAddress }",
    "systemInfo.typename": "systemInfo { __typename }",
    "networkInterfaces": "networkInterfaces { macAddress }",
    "hostname": "hostname",
    "serialNumber": "serialNumber",
    "hardware": "hardware { serialNumber }",
    "ncentralDevice.full": "ncentralDevice { deviceId server { id origin } __typename }",
    "createdAt": "createdAt",
  };
  const results = {};
  for (const [label, sel] of Object.entries(candidates)) {
    const r = await gql(key, `query($o: [ID!]!) { assetSearch(inOrganizations: $o, first: 3) { nodes { id ${sel} } } }`, { o: [so.id] });
    results[label] = { selection: sel, ok: !r.errors, errors: r.errors?.map((e) => e.message), sample: r.data?.assetSearch?.nodes };
  }
  record(side, "gql.identity-fields", { ok: true, results });

  const intro = await gql(key, '{ __type(name: "Asset") { fields { name } } }');
  record(side, "gql.introspection", { ok: !intro.errors, note: intro.errors ? "disabled (expected)" : "enabled!", ...intro });

  const scripts = await gql(key, `query($o: [ID!]!) { scriptSearch(inOrganizations: $o, first: 50) { totalCount nodes { id name language inputs { name variable type } } } }`, { o: [so.id] });
  record(side, "gql.scripts", { ok: !!scripts.data, ...scripts });
}

// ---------------------------------------------------------------- main
const which = process.argv[2] ?? "all";
for (const side of which === "all" ? ["src", "dst"] : [which]) {
  const cfg = SIDES[side];
  const jwt = secret(`ncentral:${side}`);
  const eco = secret(`ecoverse:${side}`);
  try {
    const rest = await probeRest(side, cfg, jwt);
    await probeSoap(side, cfg, jwt, rest?.soId);
    await probeEcoverse(side, cfg, eco);
  } catch (e) {
    record(side, "fatal", { ok: false, note: String(e?.stack ?? e) });
  }
}
writeFileSync(join(outDir, "summary.json"), JSON.stringify(redact(log.map(({ side, name, ok, status, note, count, totalItems, keys }) => ({ side, name, ok, status, note, count, totalItems, keys }))), null, 2));
console.log(`\nWrote ${log.length} results to ${outDir}`);
