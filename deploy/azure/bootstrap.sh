#!/usr/bin/env bash
# Idempotent bootstrap for ByoriDB on Azure AKS.
#
# Resolves the operational pitfalls observed during the 2026-05-13 deploy:
#   1. `az aks create` with `--attach-acr` + `--enable-managed-identity` cannot
#      be combined with `--no-wait`. We create the cluster first, then attach
#      the ACR in a separate, sequential step.
#   2. Any cluster-level update (e.g. `--attach-acr`) holds an operation lock
#      that blocks `az aks nodepool add` with `OperationNotAllowed`. We wait
#      for the cluster to return to `Succeeded` before adding the DB pool.
#   3. The LB Service is exposed on the public internet — `loadBalancerSourceRanges`
#      is rendered from the caller's current public IP unless `BYORIDB_LB_ALLOWED_CIDR`
#      is preset, so the cluster is never deployed with 0.0.0.0/0 open.
#
# Usage:
#   bash deploy/azure/bootstrap.sh
#
# Environment overrides (all optional, sensible defaults below):
#   BYORIDB_LOCATION         (default: koreacentral)
#   BYORIDB_RG               (default: byoridb-prod-rg)
#   BYORIDB_VNET             (default: byoridb-vnet)
#   BYORIDB_AKS              (default: byoridb-aks)
#   BYORIDB_ACR              (default: derived from RG, lowercased, hash suffix)
#   BYORIDB_NODE_VM          (default: Standard_D2s_v5)
#   BYORIDB_DB_NODE_COUNT    (default: 1)
#   BYORIDB_LB_ALLOWED_CIDR  (default: $(curl ifconfig.me)/32)
#   BYORIDB_K8S_DIR          (default: deploy/azure/k8s)

set -euo pipefail

LOCATION="${BYORIDB_LOCATION:-koreacentral}"
RG="${BYORIDB_RG:-byoridb-prod-rg}"
VNET="${BYORIDB_VNET:-byoridb-vnet}"
SUBNET_AKS="aks-subnet"
AKS="${BYORIDB_AKS:-byoridb-aks}"
NODE_VM="${BYORIDB_NODE_VM:-Standard_D2s_v5}"
DB_NODES="${BYORIDB_DB_NODE_COUNT:-1}"
K8S_DIR="${BYORIDB_K8S_DIR:-deploy/azure/k8s}"

# ACR names are global; derive a stable one from the RG, lowercased, alnum-only.
ACR_DEFAULT="$(echo "byoridbacr${RG}" | tr -cd '[:alnum:]' | tr '[:upper:]' '[:lower:]' | cut -c1-50)"
ACR="${BYORIDB_ACR:-$ACR_DEFAULT}"

# Image version: read from Cargo.toml (workspace root), override with env var.
# Convention: 0.x.y during development, 1.0.0 at GA release.
IMG_VERSION="${BYORIDB_IMG_VERSION:-$(grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\(.*\)"/\1/')}"

step() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }
note() { printf '   - %s\n' "$*"; }
exists() { az "$@" >/dev/null 2>&1; }

require() {
  command -v "$1" >/dev/null 2>&1 || { echo "missing required tool: $1" >&2; exit 1; }
}
require az
require kubectl
require curl
require openssl

step "0/8  Pre-flight: subscription + providers"
SUB="$(az account show --query name -o tsv)"
note "subscription: $SUB"
for prov in Microsoft.ContainerService Microsoft.ContainerRegistry Microsoft.Network; do
  state="$(az provider show -n "$prov" --query registrationState -o tsv 2>/dev/null || echo NotRegistered)"
  if [[ "$state" != "Registered" ]]; then
    note "registering provider $prov ..."
    az provider register -n "$prov" >/dev/null
  fi
done

step "1/8  Resource group ($RG)"
if exists group show -n "$RG"; then
  note "exists"
else
  az group create -n "$RG" -l "$LOCATION" -o none
fi

step "2/8  VNet + subnet ($VNET / $SUBNET_AKS)"
if exists network vnet show -g "$RG" -n "$VNET"; then
  note "exists"
else
  az network vnet create -g "$RG" -n "$VNET" \
    --address-prefix 10.20.0.0/16 \
    --subnet-name "$SUBNET_AKS" --subnet-prefix 10.20.0.0/22 -o none
fi
SUBNET_ID="$(az network vnet subnet show -g "$RG" --vnet-name "$VNET" -n "$SUBNET_AKS" --query id -o tsv)"

step "3/8  ACR ($ACR)"
if exists acr show -n "$ACR"; then
  note "exists"
else
  az acr create -g "$RG" -n "$ACR" --sku Standard --admin-enabled false -o none
fi
ACR_LOGIN_SERVER="$(az acr show -n "$ACR" --query loginServer -o tsv)"
note "login server: $ACR_LOGIN_SERVER"

step "4/8  Container image (byoridb-server:${IMG_VERSION})"
if az acr repository show-tags -n "$ACR" --repository byoridb-server -o tsv 2>/dev/null | grep -qxF "$IMG_VERSION"; then
  note "image ${IMG_VERSION} already present — skipping build"
else
  note "building (this takes ~20 min on a cold cache)"
  az acr build -r "$ACR" -t "byoridb-server:${IMG_VERSION}" -t byoridb-server:latest -f Dockerfile . -o none
fi

step "5/8  AKS cluster ($AKS)"
if exists aks show -g "$RG" -n "$AKS"; then
  note "exists"
else
  az aks create -g "$RG" -n "$AKS" \
    --location "$LOCATION" \
    --node-count 1 \
    --node-vm-size "$NODE_VM" \
    --vnet-subnet-id "$SUBNET_ID" \
    --network-plugin azure --network-plugin-mode overlay \
    --pod-cidr 10.244.0.0/16 \
    --enable-managed-identity \
    --enable-oidc-issuer --enable-workload-identity \
    --tier free \
    --generate-ssh-keys -o none
fi

# Wait for any pending cluster operation to settle BEFORE running the next
# cluster-level mutation (attach-acr). This avoids OperationNotAllowed.
note "waiting for cluster ProvisioningState=Succeeded"
until [[ "$(az aks show -g "$RG" -n "$AKS" --query provisioningState -o tsv)" == "Succeeded" ]]; do
  sleep 10
done

# Attach ACR — must NOT be inlined into `aks create` if we want `--no-wait`.
ATTACHED_ACR_ID="$(az aks show -g "$RG" -n "$AKS" --query "servicePrincipalProfile.clientId" -o tsv 2>/dev/null || true)"
if ! az aks check-acr -g "$RG" -n "$AKS" --acr "$ACR" >/dev/null 2>&1; then
  note "attaching ACR (separate step, blocking)"
  az aks update -g "$RG" -n "$AKS" --attach-acr "$ACR" -o none
  until [[ "$(az aks show -g "$RG" -n "$AKS" --query provisioningState -o tsv)" == "Succeeded" ]]; do
    sleep 10
  done
else
  note "ACR already attached"
fi

step "6/8  DB node pool (dbpool, $DB_NODES × $NODE_VM)"
if az aks nodepool show -g "$RG" --cluster-name "$AKS" -n dbpool >/dev/null 2>&1; then
  note "exists"
else
  az aks nodepool add -g "$RG" --cluster-name "$AKS" -n dbpool \
    --node-count "$DB_NODES" --node-vm-size "$NODE_VM" \
    --node-taints workload=byoridb:NoSchedule \
    --labels workload=byoridb \
    --zones 1 -o none
fi

step "7/8  kubeconfig + namespace + secrets"
az aks get-credentials -g "$RG" -n "$AKS" --overwrite-existing >/dev/null
kubectl apply -f "$K8S_DIR/00-namespace.yaml" >/dev/null

# Root password — only generated on first run, never overwritten.
if ! kubectl -n byoridb get secret byoridb-root >/dev/null 2>&1; then
  PW="$(openssl rand -base64 24)"
  kubectl -n byoridb create secret generic byoridb-root \
    --from-literal=BYORIDB_ROOT_PASSWORD="$PW" >/dev/null
  printf '   - root password (store this!): %s\n' "$PW"
else
  note "byoridb-root secret already present — not regenerating"
fi

step "8/8  Apply manifests (ConfigMap, StatefulSet, Services)"

# Render ACR login server + caller IP into the manifests at apply time so the
# checked-in files stay environment-agnostic.
ALLOWED_CIDR="${BYORIDB_LB_ALLOWED_CIDR:-$(curl -fsS https://ifconfig.me)/32}"
note "LB allowed CIDR: $ALLOWED_CIDR"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
cp "$K8S_DIR"/*.yaml "$tmpdir/"
# StatefulSet image: replace ACR login server and tag with current version.
sed -i.bak -E "s#image: .*/byoridb-server:.*#image: $ACR_LOGIN_SERVER/byoridb-server:$IMG_VERSION#g" "$tmpdir/03-statefulset.yaml"
# Service source ranges: replace whatever IP is currently hard-coded.
sed -i.bak -E "s#- [0-9.]+/32#- $ALLOWED_CIDR#g" "$tmpdir/04-services.yaml"

kubectl apply -f "$tmpdir/00-namespace.yaml"
kubectl apply -f "$tmpdir/01-configmap.yaml"
kubectl apply -f "$tmpdir/03-statefulset.yaml"
kubectl apply -f "$tmpdir/04-services.yaml"

note "waiting for Pod readiness (up to 3 min)"
kubectl -n byoridb rollout status statefulset/byoridb-server --timeout=180s

EXTERNAL_IP="$(kubectl -n byoridb get svc byoridb-public -o jsonpath='{.status.loadBalancer.ingress[0].ip}')"
printf '\n\033[1;32mByoriDB deployed.\033[0m\n'
printf '  HTTP:  http://%s:19669\n' "$EXTERNAL_IP"
printf '  gRPC:  %s:9669\n' "$EXTERNAL_IP"
printf '  AllowedCIDR: %s\n' "$ALLOWED_CIDR"
