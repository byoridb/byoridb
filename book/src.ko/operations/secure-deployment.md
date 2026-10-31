# TLS 단일 노드 배포

ByoriDB의 기본 리스너는 평문 HTTP와 gRPC를 사용합니다. 독립된 [`deploy/secure/compose.yml`](https://github.com/byoridb/byoridb/blob/main/deploy/secure/compose.yml) 프로필은 NGINX에서 TLS를 종료하고 비공개 Docker 네트워크의 단일 데이터베이스에 연결합니다. 프록시만 호스트 포트를 공개합니다. 이 프로필은 복제, 장애 조치, 네이티브 TLS, 저장 데이터 암호화를 제공하지 않습니다.

실행 중인 두 서비스 모두 UID/GID `10001:10001`, Linux capability 제거, 읽기 전용 루트 파일시스템을 사용합니다. 데이터베이스는 소유자가 `10001`인 named volume에 기록하며 임시 파일은 크기가 제한된 tmpfs에 저장합니다. 기존 root 소유 bind mount나 volume은 새 이미지를 시작하기 전에 소유권을 이전해야 합니다. 기존 저장소의 권한을 바꾸기 전에는 데이터베이스를 중지하고 검증된 백업을 확보하세요.

소스 이미지는 서버, 백업 도구, bulk loader, CLI를 `cargo build --locked`로 빌드합니다. Rust·런타임·NGINX 이미지는 registry digest로 고정하며, 갱신 후 배포 smoke를 다시 실행합니다. OS 패키지 업데이트는 각 빌드 시 배포판 저장소에서 결정됩니다. Prebuilt Dockerfile에는 Linux 아키텍처가 같고 Ubuntu 24.04와 glibc 호환성이 있는 바이너리가 필요합니다.

## 프로필 시작

클라이언트가 사용할 호스트 이름을 SAN에 포함한 인증서를 사용합니다. 인증 기관에서 받은 전체 인증서 체인과 대응하는 개인 키를 준비하고 원본 자격 증명은 저장소 밖에 보관합니다. 설치기는 `0750` 디렉터리 안에 `0640` 권한의 인증서·키 쌍을 버전별로 생성하며, 프록시는 보조 그룹을 통해 읽습니다.

저장소 루트에서 실행합니다.

```bash
export BYORIDB_ROOT_PASSWORD="$(openssl rand -base64 36)"
export BYORIDB_TLS_CERT_DIR="$HOME/.local/share/byoridb/tls"
export BYORIDB_TLS_CERT_GID="$(id -g)"
python3 deploy/secure/rotate_cert.py \
  --cert-dir "$BYORIDB_TLS_CERT_DIR" \
  --cert /secure-input/fullchain.pem --key /secure-input/privkey.pem \
  --no-reload

docker compose -f deploy/secure/compose.yml --project-name byoridb-secure \
  up --detach --build --wait
```

생성한 root 암호는 배포 secret manager에 보관하고 이후 시작에서도 같은 값을 사용합니다. Docker 호스트 관리자는 컨테이너 환경 변수를 볼 수 있으므로 Docker 호스트는 신뢰 경계에 포함됩니다. 별도 인증서 그룹을 사용할 때는 해당 그룹 소유로 인증서 디렉터리를 만들고 `BYORIDB_TLS_CERT_GID`에 숫자 ID를 지정합니다. 설치기를 실행하는 계정은 디렉터리에 쓰고 해당 그룹을 지정할 권한이 있어야 합니다.

기본 바인딩은 HTTPS `127.0.0.1:8443`, TLS gRPC `127.0.0.1:9443`입니다. 네트워크 클라이언트를 허용하려면 `BYORIDB_TLS_BIND`에 원하는 인터페이스를 지정하고 호스트·네트워크 방화벽으로 접근을 제한합니다. 호스트 포트는 `BYORIDB_HTTPS_PORT`, `BYORIDB_GRPCS_PORT`로 바꿉니다. 평문 loopback 리스너를 공개하는 로컬 개발용 루트 `docker-compose.yml`과 이 프로필을 함께 적용하지 마세요.

서버 호스트 이름과 인증 기관을 검증하는 클라이언트를 사용합니다. 번들 CLI는 현재 내부 평문 백엔드에 연결하므로 관리 작업은 서버 컨테이너 안에서 실행합니다. CLI 주소에 `https://`를 붙이는 것으로 TLS 지원이 추가되지는 않습니다.

## 비공개 운영 경로와 로그

프록시는 `/api/v1/session`, `/api/v1/discover`, `/api/v1/query`, `/api/v1/query/json`과 선언된 `graph.GraphService` RPC 5개만 허용합니다. `/health`, `/ready`, metrics, diagnostics, 알 수 없는 HTTP 경로·RPC 메서드에는 `403`을 반환합니다. Readiness는 서버 컨테이너 안에서 확인합니다.

```bash
docker compose -f deploy/secure/compose.yml --project-name byoridb-secure \
  exec -T server curl --fail --silent http://127.0.0.1:19669/ready
```

데이터베이스 백엔드는 호스트 포트를 공개하지 않습니다. TLS는 프록시에서 종료되며 두 컨테이너 사이 통신은 internal Docker 네트워크의 평문입니다. 신뢰하지 않는 컨테이너를 이 네트워크에 연결하지 마세요. 백엔드가 보는 상대 IP는 프록시이므로 IP별 로그인 제한은 이 프록시를 사용하는 클라이언트가 공유하며 계정별 제한은 별도로 적용됩니다.

NGINX access/error 요청 로그는 자격 증명이 포함된 URL을 기록할 수 있어 비활성화했습니다. ByoriDB는 `info` 수준에서 본문·bearer session ID 대신 문장 유형·크기·결과를 기록합니다. 이 프로필에 요청 본문·헤더 로깅을 추가하거나 debug/trace 로그를 활성화하지 마세요. 운영에는 비공개 readiness, 프로세스 상태, 애플리케이션 metrics를 사용합니다.

## 인증서 교체

같은 서비스 호스트 이름의 갱신 인증서와 대응하는 키를 준비합니다. 새 연결을 검증할 때까지 이전 인증서 디렉터리를 유지합니다.

```bash
python3 deploy/secure/rotate_cert.py \
  --cert-dir "$BYORIDB_TLS_CERT_DIR" \
  --cert /secure-input/renewed-fullchain.pem --key /secure-input/renewed-privkey.pem \
  --project byoridb-secure
```

설치기는 인증서와 키의 일치 여부 및 leaf 인증서 만료 여부를 확인하고, 완성된 버전별 인증서·키 쌍의 경로로 `active.conf` 설정 파일을 원자적으로 교체합니다. 이어 NGINX 설정을 검사하고 graceful reload를 요청합니다. 검증·reload 실패 시 이전 설정을 복원하며 보관된 인증서 디렉터리를 수동 복구에 사용할 수 있습니다. 기존 연결은 이전 TLS 세션을 유지할 수 있으므로 새 HTTPS·gRPC 연결로 갱신 인증서를 확인합니다. CA가 바뀌면 이전 CA를 제거하기 전에 클라이언트 신뢰 저장소도 갱신해야 합니다. 데이터베이스 컨테이너 재생성으로 내부 IP가 바뀌면 프록시를 reload하여 upstream 주소를 갱신합니다.

## 재현 가능한 배포 검사

Docker Engine·Compose, Python 3, OpenSSL, HTTP/2를 지원하는 curl이 필요합니다.

```bash
python3 deploy/secure/smoke.py --build
# Or validate an already-built source or prebuilt image:
python3 deploy/secure/smoke.py --image byoridb-server:local
```

Smoke는 임의 이름의 독립 Compose 프로젝트·포트, 하루짜리 테스트 CA·leaf 인증서를 만듭니다. 비root 실행, volume 소유권, 포함된 바이너리, 백엔드 포트 비공개, 운영 경로 차단, 실제 HTTPS·gRPC 인증과 쿼리 결과, 서버 재시작 뒤 정확한 데이터 유지, 인증서 신뢰·교체, 컨테이너 로그의 자격 증명·쿼리 본문 미노출을 확인합니다. 자신이 만든 컨테이너·네트워크·데이터 volume만 삭제합니다. 테스트 인증서는 운영에 사용하면 안 됩니다. Linux 릴리스 빌드용 로컬 디스크 공간이 필요하며, 이 검사는 백업 복구 훈련이나 용량 테스트를 대체하지 않습니다.

설정 근거는 공식 [NGINX HTTPS](https://nginx.org/en/docs/http/configuring_https_servers.html), [gRPC 프록시](https://nginx.org/en/docs/http/ngx_http_grpc_module.html), [graceful reload](https://nginx.org/en/docs/control.html), [Docker Compose 서비스](https://docs.docker.com/reference/compose-file/services/) 문서입니다.
