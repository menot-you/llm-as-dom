---
name: Kata + Ollama + lad-runner Stack (2026-04-15 session)
description: Complete infrastructure added in one ~4h session — microVM runners for browser CI, NVIDIA device plugin, Ollama GPU inference, baked custom runner image. All learnings, failure modes, and the state the next session inherits.
type: project
---

## What was built (commits)

- **infra-gitops**
  - `212733a` — Kata runtime class + NVIDIA device plugin + Ollama (first shot)
  - `5a4f51d` → `fd25504` → `2cc86d6` — NVIDIA device plugin iteration (envvar, strategy, privileged hostPath, back to runtime)
  - `4b0d8d5` → `746b334` — Talos sysctl `max_user_namespaces` tried then reverted (didn't solve the LSM block)
  - `73b9188` — Talos resolvers → 1.1.1.1/1.0.0.1/8.8.8.8 (router was flaking UDP/53)
  - `91b723d` — bump llm-as-dom-kata-runners maxRunners 2→4
  - `61458b0` — bake pipeline: Dockerfile + `.github/workflows/build-runner-image.yml`
  - `bd2f328` — HelmRelease swap: llm-as-dom-kata-runners → `ghcr.io/menot-you/lad-runner:latest`
  - `bf9b635` — sonhos-de-ninar-kata-runners scale-set
- **llm-as-dom**
  - `78c6eaa2` — drops apt install from fixture-tests + integration (image bakes them)
- **sonhos-de-ninar**
  - `eb58a66` — Visual Regression Playwright routes to `sonhos-de-ninar-kata-runners`
- **Talos machine config** (applied live via `talosctl patch mc`; source-of-truth in `talos/patches/nott-prod.yaml`)
  - Installer image SHA → `13cec58d6d79e06412f7f9070b43345332ea214b46e47daa89ac932e1fe53686` (v1.12.6)
  - Extensions added: `kata-containers`, `amd-ucode`, `nvme-cli`
  - Kept: `nonfree-kmod-nvidia-production`, `nvidia-container-toolkit-production`, `tailscale`, `iscsi-tools`, `util-linux-tools`
  - Resolvers: `[1.1.1.1, 1.0.0.1, 8.8.8.8]`

## Root causes hit during the session (remember these)

1. **Talos default `user.max_user_namespaces=0`**. Chromium zygote needs CLONE_NEWUSER. The kernel sysctl alone is NOT enough — Talos also has an LSM layer (SELinux `selinux=1` on the cmdline) that denies namespace creation from pods. Verified: raising sysctl to 15000 + seccomp Unconfined still yields EPERM on `unshare(CLONE_NEWPID)`. **The only fix was Kata (microVM with its own kernel).**

2. **AMD SVM disabled in BIOS**. Ryzen 9 5950X on ASUS Dark Hero V shipped with AMD SVM off. Without it `kvm_amd: SVM not supported` and `/dev/kvm` absent → Kata cannot launch microVMs (`CLH is not running`). **User had to physically enter BIOS and enable "AMD SVM Mode".**

3. **ASUS BIOS side effects**. Toggling SVM on Dark Hero also hid NVMe boot entries:
   - NVMe disappears from Boot list if `NVMe RAID mode` got toggled on adjacent to SVM. Fix: **Advanced → AMD PBS → NVMe RAID mode → Disabled**.
   - Even after fixing RAID mode, UEFI boot entries were missing from the persistent boot list but present in **Boot Override**. Rebooting via Boot Override restored the registration.
   - Talos uses A/B slot boot — pick slot B after upgrade (newer image).

4. **NVIDIA libnvidia-ml.so is NOT on the Talos host**. The `nonfree-kmod-nvidia-production` extension ships kernel driver + `nvidia-persistenced` only. Userspace libs (libnvidia-ml, libcuda) are injected into containers by `nvidia-container-runtime` — **only** when pod has `runtimeClassName: nvidia` AND `NVIDIA_VISIBLE_DEVICES=all` env. The post-SVM reboot also somehow unblocked this path; before the reboot only nvidia-uvm + nvidiactl were injected (not nvidia0), after reboot the full set appeared.

5. **LAN router DNS flake**. Home router `192.168.8.1` was timing out on UDP/53 under load. Talos `dns-resolve-cache` spammed `i/o timeout` every 2s, and apt-get inside Kata microVMs died because it couldn't resolve archive.ubuntu.com. Fixed by switching upstream DNS to public resolvers in Talos config.

6. **ARC ghost job assignments**. Repeatedly after pod delete, the next spawned runner registers to GitHub, logs "Listening for Jobs", gets an EphemeralRunner CR with a JOBID assigned, but GitHub never actually dispatches the payload. Workaround: `kubectl delete ephemeralrunner <name>` — the new pod gets the real job. Known ARC issue, no permanent fix in this session.

## State inherited by next session

- Talos node `nott-prod` (100.77.38.120 tailscale, 192.168.8.229 LAN) is single-node control-plane, `Ready`.
- Kata RuntimeClass `kata-qemu` (handler `kata`) exists. Default hypervisor is **Cloud Hypervisor** (not QEMU as the name suggests — the config at `/usr/local/share/kata-containers/configuration.toml` uses `[hypervisor.clh]`). Works fine; could be swapped to QEMU via pod annotation `io.katacontainers.config.hypervisor.type = qemu` if needed.
- NVIDIA RuntimeClass `nvidia` + device plugin DaemonSet advertise `nvidia.com/gpu: 1`. Pods that want GPU need **both** `runtimeClassName: nvidia` AND `resources.limits: nvidia.com/gpu: 1`.
- Ollama deployment on `ollama` namespace, pod `ollama-xxx`, ClusterIP svc `ollama:11434`. Model `llama3.2:3b` pre-pulled, ~190 tok/s on RTX 3090.
- Two Kata scale-sets: `llm-as-dom-kata-runners` (uses baked image, maxRunners=4) and `sonhos-de-ninar-kata-runners` (stock image, maxRunners=2).
- Three non-Kata scale-sets kept light (hardened): `llm-as-dom-runners`, `sonhos-de-ninar-runners`, plus the per-repo ones for nott/containers/apple-store-connect/infra-gitops.

## Open follow-ups (handoff C)

1. **Sops-encrypt the `ghcr-pull` docker-registry secret**. It was created manually with `kubectl create secret` from the `github-pat` token. If the ns is wiped, it won't come back. Should live in `apps/gha-runners/ghcr-pull.sops.yaml` encrypted with the cluster's age key (see existing `.sops.yaml` in infra-gitops root).
2. **Write a proper README** for `runners/images/llm-as-dom/` describing how to bump versions, test locally, rollback.
3. **Move sonhos-de-ninar-runners Lint/Typecheck/Accessibility failures** off the session's plate — those were legit code bugs in sonhos, not infra.
4. **GHCR package visibility** — `menot-you/lad-runner` is private. Making it public is a manual UI action at `github.com/orgs/menot-you/packages/container/lad-runner/settings`. Until then we need the pull secret per namespace.
5. **Consider a second Kata hypervisor runtime** (`kata-qemu` via annotation) only if CLH proves unreliable for a specific workload. CLH has been fine for every test so far.
6. **Check sonhos Playwright snapshot baseline** — first Kata run may need new snapshots regenerated (browser is slightly different OS/render pipeline).

## How to apply
Read this before making infra changes to nott-prod. Talos is immutable — extension changes require factory.talos.dev schematic regen + `talosctl upgrade`. Sysctl changes apply live via `talosctl patch mc`. Kata/NVIDIA are declarative via Flux Kustomization.
