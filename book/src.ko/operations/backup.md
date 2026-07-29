# 백업 및 복원

[English](../../operations/backup.html)

ByoriDB는 하나의 snapshot backup 도구인 `byoridb-backup`을 제공합니다. redb의
current-view와 history table을 별도 `data.redb` 파일로 복사하고 옆에 metadata를
기록합니다.

이 도구는 incremental backup, WAL archive, point-in-time recovery, space별 restore,
S3/object-store output, encryption을 구현하지 않습니다. snapshot 성공 후 off-site 복사와
암호화는 외부 도구로 구성하세요.

## 도구 빌드

```bash
cargo build --release --bin byoridb-backup
export PATH="$PWD/target/release:$PATH"
```

database 인자는 redb 파일이 아니라 `data.redb`를 담은 directory입니다. standalone
기본값은 `data/storage`이며 `BYORIDB__STORAGE__DATA_PATHS`로 변경할 수 있습니다.

## Backup에는 독점 접근이 필요합니다

standalone server와 backup CLI는 별도 process입니다. redb는 server가 이미 연 database를
CLI가 다시 여는 것을 막으므로 live backup은 현재
`Database already open. Cannot acquire lock.` 오류로 실패합니다.

다음과 같이 조정된 offline window를 사용하세요.

1. traffic을 중단하고 `byoridb-server`를 graceful하게 중지합니다.
2. 마지막 redb checkpoint가 끝나도록 server process 종료를 확인합니다.
3. data directory에 `byoridb-backup create`를 실행합니다.
4. 새 backup을 검사하고 아래 검증을 수행합니다.
5. server를 다시 시작합니다.

변경 중인 `data.redb`를 `cp`로 복사하지 마세요. backup 구현은 read transaction을 열고
`kv`와 `history`를 모두 새 redb 파일로 복사합니다.

## Snapshot 생성과 확인

```bash
byoridb-backup create \
  --db /var/lib/byoridb/data \
  --backup-dir /var/lib/byoridb/backups \
  --label "daily-before-upgrade"
```

명령은 다음과 같은 timestamp 기반 directory를 만듭니다.

```text
/var/lib/byoridb/backups/backup_1785313593/
├── backup_metadata.json
└── data.redb
```

Unix에서 새 backup root는 mode `0700`으로 설정됩니다. 파일에는 password hash와 graph
property를 포함한 raw database data가 있으므로 다른 위치에 복사한 뒤에도 제한된
소유권을 유지하세요.

catalog entry를 나열하고 확인합니다.

```bash
byoridb-backup list --backup-dir /var/lib/byoridb/backups
byoridb-backup list --backup-dir /var/lib/byoridb/backups --format json
byoridb-backup info \
  --backup-dir /var/lib/byoridb/backups \
  --backup-id backup_1785313593
```

`--no-flush`는 compatibility를 위해 CLI에 남아 있지만 redb 구현에는 별도 WAL flush가
없고 현재 no-op입니다. 이 옵션으로 live backup이 가능해지지 않습니다.

## Verification 한계

```bash
byoridb-backup verify \
  --backup-dir /var/lib/byoridb/backups \
  --backup-id backup_1785313593
```

현재 `verify` 명령은 backup을 catalog에서 찾을 수 있는지 확인하는 수준입니다. 모든
table을 순회하거나 application record를 검증하거나 row count를 비교하거나 query를
실행하지 않습니다. 실패한 `create`도 timestamp 이름의 directory를 남길 수 있으므로
`verify`만으로 사용 가능한 snapshot이라고 판단하지 마세요.

중요한 backup마다 다음을 수행하세요.

- 성공한 `create` exit code 확인
- `backup_metadata.json`과 비어 있지 않은 `data.redb` 확인
- 새 directory로 restore
- 그 directory와 production이 아닌 port로 격리된 server 시작
- 인증 후 대표적인 current query와 `AS OF` query 확인

## Restore

먼저 destination을 사용하는 process를 중지하세요. 가능하면 새 directory에 restore합니다.

```bash
byoridb-backup restore \
  --backup-dir /var/lib/byoridb/backups \
  --backup-id backup_1785313593 \
  --target /var/lib/byoridb/restored-data
```

verification server를 시작하기 전에 `BYORIDB__STORAGE__DATA_PATHS`를 restore한
directory로 지정합니다.

```bash
export BYORIDB_ROOT_PASSWORD='managed-secret-for-this-environment'
export BYORIDB__STORAGE__DATA_PATHS=/var/lib/byoridb/restored-data
byoridb-server
```

root password는 user record에서 restore되지 않습니다. root는 server 시작 시 항상
`BYORIDB_ROOT_PASSWORD`로 정의됩니다. durable non-root user record는 database
snapshot 안에 있습니다.

`restore --overwrite`는 snapshot을 복사하기 전에 기존 target directory를 재귀적으로
삭제합니다. 사용 전에 정확한 target을 검증하고 rollback copy를 보관하세요.

## Retention 명령

알고 있는 backup 하나를 삭제합니다.

```bash
byoridb-backup delete \
  --backup-dir /var/lib/byoridb/backups \
  --backup-id backup_1785313593
```

최신 catalog entry 다섯 개만 남깁니다.

```bash
byoridb-backup cleanup \
  --backup-dir /var/lib/byoridb/backups \
  --keep 5
```

두 명령 모두 `--force`가 없으면 확인을 요청합니다. 특히 create 실패가 있었다면 자동
cleanup 전에 목록을 검토하세요.

`scripts/backup.sh`는 `create`, 개수 기반 cleanup, list를 감쌉니다. server를 중지하거나
독점 접근을 조정하지 않으므로 schedule 전에 운영자가 이를 별도로 구성해야 합니다.

## 운영 checklist

- primary data volume과 다른 failure domain에 backup을 보관합니다.
- 외부의 검증된 도구로 snapshot의 at-rest/in-transit encryption을 적용합니다.
- command exit code와 예상 파일의 존재/크기를 monitoring합니다.
- full restore와 대표 temporal query를 정기적으로 시험합니다.
- 측정한 backup/restore drill로 환경별 RPO/RTO를 기록합니다. ByoriDB는 보편 수치를
  제공하지 않습니다.
- snapshot을 읽는 데 필요한 application build/configuration을 보존하고 production data를
  교체하기 전에 copy에서 upgrade를 시험합니다.
