# AKS 오프라인 bulk load 실행 안내

> [English](README.md) | **한국어**

이 디렉터리는 수동으로 적용하는 Kubernetes Job을 포함합니다. 여기의 파일은
`deploy/azure/k8s/` 아래 일반 manifest 적용 루프에 포함되지 않습니다.

`bulkload-nexprice.yaml`은 **특정 데이터셋과 환경에 맞춘 예제**입니다. Namespace,
PVC 이름, registry 경로, node selector, resource 크기, schema 이름, CSV 경로를 매번
검토해야 합니다. 이 파일이 존재한다고 현재 AKS cluster가 실행 중이라는 뜻은 아닙니다.

## 안전 모델

`byoridb-bulkloader`는 서버와 같은 redb 데이터베이스를 열고 key를 직접 씁니다.
HTTP, gRPC, session, nGQL 검증을 우회합니다.

- Loader를 시작하기 전에 대상 redb를 열 수 있는 모든 프로세스를 중지하세요.
  Kubernetes `ReadWriteOnce`만으로 프로세스 단위 배타성이 보장되지는 않습니다.
- StatefulSet을 0으로 줄이면 서비스가 중단됩니다. 운영자 승인과 검증된 backup 또는
  volume snapshot을 먼저 확보하세요.
- 이 절차에서 PVC를 삭제하거나 교체하지 마세요.
- `--durability=relaxed`는 재생성 가능한 import에만 사용하세요. Crash 시 최근 batch가
  유실될 수 있습니다. 정상 serving에는 서버 기본값인 immediate durability를 사용합니다.
- 예제는 lenient 모드입니다. 중복 node ID와 dangling edge를 집계하고 건너뜁니다.
  둘 중 하나라도 import를 중단해야 한다면 `--strict`를 추가하세요.

## Loader 동작

Loader는 다음 순서로 동작합니다.

1. redb에서 기존 space, tag, edge schema를 읽습니다.
2. Node CSV를 읽으며 순차 `INT64` VID를 할당합니다.
3. 원본 ID-to-VID mapping을 영속 저장합니다.
4. 모든 node 이후 edge CSV를 읽고 forward/reverse edge와 degree counter를 씁니다.
5. CSV column을 property로 보존하고 선언된 schema type이 있으면 값을 변환합니다.

일반 CSV와 gzip CSV를 지원합니다. 기본 column은 `id`, `src`, `dst`이며
`--id-column`, `--src-column`, `--dst-column`으로 바꿀 수 있습니다.

정확한 edge type `sameAs`는 엔진의 되돌릴 수 없는 `owl:sameAs` canonical merge에
예약되어 있습니다. 예제 데이터셋은 따라서 `same_as`를 사용합니다.

## 사전 점검

모든 명령은 저장소 root에서 실행합니다. 예제는 namespace `byoridb`, StatefulSet
`byoridb-server`를 가정하므로 환경에 맞게 변경하세요.

1. Maintenance window와 backup/restore 절차를 확인합니다.
2. 적용 전 Job을 검토합니다.

   ```bash
   kubectl apply --dry-run=client -f deploy/azure/jobs/bulkload-nexprice.yaml
   kubectl -n byoridb get statefulset byoridb-server
   kubectl -n byoridb get pvc data-byoridb-server-0
   ```

3. Scale down 전에 정확한 배포 image를 기록합니다.

   ```bash
   DEPLOYED_IMAGE=$(kubectl -n byoridb get statefulset byoridb-server \
     -o jsonpath='{.spec.template.spec.containers[0].image}')
   test -n "$DEPLOYED_IMAGE"
   echo "$DEPLOYED_IMAGE"
   ```

4. 해당 image에 `/usr/local/bin/byoridb-bulkloader`가 있는지 확인합니다. 날짜가 적힌
   문서가 아니라 image 내용이 진실원입니다.
5. Job이 참조하는 경로에 CSV를 업로드합니다. 큰 파일은 storage-native 전송 방식을
   사용하고 업로드 후 checksum을 비교하세요.

## Schema 준비

Loader는 metadata를 읽지만 schema를 만들지 않습니다. 서버가 실행 중일 때 Job이
참조하는 target space와 모든 tag/edge를 먼저 생성합니다.

예제는 `INT64` space와 다음 이름을 기대합니다.

```ngql
CREATE SPACE IF NOT EXISTS nexprice (vid_type = INT64);
USE nexprice;

CREATE TAG IF NOT EXISTS sku(...);
CREATE TAG IF NOT EXISTS product(...);
CREATE TAG IF NOT EXISTS brand(...);
CREATE TAG IF NOT EXISTS category(...);
CREATE TAG IF NOT EXISTS channel(...);

CREATE EDGE IF NOT EXISTS same_as();
CREATE EDGE IF NOT EXISTS sold_on();
CREATE EDGE IF NOT EXISTS in_category();
CREATE EDGE IF NOT EXISTS has_brand();
CREATE EDGE IF NOT EXISTS child_of();
```

`...`는 CSV header와 type에 맞는 실제 property 정의로 바꾸세요. Placeholder를
프로덕션 쿼리에 그대로 복사하면 안 됩니다.

기존 space 삭제는 파괴적이므로 이 문서에 포함하지 않습니다. Clean re-import를 위해
필요하다면 검증된 backup과 별도 검토가 있는 작업으로 수행하세요.

## Job 실행

1. 서버를 중지하고 server pod가 남아 있지 않은지 확인합니다.

   ```bash
   kubectl -n byoridb scale statefulset byoridb-server --replicas=0
   kubectl -n byoridb rollout status statefulset/byoridb-server --timeout=180s
   kubectl -n byoridb get pods -l app.kubernetes.io/name=byoridb
   ```

2. Check-in manifest를 수정하지 않고 기록한 server image로 Job을 render한 뒤 적용합니다.

   ```bash
   kubectl set image -f deploy/azure/jobs/bulkload-nexprice.yaml \
     bulkloader="$DEPLOYED_IMAGE" --local -o yaml \
     | kubectl apply -f -
   ```

   Kubernetes Job pod template은 immutable입니다. 같은 이름의 과거 Job이 있으면 먼저
   status와 log를 확인하세요. 명시적으로 확인한 뒤 해당 Job object만 삭제하고 새
   manifest를 적용합니다.

3. 진행 상황과 최종 상태를 확인합니다.

   ```bash
   kubectl -n byoridb logs -f job/byoridb-bulkload-nexprice
   kubectl -n byoridb wait --for=condition=complete \
     job/byoridb-bulkload-nexprice --timeout=24h
   kubectl -n byoridb get job/byoridb-bulkload-nexprice -o wide
   ```

최종 summary에는 vertex, tag-to-VID entry, edge, duplicate ID, dangling edge가
표시됩니다. Lenient 모드에서도 0이 아닌 duplicate/dangling 개수는 데이터 품질 신호로
처리하세요.

예제는 전체 keyspace를 scan하는 `--verify`를 생략합니다. 작거나 중간 규모 import에서
활성화하고, 큰 import 후에는 작은 범위의 실제 쿼리로 검증하세요.

## 서비스 복귀

Job pod가 종료되고 redb를 잡고 있는 프로세스가 없기 전에는 서버를 시작하지 마세요.

```bash
kubectl -n byoridb scale statefulset byoridb-server --replicas=1
kubectl -n byoridb rollout status statefulset/byoridb-server --timeout=600s
kubectl -n byoridb port-forward statefulset/byoridb-server 19669:19669
```

다른 shell에서 실행합니다.

```bash
curl --fail http://127.0.0.1:19669/health
curl --fail http://127.0.0.1:19669/ready
```

인증 후 데이터셋별 count와 작은 forward/reverse traversal 표본을 실행하세요. Loader
로그만 믿지 말고 독립적인 source file 집계와 비교합니다.

## 실패 처리

- Job 실패 시 loader pod가 종료될 때까지 서버를 중지한 상태로 둡니다.
- Log, Job YAML, image digest, CSV checksum, 최종 counter를 보존합니다.
- Relaxed-durability partial import는 재실행하거나 preflight snapshot으로 복구해야 할
  수 있습니다. 데이터셋의 문서화된 idempotency를 기준으로 선택하세요.
- redb가 다시 열리지 않아도 PVC를 삭제하지 마세요. 검증된 recovery 절차로
  escalation하고 가능하면 copy 또는 snapshot에서 작업하세요.
