# ByoriDB — 로컬 기억 substrate 설치기

Claude Code(및 임의 MCP 클라이언트)의 **영속 기억**으로 쓸 로컬 ByoriDB를 한 번에 설치한다.
설치기 하나가 서버·MCP 서버·스킬을 모두 세팅한다.

## 한 줄 설치

```sh
curl -fsSL https://github.com/byoridb/byoridb/releases/latest/download/install.sh | bash
```

> macOS(Apple Silicon/Intel) · Linux x86_64 지원. Windows 미지원.
> 요구: `curl`, `tar`, `python3`(MCP 서버 실행용). Claude Code CLI가 있으면 MCP 서버를 자동 등록한다.

## 무엇을 설치하나

| 구성 | 위치 | 역할 |
|---|---|---|
| `byoridb-server` (+`byoridb-cli`) | `~/.byoridb/bin/` | 로컬 ByoriDB (gRPC 9669 / HTTP 19669, `127.0.0.1` 바인딩) |
| `byoridb_mcp.py` | `~/.byoridb/` | `memory_remember`/`memory_recall`/`memory_query` 도구를 stdio로 노출. `claude_memory` space 자동 부트스트랩 |
| 상시 실행 서비스 | launchd `com.byoridb.local`(macOS) / systemd --user(Linux) | 부팅 시 자동 기동 + KeepAlive |
| `env` | `~/.byoridb/env` (chmod 600) | 랜덤 생성된 root 비밀번호 |
| 스킬 | `~/.claude/skills/byoridb-memory/SKILL.md` | 언제/무엇을 기억·회수할지의 정책 |
| 데이터 | `~/.byoridb/data/` | redb 파일 (로컬 전용) |

## 옵션

```sh
install.sh [--with-hooks] [--tag vX.Y.Z] [--uninstall]
           [--binary PATH] [--assets DIR]   # 로컬/오프라인 설치용
```

- `--with-hooks` — 체크포인트 자동화 훅을 `~/.claude/settings.json`에 **병합**(기본은 안 함).
- `--tag` — 특정 릴리스 태그 고정(기본: 최신 릴리스).
- `--uninstall` — 서비스 중지·해제, MCP 등록 해제, 스킬 제거. **데이터는 확인 후 보존/삭제 선택.**
- `--binary PATH` — 다운로드 대신 로컬 `byoridb-server` 바이너리 사용.
- `--assets DIR` — 다운로드 대신 로컬 repo 체크아웃(`DIR`)에서 mcp.py/템플릿/스킬을 가져옴.

환경변수: `BYORIDB_HOME`(기본 `~/.byoridb`), `BYORIDB_HTTP_PORT`(기본 19669), `BYORIDB_GRAPH_PORT`(기본 9669).
격리 테스트: `BYORIDB_HOME=/tmp/bt BYORIDB_HTTP_PORT=29669 BYORIDB_GRAPH_PORT=29670 ./install.sh --binary … --assets …`

## 관리

```sh
curl -s localhost:19669/health          # 상태
claude mcp list                         # byoridb ✔ Connected 확인
tail -f ~/.byoridb/logs/server.err      # 로그
# macOS 중지/시작
launchctl unload/load -w ~/Library/LaunchAgents/com.byoridb.local.plist
# Linux
systemctl --user stop/start byoridb-local
```

## 한계

- MCP 서버는 리마인더가 아니라 실제 데이터 도구다. **기억할지 말지의 정책은 스킬**(`byoridb-memory`)에 있다.
- current/history dual-write는 비원자적이며 같은 millisecond 재기록은 history key 충돌 위험(bitemporal v1 제약).
- 로컬 단일 노드 전용. 분산/프로덕션 배포와 무관.
