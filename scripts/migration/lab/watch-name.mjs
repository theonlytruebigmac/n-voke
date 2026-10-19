// Watches a device by name across source/destination N-central and both Ecoverse tenants.
// Usage: node watch-name.mjs <device name>
import { SIDES as LAB, secret } from "./lab-config.mjs";
import { appendFileSync } from "node:fs";
const NAME = process.argv[2];
if (!NAME) throw new Error("usage: node watch-name.mjs <device name>");
const sec = (a) => secret(a);
const UA = { "User-Agent": "N-voke-phase0-probe/0.1" };
const SIDES = Object.fromEntries(Object.entries(LAB).map(([k, v]) => [k, { host: v.host, so: v.serviceOrgId, org: v.ecoverseOrgId }]));
const tokens = {};
async function rest(side, path) {
  const s = SIDES[side];
  if (!tokens[side] || Date.now() - tokens[side].at > 50 * 60e3) {
    const a = await (await fetch(`https://${s.host}/api/auth/authenticate`, { method: "POST", headers: { ...UA, Authorization: `Bearer ${sec("ncentral:" + side)}` } })).json();
    tokens[side] = { t: a.tokens.access.token, at: Date.now() };
  }
  return (await fetch(`https://${s.host}${path}`, { headers: { ...UA, Authorization: `Bearer ${tokens[side].t}` } })).json();
}
async function eco(side) {
  const r = await fetch("https://api.n-able.com/graphql", { method: "POST", headers: { ...UA, Authorization: `Bearer ${sec("ecoverse:" + side)}`, "Content-Type": "application/json" }, body: JSON.stringify({ query: `query($o:[ID!]!){ assetSearch(inOrganizations:$o, first: 50){ nodes { id name agentConnection { status } ncentralDevice { deviceId } } } }`, variables: { o: [SIDES[side].org] } }) });
  const a = (await r.json()).data?.assetSearch?.nodes?.filter((x) => x.name?.toUpperCase() === NAME.toUpperCase()) ?? [];
  return a.length ? a.map((x) => `${x.id.slice(0, 8)} ${x.agentConnection?.status} nc=${x.ncentralDevice?.deviceId}`).join(", ") : "none";
}
async function nc(side) {
  const d = ((await rest(side, `/api/org-units/${SIDES[side].so}/devices?pageSize=1000`)).data ?? []).filter((x) => x.longName?.toUpperCase() === NAME.toUpperCase());
  return d.length ? d.map((x) => `${x.deviceId} ${x.siteName} checkin=${x.lastApplianceCheckinTime ? "yes" : "never"}`).join(", ") : "none";
}
let last = "";
for (;;) {
  let line;
  try { line = `srcNC=${await nc("src")} | srcEco=${await eco("src")} | dstNC=${await nc("dst")} | dstEco=${await eco("dst")}`; } catch (e) { line = `error ${String(e).slice(0, 120)}`; }
  if (line !== last) { const out = `${new Date().toISOString().slice(11, 19)}Z ${line}`; console.log(out); appendFileSync(new URL(`./watch-${NAME}.log`, import.meta.url), out + "\n"); last = line; }
  await new Promise((r) => setTimeout(r, 20000));
}
