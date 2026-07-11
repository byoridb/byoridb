# Agent Memory — 벼리디비를 Claude Code 장기 기억으로 (dogfooding 자산)

`docs/MEMORY_WIKI_DESIGN.md`의 설계를 실제로 구동하는 **Claude Code 측 자산의 참조 사본**이다.
라이브 원본은 리포 밖(`~/.claude/`)에 있고, 여기 사본은 버전관리·공유·재설치용이다.

## 구성

| 파일 | 라이브 위치 | 역할 |
|---|---|---|
| `byoridb-memory/SKILL.md` | `~/.claude/skills/byoridb-memory/SKILL.md` | 기억 스킬. 2레이어(quick notes + 타입드 wiki), canonical-name→vid 레시피, 인과 포착·체크포인트 규율 |
| `hooks.snippet.json` | `~/.claude/settings.json`의 `hooks` 키 | 체크포인트 자동화 훅 2개 (SessionStart recall / git commit capture 리마인더) |

전제: 로컬 상시 ByoriDB + `byoridb` MCP 서버(도구 `memory_remember`/`memory_recall`/`memory_query`).

## 설치

```bash
# 1) 스킬
mkdir -p ~/.claude/skills/byoridb-memory
cp docs/agent-memory/byoridb-memory/SKILL.md ~/.claude/skills/byoridb-memory/

# 2) 훅 — 기존 ~/.claude/settings.json에 hooks 키를 병합(덮어쓰기 아님)
jq -s '.[0] * .[1]' ~/.claude/settings.json docs/agent-memory/hooks.snippet.json > /tmp/s.json \
  && mv /tmp/s.json ~/.claude/settings.json
```

## 주의

- 훅은 MCP를 **직접 호출하지 않는다** — 리마인더 컨텍스트만 주입한다. 실제 기록/조회는 에이전트가 스킬을 따라 수행한다.
- 타입드 노드는 `INSERT VERTEX`에 INT64 vid가 필요하다. canonical name→안정적 vid 레시피는 `SKILL.md` 참조(`status`는 예약어라 상태 property는 `state` 사용).
- 이 사본은 스냅샷이다. 라이브를 고치면 여기도 갱신할 것(반대도 마찬가지).
