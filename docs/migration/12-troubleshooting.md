# Troubleshooting: the Ecoverse agent after a move

A device can be **Healthy** in N-central and still show *Waiting for Ecoverse data*: the Windows Agent checks in, but the Ecoverse agent (the UFA, service `MspAgent`) hasn't delivered the device's data. This page says what the device needs from the network, how to tell what is wrong, and what to do. The installer's own watcher handles the common cases ([UFA sync watchdog](05-endpoint-scripts.md#ufa-sync-watchdog)); this is for when it isn't enough or you want to see why.

Nothing here blocks the move itself: N-central health doesn't depend on Ecoverse.

## What the device needs from the network

All of it is outbound HTTPS (TCP 443) from the device. Nothing needs to reach into the device.

| To | Why | Seen in the lab |
|---|---|---|
| The destination N-central server | The installer download (`https://<server>/download/current/winnt/N-central/WindowsAgentSetup.exe`) and the Windows Agent's own traffic | Worked |
| The Ecoverse **auth service**, for example `auth.us-west-2.prd.cdo.system-monitor.com` | The Ecoverse agent's login-token request (`/oauth2/token`). Without a token it can't fetch its configuration, so no inventory component arrives | **The one that failed**: intermittent timeouts and connection resets |
| The Ecoverse **component CDN**, for example `cdn-component.fusion.prd.cdo.system-monitor.com` | The agent's own updates and the inventory components | Worked |
| The Ecoverse **provisioning service** | The agent's first certificate (`provision.log`) | Worked. Its host name isn't recorded in the logs we have; `provision.log` shows the requests |

The region in the host names (`us-west-2` here) belongs to this lab's tenant; read the real one from the agent's log (the line `Post "https://…/oauth2/token"`).

What to get right:

- **No TLS inspection or interception for these hosts.** The auth service wants the agent's client certificate during the handshake (mutual TLS); a bare `curl` with none gets `400 No required SSL certificate was sent`, which is what a *working* handshake looks like from a machine without the certificate. A device in the path that terminates or resets TLS is the likely cause of `400 The SSL certificate error` and of connection resets. This is the likely cause, not a proven one.
- **Allow the hosts by name**, not only by IP: the auth host resolved to several addresses.
- **The device's clock must be right.** Certificate checks fail on a skewed clock.
- The agent also tries the cloud-instance metadata address (`169.254.169.254`) and times out on any machine that isn't in a public cloud. That is normal and means nothing.

## Where to look

1. **In N-voke:** on the Track step, **Probe waiting (N)** probes every healthy device that is waiting for Ecoverse's data, one after another (the longest-waiting first, 20 per click; click again for the rest), and tells you how many are in each state. To look at one device, open it and choose **Probe**. Either way, read **Ecoverse agent on the device** in the device sheet. It shows the state in words, how old the Probe is, the restarts, and what the installer did (restarts, the one-time repair). The tracker doesn't probe by itself, so probe again to refresh.
2. **On the device:** run `ufa-log-summary.ps1` (in `scripts/migration/lab/probes/`) in an elevated PowerShell: `powershell -NoProfile -ExecutionPolicy Bypass -File ufa-log-summary.ps1`. It prints the login results (accepted, 400 SSL error, timeouts, resets), the last attempts and a `STATE:`. Read-only, no secrets.
3. **The raw files on the device:**
   - `C:\Program Files (x86)\Msp Agent\logs\msp-agent-core.log` and `provision.log`: the Ecoverse agent.
   - `C:\ProgramData\N-voke\Migration\status.json` and `logs\install-*.log`: the installer, kept for the review period.

## States, what they mean and what to do

| State (device sheet) | What the log shows | What the installer does | What you do |
|---|---|---|---|
| **Waiting for data** (`waiting`) | No failure and no inventory component yet | Nothing; watches. Data usually arrives within an hour of the install | Wait. Don't act inside the first hour |
| **Needs a restart** (`stalled`) | The first configuration fetch failed, the auth service answers again, nothing delivered since | Restarts `MspAgent`, at most twice, 15 minutes apart | Nothing, unless it stays: then restart it yourself (below) |
| **Can't reach the Ecoverse login service** (`blocked`) | Login-token requests time out or are reset | Watches it first (it often clears by itself). After 30 minutes it restarts `MspAgent` once; after 60 minutes, if a failure was logged after that restart, it does the one-time repair | Check the network path (below). If it clears, restart `MspAgent` once to fetch at once |
| **Login rejected** (`rejected`) | `400 The SSL certificate error` for the login, never accepted | After 30 minutes of it, the one-time repair. A restart doesn't help | If still refused after the repair: check TLS inspection, proxy and firewall on the path, then report it to N-able with the log |
| **Downloads refused** (`denied`) | The login works, but the component CDN answers the downloads with `403` | After 30 minutes, the one-time repair | If still refused after the repair, report it to N-able |
| **Being redeployed** | After the repair: the agent is gone and the Windows Agent is putting it back | Watches the `Msp Agent` folder for new files; restarts the Windows Agent services again (at most twice) only if the folder has been quiet for 10 minutes | Usually 5 to 15 minutes. Wait |
| **Sending data** (`synced`) | The inventory component arrived | Marks the device healthy | Nothing |

The installer watches for 360 minutes after the install once it has seen a fault (120 otherwise). After that it stops and leaves a `warning` in `status.json`; from then on it is yours.

## Checking the network path (a blocked or rejected agent)

On the device, in PowerShell or a command prompt. Use the auth host from the agent's log.

```powershell
Test-NetConnection auth.us-west-2.prd.cdo.system-monitor.com -Port 443
curl.exe -v https://auth.us-west-2.prd.cdo.system-monitor.com/oauth2/token
```

| Result | Means |
|---|---|
| `TcpTestSucceeded : True`, and `curl` ends with `400 No required SSL certificate was sent` | The handshake works. This is the healthy answer for a machine without the agent's certificate |
| `curl` fails with `Connection was reset` or `failed to receive handshake` | Something resets the TLS handshake: the network path (firewall, proxy, TLS inspection) or the service. It can be intermittent: in the lab the same request failed once and worked the next time |
| `TcpTestSucceeded : False` | Port 443 to that host is blocked |

Because the fault comes and goes, test it several times against each address before drawing a conclusion:

```powershell
foreach ($ip in '44.228.168.113','54.244.124.62','44.224.114.74') {   # the lab's addresses: use the ones `curl -v` prints for your host
  foreach ($i in 1..10) {
    $r = curl.exe -s -o NUL -w "%{http_code}" --max-time 15 --resolve "auth.us-west-2.prd.cdo.system-monitor.com:443:$ip" https://auth.us-west-2.prd.cdo.system-monitor.com/
    "$ip  try $i  -> $r"
  }
}
```

`400` means the handshake worked, `000` a reset or timeout. Mostly `400` means the path works now; `000` on one address only means that address is bad on your path; `000` at random on all of them points at the firewall or inspection device (look for resets to those addresses in its log) or at the service.

## Doing the repair by hand

Only for an agent that is `rejected` or `denied`, or `blocked` for a long time after a restart did not help. In an elevated prompt, in this order:

1. `"C:\Program Files (x86)\Msp Agent\msp-agent-core.exe" --uninstall`
2. Wait 120 seconds.
3. Restart *Windows Agent Maintenance Service*, wait 5 seconds, then restart *Windows Agent Service*.
4. Watch `C:\Program Files (x86)\Msp Agent` for new files. The agent should reappear within 5 to 15 minutes and provision a new certificate under the same agent GUID.

**Don't delete the `Msp Agent` folder or `msp-agent-core.exe`.** `--uninstall` leaves them behind, and in the lab the Windows Agent did not redeploy the agent when they were deleted by hand; it did once they were restored and both services were restarted.

To restart only the agent: `Restart-Service MspAgent`.

## When to involve N-able

- `rejected` or `denied` that is still there after the repair.
- `blocked` on devices whose network path tests clean, or on every device at once. That points at the service, not the network.

Give them the agent's `msp-agent-core.log` and `provision.log`, the device's `status.json`, and the results of the checks above.
