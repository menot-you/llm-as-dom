---
name: nott-prod cluster — operational reference
description: Single-node Talos k3s on Ryzen 9 5950X. IPs, extensions, runtimes, known gotchas, day-to-day commands. Read this before operating on nott-prod.
type: reference
---

## Topology
- **Node**: `nott-prod`, single control-plane, k3s v1.35.2 on Talos v1.12.6 (kernel 6.18.18-talos)
- **Addresses**: LAN `192.168.8.229/24` (enp6s0), Tailscale `100.77.38.120`, flannel cni `10.244.0.1/24`
- **kubeconfig context**: `admin@nott` (via Tailscale)
- **Gateway**: 192.168.8.1 (router) — **DO NOT use for DNS upstream**, it flakes under load. Public resolvers in Talos config.
- **Hardware**: Ryzen 9 5950X (32 threads), 125 GiB RAM, 2× 1.8T NVMe, RTX 3090

## Talos installer schematic
- URL: `factory.talos.dev/installer/<sha>:v1.12.6`
- Current SHA: `13cec58d6d79e06412f7f9070b43345332ea214b46e47daa89ac932e1fe53686`
- Extensions bundled: `nonfree-kmod-nvidia-production`, `nvidia-container-toolkit-production`, `tailscale`, `iscsi-tools`, `util-linux-tools`, `kata-containers`, `amd-ucode`, `nvme-cli`
- To add extensions: regenerate schematic via POST to `https://factory.talos.dev/schematics` with YAML body, then `talosctl upgrade --image ...`. Triggers node reboot.

## BIOS prerequisites (ASUS Dark Hero V)
- **AMD SVM Mode**: must be Enabled (required for KVM → Kata). Path: Advanced → AMD CBS → CPU Common Options → SVM Mode.
- **NVMe RAID mode**: must be Disabled. Path: Advanced → AMD PBS → NVMe RAID mode. If enabled, NVMe disappears from NVMe Configuration + Boot list (shows in RAIDXpert2 only).
- **Boot persistence**: major BIOS changes can wipe NVRAM entries. Recovery via **Save & Exit → Boot Override**, or add manually via `\EFI\BOOT\BOOTX64.EFI`.

## Container runtimes (containerd)
- **`runc`** (default): PodSecurity restricted, no user-ns (Talos LSM denies even with sysctl 15000). Fast, light.
- **`kata`** (handler): Cloud Hypervisor microVM per pod, own kernel. ~150MB RAM overhead. Use when workload needs unshare() / user-ns (Chromium, rootless builds). RuntimeClass `kata-qemu` (misleading name — handler routes through CLH).
- **`nvidia`** (handler): injects `/dev/nvidia*` + libnvidia-ml into pod. Requires `NVIDIA_VISIBLE_DEVICES=all` env. Use for GPU workloads.

## NVIDIA stack
- Kernel driver: loaded at Talos boot from `nonfree-kmod-nvidia-production` extension.
- `/dev/nvidia0` exists on host, mode `crw-rw-rw-`.
- `nvidia-persistenced` runs as Talos ext service (keeps GPU initialized).
- `libnvidia-ml.so` is **NOT** on host filesystem — only injected into containers that use `runtimeClassName: nvidia`.
- Device plugin advertises `nvidia.com/gpu: 1` on the node.
- 1× RTX 3090 (24GiB VRAM), idle 38°C, 24W.

## DNS
- **Talos upstream**: 1.1.1.1, 1.0.0.1, 8.8.8.8 (set via `machine.network.nameservers`)
- **CoreDNS cluster**: 10.96.0.10
- Search domains: cluster defaults
- If you see `error serving dns request [...] read udp [...] 192.168.8.1:53: i/o timeout` in Talos logs — that was the router, now bypassed.

## ARC (actions-runner-controller)
- Namespace: `actions-runner-system`
- Scale-sets: light (runc) + kata (per repo, for browser workloads)
- Known issue: **ghost job assignments**. After deleting a runner pod, next pod registers but GitHub never dispatches the payload. Workaround: `kubectl delete ephemeralrunner <name>` to recycle.
- Pull secret for private ghcr images: `ghcr-pull` (manually created from github-pat token, TODO sops-encrypt).

## Common commands
```bash
# Node state
kubectl --context admin@nott get nodes -o wide

# Talos control
talosctl --nodes 100.77.38.120 <get|read|service|patch|upgrade>

# Flux state
flux --context admin@nott get kustomizations

# Force reconcile
flux --context admin@nott reconcile source git flux-system
flux --context admin@nott reconcile kustomization <name> --with-source

# Runner state
kubectl --context admin@nott -n actions-runner-system get pods,autoscalingrunnerset,ephemeralrunner

# GPU check
kubectl --context admin@nott get node nott-prod -o jsonpath='{.status.allocatable.nvidia\.com/gpu}'

# Ollama
kubectl --context admin@nott -n ollama port-forward svc/ollama 11434:11434
curl -s http://localhost:11434/api/generate -d '{"model":"llama3.2:3b","prompt":"..."}'
```
