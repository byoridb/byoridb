# 백업 및 복원

ByoriDB는 `byoridb-backup` CLI로 redb 전체 스냅샷을 생성하고 복원합니다. 백업은
current view(`kv`)와 bitemporal history(`history`)를 모두 포함하므로 복원 후에도
`FETCH ... AS OF` 이력이 유지됩니다.

현재 구현은 데이터베이스 전체의 full snapshot만 지원합니다. space 단위, 증분 백업,
WAL 아카이빙, 클라우드 저장소 직접 전송과 특정 시각으로의 point-in-time recovery는
지원하지 않습니다.

## CLI 실행

릴리스 바이너리를 설치했다면 `byoridb-backup`을 직접 실행합니다. 소스 트리에서는 각
명령 앞의 `byoridb-backup`을 다음으로 바꿀 수 있습니다.

```bash
cargo run --locked --release --bin byoridb-backup --
```

`--db`는 `data.redb` 파일 자체가 아니라 그 파일을 담은 데이터 디렉터리입니다. 기본
서버 설정을 쓴다면 보통 `data/storage`입니다.

## 백업 생성

```bash
byoridb-backup create \
  --db data/storage \
  --backup-dir /backup/byoridb \
  --label daily
```

CLI는 `backup_<unix-seconds>` ID를 출력하고
`/backup/byoridb/<backup_id>/data.redb`와 metadata를 만듭니다. source redb의 MVCC read
snapshot에서 두 테이블을 복사하므로 백업 내부는 한 시점으로 일관됩니다.

백업 ID의 해상도는 1초입니다. 같은 디렉터리에 여러 백업을 한 초 안에 만들지 마세요.

## 조회와 검증

```bash
# 최신순 목록
byoridb-backup list --backup-dir /backup/byoridb

# JSON 목록
byoridb-backup list --backup-dir /backup/byoridb --format json

# 특정 백업 metadata
byoridb-backup info \
  --backup-dir /backup/byoridb \
  --backup-id <backup_id>

# 하나 또는 전체 백업이 CLI에서 정상 인식되는지 검사
byoridb-backup verify \
  --backup-dir /backup/byoridb \
  --backup-id <backup_id>
byoridb-backup verify --backup-dir /backup/byoridb
```

`verify`는 backup metadata와 구조를 읽을 수 있는지 확인합니다. 실제 복구 가능성까지
검증하려면 격리된 디렉터리에 정기적으로 복원한 뒤 서버를 열고 current/`AS OF` 쿼리를
실행하세요.

## 복원

운영 중인 데이터 디렉터리에 직접 덮어쓰지 말고 먼저 별도 경로에 복원합니다.

```bash
byoridb-backup restore \
  --backup-dir /backup/byoridb \
  --backup-id <backup_id> \
  --target /var/lib/byoridb-restored
```

target이 이미 존재하면 기본적으로 실패합니다. 기존 target을 교체할 의도가 명확한
경우에만 `--overwrite`를 추가하세요. 복원 결과를 검증한 뒤 서버가 참조하는 데이터
경로를 계획적으로 전환합니다.

## 보존 정책

최근 N개만 남기는 cleanup과 특정 백업 삭제를 제공합니다. 자동화에서는 확인 prompt를
피하려고 `--force`를 명시합니다.

```bash
byoridb-backup cleanup \
  --backup-dir /backup/byoridb \
  --keep 7 \
  --force

byoridb-backup delete \
  --backup-dir /backup/byoridb \
  --backup-id <backup_id> \
  --force
```

cron 예시:

```cron
0 2 * * * /usr/local/bin/byoridb-backup create --db /var/lib/byoridb --backup-dir /backup/byoridb --label daily
30 2 * * * /usr/local/bin/byoridb-backup verify --backup-dir /backup/byoridb
0 3 * * * /usr/local/bin/byoridb-backup cleanup --backup-dir /backup/byoridb --keep 7 --force
```

## 운영 주의사항

- 백업 디렉터리를 도구가 새로 만들면 Unix에서 mode `0700`을 설정합니다. 이미 존재하는
  디렉터리는 운영자가 소유권과 권한을 확인해야 합니다.
- 원본 데이터와 다른 디스크/호스트에 사본을 복제하고 필요하면 저장소 계층에서
  암호화하세요. CLI 자체는 S3 업로드나 백업 암호화를 제공하지 않습니다.
- 백업 완료/실패 exit code를 모니터링하고 정기적인 복원 훈련으로 RTO를 측정하세요.
- 현재 RPO는 마지막 full snapshot 시각입니다. 연속 WAL/PITR 기능이 있는 것으로
  가정해 복구 정책을 세우면 안 됩니다.
