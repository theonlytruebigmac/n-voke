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
| **Not installed yet** (`notInstalled`) | There is no `MspAgent` service after the install. The Windows Agent normally deploys it a few minutes after itself | Waits up to 120 minutes and restarts nothing. If the agent is registered but its program is missing it reports *Registered but broken* instead (next row) | Usually nothing for the first hour. After that, see [The Ecoverse agent never appeared](#the-ecoverse-agent-never-appeared) |
| **Registered but broken** (`broken`) | The `MSP Core Agent` is registered, `msp-agent-core.exe` is missing and there is no `MspAgent` service, seen on two runs at least 10 minutes apart. The sheet shows the Windows Installer removal failures and the last MSI error (1721: the program could not be run) | Records `ufa-registration-broken` once and reports it on each wait line. Read-only: it restarts and removes nothing, because the Windows Agent can then only try to remove the registration and never installs a new agent | Recover by hand: [The Ecoverse agent never appeared](#the-ecoverse-agent-never-appeared) |
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

## The Ecoverse agent never appeared

Seen on SUP-2190 in the reverse move ([findings](10-phase0-findings.md#the-ecoverse-agent-that-never-came-back-after-a-reverse-move-2026-10-21)): the Windows Agent installed and checks in, but there is no `MspAgent` service and nothing comes back. The Windows Agent decides from what is **registered**: with nothing registered it installs a fresh agent; with an `MSP Core Agent` registered but no service it only tries to remove it, and never installs. That removal runs `msp-agent-core.exe --uninstall` from `C:\Program Files (x86)\Msp Agent`, so it fails (MSI error 1721) once that file is gone, five times, and then nothing happens. **Most likely cause** (inferred from timestamps): the old installer's cleanup stopped processes and deleted folders while the Ecoverse agent's own removal was still running, cutting it short (1722) and deleting the exe it needs. **The cleanup no longer does that** (it waits for the removal and touches none of the Ecoverse agent's folders or processes); a device moved with v0.2.3 or earlier can still end up like this.

**Confirm it, read-only, in an elevated PowerShell:**

```powershell
Get-WinEvent -FilterHashtable @{LogName='Application'; ProviderName='MsiInstaller'; StartTime=(Get-Date).Date} | Where-Object Message -match 'MSP Core Agent' | Sort-Object TimeCreated | Select-Object TimeCreated, Id, @{n='Message';e={$_.Message.Substring(0,[Math]::Min(150,$_.Message.Length))}} | Format-Table -Wrap
Select-String -Path C:\ProgramData\MspAgent\Tools\ufamsi.log -Pattern 'Error 1721','Error 1722' -Context 1,0 | Select-Object -First 4
```

Repeated *Removal failed* with **1721** (and an earlier **1722**) is this state. The Windows Agent's own account is in `UfaMaint.log` in its log folder: *Detected installed agent* followed by *Uninstall attempts count: [ n ]* (maximum 5) is the same state; *Did not detect installed agent* followed by *Starting to install* is the healthy path.

**To recover** (this repeats what worked in the lab at 14:24: removal completed once the exe was back, and the agent was installed again at 14:48; it is not something N-voke does, and what the Windows Agent does after its five attempts is unknown):

1. Restore `C:\Program Files (x86)\Msp Agent\msp-agent-core.exe` (copy the `Msp Agent` folder from a working device).
2. Remove the registration with its own uninstall: `msiexec /x {A14A5D24-6709-4571-A0E4-1769230C3267} /qn /L*V C:\temp\ufamsi2.log` (use the product code from the `/X{...}` in `UfaMaint.log`; it is the same MSI the Windows Agent's attempts use). Wait for it to return 0.
3. Restart *Windows Agent Maintenance Service*, wait 5 seconds, then restart *Windows Agent Service*, and watch `C:\Program Files (x86)\Msp Agent` for new files for about 15 minutes. With nothing registered, the Windows Agent should take the install path.
4. If it still doesn't come back, report it to N-able with `UfaMaint.log`, `ufamsi.log` and the device's `status.json`.

If the program and its folder are already gone (for example deleted by hand), step 2 cannot work, and the registration has to be removed by hand: the uninstall entry under `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{product code}` (and `WOW6432Node`), and the product's Windows Installer records under `HKLM\SOFTWARE\Classes\Installer\Products`, `...\Features`, `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData\S-1-5-18\Products` and the upgrade code's entry under `...\Installer\UpgradeCodes`. The names under `Installer` are the product and upgrade codes with the nibbles swapped (`{A14A5D24-6709-4571-A0E4-1769230C3267}` is `42D5A41A907617540A4E719632C02376`). Export every key first, list what exists before deleting, and run it as SYSTEM if a key says access denied. Microsoft's Program Install and Uninstall troubleshooter does the same for a broken entry and is the safer first try. Not tested by the project yet.

## A device reinstalled by hand

Uninstalling the agent and installing it again by hand, for example to recover a broken Ecoverse agent, gives the device a **new N-central device ID** on the destination, and the old record disappears. N-voke follows it: when the recorded destination record answers 404, the tracker drops that ID, writes a note on the device's timeline and finds the device again by name and identity (serial and MAC) under its mapped customer or site, so the next pass tracks the new record. You don't need to do anything, but it does need the new agent to be **registered under the same customer or site** the device was mapped to (use that customer's or site's registration token). If the new agent registers somewhere else, N-voke can't find it there. If it shows *On the new server* and does not move on after the new agent has checked in, open the device and look for the note about the gone record, and check where the new device is.

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
